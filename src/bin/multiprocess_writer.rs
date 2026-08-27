//! Multi-process access diagnosis child process — spawned (twice, at
//! once) by `src/bin/multiprocess_harness.rs`. Unlike
//! `src/bin/crash_writer.rs` (the crash-safety harness's child, which
//! gets `SIGKILL`ed mid-work), every process this binary starts runs to
//! completion — the point here is two *live*, *concurrent* processes
//! genuinely racing on the same file, not one process dying.
//!
//! # The start barrier
//!
//! Every mode does its own setup (building records, in the `open-new-
//! record-race` case even opening the file once beforehand isn't
//! possible without pre-empting the race — see that mode's own doc),
//! then prints `READY` and spin-polls for `<path>.go` to exist before
//! starting the actual racing operation. The harness only creates that
//! file once *both* children have reported `READY`, so both children's
//! racing work starts within microseconds of each other — as close to
//! "genuinely concurrent" as two separate OS processes coordinated
//! through the filesystem can get.
//!
//! Usage: `multiprocess_writer <mode> <args...>`
//!
//! - `create-race <path>` — both processes call `GenericMmapStore::create`
//!   with the *same* fixed dataset on the *same* path at (as close to)
//!   the same instant as the barrier can arrange. Tests whether two
//!   processes racing to initialize the same store corrupts the header
//!   or slot data.
//! - `open-new-record-race <path> <new_id_u128> <new_value>` — opens with
//!   a fixed, pre-existing base dataset (already written by the harness,
//!   safely, before either child spawns) plus *one* new record this
//!   process alone supplies. Both children get a different new id/value,
//!   simulating two processes each independently deciding "this record
//!   doesn't exist yet, I'll append it" — the "next free slot" race.
//! - `update-race <path> <id_u128> <pattern> <iterations>` — opens a
//!   fixed, pre-existing single-record base dataset, then calls
//!   `UpdateField::update` on that *one* shared id, `iterations` times,
//!   always writing the same `pattern` value. The other process races on
//!   the identical id with a different pattern — whichever value survives
//!   is fine (last-write-wins is an acceptable outcome), the question is
//!   whether the *final* value is ever something that is neither pattern.
//! - `read-only-race <path> <id_u128> <pattern_a> <pattern_b> <iterations>`
//!   — opens the same base dataset (read-only role: never calls
//!   `update`) and calls `GetById::get` on the shared id `iterations`
//!   times, recording every distinct value observed. Paired with a
//!   `update-race`-style writer alternating between `pattern_a`/
//!   `pattern_b` on the same id, to check whether a reader can ever
//!   observe a torn value outside `{seed, pattern_a, pattern_b}`.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};
use rusty_multimodal_db::generic::query::{GetById, UpdateField};
use uuid::Uuid;

fn print_line(line: &str) {
    println!("{line}");
    std::io::stdout().flush().expect("flush stdout");
}

/// Print `READY`, then spin-poll for `go_path` to exist — see module
/// docs for why. Capped at a generous timeout so a bug elsewhere doesn't
/// hang this process forever; a diagnostic tool failing loudly is far
/// more useful than one hanging silently.
fn wait_for_go(go_path: &Path) {
    print_line("READY");
    let start = std::time::Instant::now();
    while !go_path.exists() {
        if start.elapsed() > Duration::from_secs(30) {
            panic!("timed out waiting for the go file at {go_path:?} — the harness never released the barrier");
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn fixed_base_orders() -> Vec<Order> {
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

/// Same ids as [`fixed_base_orders`], deliberately *different* values —
/// used by `create-race`'s two processes so a genuine interleaving would
/// be visible (a same-data race can't distinguish "safe" from "raced but
/// both sides wrote identical bytes anyway").
fn fixed_base_orders_variant_b() -> Vec<Order> {
    vec![
        Order {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(200),
            amount_cents: 9_999,
            status: OrderStatus::Cancelled,
            created_at_unix_ms: 5_000,
            discount_cents: 0,
        },
        Order {
            id: Uuid::from_u128(2),
            customer_id: Uuid::from_u128(200),
            amount_cents: 8_888,
            status: OrderStatus::Refunded,
            created_at_unix_ms: 6_000,
            discount_cents: 0,
        },
    ]
}

fn run_create_race(path: &str, go_path: &str, variant: &str) {
    let records = match variant {
        "a" => fixed_base_orders(),
        "b" => fixed_base_orders_variant_b(),
        other => panic!("unknown create-race variant: {other}"),
    };
    wait_for_go(Path::new(go_path));
    match GenericMmapStore::<Order, Status, Amount>::create(records, path.as_ref()) {
        Ok(_store) => print_line("CREATE_OK"),
        Err(error) => print_line(&format!("CREATE_ERR {error}")),
    }
}

fn run_open_new_record_race(path: &str, go_path: &str, new_id_u128: u128, new_value: i64) {
    let mut records = fixed_base_orders();
    records.push(Order {
        id: Uuid::from_u128(new_id_u128),
        customer_id: Uuid::from_u128(100),
        amount_cents: new_value,
        status: OrderStatus::Pending,
        created_at_unix_ms: 3_000,
        discount_cents: 0,
    });
    wait_for_go(Path::new(go_path));
    match GenericMmapStore::<Order, Status, Amount>::open(records, path.as_ref()) {
        Ok(store) => {
            let observed =
                GetById::get(&store, Uuid::from_u128(new_id_u128)).map(|order| order.amount_cents);
            print_line(&format!("OPEN_OK observed={observed:?}"));
        }
        Err(error) => print_line(&format!("OPEN_ERR {error}")),
    }
}

fn run_update_race(path: &str, go_path: &str, id_u128: u128, pattern: i64, iterations: u64) {
    let id = Uuid::from_u128(id_u128);
    let records = fixed_base_orders();
    let mut store = GenericMmapStore::<Order, Status, Amount>::open(records, path.as_ref())
        .expect("open the pre-existing base file");
    wait_for_go(Path::new(go_path));
    for _ in 0..iterations {
        UpdateField::update(&mut store, id, pattern).expect("update");
    }
    print_line("UPDATE_DONE");
}

fn run_read_only_race(
    path: &str,
    go_path: &str,
    id_u128: u128,
    pattern_a: i64,
    pattern_b: i64,
    seed: i64,
    iterations: u64,
) {
    let id = Uuid::from_u128(id_u128);
    let records = fixed_base_orders();
    let store = GenericMmapStore::<Order, Status, Amount>::open(records, path.as_ref())
        .expect("open the pre-existing base file");
    wait_for_go(Path::new(go_path));
    let mut torn_examples: Vec<i64> = Vec::new();
    for _ in 0..iterations {
        let value = GetById::get(&store, id)
            .expect("record exists")
            .amount_cents;
        if value != pattern_a
            && value != pattern_b
            && value != seed
            && !torn_examples.contains(&value)
        {
            torn_examples.push(value);
        }
    }
    print_line(&format!("READ_DONE torn_examples={torn_examples:?}"));
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("create-race") => {
            let path = args.get(2).expect("path arg");
            let go_path = args.get(3).expect("go_path arg");
            let variant = args.get(4).map(String::as_str).unwrap_or("a");
            run_create_race(path, go_path, variant);
        }
        Some("open-new-record-race") => {
            let path = args.get(2).expect("path arg");
            let go_path = args.get(3).expect("go_path arg");
            let new_id_u128: u128 = args
                .get(4)
                .expect("new_id_u128 arg")
                .parse()
                .expect("new_id_u128 is a u128");
            let new_value: i64 = args
                .get(5)
                .expect("new_value arg")
                .parse()
                .expect("new_value is an i64");
            run_open_new_record_race(path, go_path, new_id_u128, new_value);
        }
        Some("update-race") => {
            let path = args.get(2).expect("path arg");
            let go_path = args.get(3).expect("go_path arg");
            let id_u128: u128 = args
                .get(4)
                .expect("id_u128 arg")
                .parse()
                .expect("id_u128 is a u128");
            let pattern: i64 = args
                .get(5)
                .expect("pattern arg")
                .parse()
                .expect("pattern is an i64");
            let iterations: u64 = args
                .get(6)
                .expect("iterations arg")
                .parse()
                .expect("iterations is a u64");
            run_update_race(path, go_path, id_u128, pattern, iterations);
        }
        Some("read-only-race") => {
            let path = args.get(2).expect("path arg");
            let go_path = args.get(3).expect("go_path arg");
            let id_u128: u128 = args
                .get(4)
                .expect("id_u128 arg")
                .parse()
                .expect("id_u128 is a u128");
            let pattern_a: i64 = args
                .get(5)
                .expect("pattern_a arg")
                .parse()
                .expect("pattern_a is an i64");
            let pattern_b: i64 = args
                .get(6)
                .expect("pattern_b arg")
                .parse()
                .expect("pattern_b is an i64");
            let seed: i64 = args
                .get(7)
                .expect("seed arg")
                .parse()
                .expect("seed is an i64");
            let iterations: u64 = args
                .get(8)
                .expect("iterations arg")
                .parse()
                .expect("iterations is a u64");
            run_read_only_race(
                path, go_path, id_u128, pattern_a, pattern_b, seed, iterations,
            );
        }
        other => panic!("unknown or missing mode: {other:?}"),
    }
}
