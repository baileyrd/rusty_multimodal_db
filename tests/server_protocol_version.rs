//! End-to-end coverage of `SERVER-001` FR-020 (`PROTO-FR-001`–`008`,
//! ADR-0022): the wire protocol version and the optional first-frame
//! `Hello`, driven over real TCP against real domain adapters — the
//! design's acceptance criteria 3, 4, 7 and 8. Criteria 1–2 (golden
//! vectors, the constant) live in `src/server/protocol.rs`'s unit tests;
//! criterion 5 (`dispatch` → `Unsupported`) and the intercept against a
//! fixture store live in `src/server/mod.rs`'s. Criterion 6 is every
//! other suite in this crate, unchanged.
//!
//! All three domains in one file, `required-features = ["server",
//! "research"]` — the precedent `tests/server_schema_driven_client.rs`
//! set for a target that needs every domain adapter.

use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Order, OrderStatus,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic_spike::employee_impl::{
    create_employee_production_stack, Department, Employee,
};
use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::client::{
    ClientError, ConnectOptions, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::employee::EmployeeConnectionStore;
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::order::OrderConnectionStore;
use rusty_multimodal_db::server::protocol::{
    ErrorCode, Request, Response, ScanValue, PROTOCOL_VERSION,
};
use rusty_multimodal_db::server::{dispatch, serve, ServeOptions};
use rusty_multimodal_db::ProductionStore;
use std::io::{BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s identical helper for why this
/// needs to be unique per call, not just per process.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn start_dog_server(auth: ServeOptions) -> SocketAddr {
    let dir = unique_dir("proto_version_dog");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let records = vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
    ];
    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, auth));
    addr
}

fn start_order_server() -> SocketAddr {
    let dir = unique_dir("proto_version_order");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("amount.mmap");
    let orders = vec![Order {
        id: Uuid::from_u128(1),
        customer_id: Uuid::from_u128(100),
        amount_cents: 2_500,
        status: OrderStatus::Shipped,
        created_at_unix_ms: 1_000,
        discount_cents: 0,
    }];
    let stack = create_order_production_stack(orders, &path).unwrap();
    let connection_store = Arc::new(OrderConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn start_employee_server() -> SocketAddr {
    let dir = unique_dir("proto_version_employee");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("salary.mmap");
    let employees = vec![Employee {
        id: Uuid::from_u128(1),
        name: "Alex".into(),
        department: Department::Engineering,
        salary_cents: 1_200_000,
        manager_id: None,
    }];
    let edges = vec![];
    let stack = create_employee_production_stack(employees, &edges, &path).unwrap();
    let connection_store = Arc::new(EmployeeConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn connect(addr: SocketAddr) -> (BufReader<TcpStream>, BufWriter<TcpStream>) {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    let peer = stream.try_clone().unwrap();
    (BufReader::new(stream), BufWriter::new(peer))
}

fn roundtrip(
    reader: &mut BufReader<TcpStream>,
    writer: &mut BufWriter<TcpStream>,
    req: &Request,
) -> Response {
    write_message(writer, req).unwrap();
    writer.flush().unwrap();
    read_message(reader).unwrap()
}

/// Criterion 3: on an auth-configured server, `Hello` is answered before
/// any token is presented, with `min(client, PROTOCOL_VERSION)`, and the
/// auth gate behind it is intact.
#[test]
fn hello_is_answered_before_authentication_with_the_min_version() {
    let addr = start_dog_server(ServeOptions::new(Some("ro".into()), Some("rw".into())));

    // A newer client gets this build's version.
    let (mut reader, mut writer) = connect(addr);
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION + 1
            }
        ),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    assert!(matches!(
        roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
        Response::Err {
            code: ErrorCode::Unauthenticated,
            ..
        }
    ));
    // Authentication still works after the hello, and then so does a
    // real request.
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Authenticate { token: "ro".into() }
        ),
        Response::Ok
    );
    assert!(matches!(
        roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
        Response::Schema(_)
    ));

    // An older client gets its own version back.
    let (mut reader, mut writer) = connect(addr);
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: 1
            }
        ),
        Response::Hello {
            protocol_version: 1
        }
    );
}

/// Criterion 4 (`PROTO-FR-004`): version 0 and a non-first `Hello` are
/// `Malformed` with the connection left open; a client that never says
/// `Hello` is served exactly as before (`PROTO-FR-002`).
#[test]
fn hello_zero_and_a_late_hello_are_malformed_and_a_silent_client_is_served() {
    let addr = start_dog_server(ServeOptions::default());

    let (mut reader, mut writer) = connect(addr);
    assert!(matches!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: 0
            }
        ),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    // Still open, still serving.
    assert!(matches!(
        roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
        Response::Schema(_)
    ));
    // ... and a `Hello` now is no longer the first frame.
    assert!(matches!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    assert!(matches!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record { .. }
    ));

    // A version-1 client: first frame is a real request, no hello ever.
    let (mut reader, mut writer) = connect(addr);
    assert!(matches!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::GetById {
                id: Uuid::from_u128(2)
            }
        ),
        Response::Record { .. }
    ));
    assert!(matches!(
        roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
        Response::Schema(_)
    ));
}

/// Criterion 7 (`PROTO-FR-007`): the client library negotiates on every
/// domain and reports this build's version, then works as before.
#[test]
fn schema_driven_client_negotiates_the_current_version_on_every_domain() {
    let mut dog = SchemaDrivenClient::connect(start_dog_server(ServeOptions::default())).unwrap();
    assert_eq!(dog.server_protocol_version(), PROTOCOL_VERSION);
    assert_eq!(dog.server_protocol_version(), 6);
    let fields = dog.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(fields
        .iter()
        .any(|(name, value)| name == "age" && *value == ScanValue::U32(3)));

    let mut order = SchemaDrivenClient::connect(start_order_server()).unwrap();
    assert_eq!(order.server_protocol_version(), PROTOCOL_VERSION);
    assert!(order.get(Uuid::from_u128(1)).unwrap().is_some());

    let mut employee = SchemaDrivenClient::connect(start_employee_server()).unwrap();
    assert_eq!(employee.server_protocol_version(), PROTOCOL_VERSION);
    assert!(employee.get(Uuid::from_u128(1)).unwrap().is_some());
}

/// Criterion 8: a request index this build does not know (the one just
/// past `Rollback`, the highest at protocol version 3) is a decode error,
/// and the connection closes with no reply — the pre-hello failure mode,
/// unchanged and pinned. This test has moved twice: when version 3
/// appended 11–13 (ADR-0024) and when version 5 appended 14 (ADR-0027);
/// the next version that appends a variant moves it again.
#[test]
fn an_unknown_request_index_closes_the_connection_without_a_reply() {
    let addr = start_dog_server(ServeOptions::default());
    let (mut reader, mut writer) = connect(addr);

    // Frame: u32 LE length 4, then a `Request` whose declaration index is
    // 15 — one past `Request::BeginWith` (14), the highest this build knows.
    writer.write_all(&[0x04, 0, 0, 0, 0x0f, 0, 0, 0]).unwrap();
    writer.flush().unwrap();

    let reply: Result<Response, _> = read_message(&mut reader);
    assert!(
        reply.is_err(),
        "expected the connection to close with no reply, got {reply:?}"
    );
}

/// `SESS-FR-006` (design criterion 6, ADR-0024) — the first real use of
/// compatibility rules 3 and 4: the version-3 session requests are
/// `Malformed` on a silent (version-1) connection and on one that said
/// `Hello { 2 }`, with the connection open and an `UpdateField` still
/// applied immediately (never staged); on a version-3 connection `Begin`
/// is answered `Ok`.
#[test]
fn session_requests_are_malformed_below_protocol_version_3() {
    let addr = start_dog_server(ServeOptions::default());

    // Silent client: version 1.
    let (mut reader, mut writer) = connect(addr);
    assert_eq!(
        roundtrip(&mut reader, &mut writer, &Request::Begin),
        Response::Err {
            code: ErrorCode::Malformed,
            message: "the supplied value does not match this field's type".into(),
        }
    );
    assert!(matches!(
        roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
        Response::Schema(_)
    ));

    // A client pinned at version 2.
    let (mut reader, mut writer) = connect(addr);
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: 2
            }
        ),
        Response::Hello {
            protocol_version: 2
        }
    );
    for req in [Request::Begin, Request::Commit, Request::Rollback] {
        assert!(matches!(
            roundtrip(&mut reader, &mut writer, &req),
            Response::Err {
                code: ErrorCode::Malformed,
                ..
            }
        ));
    }
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::UpdateField {
                id: Uuid::from_u128(1),
                field: rusty_multimodal_db::server::dog::FIELD_AGE,
                value: ScanValue::U32(9),
            }
        ),
        Response::Ok
    );

    // Version 3: gated variants available.
    let (mut reader, mut writer) = connect(addr);
    assert_eq!(
        roundtrip(
            &mut reader,
            &mut writer,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    assert_eq!(
        roundtrip(&mut reader, &mut writer, &Request::Begin),
        Response::Ok
    );
    assert_eq!(
        roundtrip(&mut reader, &mut writer, &Request::Rollback),
        Response::Ok
    );
}

/// A *pre-hello* server (the `SERVER-001` v0.9.1 shape, protocol version
/// 1 without the `Hello` variant), emulated rather than checked out: it
/// reads a connection's first frame, and if that frame is a `Hello` —
/// which a real v0.9.1 build could not even decode — it closes the
/// connection with no reply, exactly the documented failure mode. Every
/// other request is answered by the real `dispatch` over a real
/// `DogConnectionStore`, so a client that speaks version 1 gets real
/// answers. `drop_every_first_frame` makes it a server that dies under
/// *any* first frame — the case the fallback must not paper over.
fn start_pre_hello_dog_server(drop_every_first_frame: bool) -> SocketAddr {
    let dir = unique_dir("proto_version_pre_hello");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let records = vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
    ];
    let store = Arc::new(DogConnectionStore::new(
        ProductionStore::create(records, Vec::new(), &path).unwrap(),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let store = Arc::clone(&store);
            thread::spawn(move || {
                let mut first = true;
                loop {
                    let req: Request = match read_message(&mut stream) {
                        Ok(req) => req,
                        Err(_) => return,
                    };
                    if first && (drop_every_first_frame || matches!(req, Request::Hello { .. })) {
                        return; // close with no reply
                    }
                    first = false;
                    let resp = dispatch(store.as_ref(), req);
                    if write_message(&mut stream, &resp).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// `SERVER-001` FR-026 (the reconnect-without-hello fallback, default-on):
/// against a pre-hello server `connect` succeeds on a silent second
/// connection, reports version 1, serves reads, and refuses the
/// version-gated session API; `require_hello()` restores the pre-v0.16.0
/// error; a server that dies under *any* first frame is still an error —
/// the fallback fires once and returns the second attempt's failure, it
/// does not retry forever or invent a server.
#[test]
fn a_pre_hello_server_is_reconnected_to_without_a_hello_unless_hello_is_required() {
    let addr = start_pre_hello_dog_server(false);

    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(client.server_protocol_version(), 1);
    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(fields
        .iter()
        .any(|(name, value)| name == "age" && *value == ScanValue::U32(3)));
    assert!(matches!(
        client.begin().map(|_| ()),
        Err(ClientError::Unsupported(_))
    ));
    // `RYW-FR-007`: the version-5 API is gated the same way, with no frame.
    assert!(matches!(
        client.begin_read_your_writes().map(|_| ()),
        Err(ClientError::Unsupported("read-your-writes session"))
    ));
    assert!(matches!(
        client
            .begin_with(SessionOptions::new().validate_on_stage())
            .map(|_| ()),
        Err(ClientError::Unsupported(_))
    ));

    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().require_hello()).map(|_| ())
    {
        Err(ClientError::Frame(_)) => {}
        other => panic!("expected the pre-hello EOF with require_hello, got {other:?}"),
    }

    let dying = start_pre_hello_dog_server(true);
    match SchemaDrivenClient::connect(dying).map(|_| ()) {
        Err(ClientError::Frame(_)) => {}
        other => panic!(
            "expected a server that dies under any first frame to be an error, got {other:?}"
        ),
    }
}
