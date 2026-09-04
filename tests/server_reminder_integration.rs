//! Real end-to-end coverage of the `Reminder` domain (`RMD-FR-001`–`009`,
//! ADR-0036, `docs/design/SERVER-REMINDER-DOMAIN-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`. `required-features =
//! ["server"]` only, no `research` — `Reminder` is front-door, matching
//! `tests/server_dog_integration.rs`'s own precedent, not
//! `tests/server_sql_integration.rs`'s all-three-domains shape.

use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic::reminder::{
    create_reminder_production_stack, Reminder, ReminderStatus,
};
use rusty_multimodal_db::server::client::{
    ClientError, QueryResult, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    ErrorCode, ParentLookup, Request, Response, ScanValue, TransactionOp,
};
use rusty_multimodal_db::server::reminder::{ReminderConnectionStore, FIELD_STATUS};
use rusty_multimodal_db::server::{serve, ServeOptions};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s identical helper for why this
/// needs both the process id and a monotonic counter, not just one.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn sample_reminders() -> Vec<Reminder> {
    vec![
        Reminder {
            id: Uuid::from_u128(1),
            title: "Pay rent".into(),
            due_at_unix_ms: 1_000,
            status: ReminderStatus::Pending,
        },
        Reminder {
            id: Uuid::from_u128(2),
            title: "Call dentist".into(),
            due_at_unix_ms: 2_000,
            status: ReminderStatus::Snoozed,
        },
        Reminder {
            id: Uuid::from_u128(3),
            title: "Renew passport".into(),
            due_at_unix_ms: 2_000,
            status: ReminderStatus::Done,
        },
    ]
}

fn start_server() -> SocketAddr {
    let dir = unique_dir("reminder_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reminders.mmap");
    let stack = create_reminder_production_stack(sample_reminders(), &path).unwrap();
    let connection_store = Arc::new(ReminderConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn rows(result: QueryResult) -> Vec<(Uuid, Vec<(String, ScanValue)>)> {
    match result {
        QueryResult::Rows(rows) => rows,
        QueryResult::Groups(groups) => {
            panic!("expected QueryResult::Rows, got Groups: {groups:?}")
        }
    }
}

fn groups(result: QueryResult) -> Vec<Vec<(String, ScanValue)>> {
    match result {
        QueryResult::Groups(groups) => groups,
        QueryResult::Rows(rows) => panic!("expected QueryResult::Groups, got Rows: {rows:?}"),
    }
}

/// Acceptance criterion 2: `GetById` returns every field, and a
/// `GROUP BY status` count via `Aggregate` matches a hand-computed tally.
#[test]
fn get_returns_every_field_and_group_by_status_counts_correctly() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(
        fields,
        vec![
            ("title".to_string(), ScanValue::Str("Pay rent".into())),
            ("due_at_unix_ms".to_string(), ScanValue::I64(1_000)),
            ("status".to_string(), ScanValue::U32(0)),
        ]
    );
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());

    let counts = groups(
        client
            .query("SELECT status, COUNT(*) FROM reminder GROUP BY status")
            .unwrap(),
    );
    assert_eq!(counts.len(), 3, "Pending, Snoozed, Done — one each");
    for row in &counts {
        assert_eq!(row[1], ("COUNT(*)".to_string(), ScanValue::I64(1)));
    }
}

/// Acceptance criterion 3: `FilterEq` on `due_at_unix_ms` returns exactly
/// the matching ids (two reminders share `due_at_unix_ms = 2_000`);
/// `FilterEq` on `status`/`title` is client-side `Unsupported`, no round
/// trip.
#[test]
fn filter_eq_by_due_at_and_unsupported_fields() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut matches = client
        .filter_eq("due_at_unix_ms", ScanValue::I64(2_000))
        .unwrap();
    matches.sort();
    assert_eq!(matches, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
    assert!(client
        .filter_eq("due_at_unix_ms", ScanValue::I64(9_999))
        .unwrap()
        .is_empty());

    assert!(matches!(
        client.filter_eq("status", ScanValue::U32(0)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.filter_eq("title", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 4: `UpdateField` on `status` with a valid
/// discriminant succeeds and is immediately visible; an invalid
/// discriminant is a server-side `Malformed` with nothing applied (the
/// client-side capability gate lets this one through, since `status` is
/// genuinely `update`-capable — the rejection is the server's own
/// discriminant validation). `UpdateField` on `due_at_unix_ms`/`title` is
/// client-side `Unsupported`.
#[test]
fn update_status_with_discriminant_validation_and_unsupported_fields() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(
        client
            .update(Uuid::from_u128(1), "status", ScanValue::U32(1))
            .unwrap(),
        "Done"
    );
    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(fields[2], ("status".to_string(), ScanValue::U32(1)));

    match client.update(Uuid::from_u128(1), "status", ScanValue::U32(9)) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected a server-side Malformed rejection, got {other:?}"),
    }

    assert!(matches!(
        client.update(Uuid::from_u128(1), "due_at_unix_ms", ScanValue::I64(0)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.update(Uuid::from_u128(1), "title", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 5: `parent`/`children`/`neighbors` are all
/// client-side `Unsupported`, unconditionally — `Reminder` has no
/// relation of either kind.
#[test]
fn every_relation_request_is_unsupported() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(matches!(
        client.parent(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.children(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.neighbors(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));

    let schema = client.schema();
    assert!(!schema.relations.parent_children);
    assert!(!schema.relations.neighbors);
}

/// Acceptance criterion 2 (continued): `Query` with `WHERE` on
/// `due_at_unix_ms` — the range case `FilterEq`'s equality-only shape
/// can't express, working regardless of that field's own capability
/// flags (`ADR-0034`).
#[test]
fn query_filters_on_due_at_by_range() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let due_soon = rows(
        client
            .query("SELECT title FROM reminder WHERE due_at_unix_ms < 2000")
            .unwrap(),
    );
    assert_eq!(due_soon.len(), 1);
    assert_eq!(
        due_soon[0].1,
        vec![("title".to_string(), ScanValue::Str("Pay rent".into()))]
    );
}

/// Acceptance criterion 6: `Request::Transaction` works against
/// `Reminder` with the same shape every other domain's own tests
/// already establish — full success applying every write. Issued via
/// raw framing (matching `tests/server_transaction_integration.rs`'s
/// own precedent), since `SchemaDrivenClient` has no `Transaction`
/// convenience method.
#[test]
fn transaction_applies_every_write() {
    let addr = start_server();
    let mut wire = TcpStream::connect(addr).unwrap();
    wire.set_nodelay(true).unwrap();

    let resp: Response = {
        write_message(
            &mut wire,
            &Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_STATUS,
                        value: ScanValue::U32(1),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(2),
                        field: FIELD_STATUS,
                        value: ScanValue::U32(3),
                    },
                ],
            },
        )
        .unwrap();
        read_message(&mut wire).unwrap()
    };
    assert_eq!(resp, Response::Ok);

    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(
        client.get(Uuid::from_u128(1)).unwrap().unwrap()[2],
        ("status".to_string(), ScanValue::U32(1))
    );
    assert_eq!(
        client.get(Uuid::from_u128(2)).unwrap().unwrap()[2],
        ("status".to_string(), ScanValue::U32(3))
    );
}

/// Acceptance criterion 6 (continued): a read-your-writes session sees
/// its own staged `status` write; a snapshot-isolation session's
/// `Commit` succeeds normally with no conflicting external write — the
/// same session-composition shape every other domain's own tests
/// already establish.
#[test]
fn sessions_compose_with_reminder_the_same_as_every_other_domain() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut session = client
        .begin_with(SessionOptions::new().read_your_writes())
        .unwrap();
    session
        .update(Uuid::from_u128(1), "status", ScanValue::U32(1))
        .unwrap();
    let overlaid = session.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(overlaid
        .iter()
        .any(|(name, value)| name == "status" && *value == ScanValue::U32(1)));
    session.rollback().unwrap();

    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut session = client
        .begin_with(SessionOptions::new().snapshot_isolation())
        .unwrap();
    assert!(session.get(Uuid::from_u128(2)).unwrap().is_some());
    session
        .update(Uuid::from_u128(3), "status", ScanValue::U32(0))
        .unwrap();
    session.commit().unwrap();
}

/// Acceptance criterion 5 (sanity check on the schema itself, matching
/// `Order`'s own `a_field_with_every_capability_flag_false_is_still_queryable`
/// precedent): `title` has every capability flag `false` but is still
/// fully selectable/filterable via `Query` — a full scan needs no index.
#[test]
fn title_has_every_capability_flag_false_but_is_still_queryable() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(!client.schema().fields.iter().any(|f| f.name == "title"
        && (f.capabilities.filter_eq || f.capabilities.scan || f.capabilities.update)));
    let result = rows(
        client
            .query("SELECT title FROM reminder WHERE title = 'Pay rent'")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);
}

/// `ParentLookup` is never reachable through `Reminder` (`parent_children`
/// is `false`), but confirm the client-side gate is what stops it, not a
/// wire round trip that happens to also fail — the identical distinction
/// `every_relation_request_is_unsupported` above draws for `children`/
/// `neighbors`, spelled out once more for `Parent` specifically since its
/// success type (`ParentLookup`) is otherwise unused in this file.
#[test]
fn parent_lookup_type_is_unreachable_client_side() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result: Result<ParentLookup, ClientError> = client.parent(Uuid::from_u128(1));
    assert!(matches!(result, Err(ClientError::Unsupported(_))));
}
