//! Real end-to-end coverage of the server/query layer (ADR-0010, Accepted)
//! against the `Dog` domain: a real `TcpListener`, a real client
//! `TcpStream` in a separate thread from the one running `serve`, real
//! `bincode` encoding/decoding over the wire — not just `dispatch`'s
//! in-process logic (that's covered by `src/server/mod.rs`'s own unit
//! tests). This uses a background thread with a real socket, not a
//! genuinely separate OS process (unlike the crash-safety/multiprocess
//! harnesses in `src/bin/`, which need a real process boundary for
//! `SIGKILL` semantics this test doesn't need) — sufficient for protocol
//! and concurrency correctness, not a claim of process-level isolation.

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE, FIELD_BREED};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{Request, Response, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig};
use rusty_multimodal_db::ProductionStore;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// A monotonic counter added to the process id — `cargo test` runs the
/// `#[test]` functions in one binary concurrently by default, and two
/// tests both calling `start_server()` need genuinely distinct backing
/// files, not just distinct per-process ones (confirmed directly: without
/// this, two tests sharing one mmap-backed path raced and one test
/// observed the other's writes, an intermittent test-isolation bug, not a
/// server bug).
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn start_server() -> std::net::SocketAddr {
    let dir = unique_dir("server_dog_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let records = vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
    ];
    let edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
    let store = ProductionStore::create(records, edges, &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default()));
    addr
}

/// Connects and disables Nagle's algorithm — this is a synchronous
/// request/response protocol (see `src/server/mod.rs`'s own comment on
/// why `handle_connection` does the same on the server side); leaving it
/// enabled on the client side too turns every round trip into a ~40ms
/// Nagle/delayed-ACK stall, confirmed directly while writing this test.
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
fn a_real_client_gets_filters_scans_and_updates_over_the_wire() {
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
                (FIELD_BREED, ScanValue::Str("labrador".into())),
                (FIELD_AGE, ScanValue::U32(3)),
            ],
        }
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(99)
            }
        ),
        Response::NotFound
    );

    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(9)
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
                (FIELD_BREED, ScanValue::Str("labrador".into())),
                (FIELD_AGE, ScanValue::U32(9)),
            ],
        }
    );

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Neighbors {
                id: Uuid::from_u128(1)
            }
        ),
        Response::RecordList {
            records: vec![Uuid::from_u128(2)]
        }
    );

    // Dog has no directed relation — a typed error, not a panic or a
    // silently wrong answer.
    match roundtrip(
        &mut client,
        Request::Parent {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::Err { .. } => {}
        other => panic!("expected Parent on Dog to report an error, got {other:?}"),
    }
}

/// A genuinely schema-driven round trip (ADR-0011): the client starts with
/// zero compile-time knowledge of `Dog`'s field tags — it discovers them
/// from `DescribeSchema`'s field *names*, then uses the discovered tag to
/// drive `UpdateField`/`GetById`. Not just checking `Response::Schema`'s
/// static content (that's `server::dog::tests::describe_names_both_fields_and_reports_neighbors_only`)
/// — this proves discovery is actually usable to complete a real request.
#[test]
fn a_schema_driven_client_discovers_and_uses_the_age_field() {
    let addr = start_server();
    let mut client = connect(addr);

    let schema = match roundtrip(&mut client, Request::DescribeSchema) {
        Response::Schema(schema) => schema,
        other => panic!("expected Response::Schema, got {other:?}"),
    };
    assert!(schema.relations.neighbors);
    assert!(!schema.relations.parent_children);

    let age_field = schema
        .fields
        .iter()
        .find(|f| f.name == "age")
        .expect("DescribeSchema should name an \"age\" field");
    assert!(age_field.capabilities.scan && age_field.capabilities.update);

    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: age_field.tag,
                value: ScanValue::U32(11),
            }
        ),
        Response::Ok
    );

    let breed_field = schema
        .fields
        .iter()
        .find(|f| f.name == "breed")
        .expect("DescribeSchema should name a \"breed\" field");
    match roundtrip(
        &mut client,
        Request::GetById {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::Record { fields, .. } => {
            let age_value = fields
                .iter()
                .find(|(tag, _)| *tag == age_field.tag)
                .map(|(_, v)| v.clone());
            assert_eq!(age_value, Some(ScanValue::U32(11)));
            let breed_value = fields
                .iter()
                .find(|(tag, _)| *tag == breed_field.tag)
                .map(|(_, v)| v.clone());
            assert_eq!(breed_value, Some(ScanValue::Str("labrador".into())));
        }
        other => panic!("expected Response::Record, got {other:?}"),
    }
}

#[test]
fn a_second_independent_connection_shares_the_same_store_state() {
    let addr = start_server();
    let mut first = connect(addr);
    let mut second = connect(addr);

    assert_eq!(
        roundtrip(
            &mut first,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(42)
            }
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut second,
            Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (FIELD_BREED, ScanValue::Str("labrador".into())),
                (FIELD_AGE, ScanValue::U32(42)),
            ],
        }
    );
}

/// The flagship correctness property, run through the real wire protocol:
/// many concurrent client connections issuing interleaved
/// `GetById`/`UpdateField` requests against a small contended id pool,
/// verified via the same sequential-replay-linearizability pattern this
/// crate's other flagship tests use (`production_integration.rs`'s
/// `concurrent_writers_survive_a_drop_and_reopen_with_no_lost_updates`,
/// `run_concurrency_stress_test`) — the write order each client thread
/// recorded, replayed sequentially against a fresh in-memory reference,
/// must match the server's final state exactly.
#[test]
fn concurrent_clients_over_the_wire_match_a_sequential_replay() {
    use std::collections::HashMap;
    use std::sync::Mutex;

    let dir = unique_dir("server_dog_stress");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let ids: Vec<Uuid> = (0..20).map(Uuid::from_u128).collect();
    let records: Vec<DogRecord> = ids
        .iter()
        .map(|&id| DogRecord::new(id, "labrador", 0))
        .collect();
    let store = ProductionStore::create(records.clone(), Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default()));

    const THREADS: usize = 8;
    const ITERATIONS: usize = 200;

    // Every completed write, in the real order the server acknowledged it
    // (not the order a thread issued it) — recorded under one mutex so the
    // replay below can be a genuine total order, not a per-thread partial
    // one.
    let write_log: Arc<Mutex<Vec<(Uuid, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    // Held across the write's round trip *and* the log push, as one
    // critical section — the same fix `run_concurrency_stress_test`
    // (`src/concurrency/mod.rs`) already needed and documented: without
    // it, a second thread's write-and-log can complete between this
    // thread's round trip returning and its log-append running, letting
    // the log's order for *same-id* writes diverge from the order the
    // server actually applied them. Confirmed directly — this test
    // reproduced exactly that intermittent false-positive "diverged from
    // the sequential replay" failure before this guard was added.
    let order_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let ids = ids.clone();
            let write_log = Arc::clone(&write_log);
            let order_lock = Arc::clone(&order_lock);
            thread::spawn(move || {
                let mut client = connect(addr);
                let mut rng_state: u64 = 0x9E3779B97F4A7C15u64.wrapping_add(t as u64);
                for i in 0..ITERATIONS {
                    // A small, dependency-free xorshift — determinism per
                    // seed isn't required here (unlike the dataset
                    // generator's own seeded RNG), only that every thread
                    // exercises a genuinely contended, randomized mix.
                    rng_state ^= rng_state << 13;
                    rng_state ^= rng_state >> 7;
                    rng_state ^= rng_state << 17;
                    let id = ids[(rng_state as usize) % ids.len()];

                    if i % 2 == 0 {
                        let age = (rng_state % 1000) as u32;
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
                    } else {
                        let _ = roundtrip(&mut client, Request::GetById { id });
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
        let resp = roundtrip(&mut client, Request::GetById { id });
        match resp {
            Response::Record { fields, .. } => {
                let (_, ScanValue::U32(actual)) = fields[1].clone() else {
                    panic!("expected the age field to be U32");
                };
                assert_eq!(
                    actual, expected,
                    "id {id} diverged from the sequential replay"
                );
            }
            other => panic!("expected a Record response, got {other:?}"),
        }
    }
}
