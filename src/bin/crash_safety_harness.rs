//! Crash-safety diagnosis harness — spawns `src/bin/crash_writer.rs` as a
//! real child process and `SIGKILL`s it at a controlled point, then
//! reopens the file fresh to see what actually survived.
//!
//! # Why a subprocess, not an in-process drop
//!
//! Every durability test this crate has had until now has been a
//! graceful drop+reopen: the store goes out of scope, Rust's `Drop` runs
//! (even though `GenericMmapStore` doesn't implement one itself, nothing
//! *prevents* cleanup code from running), the process unwinds cleanly,
//! and only then does the test reopen the file. That can't tell you what
//! a real crash (power loss, `kill -9`) does, because none of that
//! machinery runs during a real crash. Spawning a genuine child process
//! and sending it `SIGKILL` (`std::process::Child::kill`, which *is*
//! `SIGKILL` on Unix) is the only way to remove all of that — no `Drop`,
//! no unwind, no destructor, no chance for any cleanup code to run — same
//! as this crate's own prior benchmark/test code has never done.
//!
//! # What this can and can't prove about a *real* power loss
//!
//! `SIGKILL`ing the child does not unmap or otherwise touch its pages —
//! the OS page cache backing the mmap survives the process's death
//! untouched, because the page cache belongs to the *file* (the kernel's
//! `address_space` for that inode), not to the process that happened to
//! map it. So when this harness reopens the file in a *different*
//! process immediately afterward, it's reading the same coherent page
//! cache the child process was writing into — not a round trip through
//! physical storage. This matters: a real power loss would additionally
//! lose whatever was still *dirty* in that cache and hadn't reached the
//! disk yet, which killing a process alone cannot demonstrate without
//! either root-level cache manipulation or an actual power interruption,
//! neither of which this harness attempts. Trial 1's own finding (below)
//! is precisely about this gap between "survives a process kill" and
//! "durable to physical storage" — read the report's caveat on it rather
//! than treating "survived" as proof of true crash-durability.
//!
//! # The three trials
//!
//! 1. **Unflushed data loss** (`trial_unflushed`) — create a store
//!    (flushed once, as `create` always does — that's the durable
//!    starting point, not what's measured), then run `RECORD_COUNT`
//!    `update` calls with no `Flush` at all, killing the writer right
//!    after it reports having completed `KILL_AFTER` of them. Reopen and
//!    check how many of those `KILL_AFTER` updates are visible.
//! 2. **Does `Flush` actually guarantee durability** (`trial_flushed`) —
//!    identical, but the writer calls `Flush::flush` once after all
//!    updates and only then is killed (triggered by its `FLUSHED` line,
//!    so the kill provably happens after `flush()` returned). Reopen and
//!    check whether every update survived, every trial.
//! 3. **Torn writes** (`trial_torn_write`) — append one new `(id, value)`
//!    slot to an already-valid file the same way `GenericMmapStore::open`'s
//!    new-slot path does (id half, then value half, no barrier between),
//!    with the writer killed in the gap between those two writes. Inspect
//!    the raw slot bytes afterward: did the id write land without the
//!    value write, or vice versa?
//!
//! Each trial repeats [`TRIALS`] times — a single run of a crash test
//! proves little either way; consistency (or its absence) across repeats
//! is itself part of the finding.
//!
//! Diagnostic tool only — no production code changed to build this. Run
//! manually: `cargo build --release --bin crash_writer --bin
//! crash_safety_harness && ./target/release/crash_safety_harness`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusty_multimodal_db::bench_support::fresh_temp_dir;
use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};
use rusty_multimodal_db::generic::query::GetById;
use uuid::Uuid;

/// A handful of repeats, not one — see module docs.
const TRIALS: usize = 8;
/// How many records the update-stream trials write.
const RECORD_COUNT: usize = 500;
/// Kill after this many `WROTE <i>` lines have been observed — well
/// before the writer would finish `RECORD_COUNT` on its own.
const KILL_AFTER: usize = 250;

/// Mirrors `src/bin/crash_writer.rs`'s own copy of this layout, which in
/// turn mirrors the private layout `src/generic/mmap_store.rs`'s module
/// doc documents. See that binary's top-of-file comment for why this
/// isn't a shared `pub` constant instead.
const HEADER_LEN: usize = 12;
const ID_WIDTH: usize = 16;
const VALUE_WIDTH: usize = 8;
const SLOT_WIDTH: usize = ID_WIDTH + VALUE_WIDTH;

fn writer_binary_path() -> PathBuf {
    let mut path = std::env::current_exe().expect("locate this binary's own path");
    path.pop();
    path.push("crash_writer");
    assert!(
        path.exists(),
        "expected sibling binary at {path:?} — build with \
         `cargo build --release --bin crash_writer --bin crash_safety_harness` \
         (or the --release-less debug equivalent) so both land in the same target dir"
    );
    path
}

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

/// Spawn `binary` with `args`, read its stdout line by line until a line
/// equal to `target_line` is seen, then `SIGKILL` it immediately and
/// reap it. Returns every line observed before (and including) the
/// target line, for callers that want to double check what the child
/// reported before dying.
fn spawn_and_kill_after(binary: &Path, args: &[&str], target_line: &str) -> Vec<String> {
    let mut child = Command::new(binary)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn crash_writer");
    let stdout = child.stdout.take().expect("child stdout was piped");
    let reader = BufReader::new(stdout);
    let mut seen = Vec::new();
    for line in reader.lines() {
        let line = line.expect("read a line from child stdout");
        let hit_target = line == target_line;
        seen.push(line);
        if hit_target {
            break;
        }
    }
    child.kill().expect("SIGKILL the child");
    let status = child.wait().expect("reap the killed child");
    assert!(
        !status.success(),
        "child exited cleanly before we could kill it — trial parameters (RECORD_COUNT/\
         KILL_AFTER) need to leave a bigger window; got status {status:?}"
    );
    seen
}

struct UnflushedTrialResult {
    /// Updates the child provably completed (its own `WROTE <i>` line was
    /// observed) before the kill signal was sent.
    confirmed_before_kill: usize,
    /// Of those, how many are visible after a fresh reopen.
    survived: usize,
    /// Across *all* `RECORD_COUNT` records (not just the confirmed
    /// prefix) — how many show *any* updated value at all, confirmed or
    /// not. Purely informational: shows how much further than
    /// `confirmed_before_kill` the writer may have actually gotten before
    /// the signal landed.
    any_update_visible: usize,
}

fn trial_unflushed(binary: &Path) -> Vec<UnflushedTrialResult> {
    let target_line = format!("WROTE {}", KILL_AFTER - 1);
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("crash_unflushed_{trial}"))
                .expect("fresh temp dir for unflushed trial");
            let path = dir.join("orders.mmap");
            spawn_and_kill_after(
                binary,
                &[
                    "unflushed-updates",
                    path.to_str().expect("utf8 path"),
                    &RECORD_COUNT.to_string(),
                ],
                &target_line,
            );

            let orders = make_orders(RECORD_COUNT);
            let store = GenericMmapStore::<Order, Status, Amount>::open(orders, &path)
                .expect("reopen the crashed file");

            let survived = (0..KILL_AFTER)
                .filter(|&i| {
                    let id = Uuid::from_u128((i + 1) as u128);
                    let record = GetById::get(&store, id).expect("record exists");
                    record.amount_cents == 1_000_000 + i as i64
                })
                .count();
            let any_update_visible = (0..RECORD_COUNT)
                .filter(|&i| {
                    let id = Uuid::from_u128((i + 1) as u128);
                    let record = GetById::get(&store, id).expect("record exists");
                    record.amount_cents != i as i64
                })
                .count();

            let _ = std::fs::remove_dir_all(&dir);
            UnflushedTrialResult {
                confirmed_before_kill: KILL_AFTER,
                survived,
                any_update_visible,
            }
        })
        .collect()
}

struct FlushedTrialResult {
    survived: usize,
    expected: usize,
}

fn trial_flushed(binary: &Path) -> Vec<FlushedTrialResult> {
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("crash_flushed_{trial}"))
                .expect("fresh temp dir for flushed trial");
            let path = dir.join("orders.mmap");
            spawn_and_kill_after(
                binary,
                &[
                    "flushed-updates",
                    path.to_str().expect("utf8 path"),
                    &RECORD_COUNT.to_string(),
                ],
                "FLUSHED",
            );

            let orders = make_orders(RECORD_COUNT);
            let store = GenericMmapStore::<Order, Status, Amount>::open(orders, &path)
                .expect("reopen the crashed file");
            let survived = (0..RECORD_COUNT)
                .filter(|&i| {
                    let id = Uuid::from_u128((i + 1) as u128);
                    let record = GetById::get(&store, id).expect("record exists");
                    record.amount_cents == 1_000_000 + i as i64
                })
                .count();

            let _ = std::fs::remove_dir_all(&dir);
            FlushedTrialResult {
                survived,
                expected: RECORD_COUNT,
            }
        })
        .collect()
}

struct TornWriteResult {
    id_written: bool,
    value_written: bool,
}

fn read_slot(path: &Path, position: usize) -> (Vec<u8>, Vec<u8>) {
    let bytes = std::fs::read(path).expect("read the raw file");
    let start = HEADER_LEN + position * SLOT_WIDTH;
    let id_bytes = bytes[start..start + ID_WIDTH].to_vec();
    let value_bytes = bytes[start + ID_WIDTH..start + SLOT_WIDTH].to_vec();
    (id_bytes, value_bytes)
}

fn trial_torn_write(binary: &Path) -> Vec<TornWriteResult> {
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("crash_torn_{trial}"))
                .expect("fresh temp dir for torn-write trial");
            let path = dir.join("orders.mmap");
            let existing = make_orders(3);
            let existing_slot_count = existing.len();
            {
                // A normal, un-killed create — the durable baseline this
                // trial appends one new slot to. Dropped before spawning
                // the child so the child gets exclusive access to the
                // file, same single-process-exclusive-access assumption
                // every mapping in this crate documents.
                let _store = GenericMmapStore::<Order, Status, Amount>::create(existing, &path)
                    .expect("create baseline file");
            }

            let new_id = Uuid::from_u128(9_999);
            let new_value: i64 = 424_242;
            spawn_and_kill_after(
                binary,
                &[
                    "torn-write",
                    path.to_str().expect("utf8 path"),
                    &existing_slot_count.to_string(),
                    &new_id.as_u128().to_string(),
                    &new_value.to_string(),
                ],
                "ID_WRITTEN",
            );

            let (id_bytes, value_bytes) = read_slot(&path, existing_slot_count);
            let id_written = id_bytes == new_id.as_bytes();
            let value_written = i64::from_le_bytes(
                value_bytes
                    .as_slice()
                    .try_into()
                    .expect("value slot is VALUE_WIDTH bytes"),
            ) == new_value;

            let _ = std::fs::remove_dir_all(&dir);
            TornWriteResult {
                id_written,
                value_written,
            }
        })
        .collect()
}

/// Uninterrupted control run for the torn-write trial: proves the
/// harness itself is sound by confirming both halves land when nothing
/// kills the writer.
fn torn_write_control(binary: &Path) -> TornWriteResult {
    let dir = fresh_temp_dir("crash_torn_control").expect("fresh temp dir for control run");
    let path = dir.join("orders.mmap");
    let existing = make_orders(3);
    let existing_slot_count = existing.len();
    {
        let _store = GenericMmapStore::<Order, Status, Amount>::create(existing, &path)
            .expect("create baseline file");
    }
    let new_id = Uuid::from_u128(9_999);
    let new_value: i64 = 424_242;
    let status = Command::new(binary)
        .args([
            "torn-write",
            path.to_str().expect("utf8 path"),
            &existing_slot_count.to_string(),
            &new_id.as_u128().to_string(),
            &new_value.to_string(),
        ])
        .status()
        .expect("run crash_writer to completion, uninterrupted");
    assert!(status.success(), "control writer should exit cleanly");

    let (id_bytes, value_bytes) = read_slot(&path, existing_slot_count);
    let id_written = id_bytes == new_id.as_bytes();
    let value_written = i64::from_le_bytes(
        value_bytes
            .as_slice()
            .try_into()
            .expect("value slot is VALUE_WIDTH bytes"),
    ) == new_value;
    let _ = std::fs::remove_dir_all(&dir);
    TornWriteResult {
        id_written,
        value_written,
    }
}

fn main() {
    let binary = writer_binary_path();
    println!("Using writer binary: {}", binary.display());
    println!();

    println!(
        "=== Trial 1: unflushed updates, killed mid-stream (after {KILL_AFTER}/{RECORD_COUNT} confirmed) ==="
    );
    for (trial, result) in trial_unflushed(&binary).into_iter().enumerate() {
        println!(
            "  trial {trial}: {}/{} confirmed-before-kill updates survived; {} of {RECORD_COUNT} records show any update at all",
            result.survived, result.confirmed_before_kill, result.any_update_visible
        );
    }
    println!();

    println!("=== Trial 2: Flush called, then killed immediately after it returns ===");
    for (trial, result) in trial_flushed(&binary).into_iter().enumerate() {
        println!(
            "  trial {trial}: {}/{} updates survived",
            result.survived, result.expected
        );
    }
    println!();

    println!("=== Trial 3: torn id/value slot write (killed between the two halves) ===");
    let control = torn_write_control(&binary);
    println!(
        "  uninterrupted control run: id_written={} value_written={} (expect true/true)",
        control.id_written, control.value_written
    );
    for (trial, result) in trial_torn_write(&binary).into_iter().enumerate() {
        println!(
            "  trial {trial}: id_written={} value_written={}",
            result.id_written, result.value_written
        );
    }
}
