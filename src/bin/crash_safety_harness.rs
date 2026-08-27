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
//! # The trials
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
//! 3. **Torn writes, against this round's fix** (`trial_torn_write`) —
//!    append one new slot to an already-valid file the same way
//!    `GenericMmapStore::open`'s new-slot path does since the fix: id,
//!    then value, then a trailing commit-marker byte, three independent
//!    writes with no barrier between any of them. Two sub-trials kill in
//!    each of the two gaps (`kill_after_id`/`kill_after_value`). Verified
//!    through the real public API, not just raw bytes: the harness
//!    reopens with `GenericMmapStore::open` using `records` that include
//!    a *fresh* seed value for the new id (deliberately different from
//!    both the torn write's attempted value and the pre-existing
//!    records), and checks what a caller actually observes — the fix is
//!    confirmed only if every killed trial reads back that fresh reseed
//!    value (proving the torn slot was excluded from reconciliation and
//!    a clean new slot took its place), never the value the interrupted
//!    write was attempting and never anything else. An uninterrupted
//!    control run confirms the harness itself is sound (the *original*
//!    attempted value survives when nothing kills the writer).
//! 4. **Torn updates** (`trial_torn_update`) — the diagnosis only tested
//!    slot *creation*; this checks the *update* path separately, rather
//!    than assuming the same fix covers it (it doesn't touch the update
//!    path at all — see `GenericMmapStore::is_committed`'s own doc
//!    comment for why). Runs many rapid, unsynchronized, unpaced
//!    `UpdateField::update` calls on one already-committed record,
//!    alternating between two maximally bit-different values, killed at
//!    an uncontrolled point relative to individual writes (a short,
//!    fixed sleep, not a readiness line — any synchronization point would
//!    only ever land the kill *between* calls, never inside one). Checks
//!    whether the final persisted value is ever anything other than
//!    exactly one of the two known patterns.
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
use std::time::Duration;

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
/// Iterations `torn-update` is asked to attempt — deliberately far more
/// than it can complete before the short kill sleep below elapses; the
/// point is to be killed mid-burst, not to finish.
const TORN_UPDATE_ITERATIONS: u64 = 500_000_000;
/// How long the parent waits before killing the `torn-update` child.
/// Short and fixed, not tied to any readiness line from the child — a
/// line-based sync point would only ever let the kill land *between*
/// update calls, never inside one, defeating the point of this trial.
const TORN_UPDATE_KILL_DELAY: Duration = Duration::from_millis(2);

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

/// Spawn `binary` with `args`, wait `delay`, then `SIGKILL` it and reap
/// it — no readiness-line synchronization, for trials where any such
/// synchronization would itself prevent the kill from landing where it
/// needs to.
fn spawn_and_kill_after_delay(binary: &Path, args: &[&str], delay: Duration) {
    let mut child = Command::new(binary)
        .args(args)
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn crash_writer");
    std::thread::sleep(delay);
    child.kill().expect("SIGKILL the child");
    let status = child.wait().expect("reap the killed child");
    assert!(
        !status.success(),
        "child exited cleanly before we could kill it — TORN_UPDATE_ITERATIONS/\
         TORN_UPDATE_KILL_DELAY need adjusting; got status {status:?}"
    );
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

/// What a caller observes through the real `GenericMmapStore` public API
/// after a torn-write attempt — the authoritative check, not a raw-byte
/// inspection. `Some(value)` if the id is visible at all (it always
/// should be, either as the reseed value or, in the control case, the
/// originally-attempted one); this trial only ever supplies ids that are
/// present in `records`, so `None` would itself be a bug in the harness.
type TornWriteObserved = i64;

const TORN_WRITE_NEW_ID: u128 = 9_999;
/// The value the (possibly-interrupted) writer attempts to persist.
const TORN_WRITE_ATTEMPTED_VALUE: i64 = 424_242;
/// The value the *record* the harness supplies to `open` carries as its
/// own seed — deliberately different from
/// [`TORN_WRITE_ATTEMPTED_VALUE`], so a reopened value of exactly this
/// number can only mean "the torn slot was excluded and a fresh one was
/// seeded from the caller's own record," not a coincidence.
const TORN_WRITE_RESEED_VALUE: i64 = 999_999;

fn build_torn_write_baseline() -> (PathBuf, tempdir_guard::TempDirGuard, usize) {
    let dir = fresh_temp_dir("crash_torn_write").expect("fresh temp dir for torn-write trial");
    let path = dir.join("orders.mmap");
    let existing = make_orders(3);
    let existing_slot_count = existing.len();
    {
        // A normal, un-killed create — the durable baseline this trial
        // appends one new slot to. Dropped before spawning the child so
        // the child gets exclusive access to the file, same single-
        // process-exclusive-access assumption every mapping in this
        // crate documents.
        let _store = GenericMmapStore::<Order, Status, Amount>::create(existing, &path)
            .expect("create baseline file");
    }
    (path, tempdir_guard::TempDirGuard(dir), existing_slot_count)
}

/// Reopen through the real public API with `records` = the original
/// baseline plus one record for [`TORN_WRITE_NEW_ID`] seeded at
/// [`TORN_WRITE_RESEED_VALUE`], and return what a caller actually
/// observes for that id.
fn observe_after_torn_write(path: &Path) -> TornWriteObserved {
    let mut records = make_orders(3);
    records.push(Order {
        id: Uuid::from_u128(TORN_WRITE_NEW_ID),
        customer_id: Uuid::from_u128(1),
        amount_cents: TORN_WRITE_RESEED_VALUE,
        status: OrderStatus::Pending,
        created_at_unix_ms: 0,
        discount_cents: 0,
    });
    let store = GenericMmapStore::<Order, Status, Amount>::open(records, path)
        .expect("reopen after a torn-write attempt");
    GetById::get(&store, Uuid::from_u128(TORN_WRITE_NEW_ID))
        .expect("the new id is always in `records`")
        .amount_cents
}

/// Run the writer to completion, uninterrupted — proves the harness
/// itself is sound (the attempted value survives when nothing kills the
/// writer) before trusting what a kill produces.
fn torn_write_control(binary: &Path) -> TornWriteObserved {
    let (path, _dir_guard, existing_slot_count) = build_torn_write_baseline();
    let status = Command::new(binary)
        .args([
            "torn-write",
            path.to_str().expect("utf8 path"),
            &existing_slot_count.to_string(),
            &TORN_WRITE_NEW_ID.to_string(),
            &TORN_WRITE_ATTEMPTED_VALUE.to_string(),
        ])
        .status()
        .expect("run crash_writer to completion, uninterrupted");
    assert!(status.success(), "control writer should exit cleanly");
    observe_after_torn_write(&path)
}

/// Kill after `kill_on` (`"ID_WRITTEN"` or `"VALUE_WRITTEN"`) is observed
/// on the child's stdout, `TRIALS` times, returning what a caller
/// observes for the new id after each kill.
fn trial_torn_write(binary: &Path, kill_on: &str) -> Vec<TornWriteObserved> {
    (0..TRIALS)
        .map(|_trial| {
            let (path, _dir_guard, existing_slot_count) = build_torn_write_baseline();
            spawn_and_kill_after(
                binary,
                &[
                    "torn-write",
                    path.to_str().expect("utf8 path"),
                    &existing_slot_count.to_string(),
                    &TORN_WRITE_NEW_ID.to_string(),
                    &TORN_WRITE_ATTEMPTED_VALUE.to_string(),
                ],
                kill_on,
            );
            observe_after_torn_write(&path)
        })
        .collect()
}

const TORN_UPDATE_ID: u128 = 55_555;

fn torn_update_patterns() -> (i64, i64) {
    (
        i64::from_le_bytes([0x11u8; 8]),
        i64::from_le_bytes([0x22u8; 8]),
    )
}

/// The final on-disk value after a kill mid-burst of unsynchronized
/// updates — checked against exactly the two known patterns.
fn trial_torn_update(binary: &Path) -> Vec<i64> {
    let (pattern_a, _pattern_b) = torn_update_patterns();
    let id = Uuid::from_u128(TORN_UPDATE_ID);
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("crash_torn_update_{trial}"))
                .expect("fresh temp dir for torn-update trial");
            let path = dir.join("orders.mmap");
            let seed_record = Order {
                id,
                customer_id: Uuid::from_u128(1),
                amount_cents: pattern_a,
                status: OrderStatus::Pending,
                created_at_unix_ms: 0,
                discount_cents: 0,
            };
            {
                let _store = GenericMmapStore::<Order, Status, Amount>::create(
                    vec![seed_record.clone()],
                    &path,
                )
                .expect("create baseline file");
            }

            spawn_and_kill_after_delay(
                binary,
                &[
                    "torn-update",
                    path.to_str().expect("utf8 path"),
                    &id.as_u128().to_string(),
                    &TORN_UPDATE_ITERATIONS.to_string(),
                ],
                TORN_UPDATE_KILL_DELAY,
            );

            let store = GenericMmapStore::<Order, Status, Amount>::open(vec![seed_record], &path)
                .expect("reopen after the crashed update burst");
            let value = GetById::get(&store, id)
                .expect("record exists")
                .amount_cents;

            let _ = std::fs::remove_dir_all(&dir);
            value
        })
        .collect()
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

    println!("=== Trial 3: torn slot write, verified against this round's fix ===");
    println!(
        "  attempted value: {TORN_WRITE_ATTEMPTED_VALUE}, reseed value: {TORN_WRITE_RESEED_VALUE}"
    );
    let control = torn_write_control(&binary);
    println!(
        "  uninterrupted control run: observed={control} (expect {TORN_WRITE_ATTEMPTED_VALUE}, the attempted value)"
    );
    println!("  -- killed after ID_WRITTEN (before value+marker) --");
    for (trial, observed) in trial_torn_write(&binary, "ID_WRITTEN")
        .into_iter()
        .enumerate()
    {
        let verdict = if observed == TORN_WRITE_RESEED_VALUE {
            "PASS (correctly excluded, fresh reseed)"
        } else if observed == TORN_WRITE_ATTEMPTED_VALUE {
            "FAIL (torn slot wrongly treated as committed)"
        } else {
            "FAIL (unexpected value)"
        };
        println!("    trial {trial}: observed={observed} — {verdict}");
    }
    println!("  -- killed after VALUE_WRITTEN (before marker) --");
    for (trial, observed) in trial_torn_write(&binary, "VALUE_WRITTEN")
        .into_iter()
        .enumerate()
    {
        let verdict = if observed == TORN_WRITE_RESEED_VALUE {
            "PASS (correctly excluded, fresh reseed)"
        } else if observed == TORN_WRITE_ATTEMPTED_VALUE {
            "FAIL (torn slot wrongly treated as committed)"
        } else {
            "FAIL (unexpected value)"
        };
        println!("    trial {trial}: observed={observed} — {verdict}");
    }
    println!();

    println!(
        "=== Trial 4: torn in-place update (checked, not assumed, per this round's diagnosis) ==="
    );
    let (pattern_a, pattern_b) = torn_update_patterns();
    println!("  pattern A={pattern_a}, pattern B={pattern_b}");
    for (trial, observed) in trial_torn_update(&binary).into_iter().enumerate() {
        let verdict = if observed == pattern_a || observed == pattern_b {
            "PASS (exactly one known pattern)"
        } else {
            "FAIL (torn value — neither pattern)"
        };
        println!("  trial {trial}: observed={observed} — {verdict}");
    }
}

/// A tiny RAII guard so `build_torn_write_baseline`'s temp dir gets
/// cleaned up wherever its returned tuple eventually drops, without
/// every call site needing to remember to do it manually.
mod tempdir_guard {
    use std::path::PathBuf;

    pub struct TempDirGuard(pub PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
