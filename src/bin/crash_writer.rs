//! Crash-safety diagnosis child process — spawned and `SIGKILL`ed by
//! `src/bin/crash_safety_harness.rs`, which owns the full rationale for
//! why this needs to be a real subprocess kill rather than an in-process
//! drop. This binary is diagnostic harness code only, not part of the
//! production build: nothing in `src/generic/` depends on it or is
//! changed to support it.
//!
//! Usage: `crash_writer <mode> <args...>`
//!
//! - `unflushed-updates <path> <count>` — create a store at `path`
//!   (flushed once, as `GenericMmapStore::create` always does — this is
//!   the durable *baseline* the trial starts from, not part of what's
//!   being measured), then perform `count` `UpdateField::update` calls
//!   with **no** `Flush` call at all, printing `WROTE <i>` (stdout) right
//!   after update `i` returns, then `ALL_DONE` and sleeps. Trial 1: how
//!   much of an unflushed write stream survives a kill mid-stream.
//! - `flushed-updates <path> <count>` — identical, but calls
//!   `Flush::flush` once after all `count` updates, printing `FLUSHED`
//!   before sleeping. Trial 2: does a completed `Flush` actually
//!   guarantee durability.
//! - `torn-write <path> <existing_slot_count> <new_id_u128> <new_value>`
//!   — appends exactly one new `(id, value)` slot to an already-valid
//!   store file (built beforehand by the harness itself via the real
//!   `GenericMmapStore::create`, not by this binary), the same way
//!   `GenericMmapStore::open`'s new-slot path does: extend the file,
//!   map it, write the id half, THEN write the value half — printing
//!   `ID_WRITTEN` / `VALUE_WRITTEN` (stdout) around the gap between
//!   those two writes, with a deliberate pause in between, so the
//!   harness can land a kill inside that gap deterministically instead
//!   of relying on chance. The gap itself (two independent
//!   `copy_from_slice` calls, no barrier between them) is the real
//!   property under test — the pause only makes hitting it reliable.
//!   Trial 3: can a crash leave a slot with a valid id but a stale/
//!   garbage value.

use std::io::Write;
use std::time::Duration;

use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};
use rusty_multimodal_db::generic::query::UpdateField;
use rusty_multimodal_db::generic::store::Flush;
use uuid::Uuid;

/// Mirrors the private on-disk layout `src/generic/mmap_store.rs`'s
/// module doc documents (`MAGIC` + `u32 SCHEMA_VERSION` header, then
/// `[id: 16 bytes][value: 8 bytes]` slots for `Order`/`Amount`
/// specifically, since `Uuid::BYTE_WIDTH == 16` and `i64::BYTE_WIDTH == 8`)
/// — kept in sync by hand since this diagnostic binary deliberately
/// doesn't depend on those private constants.
const HEADER_LEN: usize = 12;
const ID_WIDTH: usize = 16;
const VALUE_WIDTH: usize = 8;
const SLOT_WIDTH: usize = ID_WIDTH + VALUE_WIDTH;

fn make_orders(count: usize) -> Vec<Order> {
    (0..count)
        .map(|i| Order {
            id: Uuid::from_u128((i + 1) as u128),
            customer_id: Uuid::from_u128(1),
            amount_cents: i as i64,
            status: OrderStatus::Pending,
            created_at_unix_ms: 0,
            discount_cents: 0,
        })
        .collect()
}

fn print_line(line: &str) {
    println!("{line}");
    std::io::stdout().flush().expect("flush stdout");
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn run_updates(path: &str, count: usize, then_flush: bool) -> ! {
    let orders = make_orders(count);
    let mut store = GenericMmapStore::<Order, Status, Amount>::create(orders, path.as_ref())
        .expect("create store");
    for i in 0..count {
        let id = Uuid::from_u128((i + 1) as u128);
        UpdateField::update(&mut store, id, 1_000_000 + i as i64).expect("update");
        print_line(&format!("WROTE {i}"));
    }
    if then_flush {
        Flush::flush(&store).expect("flush");
        print_line("FLUSHED");
    } else {
        print_line("ALL_DONE");
    }
    sleep_forever()
}

fn run_torn_write(path: &str, existing_slot_count: usize, new_id_u128: u128, new_value: i64) {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("open existing store file");
    let new_total_slots = existing_slot_count + 1;
    file.set_len((HEADER_LEN + new_total_slots * SLOT_WIDTH) as u64)
        .expect("extend file for the new slot");

    // SAFETY: this binary is the only process touching this file at this
    // point (the harness that created it has already dropped its own
    // mapping before spawning this process) — the same single-process
    // exclusive-access assumption every mapping in this crate documents.
    let mut mmap = unsafe { memmap2::MmapMut::map_mut(&file).expect("map file") };

    let slot_start = HEADER_LEN + existing_slot_count * SLOT_WIDTH;
    let new_id = Uuid::from_u128(new_id_u128);

    mmap[slot_start..slot_start + ID_WIDTH].copy_from_slice(new_id.as_bytes());
    print_line("ID_WRITTEN");

    // Widen the real gap between the two writes so a kill lands inside
    // it reliably. The two writes have no atomicity between them with or
    // without this pause — it only turns a nanosecond-scale race into a
    // deterministic one.
    std::thread::sleep(Duration::from_millis(150));

    mmap[slot_start + ID_WIDTH..slot_start + SLOT_WIDTH].copy_from_slice(&new_value.to_le_bytes());
    print_line("VALUE_WRITTEN");

    mmap.flush().expect("flush new slot");
    print_line("SLOT_FLUSHED");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("unflushed-updates") => {
            let path = args.get(2).expect("path arg");
            let count: usize = args
                .get(3)
                .expect("count arg")
                .parse()
                .expect("count is a usize");
            run_updates(path, count, false);
        }
        Some("flushed-updates") => {
            let path = args.get(2).expect("path arg");
            let count: usize = args
                .get(3)
                .expect("count arg")
                .parse()
                .expect("count is a usize");
            run_updates(path, count, true);
        }
        Some("torn-write") => {
            let path = args.get(2).expect("path arg");
            let existing_slot_count: usize = args
                .get(3)
                .expect("existing_slot_count arg")
                .parse()
                .expect("existing_slot_count is a usize");
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
            run_torn_write(path, existing_slot_count, new_id_u128, new_value);
        }
        other => panic!("unknown or missing mode: {other:?}"),
    }
}
