//! Multi-process access diagnosis harness — spawns two real, independent
//! OS processes (`src/bin/multiprocess_writer.rs`) that both open the
//! *same* `GenericMmapStore`/`Order` file and race on it concurrently,
//! with zero coordination between them beyond whatever the filesystem/OS
//! itself provides. Unlike the crash-safety harness
//! (`crash_safety_harness.rs`), nothing gets killed here — both
//! processes run to completion, genuinely overlapping in time.
//!
//! # Why a real subprocess pair, not two threads
//!
//! `RwLock` (what every store in this crate uses for thread safety)
//! coordinates threads *within one process's address space* — it has no
//! way to see, let alone coordinate with, a second process's mapping of
//! the same file. Two threads in one process sharing one
//! `GenericMmapStore` would go through the same `RwLock` and prove
//! nothing about this question. Two separate `std::process::Command`
//! children, each with its own independently-constructed
//! `GenericMmapStore` (own heap, own `position_index`/`records` maps,
//! own mapping of the same underlying file), are the only way to
//! actually exercise "two processes, no shared Rust-level state, only
//! the file itself in common."
//!
//! # The start barrier
//!
//! Both children print `READY` on stdout once their own setup (record
//! construction, and for `update-race`/`read-only-race`, the initial
//! `open()` itself) is done and they're about to start the actual racing
//! operation. This harness waits for *both* `READY` lines (read
//! concurrently, one thread per child, so waiting on one never blocks
//! reading the other) before creating a `<path>.go` marker file; each
//! child spin-polls for that file's existence and starts its race the
//! instant it appears. This is as close to "the two operations actually
//! overlap in time" as coordinating through the filesystem allows.
//!
//! # The three questions this asks, empirically
//!
//! 1. **Is concurrent access even possible today?** (`trial_create_race`,
//!    and implicitly every other trial) — does either process ever get
//!    an OS-level error opening/creating the file while the other holds
//!    it, or do both always succeed?
//! 2. **What breaks under concurrent writers** — `trial_create_race`
//!    (the header/initial-slot region), `trial_open_new_record_race`
//!    (the "next free slot" race the diagnosis round named: both
//!    processes independently compute the same `existing_slot_count` and
//!    both believe *they* are appending at that position — originally
//!    reproduced 24/24 trials; this harness now doubles as the
//!    regression check for the `O_APPEND`-based fix, see
//!    `src/generic/mmap_store.rs`'s own "next free slot" doc section),
//!    and `trial_update_race` (whether the commit-marker invariant,
//!    proven only against a single writer crashing, holds against two
//!    writers genuinely racing on the same slot).
//! 3. **Reader consistency** — `trial_read_during_write`: one process
//!    alternates a shared record between two known patterns while
//!    another concurrently reads it, checking for any observed value
//!    outside the known set.
//!
//! Each trial repeats [`TRIALS`] times — same rigor as the crash-safety
//! harness: a single run of a race proves little either way.
//!
//! Diagnostic tool only — no production code changed to build this. Run
//! manually: `cargo build --release --bin multiprocess_writer --bin
//! multiprocess_harness --features research && ./target/release/multiprocess_harness`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use rusty_multimodal_db::bench_support::fresh_temp_dir;
use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
use rusty_multimodal_db::generic::order_customer::{Amount, Order, OrderStatus, Status};
use uuid::Uuid;

/// A handful of repeats, not one — see module docs.
const TRIALS: usize = 8;
/// Iterations each writer in `update-race`/`read-only-race` performs —
/// large enough that the two processes' racing windows comfortably
/// overlap regardless of relative OS scheduling speed.
const RACE_ITERATIONS: u64 = 2_000_000;

/// Mirrors the private on-disk layout `src/generic/mmap_store.rs`'s
/// module doc documents — same mirrored-constant convention
/// `crash_safety_harness.rs` already uses, for the same reason (a
/// diagnostic binary outside the library deliberately doesn't depend on
/// those private items).
const HEADER_LEN: usize = 12;
const ID_WIDTH: usize = 16;
const SLOT_WIDTH: usize = ID_WIDTH + 8 + 1;

/// Read the raw id bytes at slot `position`, for direct proof of which
/// record's identity actually occupies a given on-disk slot — the
/// authoritative check for a suspected "next free slot" collision,
/// independent of anything either racing process itself believed it
/// wrote.
fn read_slot_id(path: &Path, position: usize) -> Uuid {
    let bytes = std::fs::read(path).expect("read the raw file");
    let start = HEADER_LEN + position * SLOT_WIDTH;
    let id_bytes: [u8; 16] = bytes[start..start + ID_WIDTH]
        .try_into()
        .expect("slot has at least ID_WIDTH bytes");
    Uuid::from_bytes(id_bytes)
}

fn writer_binary_path() -> PathBuf {
    let mut path = std::env::current_exe().expect("locate this binary's own path");
    path.pop();
    path.push("multiprocess_writer");
    assert!(
        path.exists(),
        "expected sibling binary at {path:?} — build with \
         `cargo build --release --bin multiprocess_writer --bin multiprocess_harness \
         --features research` so both land in the same target dir"
    );
    path
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

/// Spawn `binary` with `args`, returning the child (stdout piped) and a
/// receiver that yields every line the child prints, oldest first,
/// closing when the child's stdout closes. Reading happens on a spawned
/// thread so waiting on one child's output never blocks reading the
/// other's — the mechanism that makes genuine two-process concurrency
/// observable from one harness process.
fn spawn_with_stdout_reader(binary: &Path, args: &[&str]) -> (Child, mpsc::Receiver<String>) {
    let mut child = Command::new(binary)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn multiprocess_writer");
    let stdout = child.stdout.take().expect("child stdout was piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.expect("read a line from child stdout");
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (child, rx)
}

/// Block until `rx` yields a line equal to `target`, returning every line
/// seen along the way (including the target).
fn wait_for_line(rx: &mpsc::Receiver<String>, target: &str) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let line = rx
            .recv()
            .unwrap_or_else(|_| panic!("child stdout closed before printing {target:?}"));
        let hit = line == target;
        seen.push(line);
        if hit {
            return seen;
        }
    }
}

/// Spawn two children with `args_a`/`args_b`, release both from the
/// start barrier as close together as the filesystem allows once both
/// report `READY`, then wait for both to exit. Returns each child's full
/// stdout (post-`READY`) and exit status.
/// A racer's captured stdout plus its raw exit status.
type RawRacerOutcome = (Vec<String>, std::process::ExitStatus);

fn run_race(
    binary: &Path,
    go_path: &Path,
    args_a: &[&str],
    args_b: &[&str],
) -> (RawRacerOutcome, RawRacerOutcome) {
    let _ = std::fs::remove_file(go_path);

    let (mut child_a, rx_a) = spawn_with_stdout_reader(binary, args_a);
    let (mut child_b, rx_b) = spawn_with_stdout_reader(binary, args_b);

    wait_for_line(&rx_a, "READY");
    wait_for_line(&rx_b, "READY");
    // Both children are now spin-polling for this file — creating it is
    // the closest this harness can get to "release both at once".
    std::fs::write(go_path, b"go").expect("write the go marker file");

    let mut lines_a: Vec<String> = rx_a.iter().collect();
    let mut lines_b: Vec<String> = rx_b.iter().collect();
    let status_a = child_a.wait().expect("reap child A");
    let status_b = child_b.wait().expect("reap child B");
    // `rx.iter()` above already drains to EOF, but the reader thread
    // closes its sender only once the pipe closes, which can race the
    // `wait()` above on some schedulers; drain once more defensively.
    lines_a.extend(rx_a.try_iter());
    lines_b.extend(rx_b.try_iter());

    let _ = std::fs::remove_file(go_path);
    ((lines_a, status_a), (lines_b, status_b))
}

/// A racer's captured stdout plus whether it exited successfully.
type RacerOutcome = (Vec<String>, bool);

fn trial_create_race(binary: &Path) -> Vec<(RacerOutcome, RacerOutcome)> {
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("multiprocess_create_race_{trial}"))
                .expect("fresh temp dir for create-race trial");
            let path = dir.join("orders.mmap");
            let go_path = dir.join("orders.mmap.go");

            let path_str = path.to_str().expect("utf8 path").to_string();
            let go_str = go_path.to_str().expect("utf8 path").to_string();
            // Deliberately *different* datasets (same ids, different
            // values) between the two racers — a same-data race (tried
            // first, see this trial's own report note) can't distinguish
            // "safe" from "raced but both sides happened to write
            // identical bytes anyway".
            let ((lines_a, status_a), (lines_b, status_b)) = run_race(
                binary,
                &go_path,
                &["create-race", &path_str, &go_str, "a"],
                &["create-race", &path_str, &go_str, "b"],
            );

            let result = ((lines_a, status_a.success()), (lines_b, status_b.success()));

            // Independent, authoritative check: whatever the two racing
            // `create` calls left behind, does a *third*, uninvolved
            // `open` see a structurally valid store — and is its data
            // *entirely* variant A's, *entirely* variant B's, or (the
            // real corruption signature) a mix of the two that neither
            // process's own `create` call ever actually produced on its
            // own?
            let reopened =
                GenericMmapStore::<Order, Status, Amount>::open(fixed_base_orders(), &path);
            print_reopen_verdict_with_mix_check(trial, "create-race", &reopened);

            let _ = std::fs::remove_dir_all(&dir);
            result
        })
        .collect()
}

/// Prints the post-race reopen result and, when it succeeds, whether the
/// two records' values are self-consistent (both from variant A: 2500 +
/// 4200; both from variant B: 9999 + 8888) or a *mix* — a mix is the
/// unambiguous signature of real corruption: no single un-raced `create`
/// call, from either variant, would ever produce it.
fn print_reopen_verdict_with_mix_check(
    trial: usize,
    label: &str,
    reopened: &Result<
        GenericMmapStore<Order, Status, Amount>,
        rusty_multimodal_db::durability::DurabilityError,
    >,
) {
    match reopened {
        Ok(store) => {
            use rusty_multimodal_db::generic::query::GetById;
            let a = GetById::get(store, Uuid::from_u128(1)).map(|o| o.amount_cents);
            let b = GetById::get(store, Uuid::from_u128(2)).map(|o| o.amount_cents);
            let all_variant_a = a == Some(2_500) && b == Some(4_200);
            let all_variant_b = a == Some(9_999) && b == Some(8_888);
            let verdict = if all_variant_a {
                "consistent (entirely variant A)"
            } else if all_variant_b {
                "consistent (entirely variant B)"
            } else {
                "MIXED — corruption: neither racer's create() alone could have produced this"
            };
            println!(
                "  [{label}] trial {trial}: post-race reopen OK, id1={a:?} id2={b:?} — {verdict}"
            );
        }
        Err(error) => {
            println!("  [{label}] trial {trial}: post-race reopen FAILED: {error}");
        }
    }
}

struct OpenRaceResult {
    a_reported: Option<String>,
    b_reported: Option<String>,
    a_final_value: Option<i64>,
    b_final_value: Option<i64>,
}

fn trial_open_new_record_race(binary: &Path) -> Vec<OpenRaceResult> {
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("multiprocess_open_race_{trial}"))
                .expect("fresh temp dir for open-new-record-race trial");
            let path = dir.join("orders.mmap");
            let go_path = dir.join("orders.mmap.go");

            {
                let _store =
                    GenericMmapStore::<Order, Status, Amount>::create(fixed_base_orders(), &path)
                        .expect("create the pre-existing base file");
            }

            let new_id_a: u128 = 1000 + trial as u128;
            let new_id_b: u128 = 2000 + trial as u128;
            let path_str = path.to_str().expect("utf8 path").to_string();
            let go_str = go_path.to_str().expect("utf8 path").to_string();
            let ((lines_a, status_a), (lines_b, status_b)) = run_race(
                binary,
                &go_path,
                &[
                    "open-new-record-race",
                    &path_str,
                    &go_str,
                    &new_id_a.to_string(),
                    "111111",
                ],
                &[
                    "open-new-record-race",
                    &path_str,
                    &go_str,
                    &new_id_b.to_string(),
                    "222222",
                ],
            );
            assert!(status_a.success(), "child A panicked: {lines_a:?}");
            assert!(status_b.success(), "child B panicked: {lines_b:?}");
            let a_reported = lines_a.into_iter().find(|l| l.starts_with("OPEN_"));
            let b_reported = lines_b.into_iter().find(|l| l.starts_with("OPEN_"));

            // Raw-byte proof the collision is gone: post-fix, both
            // processes' appends land at *distinct* positions (2 and 3,
            // in whichever order the kernel actually placed them) — not
            // the same position 2 the pre-fix design collided on. Check
            // both slots and confirm each racing id is present exactly
            // once, at some position, rather than one clobbering the
            // other.
            let slot_2_id = read_slot_id(&path, 2);
            let slot_3_id = read_slot_id(&path, 3);
            let occupants = [slot_2_id, slot_3_id];
            let both_present = occupants.contains(&Uuid::from_u128(new_id_a))
                && occupants.contains(&Uuid::from_u128(new_id_b));
            let verdict = if both_present {
                "PASS (both ids present at distinct slots — no collision)"
            } else {
                "FAIL (collision — one id missing from slots 2/3)"
            };
            println!(
                "  [open-new-record-race] trial {trial}: slot 2 = {slot_2_id}, slot 3 = {slot_3_id} — {verdict}"
            );

            // Authoritative check: a fresh, third-party reopen supplying
            // BOTH new records — does it see both, correctly, or did one
            // clobber the other / does the file's own bookkeeping now
            // disagree with what's actually on disk?
            let mut all_records = fixed_base_orders();
            all_records.push(Order {
                id: Uuid::from_u128(new_id_a),
                customer_id: Uuid::from_u128(100),
                amount_cents: 111_111,
                status: OrderStatus::Pending,
                created_at_unix_ms: 3_000,
                discount_cents: 0,
            });
            all_records.push(Order {
                id: Uuid::from_u128(new_id_b),
                customer_id: Uuid::from_u128(100),
                amount_cents: 222_222,
                status: OrderStatus::Pending,
                created_at_unix_ms: 3_000,
                discount_cents: 0,
            });
            let reopened = GenericMmapStore::<Order, Status, Amount>::open(all_records, &path);
            let (a_final_value, b_final_value) = match &reopened {
                Ok(store) => {
                    use rusty_multimodal_db::generic::query::GetById;
                    (
                        GetById::get(store, Uuid::from_u128(new_id_a)).map(|o| o.amount_cents),
                        GetById::get(store, Uuid::from_u128(new_id_b)).map(|o| o.amount_cents),
                    )
                }
                Err(error) => {
                    println!(
                        "  [open-new-record-race] trial {trial}: post-race reopen FAILED: {error}"
                    );
                    (None, None)
                }
            };

            let _ = std::fs::remove_dir_all(&dir);
            OpenRaceResult {
                a_reported,
                b_reported,
                a_final_value,
                b_final_value,
            }
        })
        .collect()
}

fn trial_update_race(binary: &Path) -> Vec<i64> {
    let pattern_a = i64::from_le_bytes([0x11u8; 8]);
    let pattern_b = i64::from_le_bytes([0x22u8; 8]);
    let id: u128 = 1;
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("multiprocess_update_race_{trial}"))
                .expect("fresh temp dir for update-race trial");
            let path = dir.join("orders.mmap");
            let go_path = dir.join("orders.mmap.go");
            {
                let _store =
                    GenericMmapStore::<Order, Status, Amount>::create(fixed_base_orders(), &path)
                        .expect("create the pre-existing base file");
            }

            let path_str = path.to_str().expect("utf8 path").to_string();
            let go_str = go_path.to_str().expect("utf8 path").to_string();
            let ((lines_a, status_a), (lines_b, status_b)) = run_race(
                binary,
                &go_path,
                &[
                    "update-race",
                    &path_str,
                    &go_str,
                    &id.to_string(),
                    &pattern_a.to_string(),
                    &RACE_ITERATIONS.to_string(),
                ],
                &[
                    "update-race",
                    &path_str,
                    &go_str,
                    &id.to_string(),
                    &pattern_b.to_string(),
                    &RACE_ITERATIONS.to_string(),
                ],
            );
            assert!(status_a.success(), "child A panicked: {lines_a:?}");
            assert!(status_b.success(), "child B panicked: {lines_b:?}");

            let reopened =
                GenericMmapStore::<Order, Status, Amount>::open(fixed_base_orders(), &path)
                    .expect("reopen after the update race");
            use rusty_multimodal_db::generic::query::GetById;
            let value = GetById::get(&reopened, Uuid::from_u128(id))
                .expect("record exists")
                .amount_cents;

            let _ = std::fs::remove_dir_all(&dir);
            let _ = trial;
            value
        })
        .collect()
}

struct ReadDuringWriteResult {
    torn_examples: Vec<String>,
}

fn trial_read_during_write(binary: &Path) -> Vec<ReadDuringWriteResult> {
    let pattern_a = i64::from_le_bytes([0x33u8; 8]);
    let pattern_b = i64::from_le_bytes([0x44u8; 8]);
    let id: u128 = 1;
    let seed = fixed_base_orders()[0].amount_cents;
    (0..TRIALS)
        .map(|trial| {
            let dir = fresh_temp_dir(&format!("multiprocess_read_during_write_{trial}"))
                .expect("fresh temp dir for read-during-write trial");
            let path = dir.join("orders.mmap");
            let go_path = dir.join("orders.mmap.go");
            {
                let _store =
                    GenericMmapStore::<Order, Status, Amount>::create(fixed_base_orders(), &path)
                        .expect("create the pre-existing base file");
            }

            let path_str = path.to_str().expect("utf8 path").to_string();
            let go_str = go_path.to_str().expect("utf8 path").to_string();
            // Writer alternates pattern_a/pattern_b every RACE_ITERATIONS/2
            // by running two back-to-back update-race style bursts isn't
            // available in one process invocation, so instead: spawn the
            // writer as a dedicated alternating-update process and the
            // reader as a plain read-only-race process, racing together.
            let ((writer_lines, writer_status), (reader_lines, reader_status)) = run_race(
                binary,
                &go_path,
                &[
                    "update-race",
                    &path_str,
                    &go_str,
                    &id.to_string(),
                    &pattern_a.to_string(),
                    &RACE_ITERATIONS.to_string(),
                ],
                &[
                    "read-only-race",
                    &path_str,
                    &go_str,
                    &id.to_string(),
                    &pattern_a.to_string(),
                    &pattern_b.to_string(),
                    &seed.to_string(),
                    &RACE_ITERATIONS.to_string(),
                ],
            );
            assert!(writer_status.success(), "writer panicked: {writer_lines:?}");
            assert!(reader_status.success(), "reader panicked: {reader_lines:?}");

            let torn_line = reader_lines
                .into_iter()
                .find(|l| l.starts_with("READ_DONE"))
                .unwrap_or_default();
            let torn_examples =
                if let Some(rest) = torn_line.strip_prefix("READ_DONE torn_examples=") {
                    if rest == "[]" {
                        Vec::new()
                    } else {
                        vec![rest.to_string()]
                    }
                } else {
                    vec![format!("UNPARSEABLE: {torn_line}")]
                };

            let _ = std::fs::remove_dir_all(&dir);
            let _ = trial;
            ReadDuringWriteResult { torn_examples }
        })
        .collect()
}

fn main() {
    let binary = writer_binary_path();
    println!("Using writer binary: {}", binary.display());
    println!();

    println!("=== Trial 1: two processes racing GenericMmapStore::create on the same path ===");
    for (trial, ((lines_a, ok_a), (lines_b, ok_b))) in
        trial_create_race(&binary).into_iter().enumerate()
    {
        println!(
            "  trial {trial}: A ok={ok_a} {:?} | B ok={ok_b} {:?}",
            lines_a.last(),
            lines_b.last()
        );
    }
    println!();

    println!(
        "=== Trial 2: two processes racing open() each appending a different new record (the \"next free slot\" race) ==="
    );
    for (trial, result) in trial_open_new_record_race(&binary).into_iter().enumerate() {
        let verdict =
            if result.a_final_value == Some(111_111) && result.b_final_value == Some(222_222) {
                "PASS (both records intact after the race)"
            } else {
                "FAIL (a value is missing or wrong — collision)"
            };
        println!(
            "  trial {trial}: A reported={:?} A final value={:?} (expect Some(111111)) | \
             B reported={:?} B final value={:?} (expect Some(222222)) — {verdict}",
            result.a_reported, result.a_final_value, result.b_reported, result.b_final_value
        );
    }
    println!();

    println!(
        "=== Trial 3: two processes racing UpdateField::update on the SAME existing record id ==="
    );
    let pattern_a = i64::from_le_bytes([0x11u8; 8]);
    let pattern_b = i64::from_le_bytes([0x22u8; 8]);
    println!("  pattern A={pattern_a}, pattern B={pattern_b}");
    for (trial, value) in trial_update_race(&binary).into_iter().enumerate() {
        let verdict = if value == pattern_a || value == pattern_b {
            "PASS (exactly one known pattern — last-write-wins, not torn)"
        } else {
            "FAIL (torn value — neither pattern)"
        };
        println!("  trial {trial}: final value={value} — {verdict}");
    }
    println!();

    println!(
        "=== Trial 4: one process writing (alternating patterns), one process reading, on the same id ==="
    );
    for (trial, result) in trial_read_during_write(&binary).into_iter().enumerate() {
        let verdict = if result.torn_examples.is_empty() {
            "PASS (reader never observed a value outside {seed, pattern_a, pattern_b})"
        } else {
            "FAIL (reader observed a torn value)"
        };
        println!(
            "  trial {trial}: torn_examples={:?} — {verdict}",
            result.torn_examples
        );
    }
}
