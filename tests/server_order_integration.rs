//! Real end-to-end coverage of the server/query layer (ADR-0010, Accepted)
//! against `Order`/`Customer` — the second validation domain, exercising
//! `Parent`/`Children` (a real directed relation `Dog` doesn't have) the
//! way `server_dog_integration.rs` exercises `Neighbors` (a real symmetric
//! relation `Order`/`Customer` doesn't have). See that file's own module
//! doc comment for what "real end-to-end" means here (a background thread
//! with a real socket, not a genuinely separate OS process).

use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Order, OrderStatus,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::order::{
    OrderConnectionStore, FIELD_AMOUNT, FIELD_CREATED_AT, FIELD_DISCOUNT, FIELD_STATUS,
};
use rusty_multimodal_db::server::protocol::{Request, Response, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

fn sample_orders() -> Vec<Order> {
    vec![
        Order {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(100),
            amount_cents: 2_500,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 1_000,
            discount_cents: 0,
        },
        Order {
            id: Uuid::from_u128(2),
            customer_id: Uuid::from_u128(100),
            amount_cents: 4_200,
            status: OrderStatus::Pending,
            created_at_unix_ms: 2_000,
            discount_cents: 0,
        },
    ]
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
    let dir = unique_dir("server_order_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("amount.mmap");

    let stack = create_order_production_stack(sample_orders(), &path).unwrap();
    let connection_store = Arc::new(OrderConnectionStore::new(GenericProductionStore::new(
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
fn a_real_client_gets_filters_scans_updates_parent_and_children_over_the_wire() {
    let addr = start_server();
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (FIELD_AMOUNT, ScanValue::I64(2_500)),
                (FIELD_STATUS, ScanValue::U32(1)), // Shipped
                (FIELD_CREATED_AT, ScanValue::I64(1_000)),
                (FIELD_DISCOUNT, ScanValue::I64(0)),
            ],
        }
    );

    assert_eq!(
        roundtrip(
            &mut client,
            Request::FilterEq {
                field: FIELD_STATUS,
                value: ScanValue::U32(1)
            }
        ),
        Response::RecordList {
            records: vec![Uuid::from_u128(1)]
        }
    );

    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AMOUNT,
                value: ScanValue::I64(9_000)
            }
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (FIELD_AMOUNT, ScanValue::I64(9_000)),
                (FIELD_STATUS, ScanValue::U32(1)),
                (FIELD_CREATED_AT, ScanValue::I64(1_000)),
                (FIELD_DISCOUNT, ScanValue::I64(0)),
            ],
        }
    );

    // Parent: an Order id in, a Customer id out.
    assert_eq!(
        roundtrip(
            &mut client,
            Request::Parent {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Id {
            id: Uuid::from_u128(100)
        }
    );

    // Children: a Customer id in, Order ids out.
    match roundtrip(
        &mut client,
        Request::Children {
            id: Uuid::from_u128(100),
        },
    ) {
        Response::RecordList { mut records } => {
            records.sort();
            assert_eq!(records, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        }
        other => panic!("expected a RecordList response, got {other:?}"),
    }

    // Order/Customer has no symmetric relation — a typed error, not a
    // panic or a silently wrong answer.
    match roundtrip(
        &mut client,
        Request::Neighbors {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::Err { .. } => {}
        other => panic!("expected Neighbors on Order/Customer to report an error, got {other:?}"),
    }
}

/// The `Order`/`Customer` half of the schema-driven round trip (ADR-0011)
/// — see `tests/server_dog_integration.rs`'s identical-purpose test for
/// why this proves discovery is actually usable, not just that
/// `DescribeSchema` returns static data. Discovers the "status" field by
/// name, then uses its tag to filter — the operation `Order`/`Customer`
/// supports that `Dog` doesn't (`FilterEq`, not `Neighbors`).
#[test]
fn a_schema_driven_client_discovers_and_uses_the_status_field() {
    let addr = start_server();
    let mut client = connect(addr);

    let schema = match roundtrip(&mut client, Request::DescribeSchema) {
        Response::Schema(schema) => schema,
        other => panic!("expected Response::Schema, got {other:?}"),
    };
    assert!(schema.relations.parent_children);
    assert!(!schema.relations.neighbors);

    let status_field = schema
        .fields
        .iter()
        .find(|f| f.name == "status")
        .expect("DescribeSchema should name a \"status\" field");
    assert!(status_field.capabilities.filter_eq);

    // Shipped == 1, per server::order's own status_to_u32 mapping — the
    // client doesn't need to know that encoding beyond "this is the value
    // GetById returned for order 1's status field."
    let order_1_status = match roundtrip(
        &mut client,
        Request::GetById {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::Record { fields, .. } => fields
            .into_iter()
            .find(|(tag, _)| *tag == status_field.tag)
            .map(|(_, v)| v)
            .expect("GetById should include the status field"),
        other => panic!("expected Response::Record, got {other:?}"),
    };

    assert_eq!(
        roundtrip(
            &mut client,
            Request::FilterEq {
                field: status_field.tag,
                value: order_1_status
            }
        ),
        Response::RecordList {
            records: vec![Uuid::from_u128(1)]
        }
    );
}
