//! Real end-to-end coverage of the `Reminder` domain (`RMD-FR-001`–`009`,
//! ADR-0036, `docs/design/SERVER-REMINDER-DOMAIN-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`. `required-features =
//! ["server"]` only, no `research` — `Reminder` is front-door, matching
//! `tests/server_dog_integration.rs`'s own precedent, not
//! `tests/server_sql_integration.rs`'s all-three-domains shape.

use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic::reminder::{
    create_reminder_production_stack, Reminder, ReminderProductionStack, ReminderStatus,
};
use rusty_multimodal_db::server::client::{
    BatchOp, ClientError, QueryResult, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    CompareOp, ErrorCode, ParentLookup, Request, Response, ScanValue, TransactionOp, WriteResult,
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
    start_server_at(unique_dir("reminder_integration"))
}

fn start_server_at(dir: std::path::PathBuf) -> SocketAddr {
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reminders.mmap");
    let stack = if path.exists() {
        ReminderProductionStack::open_portable(&path).unwrap()
    } else {
        create_reminder_production_stack(sample_reminders(), &path).unwrap()
    };
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
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
}

fn groups(result: QueryResult) -> Vec<Vec<(String, ScanValue)>> {
    match result {
        QueryResult::Groups(groups) => groups,
        QueryResult::Rows(rows) => panic!("expected QueryResult::Groups, got Rows: {rows:?}"),
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
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
/// relation of either kind. `LNK-FR-012` (ADR-0047): and so is `link`.
#[test]
fn every_relation_request_is_unsupported() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(matches!(
        client.link(Uuid::from_u128(1), Uuid::from_u128(2), "x"),
        Err(ClientError::Unsupported("Link on this domain"))
    ));

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

/// `JOIN` acceptance criterion 5 (ADR-0044): a domain with no relation of
/// any kind lists nothing, so every `JOIN` is refused client-side with
/// no frame sent.
#[test]
fn a_domain_with_no_relation_lists_nothing_and_refuses_every_join() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(client.relations().is_empty());
    for sql in [
        "SELECT a.title, b.title FROM reminder a JOIN reminder b ON neighbors",
        "SELECT a.title, b.title FROM reminder a JOIN reminder b ON parent",
    ] {
        assert!(
            matches!(client.query(sql), Err(ClientError::Sql(_))),
            "{sql}"
        );
    }
    assert!(client.get(Uuid::from_u128(1)).unwrap().is_some());
}

/// `INS` acceptance criterion 4 (ADR-0046) over a real socket: `insert`
/// then `get` returns the fields sent; the repeat is `Duplicate` with the
/// record unchanged; every malformed field list is refused with nothing
/// inserted; a bad `status` discriminant is `Malformed`; an unknown name
/// is refused client-side with no frame; inside a session the server
/// answers `SessionOpen` and the session still commits; and a server
/// restarted on the same directory serves the inserted record.
#[test]
fn insert_over_the_wire_is_validated_durable_and_served_after_a_restart() {
    let dir = unique_dir("reminder_insert");
    let addr = start_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128(4);
    let fields = [
        ("title", ScanValue::Str("Water plants".into())),
        ("due_at_unix_ms", ScanValue::I64(3_000)),
        ("status", ScanValue::U32(0)),
    ];
    client.insert(id, &fields).unwrap();
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got,
        fields
            .iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        client
            .filter_eq("due_at_unix_ms", ScanValue::I64(3_000))
            .unwrap(),
        vec![id]
    );

    match client.insert(
        id,
        &[
            ("title", ScanValue::Str("x".into())),
            ("due_at_unix_ms", ScanValue::I64(0)),
            ("status", ScanValue::U32(1)),
        ],
    ) {
        Err(ClientError::Server(ErrorCode::Duplicate, _)) => {}
        other => panic!("expected Duplicate, got {other:?}"),
    }
    assert_eq!(
        client.get(id).unwrap().unwrap()[0].1,
        ScanValue::Str("Water plants".into())
    );

    let other = Uuid::from_u128(5);
    for (fields, code) in [
        (
            vec![
                ("title", ScanValue::Str("x".into())),
                ("due_at_unix_ms", ScanValue::I64(0)),
                ("status", ScanValue::U32(9)),
            ],
            ErrorCode::Malformed,
        ),
        (
            vec![
                ("title", ScanValue::Str("x".into())),
                ("status", ScanValue::U32(0)),
            ],
            ErrorCode::Malformed,
        ),
        (
            vec![
                ("title", ScanValue::Str("x".into())),
                ("title", ScanValue::Str("y".into())),
                ("due_at_unix_ms", ScanValue::I64(0)),
                ("status", ScanValue::U32(0)),
            ],
            ErrorCode::Malformed,
        ),
        (
            vec![
                ("title", ScanValue::U32(1)),
                ("due_at_unix_ms", ScanValue::I64(0)),
                ("status", ScanValue::U32(0)),
            ],
            ErrorCode::Malformed,
        ),
    ] {
        match client.insert(other, &fields) {
            Err(ClientError::Server(got, _)) if got == code => {}
            other => panic!("expected {code:?} for {fields:?}, got {other:?}"),
        }
        assert!(client.get(other).unwrap().is_none(), "nothing inserted");
    }
    assert!(matches!(
        client.insert(other, &[("colour", ScanValue::Str("x".into()))]),
        Err(ClientError::UnknownField(_))
    ));

    // Inside a session: refused, and the session is unaffected.
    let mut session = client.begin().unwrap();
    session
        .update(Uuid::from_u128(1), "status", ScanValue::U32(1))
        .unwrap();
    session.commit().unwrap();
    let mut raw = TcpStream::connect(addr).unwrap();
    let hello = |raw: &mut TcpStream| {
        write_message(
            raw,
            &Request::Hello {
                protocol_version: rusty_multimodal_db::server::protocol::PROTOCOL_VERSION,
            },
        )
        .unwrap();
        let _: Response = read_message(raw).unwrap();
    };
    hello(&mut raw);
    write_message(&mut raw, &Request::Begin).unwrap();
    assert_eq!(read_message::<_, Response>(&mut raw).unwrap(), Response::Ok);
    write_message(
        &mut raw,
        &Request::Insert {
            id: other,
            fields: vec![],
        },
    )
    .unwrap();
    assert!(matches!(
        read_message::<_, Response>(&mut raw).unwrap(),
        Response::Err {
            code: ErrorCode::SessionOpen,
            ..
        }
    ));
    write_message(
        &mut raw,
        &Request::UpdateField {
            id: Uuid::from_u128(2),
            field: FIELD_STATUS,
            value: ScanValue::U32(1),
        },
    )
    .unwrap();
    assert_eq!(
        read_message::<_, Response>(&mut raw).unwrap(),
        Response::Staged { index: 0 }
    );
    write_message(&mut raw, &Request::Commit).unwrap();
    assert_eq!(read_message::<_, Response>(&mut raw).unwrap(), Response::Ok);
    drop(raw);
    drop(client);

    // A new server process on the same directory: the fold at open
    // carries the insert.
    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got[0],
        ("title".to_string(), ScanValue::Str("Water plants".into()))
    );
    assert_eq!(got[2], ("status".to_string(), ScanValue::U32(0)));
    assert_eq!(client.get(other).unwrap(), None);
    let all = rows(client.query("SELECT title FROM reminder").unwrap());
    assert_eq!(all.len(), 4);
}

/// F2: Reminder supports every runtime record write in either batch mode.
#[test]
fn reminder_batches_apply_all_record_write_kinds() {
    let fields = |title: &str, status| {
        vec![
            ("title", ScanValue::Str(title.into())),
            ("due_at_unix_ms", ScanValue::I64(4_000)),
            ("status", ScanValue::U32(status)),
        ]
    };
    for atomic in [false, true] {
        let mut client = SchemaDrivenClient::connect(start_server()).unwrap();
        let id = Uuid::from_u128(4);
        let initial = fields("new", 0);
        let replaced = fields("replaced", 1);
        let guarded = fields("guarded", 2);
        let result = client
            .write_batch(
                &[
                    BatchOp::Insert {
                        id,
                        fields: &initial,
                    },
                    BatchOp::Insert {
                        id,
                        fields: &initial,
                    },
                    BatchOp::Replace {
                        id,
                        fields: &replaced,
                    },
                    BatchOp::ReplaceIf {
                        id,
                        fields: &guarded,
                        guard: ("status", CompareOp::Eq, ScanValue::U32(1)),
                    },
                    BatchOp::ReplaceIf {
                        id,
                        fields: &initial,
                        guard: ("status", CompareOp::Eq, ScanValue::U32(1)),
                    },
                    BatchOp::Replace {
                        id: Uuid::from_u128(99),
                        fields: &initial,
                    },
                    BatchOp::ReplaceIf {
                        id: Uuid::from_u128(99),
                        fields: &initial,
                        guard: ("status", CompareOp::Eq, ScanValue::U32(0)),
                    },
                    BatchOp::Delete {
                        id: Uuid::from_u128(3),
                    },
                    BatchOp::Delete {
                        id: Uuid::from_u128(3),
                    },
                ],
                atomic,
            )
            .unwrap();
        assert_eq!(
            result,
            vec![
                WriteResult::Inserted,
                WriteResult::Duplicate,
                WriteResult::Replaced,
                WriteResult::Replaced,
                WriteResult::GuardFailed,
                WriteResult::NotFound,
                WriteResult::NotFound,
                WriteResult::Deleted,
                WriteResult::NotFound
            ]
        );
        let stored = client.get(id).unwrap().unwrap();
        assert_eq!(stored[0].1, ScanValue::Str("guarded".into()));
        assert_eq!(stored[2].1, ScanValue::U32(2));
        assert!(client.get(Uuid::from_u128(3)).unwrap().is_none());
    }
}

/// F2: the earliest malformed op rejects an atomic batch without any writes;
/// pipelined operations before and after that failure keep their own outcomes.
#[test]
fn reminder_batch_rejection_is_atomic_and_pipelined_results_are_independent() {
    let full = vec![
        ("title", ScanValue::Str("new".into())),
        ("due_at_unix_ms", ScanValue::I64(4_000)),
        ("status", ScanValue::U32(0)),
    ];
    let short = vec![("title", ScanValue::Str("incomplete".into()))];
    let bad_status = vec![
        ("title", ScanValue::Str("bad status".into())),
        ("due_at_unix_ms", ScanValue::I64(4_000)),
        ("status", ScanValue::U32(99)),
    ];
    for invalid in [&short, &bad_status] {
        for atomic in [false, true] {
            let mut client = SchemaDrivenClient::connect(start_server()).unwrap();
            let id = Uuid::from_u128(4);
            let result = client.write_batch(
                &[
                    BatchOp::Insert { id, fields: &full },
                    BatchOp::Insert {
                        id: Uuid::from_u128(5),
                        fields: invalid,
                    },
                    BatchOp::Delete {
                        id: Uuid::from_u128(1),
                    },
                ],
                atomic,
            );
            if atomic {
                assert!(matches!(
                    result,
                    Err(ClientError::TransactionFailed {
                        index: 1,
                        code: ErrorCode::Malformed,
                        ..
                    })
                ));
            } else {
                assert_eq!(
                    result.unwrap(),
                    vec![
                        WriteResult::Inserted,
                        WriteResult::Failed(ErrorCode::Malformed),
                        WriteResult::Deleted
                    ]
                );
            }
            assert_eq!(client.get(id).unwrap().is_none(), atomic);
            assert!(client.get(Uuid::from_u128(5)).unwrap().is_none());
            assert_eq!(client.get(Uuid::from_u128(1)).unwrap().is_some(), atomic);
        }
    }
    // Send a raw guard field so client-side name resolution cannot mask server validation.
    use rusty_multimodal_db::server::protocol::{Predicate, WriteOp, PROTOCOL_VERSION};
    let mut wire = TcpStream::connect(start_server()).unwrap();
    write_message(
        &mut wire,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .unwrap();
    let _: Response = read_message(&mut wire).unwrap();
    let id = Uuid::from_u128(1);
    let fields = vec![
        (0, ScanValue::Str("new".into())),
        (1, ScanValue::I64(4_000)),
        (2, ScanValue::U32(0)),
    ];
    let guard = Predicate {
        field: u16::MAX,
        op: CompareOp::Eq,
        value: ScanValue::U32(0),
    };
    write_message(
        &mut wire,
        &Request::ReplaceIf {
            id,
            fields: fields.clone(),
            guard: guard.clone(),
        },
    )
    .unwrap();
    assert!(matches!(
        read_message::<_, Response>(&mut wire).unwrap(),
        Response::Err {
            code: ErrorCode::UnknownField,
            ..
        }
    ));
    write_message(
        &mut wire,
        &Request::WriteBatch {
            ops: vec![WriteOp::ReplaceIf { id, fields, guard }],
            atomic: false,
        },
    )
    .unwrap();
    assert_eq!(
        read_message::<_, Response>(&mut wire).unwrap(),
        Response::BatchResults {
            results: vec![WriteResult::Failed(ErrorCode::UnknownField)]
        }
    );
}
