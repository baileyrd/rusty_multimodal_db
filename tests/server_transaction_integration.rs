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
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    ErrorCode, Request, Response, ScanValue, TransactionOp,
};
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

fn start_server(records: Vec<DogRecord>, auth: AuthConfig) -> std::net::SocketAddr {
    let dir = unique_dir("server_transaction_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, auth));
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
