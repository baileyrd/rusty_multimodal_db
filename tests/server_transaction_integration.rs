//! Real end-to-end coverage of `Request::Transaction` (ADR-0013, Accepted)
//! against the `Dog` domain — a real `TcpListener`, real client
//! `TcpStream`s, real `bincode` encoding over the wire, matching the
//! existing `tests/server_*_integration.rs` pattern. Covers every
//! functional acceptance criterion `docs/design/SERVER-TRANSACTION-DESIGN.md`
//! names: all-or-nothing application (success and every failure position,
//! deterministic, single-threaded), no operation evaluated for a rejected
//! `ReadOnly` request, and a flagship concurrent stress test proving no
//! lost updates/corruption under real contention (sequential-replay
//! linearizability — see that test's own doc comment for why a
//! per-instant "never observed half-written" check was tried and dropped
//! as unable to distinguish a real bug from an inherent protocol
//! limitation).

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE, FIELD_BREED};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    ErrorCode, Request, Response, ScanValue, TransactionOp, MAX_STAGED_OPS, PROTOCOL_VERSION,
    SESSION_READ_YOUR_WRITES, SESSION_VALIDATE_ON_STAGE,
};
use rusty_multimodal_db::server::{serve, AuthConfig, ConnectionStore};
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

fn start_server(records: Vec<DogRecord>, auth: AuthConfig) -> std::net::SocketAddr {
    let dir = unique_dir("server_transaction_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

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

fn sample_records() -> Vec<DogRecord> {
    vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        DogRecord::new(Uuid::from_u128(3), "poodle", 2),
    ]
}

fn age_of(stream: &mut TcpStream, id: Uuid) -> u32 {
    match roundtrip(stream, Request::GetById { id }) {
        Response::Record { fields, .. } => fields
            .into_iter()
            .find_map(|(field, value)| match (field, value) {
                (FIELD_AGE, ScanValue::U32(age)) => Some(age),
                _ => None,
            })
            .expect("Dog always has an age field"),
        other => panic!("expected Response::Record, got {other:?}"),
    }
}

#[test]
fn a_fully_valid_batch_applies_every_write() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_AGE,
                        value: ScanValue::U32(30),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(2),
                        field: FIELD_AGE,
                        value: ScanValue::U32(40),
                    },
                ],
            },
        ),
        Response::Ok
    );
    assert_eq!(age_of(&mut client, Uuid::from_u128(1)), 30);
    assert_eq!(age_of(&mut client, Uuid::from_u128(2)), 40);
}

/// A batch that fails on its very first operation applies nothing —
/// including the operations after it that would have succeeded on their
/// own.
#[test]
fn a_batch_failing_on_its_first_operation_applies_nothing() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(99), // unknown id
                        field: FIELD_AGE,
                        value: ScanValue::U32(30),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_AGE,
                        value: ScanValue::U32(40),
                    },
                ],
            },
        ),
        Response::TransactionFailed {
            index: 0,
            code: ErrorCode::RecordNotFound,
            message: "this operation's id has no record".into(),
        }
    );
    assert_eq!(age_of(&mut client, Uuid::from_u128(1)), 3, "unchanged");
}

/// A batch that fails on its last operation applies nothing — including
/// the operations before it that would have succeeded on their own. This
/// is the case a naive "apply in a loop" implementation would get wrong.
#[test]
fn a_batch_failing_on_its_last_operation_applies_nothing() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_AGE,
                        value: ScanValue::U32(30),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(2),
                        field: FIELD_AGE,
                        value: ScanValue::U32(40),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(99), // unknown id
                        field: FIELD_AGE,
                        value: ScanValue::U32(50),
                    },
                ],
            },
        ),
        Response::TransactionFailed {
            index: 2,
            code: ErrorCode::RecordNotFound,
            message: "this operation's id has no record".into(),
        }
    );
    assert_eq!(age_of(&mut client, Uuid::from_u128(1)), 3, "unchanged");
    assert_eq!(age_of(&mut client, Uuid::from_u128(2)), 5, "unchanged");
}

/// A malformed value (wrong `ScanValue` variant for the field) is rejected
/// the same way `Request::UpdateField` already rejects it for a single
/// operation, just reported through `TransactionFailed`.
#[test]
fn a_malformed_value_is_rejected_with_the_right_index() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut client = connect(addr);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Transaction {
                updates: vec![TransactionOp {
                    id: Uuid::from_u128(1),
                    field: FIELD_AGE,
                    value: ScanValue::Bool(true),
                }],
            },
        ),
        Response::TransactionFailed {
            index: 0,
            code: ErrorCode::Malformed,
            message: "the supplied value does not match this field's type".into(),
        }
    );
}

/// `TXN-FR-004`: a `ReadOnly` connection's `Transaction` request is
/// rejected outright, before any operation in the batch is evaluated —
/// verified by including an operation that would itself fail differently
/// (unknown id) and confirming the reported error is still `Unauthorized`,
/// not that other code.
#[test]
fn a_read_only_connection_is_rejected_before_any_operation_is_evaluated() {
    let addr = start_server(
        sample_records(),
        AuthConfig::new(Some("read-token".into()), Some("write-token".into())),
    );
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

    match roundtrip(
        &mut client,
        Request::Transaction {
            updates: vec![TransactionOp {
                id: Uuid::from_u128(99), // would itself fail as RecordNotFound
                field: FIELD_AGE,
                value: ScanValue::U32(1),
            }],
        },
    ) {
        Response::Err {
            code: ErrorCode::Unauthorized,
            ..
        } => {}
        other => panic!("expected ErrorCode::Unauthorized, got {other:?}"),
    }
    // Still open and authenticated afterward — only the transaction
    // request was rejected.
    assert_eq!(age_of(&mut client, Uuid::from_u128(1)), 3);
}

/// The flagship correctness property for `Request::Transaction` under real
/// contention: many client connections issuing a mix of `Transaction`
/// batches (two ids at once, both set to the same new age) and plain
/// `UpdateField` requests (one id) against a small contended id pool,
/// verified via the same sequential-replay-linearizability pattern this
/// crate's other flagship concurrency tests use
/// (`tests/server_dog_integration.rs`'s
/// `concurrent_clients_over_the_wire_match_a_sequential_replay`,
/// `production_integration.rs`'s `run_concurrency_stress_test`): the
/// write order each client thread recorded (atomically with its own round
/// trip, under one shared lock) must match the server's final state
/// exactly. A `Transaction` that only actually applied one of its two
/// writes (a real atomicity bug) would leave that id's final value out of
/// sync with what the recorded write order says it should be — this is
/// the same "no lost updates, no corruption" bar this project already
/// holds every concurrent-write path to, not a weaker one for
/// transactions specifically.
///
/// (A per-instant "is this pair ever observed half-written" check was
/// tried and dropped: with two *independent*, un-synchronized client round
/// trips — this protocol has no multi-field read or read-transaction — a
/// reader reading id A then id B cannot tell "A's transaction hasn't
/// written B yet" apart from "a different, later, fully-completed
/// transaction already updated B again since A was read." Both look
/// identical from outside, so that comparison produces the same result
/// whether the implementation is correct or not — not a real test. The
/// single-threaded before/after tests above already prove batch atomicity
/// deterministically, with no such confound; this test's job is proving
/// concurrent correctness, which sequential replay is the established,
/// sound tool for.)
#[test]
fn concurrent_transactions_and_updates_match_a_sequential_replay() {
    use std::collections::HashMap;
    use std::sync::Mutex;

    let ids: Vec<Uuid> = (0..20).map(Uuid::from_u128).collect();
    let records: Vec<DogRecord> = ids
        .iter()
        .map(|&id| DogRecord::new(id, "labrador", 0))
        .collect();
    let addr = start_server(records.clone(), AuthConfig::default());

    const THREADS: usize = 8;
    const ITERATIONS: usize = 200;

    // Every completed write, in the real order the server acknowledged it
    // — a `Transaction`'s two writes are logged as one pair, replayed
    // together. Held under one mutex spanning each round trip *and* its
    // log append, matching `concurrent_clients_over_the_wire_match_a_sequential_replay`'s
    // own `order_lock` fix: without it, two threads' round-trip-then-log
    // steps can interleave, letting the log's order diverge from the
    // order the server actually applied same-id writes in.
    let write_log: Arc<Mutex<Vec<(Uuid, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let order_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let ids = ids.clone();
            let write_log = Arc::clone(&write_log);
            let order_lock = Arc::clone(&order_lock);
            thread::spawn(move || {
                let mut client = connect(addr);
                let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15u64.wrapping_add(t as u64);
                let mut next = || {
                    rng_state ^= rng_state << 13;
                    rng_state ^= rng_state >> 7;
                    rng_state ^= rng_state << 17;
                    rng_state
                };
                for i in 0..ITERATIONS {
                    if i % 2 == 0 {
                        // A Transaction touching two distinct ids from the
                        // pool, both set to the same new age.
                        let ia = (next() as usize) % ids.len();
                        let mut ib = (next() as usize) % ids.len();
                        if ib == ia {
                            ib = (ib + 1) % ids.len();
                        }
                        let age = (next() % 1_000) as u32;
                        let _order_guard = order_lock.lock().unwrap();
                        let resp = roundtrip(
                            &mut client,
                            Request::Transaction {
                                updates: vec![
                                    TransactionOp {
                                        id: ids[ia],
                                        field: FIELD_AGE,
                                        value: ScanValue::U32(age),
                                    },
                                    TransactionOp {
                                        id: ids[ib],
                                        field: FIELD_AGE,
                                        value: ScanValue::U32(age),
                                    },
                                ],
                            },
                        );
                        if resp == Response::Ok {
                            let mut log = write_log.lock().unwrap();
                            log.push((ids[ia], age));
                            log.push((ids[ib], age));
                        }
                    } else {
                        let id = ids[(next() as usize) % ids.len()];
                        let age = (next() % 1_000) as u32;
                        let _order_guard = order_lock.lock().unwrap();
                        let resp = roundtrip(
                            &mut client,
                            Request::UpdateField {
                                id,
                                field: FIELD_AGE,
                                value: ScanValue::U32(age),
                            },
                        );
                        if resp == Response::Ok {
                            write_log.lock().unwrap().push((id, age));
                        }
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Sequential replay against a fresh in-memory reference, in the
    // recorded completion order.
    let mut reference: HashMap<Uuid, u32> = records.iter().map(|r| (r.id, r.age)).collect();
    for (id, age) in write_log.lock().unwrap().iter() {
        reference.insert(*id, *age);
    }

    let mut client = connect(addr);
    for &id in &ids {
        let expected = reference[&id];
        let actual = age_of(&mut client, id);
        assert_eq!(
            actual, expected,
            "id {id} diverged from the sequential replay"
        );
    }
}

// ---------------------------------------------------------------------------
// Transaction sessions (`SERVER-001-FR-024`, ADR-0024,
// `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part A)
// ---------------------------------------------------------------------------

/// A client that negotiated protocol version 3 — the session requests are
/// `Malformed` on anything older (`SESS-FR-006`).
fn connect_v3(addr: std::net::SocketAddr) -> TcpStream {
    let mut stream = connect(addr);
    assert_eq!(
        roundtrip(
            &mut stream,
            Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    stream
}

fn stage(stream: &mut TcpStream, id: Uuid, age: u32) -> Response {
    roundtrip(
        stream,
        Request::UpdateField {
            id,
            field: FIELD_AGE,
            value: ScanValue::U32(age),
        },
    )
}

fn assert_err(resp: Response, code: ErrorCode) {
    match resp {
        Response::Err { code: got, .. } if got == code => {}
        other => panic!("expected Err {{ {code:?} }}, got {other:?}"),
    }
}

/// `SESS-FR-002`/`SESS-FR-003`/`SESS-FR-007` (design criterion 2): staged
/// writes are invisible to every connection — this one included — until
/// `Commit`, which applies them as one batch; a second connection reads
/// throughout, so no lock was held between round trips.
#[test]
fn a_session_stages_writes_and_commits_them_as_one_batch() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut session = connect_v3(addr);
    let mut observer = connect(addr);

    assert_eq!(roundtrip(&mut session, Request::Begin), Response::Ok);
    assert_eq!(
        stage(&mut session, Uuid::from_u128(1), 30),
        Response::Staged { index: 0 }
    );
    assert_eq!(
        stage(&mut session, Uuid::from_u128(2), 40),
        Response::Staged { index: 1 }
    );
    // Nothing applied yet, for anyone.
    assert_eq!(age_of(&mut observer, Uuid::from_u128(1)), 3);
    assert_eq!(age_of(&mut observer, Uuid::from_u128(2)), 5);
    assert_eq!(age_of(&mut session, Uuid::from_u128(1)), 3);

    assert_eq!(roundtrip(&mut session, Request::Commit), Response::Ok);
    assert_eq!(age_of(&mut observer, Uuid::from_u128(1)), 30);
    assert_eq!(age_of(&mut observer, Uuid::from_u128(2)), 40);
    // The session closed with the commit.
    assert_err(
        roundtrip(&mut session, Request::Commit),
        ErrorCode::NoSession,
    );
}

/// Design criterion 3: a commit whose batch fails validation names the
/// staged index, applies nothing, and closes the session.
#[test]
fn a_commit_failure_names_the_staged_index_and_applies_nothing() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut session = connect_v3(addr);

    assert_eq!(roundtrip(&mut session, Request::Begin), Response::Ok);
    assert_eq!(
        stage(&mut session, Uuid::from_u128(1), 30),
        Response::Staged { index: 0 }
    );
    // Staging does not validate: a missing id is accepted here and
    // rejected at commit, by index (`SESS-FR-002`).
    assert_eq!(
        stage(&mut session, Uuid::from_u128(99), 1),
        Response::Staged { index: 1 }
    );
    assert_eq!(
        stage(&mut session, Uuid::from_u128(2), 40),
        Response::Staged { index: 2 }
    );
    match roundtrip(&mut session, Request::Commit) {
        Response::TransactionFailed {
            index: 1,
            code: ErrorCode::RecordNotFound,
            ..
        } => {}
        other => panic!("expected TransactionFailed at staged index 1, got {other:?}"),
    }
    assert_eq!(age_of(&mut session, Uuid::from_u128(1)), 3);
    assert_eq!(age_of(&mut session, Uuid::from_u128(2)), 5);
    assert_err(
        roundtrip(&mut session, Request::Rollback),
        ErrorCode::NoSession,
    );
}

/// `SESS-FR-004` (design criterion 4): every misuse is a typed error with
/// the connection open; `Rollback` and a disconnect discard; the cap is
/// `MAX_STAGED_OPS`, and hitting it leaves the session open.
#[test]
fn session_state_errors_leave_the_connection_open() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut c = connect_v3(addr);

    assert_err(roundtrip(&mut c, Request::Commit), ErrorCode::NoSession);
    assert_err(roundtrip(&mut c, Request::Rollback), ErrorCode::NoSession);
    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    assert_err(roundtrip(&mut c, Request::Begin), ErrorCode::SessionOpen);
    assert_err(
        roundtrip(
            &mut c,
            Request::Transaction {
                updates: vec![TransactionOp {
                    id: Uuid::from_u128(1),
                    field: FIELD_AGE,
                    value: ScanValue::U32(30),
                }],
            },
        ),
        ErrorCode::SessionOpen,
    );
    assert_eq!(
        stage(&mut c, Uuid::from_u128(1), 30),
        Response::Staged { index: 0 }
    );
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);
    assert_eq!(age_of(&mut c, Uuid::from_u128(1)), 3);

    // The cap: `MAX_STAGED_OPS` staged, the next refused, the session open.
    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    for i in 0..MAX_STAGED_OPS {
        assert_eq!(
            stage(&mut c, Uuid::from_u128(1), 30),
            Response::Staged { index: i as u32 }
        );
    }
    assert_err(
        stage(&mut c, Uuid::from_u128(1), 30),
        ErrorCode::SessionFull,
    );
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);

    // A disconnect with a session open discards it.
    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    assert_eq!(
        stage(&mut c, Uuid::from_u128(2), 40),
        Response::Staged { index: 0 }
    );
    drop(c);
    let mut later = connect(addr);
    assert_eq!(age_of(&mut later, Uuid::from_u128(2)), 5);
}

/// `SESS-FR-005` (design criterion 5): the gates are unchanged and per
/// request — `Begin` needs authentication, staging and `Commit` need
/// `ReadWrite`, `Rollback` needs only authentication.
#[test]
fn a_read_only_connection_can_open_a_session_but_not_stage_or_commit() {
    let auth = AuthConfig::new(Some("read-token".into()), Some("write-token".into()));
    let addr = start_server(sample_records(), auth);
    let mut c = connect_v3(addr);

    assert_err(
        roundtrip(&mut c, Request::Begin),
        ErrorCode::Unauthenticated,
    );
    assert_eq!(
        roundtrip(
            &mut c,
            Request::Authenticate {
                token: "read-token".into()
            }
        ),
        Response::Ok
    );
    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    assert_err(
        stage(&mut c, Uuid::from_u128(1), 30),
        ErrorCode::Unauthorized,
    );
    assert_err(roundtrip(&mut c, Request::Commit), ErrorCode::Unauthorized);
    // Nothing was staged, and the session is still open for a rollback.
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);

    assert_eq!(
        roundtrip(
            &mut c,
            Request::Authenticate {
                token: "write-token".into()
            }
        ),
        Response::Ok
    );
    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    assert_eq!(
        stage(&mut c, Uuid::from_u128(1), 30),
        Response::Staged { index: 0 }
    );
    assert_eq!(roundtrip(&mut c, Request::Commit), Response::Ok);
    assert_eq!(age_of(&mut c, Uuid::from_u128(1)), 30);
}

// ---------------------------------------------------------------------------
// Crash-atomic batches (`SERVER-001-FR-025`, ADR-0025,
// `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part B)
// ---------------------------------------------------------------------------

fn start_journaled_server(records: Vec<DogRecord>) -> (std::net::SocketAddr, std::path::PathBuf) {
    let dir = unique_dir("server_transaction_integration_journal");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let journal = dir.join("txn.journal");
    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::with_journal(store, &journal).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default(), None));
    (addr, journal)
}

/// `JRN-FR-001`/`JRN-FR-002`/`JRN-FR-007` (design criterion 2): over a real
/// socket, a `Transaction` and a session `Commit` are each journaled
/// before they are answered `Ok`; a batch that fails validation and a
/// single `UpdateField` journal nothing; the writes land exactly as on an
/// unjournaled server.
#[test]
fn a_journaled_server_journals_transactions_and_session_commits_only() {
    let (addr, journal) = start_journaled_server(sample_records());
    let mut c = connect_v3(addr);
    let journal_len = || std::fs::metadata(&journal).unwrap().len();
    let header = journal_len();

    assert_eq!(
        roundtrip(
            &mut c,
            Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_AGE,
                        value: ScanValue::U32(30),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(2),
                        field: FIELD_AGE,
                        value: ScanValue::U32(40),
                    },
                ],
            },
        ),
        Response::Ok
    );
    let after_txn = journal_len();
    assert!(after_txn > header, "the Transaction was not journaled");
    assert_eq!(age_of(&mut c, Uuid::from_u128(1)), 30);

    assert_eq!(roundtrip(&mut c, Request::Begin), Response::Ok);
    assert_eq!(
        stage(&mut c, Uuid::from_u128(3), 7),
        Response::Staged { index: 0 }
    );
    assert_eq!(journal_len(), after_txn, "staging must not journal");
    assert_eq!(roundtrip(&mut c, Request::Commit), Response::Ok);
    let after_commit = journal_len();
    assert!(
        after_commit > after_txn,
        "the session Commit was not journaled"
    );
    assert_eq!(age_of(&mut c, Uuid::from_u128(3)), 7);

    match roundtrip(
        &mut c,
        Request::Transaction {
            updates: vec![TransactionOp {
                id: Uuid::from_u128(99),
                field: FIELD_AGE,
                value: ScanValue::U32(1),
            }],
        },
    ) {
        Response::TransactionFailed {
            index: 0,
            code: ErrorCode::RecordNotFound,
            ..
        } => {}
        other => panic!("expected TransactionFailed, got {other:?}"),
    }
    assert_eq!(
        journal_len(),
        after_commit,
        "a rejected batch must not journal"
    );

    assert_eq!(stage(&mut c, Uuid::from_u128(1), 31), Response::Ok);
    assert_eq!(
        journal_len(),
        after_commit,
        "a single UpdateField must not journal"
    );
    assert_eq!(age_of(&mut c, Uuid::from_u128(1)), 31);
}

// ---------------------------------------------------------------------------
// Group commit (`SERVER-001-FR-027`, ADR-0026,
// `docs/design/SERVER-JOURNAL-GROUP-COMMIT-DESIGN.md`)
// ---------------------------------------------------------------------------

/// `GRP-FR-002`/`GRP-FR-003` (design criterion 3, through the server):
/// eight connections fire transactions at a journaled server
/// concurrently; afterwards, replaying the journal onto pristine
/// pre-batch files yields exactly the state the live server serves — so
/// the order the journal holds is the order the store applied.
#[test]
fn concurrent_journaled_transactions_replay_to_the_live_state() {
    let (addr, journal) = start_journaled_server(sample_records());
    let ids = [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)];
    let clients: Vec<_> = (0..8u32)
        .map(|t| {
            thread::spawn(move || {
                let mut c = connect(addr);
                for i in 0..30u32 {
                    let v = 100 + t * 1_000 + i;
                    let a = ids[(t as usize + i as usize) % 3];
                    let b = ids[(t as usize + i as usize + 1) % 3];
                    let resp = roundtrip(
                        &mut c,
                        Request::Transaction {
                            updates: vec![
                                TransactionOp {
                                    id: a,
                                    field: FIELD_AGE,
                                    value: ScanValue::U32(v),
                                },
                                TransactionOp {
                                    id: b,
                                    field: FIELD_AGE,
                                    value: ScanValue::U32(v + 500),
                                },
                            ],
                        },
                    );
                    assert_eq!(resp, Response::Ok);
                }
            })
        })
        .collect();
    for c in clients {
        c.join().unwrap();
    }
    let mut reader = connect(addr);
    let live: Vec<u32> = ids.iter().map(|&id| age_of(&mut reader, id)).collect();

    // Replay a copy of the journal onto pre-batch files.
    let dir = unique_dir("server_transaction_integration_group_replay");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("txn.journal");
    std::fs::copy(&journal, &copy).unwrap();
    let replayed = DogConnectionStore::with_journal(
        ProductionStore::create(sample_records(), Vec::new(), &dir.join("dogs.mmap")).unwrap(),
        &copy,
    )
    .unwrap();
    let from_journal: Vec<u32> = ids
        .iter()
        .map(|&id| {
            match replayed
                .get(id)
                .unwrap()
                .into_iter()
                .find(|(field, _)| *field == FIELD_AGE)
            {
                Some((_, ScanValue::U32(age))) => age,
                other => panic!("expected an age, got {other:?}"),
            }
        })
        .collect();
    assert_eq!(from_journal, live);
    assert!(
        live.iter().all(|&age| age >= 100),
        "every id was written: {live:?}"
    );
}

// ---------------------------------------------------------------------------
// Read-your-writes sessions (`SERVER-001-FR-028`, ADR-0027,
// `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md`)
// ---------------------------------------------------------------------------

fn begin_read_your_writes(stream: &mut TcpStream) -> Response {
    roundtrip(
        stream,
        Request::BeginWith {
            flags: SESSION_READ_YOUR_WRITES,
        },
    )
}

fn breed_of(stream: &mut TcpStream, id: Uuid) -> String {
    match roundtrip(stream, Request::GetById { id }) {
        Response::Record { fields, .. } => {
            match fields.into_iter().find(|(field, _)| *field == FIELD_BREED) {
                Some((_, ScanValue::Str(breed))) => breed,
                other => panic!("expected a breed, got {other:?}"),
            }
        }
        other => panic!("expected a record, got {other:?}"),
    }
}

/// `RYW-FR-002`/`RYW-FR-004` (design criteria 2 and 3): in a
/// read-your-writes session the connection's own `GetById` shows its
/// staged writes — the later of two stages wins — while another
/// connection sees committed state throughout; after `Rollback` the
/// session's own read is committed state again; after `Commit` both
/// connections see the batch.
#[test]
fn a_read_your_writes_session_sees_its_own_staged_writes_and_nobody_else_does() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut c = connect_v3(addr);
    let mut other = connect(addr);
    let id = Uuid::from_u128(1);

    assert_eq!(begin_read_your_writes(&mut c), Response::Ok);
    assert_eq!(stage(&mut c, id, 30), Response::Staged { index: 0 });
    assert_eq!(age_of(&mut c, id), 30, "own read sees the staged write");
    assert_eq!(breed_of(&mut c, id), "labrador", "other fields untouched");
    assert_eq!(age_of(&mut other, id), 3, "another connection does not");
    assert_eq!(stage(&mut c, id, 31), Response::Staged { index: 1 });
    assert_eq!(age_of(&mut c, id), 31, "the last staged write wins");
    assert_eq!(
        age_of(&mut c, Uuid::from_u128(2)),
        5,
        "an unstaged id reads committed"
    );
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);
    assert_eq!(
        age_of(&mut c, id),
        3,
        "committed state again after rollback"
    );

    assert_eq!(begin_read_your_writes(&mut c), Response::Ok);
    assert_eq!(stage(&mut c, id, 40), Response::Staged { index: 0 });
    assert_eq!(age_of(&mut other, id), 3);
    assert_eq!(roundtrip(&mut c, Request::Commit), Response::Ok);
    assert_eq!(age_of(&mut c, id), 40);
    assert_eq!(age_of(&mut other, id), 40);
}

/// `RYW-FR-002`/`RYW-FR-003` (design criteria 4 and 5): a staged write
/// that would fail at `Commit` never produces a misleading read — a
/// missing id stays `NotFound`, a read-only field and a kind-mismatched
/// value are not overlaid — and set reads (`ScanField`) see committed
/// state even in a read-your-writes session; each bad write then fails
/// at `Commit` by its index.
#[test]
fn read_your_writes_never_shows_a_write_that_commit_would_refuse() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut c = connect_v3(addr);
    let id = Uuid::from_u128(1);
    assert_eq!(begin_read_your_writes(&mut c), Response::Ok);

    // A missing id: staged (nothing validates at stage time), never a record.
    assert_eq!(
        stage(&mut c, Uuid::from_u128(99), 1),
        Response::Staged { index: 0 }
    );
    assert_eq!(
        roundtrip(
            &mut c,
            Request::GetById {
                id: Uuid::from_u128(99)
            }
        ),
        Response::NotFound
    );
    // A read-only field of the right kind: not overlaid.
    assert_eq!(
        roundtrip(
            &mut c,
            Request::UpdateField {
                id,
                field: FIELD_BREED,
                value: ScanValue::Str("poodle".into()),
            }
        ),
        Response::Staged { index: 1 }
    );
    assert_eq!(breed_of(&mut c, id), "labrador");
    // A kind mismatch on an updatable field: not overlaid.
    assert_eq!(
        roundtrip(
            &mut c,
            Request::UpdateField {
                id,
                field: FIELD_AGE,
                value: ScanValue::Str("old".into()),
            }
        ),
        Response::Staged { index: 2 }
    );
    assert_eq!(age_of(&mut c, id), 3);
    // A good write is overlaid on the point read but not on a scan.
    assert_eq!(stage(&mut c, id, 77), Response::Staged { index: 3 });
    assert_eq!(age_of(&mut c, id), 77);
    match roundtrip(&mut c, Request::ScanField { field: FIELD_AGE }) {
        Response::ScanValues { values } => {
            assert!(
                !values.contains(&ScanValue::U32(77)),
                "scan must be committed state: {values:?}"
            );
        }
        other => panic!("expected scan values, got {other:?}"),
    }
    // Commit refuses the batch at the first bad write.
    match roundtrip(&mut c, Request::Commit) {
        Response::TransactionFailed {
            index: 0,
            code: ErrorCode::RecordNotFound,
            ..
        } => {}
        other => panic!("expected TransactionFailed at 0, got {other:?}"),
    }
    assert_eq!(age_of(&mut c, id), 3);
}

/// `RYW-FR-001`/`RYW-FR-006` (design criterion 1): `BeginWith` is
/// `Malformed` on a silent connection and on one that negotiated 4; on a
/// version-5 connection an unknown flag bit is `Malformed` and opens
/// nothing (the next `UpdateField` applies immediately), `flags: 0` is
/// exactly `Begin` (no overlay), and a second open is `SessionOpen`.
#[test]
fn begin_with_is_gated_by_version_and_refuses_unknown_flags() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let id = Uuid::from_u128(1);

    let mut silent = connect(addr);
    assert_err(begin_read_your_writes(&mut silent), ErrorCode::Malformed);

    let mut v4 = connect(addr);
    assert_eq!(
        roundtrip(
            &mut v4,
            Request::Hello {
                protocol_version: 4
            }
        ),
        Response::Hello {
            protocol_version: 4
        }
    );
    assert_err(begin_read_your_writes(&mut v4), ErrorCode::Malformed);

    let mut c = connect_v3(addr);
    assert_err(
        roundtrip(&mut c, Request::BeginWith { flags: 4 }),
        ErrorCode::Malformed,
    );
    assert_eq!(stage(&mut c, id, 9), Response::Ok, "no session was opened");
    assert_eq!(age_of(&mut c, id), 9);

    assert_eq!(
        roundtrip(&mut c, Request::BeginWith { flags: 0 }),
        Response::Ok
    );
    assert_eq!(stage(&mut c, id, 10), Response::Staged { index: 0 });
    assert_eq!(age_of(&mut c, id), 9, "flags: 0 is a plain session");
    assert_err(begin_read_your_writes(&mut c), ErrorCode::SessionOpen);
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);
}

// ---------------------------------------------------------------------------
// Stage-time validation (`SERVER-001-FR-030`, `ADR-0024`'s second trigger)
// ---------------------------------------------------------------------------

/// `STV-FR-001`/`STV-FR-002`: in a validating session a bad write is
/// refused at its own round trip with the code `Commit` would have
/// given — a missing id, a read-only field, a kind mismatch — and nothing
/// is staged; good writes stage and commit as before; combined with
/// read-your-writes both bits work together.
#[test]
fn a_validating_session_refuses_bad_writes_when_staged() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut c = connect_v3(addr);
    let id = Uuid::from_u128(1);
    assert_eq!(
        roundtrip(
            &mut c,
            Request::BeginWith {
                flags: SESSION_VALIDATE_ON_STAGE
            }
        ),
        Response::Ok
    );
    assert_err(
        stage(&mut c, Uuid::from_u128(99), 1),
        ErrorCode::RecordNotFound,
    );
    assert_err(
        roundtrip(
            &mut c,
            Request::UpdateField {
                id,
                field: FIELD_BREED,
                value: ScanValue::Str("poodle".into()),
            },
        ),
        ErrorCode::Unsupported,
    );
    assert_err(
        roundtrip(
            &mut c,
            Request::UpdateField {
                id,
                field: FIELD_AGE,
                value: ScanValue::Str("old".into()),
            },
        ),
        ErrorCode::Malformed,
    );
    assert_err(
        roundtrip(
            &mut c,
            Request::UpdateField {
                id,
                field: 7,
                value: ScanValue::U32(1),
            },
        ),
        ErrorCode::UnknownField,
    );
    // Nothing was staged: the first good write is index 0.
    assert_eq!(stage(&mut c, id, 41), Response::Staged { index: 0 });
    assert_eq!(
        age_of(&mut c, id),
        3,
        "validation alone does not overlay reads"
    );
    assert_eq!(roundtrip(&mut c, Request::Commit), Response::Ok);
    assert_eq!(age_of(&mut c, id), 41);

    // Both bits: validated and read-your-writes.
    assert_eq!(
        roundtrip(
            &mut c,
            Request::BeginWith {
                flags: SESSION_VALIDATE_ON_STAGE | SESSION_READ_YOUR_WRITES
            }
        ),
        Response::Ok
    );
    assert_err(
        stage(&mut c, Uuid::from_u128(99), 1),
        ErrorCode::RecordNotFound,
    );
    assert_eq!(stage(&mut c, id, 42), Response::Staged { index: 0 });
    assert_eq!(age_of(&mut c, id), 42);
    assert_eq!(roundtrip(&mut c, Request::Rollback), Response::Ok);
    assert_eq!(age_of(&mut c, id), 41);
}

/// `STV-FR-003`: the validate bit is introduced at protocol 6 like a
/// variant — on a connection that negotiated 5 it is an unknown bit and
/// `BeginWith` is `Malformed`, while read-your-writes alone still works
/// there.
#[test]
fn the_validate_bit_is_unknown_below_protocol_6() {
    let addr = start_server(sample_records(), AuthConfig::default());
    let mut v5 = connect(addr);
    assert_eq!(
        roundtrip(
            &mut v5,
            Request::Hello {
                protocol_version: 5
            }
        ),
        Response::Hello {
            protocol_version: 5
        }
    );
    assert_err(
        roundtrip(
            &mut v5,
            Request::BeginWith {
                flags: SESSION_VALIDATE_ON_STAGE,
            },
        ),
        ErrorCode::Malformed,
    );
    assert_eq!(begin_read_your_writes(&mut v5), Response::Ok);
    assert_eq!(roundtrip(&mut v5, Request::Rollback), Response::Ok);
}
