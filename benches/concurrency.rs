//! Custom (non-Criterion) throughput harness for the concurrency
//! prototypes (`src/concurrency/*.rs`, `STORAGE-010`).
//!
//! # Why not Criterion
//!
//! Every other bench target in this crate uses Criterion, timing repeated
//! calls to one closure on one thread. This benchmark's actual question —
//! "what's the aggregate operations/second across N concurrently-running
//! threads, and how does that scale with N" — isn't what Criterion's
//! `b.iter` model measures: it has no built-in notion of "spawn N threads,
//! run them concurrently for a while, sum their throughput." Rather than
//! force that shape through Criterion, this is a small, custom harness:
//! spawn `threads` worker threads, synchronize their start with a
//! `Barrier`, have each run a fixed number of `MixedWorkloadDriver`
//! operations against one shared store, and take the wall-clock time of
//! the *slowest* thread (the real bound on when "everyone is done") to
//! compute aggregate ops/sec.
//!
//! # Reusing `MixedWorkloadDriver`, not a new workload generator
//!
//! `MixedWorkloadDriver::run_one_concurrent` (`src/bench_support.rs`, new
//! this pass, additive — `run_one` and every benchmark using it are
//! unchanged) drives the exact same blended `get`/`update_age`/`scan_ages`
//! sequence the mixed-workload round already established, just against a
//! `ConcurrentStore` (`&S`, shared) instead of a `DogStore` (`&mut S`,
//! exclusive). Each thread owns its own driver (seeded independently, so
//! each draws its own op sequence) — only the store instance is shared.
//!
//! # Sizes: 1K and 100K, not 1M
//!
//! Thread count is now a second swept axis on top of size and write ratio,
//! and the full size × ratio × thread-count × variant matrix grows fast:
//! at the full `SIZES` used elsewhere in this crate (1K/100K/1M), this
//! would be 3 × 3 × 4 × 4 = 144 cases. 1M is dropped here specifically:
//! it's the size where the *other* benchmarks' overhead is already
//! dominated by cache effects at this crate's record scale, and repeating
//! that story a third time under a second, already-large sweep axis
//! wasn't judged worth roughly 50% more total runtime for this pass. 1K
//! and 100K still span two orders of magnitude, enough to see whether the
//! qualitative thread-scaling story holds across scale — if it doesn't,
//! that's exactly the kind of thing `RESULTS.md`'s open questions exist to
//! flag for a 1M follow-up.
//!
//! # Thread counts: 1/4/32/64, picked for `baileyai`'s real core count
//!
//! The original container run swept 1/4/8/16 unconditionally on a 4-core
//! container, so 8 and 16 were honest but oversubscribed data points, not
//! genuine added parallelism. A second pass substituted the owner's Windows
//! dev machine (24 logical / 12 physical cores) because `baileyai` was
//! unreachable over SSH from that session, and swept 1/4/24/48 to match. A
//! third pass (this one) runs directly on `baileyai` itself — this session
//! executes on that machine, so no SSH substitution is needed. `baileyai`
//! reports 32 logical processors via both `std::thread::available_parallelism()`
//! and `nproc` (16 physical cores, AMD Ryzen AI MAX+ 395, SMT enabled) — so
//! the swept counts are changed to match: `1` (serial baseline), `4` (kept,
//! specifically so it's directly comparable to both the container's and the
//! Windows machine's own non-oversubscribed 4-thread rows), `32` (this
//! machine's actual core count — the first *genuinely* non-oversubscribed
//! high-thread-count data point this environment can produce), and `64` (2x
//! cores, meaningfully past it, on purpose, matching the "2x cores" pattern
//! the Windows-machine pass already established at 48). 8/16/24/48 are
//! dropped here since none of them are at or past this machine's real
//! headroom — they'd land in the middle of it instead.

use rusty_multimodal_db::bench_support::{
    build_dataset, Dataset, MixedWorkloadConfig, MixedWorkloadDriver, MIXED_WRITE_RATIOS, SEED,
};
use rusty_multimodal_db::concurrency::{
    ActorStore, ConcurrentStore, DashMapStore, GlobalRwLockStore, ShardedStore,
};
use rusty_multimodal_db::ProductionStore;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

/// Dataset sizes swept — see module docs for why 1M is excluded here.
const SIZES: [usize; 2] = [1_000, 100_000];
/// Thread counts swept — see module docs for why these match `baileyai`'s
/// real core count rather than reusing an earlier environment's list.
const THREAD_COUNTS: [usize; 4] = [1, 4, 32, 64];
/// Operations each worker thread performs per (variant, size, ratio,
/// thread-count) case — fixed regardless of thread count, so higher
/// thread counts do proportionally more total work, which is what "does
/// aggregate throughput keep scaling with more threads" needs to measure.
const OPS_PER_THREAD: usize = 10_000;

/// Run one (store, write ratio, thread count) case: `threads` worker
/// threads, each with its own independently-seeded `MixedWorkloadDriver`,
/// synchronized to start together via `barrier`, each performing
/// `OPS_PER_THREAD` operations against the one shared `store`. Returns
/// aggregate throughput in operations/second, computed from the *slowest*
/// thread's elapsed time (the real bound on when the whole batch is done),
/// not the average — a slow straggler thread should show up as lower
/// throughput, not be averaged away by faster ones.
fn run_throughput<S: ConcurrentStore + 'static>(
    store: &Arc<S>,
    dataset: &Dataset,
    write_ratio: f64,
    threads: usize,
) -> f64 {
    let Ok(config) = MixedWorkloadConfig::new(write_ratio) else {
        // MIXED_WRITE_RATIOS (bench_support.rs) is a fixed [0.0, 1.0]
        // constant array — this branch is unreachable.
        return 0.0;
    };
    let barrier = Arc::new(Barrier::new(threads));
    let sample_ids = Arc::new(dataset.sample_ids.clone());

    let mut handles = Vec::with_capacity(threads);
    for thread_index in 0..threads {
        let store = Arc::clone(store);
        let barrier = Arc::clone(&barrier);
        let sample_ids = Arc::clone(&sample_ids);
        handles.push(thread::spawn(move || {
            let mut driver =
                MixedWorkloadDriver::new(config, SEED ^ thread_index as u64, sample_ids.len());
            barrier.wait();
            let start = Instant::now();
            for _ in 0..OPS_PER_THREAD {
                let _ = driver.run_one_concurrent(store.as_ref(), &sample_ids);
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

fn bench_variant<S: ConcurrentStore + 'static>(name: &str, size: usize, dataset: &Dataset) {
    let store = Arc::new(S::new(dataset.records.clone(), dataset.edges.clone()));
    for &write_ratio in &MIXED_WRITE_RATIOS {
        for &threads in &THREAD_COUNTS {
            let ops_per_sec = run_throughput(&store, dataset, write_ratio, threads);
            println!(
                "{name:<14} {size:>10} {:>7.0}% {threads:>8} {ops_per_sec:>16.0}",
                write_ratio * 100.0
            );
        }
    }
}

fn main() {
    let available_parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    println!("std::thread::available_parallelism() reports: {available_parallelism}");
    println!(
        "(thread counts below are swept at 1/4/32/64 — 32 is this machine's \
         reported core count, so that row and the 4-thread row are genuine, \
         non-oversubscribed parallelism; the 64-thread row is deliberate \
         oversubscription (2x cores); see RESULTS.md's ## Concurrency section \
         for how to read them)"
    );
    println!();
    println!(
        "{:<14} {:>10} {:>8} {:>8} {:>16}",
        "variant", "size", "write%", "threads", "ops/sec"
    );

    for &size in &SIZES {
        let dataset = build_dataset(size);
        // `production` first: the crate's recommended entry point (see
        // src/production.rs) — RwLock<MmapAgeStore>, i.e. this same
        // GlobalRwLockStore locking scheme layered over mmap durability
        // instead of a plain in-memory CanonicalCachedStore. Included here
        // so its throughput is directly comparable, in the same sweep, to
        // the four in-memory-only variants that motivated this recommendation.
        bench_variant::<ProductionStore>("production", size, &dataset);
        bench_variant::<GlobalRwLockStore>("global_rwlock", size, &dataset);
        bench_variant::<ShardedStore>("sharded", size, &dataset);
        bench_variant::<DashMapStore>("dashmap", size, &dataset);
        bench_variant::<ActorStore>("actor", size, &dataset);
    }
}
