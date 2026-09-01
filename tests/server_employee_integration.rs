//! Real end-to-end coverage of the server/query layer (ADR-0010, Accepted)
//! against `Employee` — the third validation domain, and the first where
//! `Parent`/`Children` *and* `Neighbors` are both real operations over the
//! wire (no domain-shaped `Response::Err` for either). See
//! `tests/server_dog_integration.rs`'s own module doc comment for what
//! "real end-to-end" means here (a background thread with a real socket,
//! not a genuinely separate OS process), and `src/server/employee.rs`'s
//! module docs for the gap this domain found and fixed directly in
//! `crate::generic::{store,production}` before this test could even compile.

use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic_spike::employee_impl::{
    create_employee_production_stack, Department, Employee,
};
use rusty_multimodal_db::server::employee::{
    EmployeeConnectionStore, FIELD_DEPARTMENT, FIELD_NAME, FIELD_SALARY,
};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{Request, Response, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

fn sample_employees() -> Vec<Employee> {
    vec![
        Employee {
            id: Uuid::from_u128(1),
            name: "Alex".into(),
            department: Department::Engineering,
            salary_cents: 1_200_000,
            manager_id: None,
        },
        Employee {
            id: Uuid::from_u128(2),
            name: "Bel".into(),
            department: Department::Engineering,
            salary_cents: 950_000,
            manager_id: Some(Uuid::from_u128(1)),
        },
        Employee {
            id: Uuid::from_u128(3),
            name: "Cas".into(),
            department: Department::Sales,
            salary_cents: 800_000,
            manager_id: Some(Uuid::from_u128(1)),
        },
    ]
}

/// Bel and Cas collaborate — the one symmetric edge this dataset needs to
/// exercise `Neighbors` alongside `Parent`/`Children` on the same record.
fn sample_edges() -> Vec<(Uuid, Uuid)> {
    vec![(Uuid::from_u128(2), Uuid::from_u128(3))]
}

/// See `tests/server_dog_integration.rs`'s identical helper for why this
/// needs to be unique per call, not just per process.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn start_server() -> std::net::SocketAddr {
    let dir = unique_dir("server_employee_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("salary.mmap");

    let stack =
        create_employee_production_stack(sample_employees(), &sample_edges(), &path).unwrap();
    let connection_store = Arc::new(EmployeeConnectionStore::new(GenericProductionStore::new(
        stack,
    )));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default(), None));
    addr
}

/// Connects and disables Nagle's algorithm — see
/// `tests/server_dog_integration.rs`'s identical helper for why (a
/// synchronous request/response protocol pays a real ~40ms-per-round-trip
/// cost otherwise).
fn connect(addr: std::net::SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

fn roundtrip(stream: &mut TcpStream, req: Request) -> Response {
    write_message(stream, &req).unwrap();
    read_message(stream).unwrap()
}

#[test]
fn a_real_client_gets_filters_scans_updates_parent_children_and_neighbors_over_the_wire() {
    let addr = start_server();
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(2)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(2),
            fields: vec![
                (FIELD_NAME, ScanValue::Str("Bel".into())),
                (FIELD_DEPARTMENT, ScanValue::U32(0)), // Engineering
                (FIELD_SALARY, ScanValue::I64(950_000)),
            ],
        }
    );

    match roundtrip(
        &mut client,
        Request::FilterEq {
            field: FIELD_DEPARTMENT,
            value: ScanValue::U32(0),
        },
    ) {
        Response::RecordList { mut records } => {
            records.sort();
            assert_eq!(records, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        }
        other => panic!("expected a RecordList response, got {other:?}"),
    }

    match roundtrip(
        &mut client,
        Request::ScanField {
            field: FIELD_SALARY,
        },
    ) {
        Response::ScanValues { mut values } => {
            values.sort_by_key(|v| match v {
                ScanValue::I64(n) => *n,
                other => panic!("expected I64 scan values, got {other:?}"),
            });
            assert_eq!(
                values,
                vec![
                    ScanValue::I64(800_000),
                    ScanValue::I64(950_000),
                    ScanValue::I64(1_200_000),
                ]
            );
        }
        other => panic!("expected a ScanValues response, got {other:?}"),
    }

    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(2),
                field: FIELD_SALARY,
                value: ScanValue::I64(1_000_000)
            }
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(2)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(2),
            fields: vec![
                (FIELD_NAME, ScanValue::Str("Bel".into())),
                (FIELD_DEPARTMENT, ScanValue::U32(0)),
                (FIELD_SALARY, ScanValue::I64(1_000_000)),
            ],
        }
    );

    // Parent: Bel reports to Alex.
    assert_eq!(
        roundtrip(
            &mut client,
            Request::Parent {
                id: Uuid::from_u128(2)
            }
        ),
        Response::Id {
            id: Uuid::from_u128(1)
        }
    );

    // Children: Alex manages Bel and Cas.
    match roundtrip(
        &mut client,
        Request::Children {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::RecordList { mut records } => {
            records.sort();
            assert_eq!(records, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        }
        other => panic!("expected a RecordList response, got {other:?}"),
    }

    // Neighbors: Bel and Cas collaborate — real, not a domain-shaped error,
    // the first time this has been true for both relation kinds at once.
    assert_eq!(
        roundtrip(
            &mut client,
            Request::Neighbors {
                id: Uuid::from_u128(2)
            }
        ),
        Response::RecordList {
            records: vec![Uuid::from_u128(3)]
        }
    );
}

/// The `Employee` half of the schema-driven round trip (ADR-0011) — see
/// `tests/server_dog_integration.rs`'s identical-purpose test for why this
/// proves discovery is actually usable, not just that `DescribeSchema`
/// returns static data. Discovers the "department" field by name, then
/// uses its tag to filter, and confirms the schema itself reports both
/// relation kinds as supported — the one thing that distinguishes
/// `Employee` from either domain that came before it.
#[test]
fn a_schema_driven_client_sees_both_relation_kinds_and_uses_the_department_field() {
    let addr = start_server();
    let mut client = connect(addr);

    let schema = match roundtrip(&mut client, Request::DescribeSchema) {
        Response::Schema(schema) => schema,
        other => panic!("expected Response::Schema, got {other:?}"),
    };
    assert!(schema.relations.parent_children);
    assert!(schema.relations.neighbors);

    let department_field = schema
        .fields
        .iter()
        .find(|f| f.name == "department")
        .expect("DescribeSchema should name a \"department\" field");
    assert!(department_field.capabilities.filter_eq);

    let cas_department = match roundtrip(
        &mut client,
        Request::GetById {
            id: Uuid::from_u128(3),
        },
    ) {
        Response::Record { fields, .. } => fields
            .into_iter()
            .find(|(tag, _)| *tag == department_field.tag)
            .map(|(_, v)| v)
            .expect("GetById should include the department field"),
        other => panic!("expected Response::Record, got {other:?}"),
    };

    assert_eq!(
        roundtrip(
            &mut client,
            Request::FilterEq {
                field: department_field.tag,
                value: cas_department
            }
        ),
        Response::RecordList {
            records: vec![Uuid::from_u128(3)]
        }
    );
}

/// `Parent` on the record with no manager, and `Neighbors` on a record
/// with no collaborators — the "real relation kind, empty result" cases
/// neither prior domain's tests needed since Dog has no `Parent`/`Children`
/// at all and Order/Customer has no `Neighbors` at all.
#[test]
fn parent_and_neighbors_report_empty_correctly_not_as_errors() {
    let addr = start_server();
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Parent {
                id: Uuid::from_u128(1)
            }
        ),
        Response::NoParent
    );

    match roundtrip(
        &mut client,
        Request::Neighbors {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::RecordList { records } => assert!(records.is_empty()),
        other => panic!("expected an empty RecordList, got {other:?}"),
    }
}
