//! Measures the real memory footprint of each `DogStore` backend, using a
//! counting global allocator rather than guessing at struct-size math.
//! Written for two of `RESULTS.md`'s open questions: "memory overhead per
//! backend" (unmeasured until now), and the `scan_ages` 100K cache-miss
//! crossover (this data doesn't resolve that on its own — see
//! `benches/scan_ages_crossover.rs` for the finer-grained perf-counter
//! follow-up — but it rules out the simplest version of the "more
//! co-resident bookkeeping bytes always means more cache misses"
//! hypothesis; see `RESULTS.md` for why).
//!
//! Cross-platform — no `perf_event_open`/PMU access needed, just the
//! standard allocator API, so (unlike `benches/cache_events.rs`) this
//! runs anywhere including this crate's own development sessions.
//!
//! Run with: `cargo run --release --example memory_footprint`

use rusty_multimodal_db::bench_support::build_dataset;
use rusty_multimodal_db::store::{AosStore, CanonicalCachedStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::DogRecord;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::SeqCst);
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn live() -> i64 {
    LIVE_BYTES.load(Ordering::SeqCst) as i64
}

/// Reports two numbers for `ctor(records)`:
/// - `live_after`: total process-wide live bytes once the store exists
///   (includes whatever else happens to be live at the time — comparable
///   *across backends within the same `n`*, since that ambient baseline
///   is identical for all four calls in one iteration, but not
///   meaningful as an absolute number on its own).
/// - `delta`: the net change in live bytes caused specifically by this
///   construction call — can be negative (e.g. `SoaStore` frees more
///   than it allocates, see the doc comment on `main`), since `records`
///   is consumed/moved into the new structure rather than cloned again.
fn measure<S>(
    label: &str,
    n: usize,
    records: Vec<DogRecord>,
    ctor: impl FnOnce(Vec<DogRecord>) -> S,
) {
    let before = live();
    let store = ctor(records);
    let after = live();
    let delta = after - before;
    println!(
        "{label:>22} n={n:>8}  live_after={after:>12}  delta={delta:>12} bytes  ({:>8.2} bytes/record)",
        delta as f64 / n as f64
    );
    std::hint::black_box(&store);
    drop(store);
}

fn main() {
    println!(
        "size_of::<DogRecord>() = {} bytes",
        std::mem::size_of::<DogRecord>()
    );
    println!(
        "size_of::<uuid::Uuid>() = {} bytes",
        std::mem::size_of::<uuid::Uuid>()
    );
    println!(
        "size_of::<String>() = {} bytes (heap payload not included)",
        std::mem::size_of::<String>()
    );
    println!();
    println!("Note on delta's sign: AosStore/SoaStore/CanonicalStore/CanonicalCachedStore");
    println!("all consume `records: Vec<DogRecord>` by value. AosStore just moves it in");
    println!("(delta ~0). SoaStore/CanonicalStore/CanonicalCachedStore free the original");
    println!("Vec<DogRecord> spine buffer once they've moved each record's fields out of");
    println!("it, so `delta` is (newly allocated) minus (that freed buffer) — legitimately");
    println!("negative for SoaStore, whose three parallel-array headers are smaller than");
    println!("one packed DogRecord array. Breed String heap payloads are moved, not");
    println!("cloned, in every case, so they don't show up in any backend's delta.");
    println!();

    for &n in &[1_000usize, 100_000, 1_000_000] {
        let dataset = build_dataset(n);
        let records = dataset.records;

        let r = records.clone();
        measure("AosStore", n, r, AosStore::from);

        let r = records.clone();
        measure("SoaStore", n, r, SoaStore::from);

        let r = records.clone();
        measure("CanonicalStore", n, r, CanonicalStore::from);

        let r = records.clone();
        measure("CanonicalCachedStore", n, r, CanonicalCachedStore::from);

        println!();
    }
}
