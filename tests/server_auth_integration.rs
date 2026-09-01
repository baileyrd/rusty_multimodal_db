//! Real end-to-end coverage of `AuthConfig`'s connection gating
//! (`docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012, Accepted) against the
//! `Dog` domain — a real `TcpListener`, a real client `TcpStream`, real
//! `bincode` encoding over the wire, exercising every functional
//! acceptance criterion the design document names. `src/server/mod.rs`'s
//! own unit tests cover `AuthConfig::check` and `dispatch`'s in-process
//! logic; this file covers the same ground `tests/server_dog_integration.rs`
//! does for the rest of the protocol — a real socket, not just a function
//! call.

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{ErrorCode, Request, Response, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig};
use rusty_multimodal_db::ProductionStore;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s own `unique_dir` for why this
/// needs both the process id and a monotonic counter, not just one.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn start_server(auth: AuthConfig) -> std::net::SocketAddr {
    let dir = unique_dir("server_auth_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let records = vec![DogRecord::new(Uuid::from_u128(1), "labrador", 3)];
    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, auth, None));
    addr
}

fn connect(addr: std::net::SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

fn roundtrip(stream: &mut TcpStream, req: Request) -> Response {
    write_message(stream, &req).unwrap();
    read_message(stream).unwrap()
}

fn assert_unauthenticated(resp: Response) {
    match resp {
        Response::Err {
            code: ErrorCode::Unauthenticated,
            ..
        } => {}
        other => panic!("expected ErrorCode::Unauthenticated, got {other:?}"),
    }
}

fn assert_unauthorized(resp: Response) {
    match resp {
        Response::Err {
            code: ErrorCode::Unauthorized,
            ..
        } => {}
        other => panic!("expected ErrorCode::Unauthorized, got {other:?}"),
    }
}

fn auth_config() -> AuthConfig {
    AuthConfig::new(Some("read-token".into()), Some("write-token".into()))
}

/// `AUTH-FR-002`: a connection that never authenticates is rejected for
/// every request kind but `Authenticate` — including `DescribeSchema`,
/// the case this design deliberately does not carve an exception for.
#[test]
fn an_unauthenticated_connection_is_rejected_for_every_request_kind_including_describe_schema() {
    let addr = start_server(auth_config());
    let mut client = connect(addr);

    assert_unauthenticated(roundtrip(
        &mut client,
        Request::GetById {
            id: Uuid::from_u128(1),
        },
    ));
    assert_unauthenticated(roundtrip(&mut client, Request::DescribeSchema));
    assert_unauthenticated(roundtrip(
        &mut client,
        Request::UpdateField {
            id: Uuid::from_u128(1),
            field: FIELD_AGE,
            value: ScanValue::U32(9),
        },
    ));
}

/// `AUTH-FR-001`: a wrong token gets exactly the same error shape as never
/// authenticating at all — a client can't distinguish "wrong token" from
/// "no token sent yet" from the response alone.
#[test]
fn a_wrong_token_is_indistinguishable_from_never_authenticating() {
    let addr = start_server(auth_config());
    let mut client = connect(addr);

    assert_unauthenticated(roundtrip(
        &mut client,
        Request::Authenticate {
            token: "not-a-real-token".into(),
        },
    ));
    // And the connection is still unauthenticated afterward — a failed
    // Authenticate attempt doesn't leave the connection in some partial
    // state.
    assert_unauthenticated(roundtrip(
        &mut client,
        Request::GetById {
            id: Uuid::from_u128(1),
        },
    ));
}

/// `AUTH-FR-003`: `ReadOnly` succeeds on every read-shaped request kind and
/// gets `Unauthorized` specifically for `UpdateField`.
#[test]
fn a_read_only_token_can_read_but_not_write() {
    let addr = start_server(auth_config());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Authenticate {
                token: "read-token".into(),
            },
        ),
        Response::Ok
    );

    assert!(matches!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1),
            },
        ),
        Response::Record { .. }
    ));
    assert!(matches!(
        roundtrip(&mut client, Request::DescribeSchema),
        Response::Schema(_)
    ));

    assert_unauthorized(roundtrip(
        &mut client,
        Request::UpdateField {
            id: Uuid::from_u128(1),
            field: FIELD_AGE,
            value: ScanValue::U32(9),
        },
    ));

    // The connection stays open and authenticated after a rejected write —
    // a subsequent read still succeeds, matching the design's "only that
    // one request is rejected" invariant.
    assert!(matches!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1),
            },
        ),
        Response::Record { .. }
    ));
}

/// `AUTH-FR-003`: `ReadWrite` succeeds on everything, including
/// `UpdateField`.
#[test]
fn a_read_write_token_can_read_and_write() {
    let addr = start_server(auth_config());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Authenticate {
                token: "write-token".into(),
            },
        ),
        Response::Ok
    );

    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(9),
            },
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1),
            },
        ),
        Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (
                    rusty_multimodal_db::server::dog::FIELD_BREED,
                    ScanValue::Str("labrador".into())
                ),
                (FIELD_AGE, ScanValue::U32(9)),
            ],
        }
    );
}

/// `AUTH-FR-007`: no tokens configured at all reproduces exactly today's
/// pre-ADR-0012 unauthenticated behavior — `Authenticate` itself still
/// round-trips successfully (a no-op), and every other request kind
/// succeeds without ever sending it.
#[test]
fn no_configured_tokens_reproduces_the_original_unauthenticated_behavior() {
    let addr = start_server(AuthConfig::default());
    let mut client = connect(addr);

    assert!(matches!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1),
            },
        ),
        Response::Record { .. }
    ));
    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(9),
            },
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::Authenticate {
                token: "anything-at-all".into(),
            },
        ),
        Response::Ok
    );
}
