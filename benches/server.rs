//! Custom (non-Criterion) throughput/latency harness for the server/query
//! layer (`src/server/**`, `SERVER-001`) — the benchmark `SERVER-001`'s
//! own "Open questions" named as a real, unscoped follow-up since the
//! layer's initial implementation (v0.1.0's acceptance criteria were
//! correctness, not performance). The owner's "2" in "3 then 2": a third
//! validation domain (`Employee`, `SERVER-001` v0.3.0) first, then this.
//!
//! # Why not Criterion
//!
//! Same reasoning as `benches/concurrency.rs`: the real questions here —
//! "what does one request/response round trip cost over a real socket,"
//! and "how does aggregate throughput scale with the number of concurrent
//! client connections under the thread-per-connection model" — aren't
//! what Criterion's `b.iter` model measures (one closure, repeated, timed
//! on one thread). This reuses `benches/concurrency.rs`'s own custom-harness
//! shape instead: a `Barrier`-synchronized thread sweep, aggregate ops/sec
//! computed from the slowest thread's wall-clock time, not the average.
//!
//! # Real sockets, not `dispatch` in-process
//!
//! Every prior server-layer benchmark-shaped measurement is either
//! `dispatch`'s in-process unit tests (no I/O at all) or a real socket used
//! only for correctness (`tests/server_*_integration.rs`, pass/fail, not
//! timed). This is the first benchmark to put a real
//! `TcpListener`/`TcpStream` pair (loopback) in the timed path — the
//! questions above only exist at that layer.
//!
//! # `TCP_NODELAY`, non-negotiable here
//!
//! `SERVER-001-FR-006` already found and fixed a real ~40ms-per-round-trip
//! Nagle/delayed-ACK cost when `TCP_NODELAY` isn't set on both ends of a
//! synchronous request/response protocol. Every client connection this
//! benchmark opens sets it, same as every existing integration test —
//! omitting it here wouldn't just skew the numbers, it would make them
//! meaningless (measuring TCP's delayed-ACK timer, not this server).
//!
//! # Dataset: a small, fixed 20-id pool per domain, not `SIZES`
//!
//! Matches `tests/server_dog_integration.rs`'s own flagship concurrent-client
//! stress test precedent (`concurrent_clients_over_the_wire_match_a_sequential_replay`,
//! a 20-id contended pool) rather than this crate's usual 1K/100K/1M
//! `SIZES` sweep: those sizes measure in-process lookup cost, which the
//! existing `benches/workloads.rs`/`benches/generic_production.rs` suites
//! already cover per domain. What this benchmark adds is the network/dispatch
//! layer *on top* of that already-measured lookup cost, so a small, realistic
//! id pool is enough — a bigger dataset would mostly re-measure
//! `GetById`'s already-known in-process cost under a much larger, mostly
//! irrelevant fixed setup cost.
//!
//! # Thread counts: 1/4/8/16, this session's container
//!
//! Matches `benches/concurrency.rs`'s own documented "original container
//! run" precedent for an unknown/shared-container environment: this
//! session's `std::thread::available_parallelism()` (reported at the top
//! of this binary's own output) determines which rows, if any, are
//! genuinely non-oversubscribed. A `baileyai` (or other known-hardware)
//! follow-up pass, matching `benches/concurrency.rs`'s own history, would
//! be needed for a real per-core throughput ceiling — named here as an
//! open question, not assumed answered by this pass.
//!
//! # One request kind (`GetById`), all three domains
//!
//! `GetById` is the one request kind every `ConnectionStore` adapter
//! implements as a real operation (`Dog`, `Order`/`Customer`, `Employee`
//! alike) — the only kind that lets the three domains' numbers sit in one
//! comparable table. Per-request-kind cost differences (a `filter_eq`
//! linear scan vs. an indexed `get`, `Employee`'s extra field) are already
//! covered by each domain's own in-process benchmark
//! (`benches/workloads.rs`, `benches/generic_production.rs`); this
//! benchmark's own question is what the network/dispatch layer adds on
//! top, which one representative request kind answers without tripling
//! this benchmark's own already-large thread-count/domain matrix.

use rusty_multimodal_db::bench_support::{fresh_temp_dir, RoundRobin};
use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Order, OrderStatus,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic_spike::employee_impl::{
    create_employee_production_stack, Department, Employee,
};
use rusty_multimodal_db::production::ProductionStore;
use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::employee::EmployeeConnectionStore;
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::order::OrderConnectionStore;
use rusty_multimodal_db::server::protocol::{Request, Response};
use rusty_multimodal_db::server::serve;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Matches `tests/server_dog_integration.rs`'s flagship stress test's own
/// contended-pool size — see this module's own doc comment.
const POOL_SIZE: usize = 20;
/// See this module's own doc comment on why this differs from
/// `benches/concurrency.rs`'s `baileyai`-specific `[1, 4, 32, 64]`.
const THREAD_COUNTS: [usize; 4] = [1, 4, 8, 16];
/// Real-socket round trips are far more expensive per-op than the
/// in-process operations `benches/concurrency.rs` sweeps (`OPS_PER_THREAD
/// = 10_000` there), so this is lower to keep the full domain × thread-count
/// matrix's total wall-clock time tractable.
const OPS_PER_THREAD: usize = 2_000;
/// Iterations for the single-connection, zero-contention latency baseline
/// each domain reports before its throughput sweep.
const LATENCY_ITERATIONS: usize = 5_000;

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

fn roundtrip(stream: &mut TcpStream, req: &Request) -> Response {
    write_message(stream, req).unwrap();
    read_message(stream).unwrap()
}

fn start_dog_server() -> (SocketAddr, Vec<Uuid>) {
    let dir = fresh_temp_dir("server_bench_dog").expect("fresh temp dir for dog server bench");
    let path = dir.join("dogs.mmap");
    let ids: Vec<Uuid> = (0..POOL_SIZE as u128).map(Uuid::from_u128).collect();
    let records: Vec<DogRecord> = ids
        .iter()
        .map(|&id| DogRecord::new(id, "labrador", 3))
        .collect();
    let store = ProductionStore::create(records, Vec::new(), &path)
        .expect("create ProductionStore for dog server bench");
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store));
    (addr, ids)
}

fn start_order_server() -> (SocketAddr, Vec<Uuid>) {
    let dir = fresh_temp_dir("server_bench_order").expect("fresh temp dir for order server bench");
    let path = dir.join("amount.mmap");
    let ids: Vec<Uuid> = (0..POOL_SIZE as u128)
        .map(|n| Uuid::from_u128(1_000 + n))
        .collect();
    let orders: Vec<Order> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| Order {
            id,
            customer_id: Uuid::from_u128(9_000 + (i as u128 % 4)),
            amount_cents: 1_000 + i as i64,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 0,
            discount_cents: 0,
        })
        .collect();
    let stack = create_order_production_stack(orders, &path)
        .expect("create OrderProductionStack for order server bench");
    let connection_store = Arc::new(OrderConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store));
    (addr, ids)
}

fn start_employee_server() -> (SocketAddr, Vec<Uuid>) {
    let dir =
        fresh_temp_dir("server_bench_employee").expect("fresh temp dir for employee server bench");
    let path = dir.join("salary.mmap");
    let ids: Vec<Uuid> = (0..POOL_SIZE as u128)
        .map(|n| Uuid::from_u128(2_000 + n))
        .collect();
    let manager_id = ids[0];
    let employees: Vec<Employee> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| Employee {
            id,
            name: format!("employee-{i}"),
            department: Department::Engineering,
            salary_cents: 100_000 + i as i64,
            manager_id: if id == manager_id {
                None
            } else {
                Some(manager_id)
            },
        })
        .collect();
    // A small collaboration chain (i collaborates_with i+1) — enough for a
    // real, non-empty `SymmetricRelation`, not exercised by `GetById`
    // itself but built the same way every other `Employee` fixture is.
    let edges: Vec<(Uuid, Uuid)> = ids.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let stack = create_employee_production_stack(employees, &edges, &path)
        .expect("create EmployeeProductionStack for employee server bench");
    let connection_store = Arc::new(EmployeeConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store));
    (addr, ids)
}

/// Single-connection, zero-contention `GetById` round-trip latency —
/// average over [`LATENCY_ITERATIONS`] sequential requests on one client
/// connection, no concurrency at all. The baseline every throughput-sweep
/// row below is scaling up from.
fn measure_latency(addr: SocketAddr, ids: &[Uuid]) -> Duration {
    let mut client = connect(addr);
    let mut cursor = RoundRobin::new(ids.len());
    let start = Instant::now();
    for _ in 0..LATENCY_ITERATIONS {
        let id = ids[cursor.advance()];
        let resp = roundtrip(&mut client, &Request::GetById { id });
        debug_assert!(matches!(resp, Response::Record { .. }));
    }
    start.elapsed() / LATENCY_ITERATIONS as u32
}

/// `threads` real client connections, each opened before the barrier so
/// connection setup itself isn't counted, each firing [`OPS_PER_THREAD`]
/// sequential `GetById` requests against a shared, contended id pool once
/// every thread is ready. Aggregate ops/sec from the *slowest* thread's
/// elapsed time — same rationale as `benches/concurrency.rs`'s own
/// `run_throughput`: a slow straggler should show up as lower throughput,
/// not be averaged away.
fn run_throughput(addr: SocketAddr, ids: &Arc<Vec<Uuid>>, threads: usize) -> f64 {
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);
    for thread_index in 0..threads {
        let ids = Arc::clone(ids);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mut client = connect(addr);
            let mut cursor = RoundRobin::new(ids.len());
            // Offset each thread's starting position so threads don't all
            // hammer the same id on the same iteration — genuine, not
            // artificially synchronized, contention across the pool.
            for _ in 0..thread_index {
                cursor.advance();
            }
            barrier.wait();
            let start = Instant::now();
            for _ in 0..OPS_PER_THREAD {
                let id = ids[cursor.advance()];
                let resp = roundtrip(&mut client, &Request::GetById { id });
                debug_assert!(matches!(resp, Response::Record { .. }));
            }
            start.elapsed()
        }));
    }

    let mut slowest = Duration::ZERO;
    for handle in handles {
        if let Ok(elapsed) = handle.join() {
            slowest = slowest.max(elapsed);
        }
    }

    let total_ops = (threads * OPS_PER_THREAD) as f64;
    total_ops / slowest.as_secs_f64()
}

fn bench_domain(name: &str, addr: SocketAddr, ids: Vec<Uuid>) {
    let latency = measure_latency(addr, &ids);
    println!(
        "{name:<10} {:>10} {:>14.1}",
        "latency",
        latency.as_secs_f64() * 1_000_000.0
    );

    let ids = Arc::new(ids);
    for &threads in &THREAD_COUNTS {
        let ops_per_sec = run_throughput(addr, &ids, threads);
        println!("{name:<10} {threads:>10} {ops_per_sec:>14.0}");
    }
}

fn main() {
    let available_parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    println!("std::thread::available_parallelism() reports: {available_parallelism}");
    println!(
        "(thread counts below are swept at 1/4/8/16 — see this module's own doc \
         comment for why this differs from benches/concurrency.rs's \
         baileyai-specific sweep; only rows at or below the reported \
         parallelism above are genuinely non-oversubscribed)"
    );
    println!();
    println!(
        "first row per domain is the single-connection latency baseline (µs/op); \
         remaining rows are aggregate throughput (ops/sec) at that thread count"
    );
    println!();
    println!("{:<10} {:>10} {:>14}", "domain", "threads", "value");

    let (addr, ids) = start_dog_server();
    bench_domain("dog", addr, ids);

    let (addr, ids) = start_order_server();
    bench_domain("order", addr, ids);

    let (addr, ids) = start_employee_server();
    bench_domain("employee", addr, ids);
}
