//! Targeted diagnostic for the `scan_ages` 100K cache-miss crossover
//! flagged in `RESULTS.md`: at 100K records, `SoaStore` has *fewer*
//! generic `cache-misses` than `CanonicalCachedStore` on `scan_ages`, the
//! opposite of the 1M result — despite both backends running the
//! identical `.clone()`-of-a-packed-`Vec<u32>` code path at every size
//! (re-verified in `src/store/soa.rs` / `src/store/canonical_cached.rs`;
//! this file makes no `src/` changes).
//!
//! `benches/cache_events.rs` only captures the generic
//! `HardwareEventType::CacheMisses`/`CacheReferences` events, which don't
//! say *which* cache tier (L1 vs last-level) the misses happen at. This
//! target uses `perfcnt`'s `from_cache_event` to separate L1 data-cache
//! read accesses/misses from last-level-cache read accesses/misses,
//! narrowly scoped to `scan_ages` on `SoaStore` vs. `CanonicalCachedStore`
//! at 100K and 1M — the two sizes that bracket the crossover. Narrow on
//! purpose: this is a follow-up diagnostic, not a rewrite of
//! `benches/cache_events.rs`'s general coverage.
//!
//! **Each measurement type gets its own Criterion benchmark-group name**
//! (`l1d_access`, `l1d_miss`, `ll_access`, `ll_miss`), not a shared
//! `scan_ages` group repeated four times. Criterion's on-disk baseline
//! storage is keyed by group/function/parameter, not by which
//! `criterion_group!` produced it — reusing one group name across four
//! different measurement types would make each later group's `change:
//! ...%` line compare against the *previous* measurement type's numbers
//! (e.g. L1-miss vs. L1-access) rather than a real historical baseline.
//! An early version of this file had exactly that bug; fixed after the
//! first real run on `baileyai` produced nonsense deltas.
//!
//! **The `ll_*` groups are expected to fail on AMD hardware.** Confirmed
//! on `baileyai` (AMD Ryzen AI Max+ 395): `CacheId::LL` via the generic
//! `PERF_TYPE_HW_CACHE` interface returns `ENOENT`
//! (`Could not create counter: Os { code: 2, kind: NotFound, ... }`) —
//! AMD's PMU doesn't implement that generic last-level-cache descriptor
//! the way Intel's does. Getting L2/L3-specific counts on AMD needs raw,
//! model-specific perf event codes (documented per-chip in AMD's PPR),
//! which is out of scope for this benchmark harness. `l1d_access`/
//! `l1d_miss` do work on AMD and are the useful signal this diagnostic
//! can actually get from this hardware; see `RESULTS.md` for what they
//! showed.
//!
//! Same platform/feature gating as `benches/cache_events.rs` — Linux
//! only, behind the `perf-events` feature, requires real PMU access
//! (`baileyai` or equivalent bare-metal Linux; see
//! `docs/decisions/ADR-0002-cache-miss-instrumentation-platform.md`).
//!
//! Run with: `cargo bench --features perf-events --bench scan_ages_crossover`

#![cfg(target_os = "linux")]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use criterion_perf_events::Perf;
use perfcnt::linux::{CacheId, CacheOpId, CacheOpResultId, PerfCounterBuilderLinux as Builder};
use rusty_multimodal_db::bench_support::build_dataset;
use rusty_multimodal_db::store::{CanonicalCachedStore, SoaStore};
use rusty_multimodal_db::DogStore;

/// The two sizes that bracket the crossover: 100K (SoA had fewer misses
/// on the generic counters) and 1M (Canonical+cache had fewer misses).
const CROSSOVER_SIZES: [usize; 2] = [100_000, 1_000_000];

/// Runs `scan_ages` for both backends at both crossover sizes under
/// Criterion group `group_name` — a distinct name per measurement type,
/// so each one gets its own on-disk baseline (see the module doc comment
/// on why that matters).
fn bench_scan_ages(c: &mut Criterion<Perf>, group_name: &str) {
    let mut group = c.benchmark_group(group_name);
    for &n in &CROSSOVER_SIZES {
        let dataset = build_dataset(n);
        let soa = SoaStore::from(dataset.records.clone());
        let canonical_cached = CanonicalCachedStore::from(dataset.records.clone());

        group.bench_with_input(BenchmarkId::new("soa", n), &n, |b, _| {
            b.iter(|| black_box(soa.scan_ages()));
        });
        group.bench_with_input(BenchmarkId::new("canonical_cached", n), &n, |b, _| {
            b.iter(|| black_box(canonical_cached.scan_ages()));
        });
    }
    group.finish();
}

fn l1d_access_bench(c: &mut Criterion<Perf>) {
    bench_scan_ages(c, "l1d_access");
}
fn l1d_miss_bench(c: &mut Criterion<Perf>) {
    bench_scan_ages(c, "l1d_miss");
}
fn ll_access_bench(c: &mut Criterion<Perf>) {
    bench_scan_ages(c, "ll_access");
}
fn ll_miss_bench(c: &mut Criterion<Perf>) {
    bench_scan_ages(c, "ll_miss");
}

criterion_group!(
    name = l1d_access;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::L1D,
        CacheOpId::Read,
        CacheOpResultId::Access
    )));
    targets = l1d_access_bench
);
criterion_group!(
    name = l1d_miss;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::L1D,
        CacheOpId::Read,
        CacheOpResultId::Miss
    )));
    targets = l1d_miss_bench
);
criterion_group!(
    name = ll_access;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::LL,
        CacheOpId::Read,
        CacheOpResultId::Access
    )));
    targets = ll_access_bench
);
criterion_group!(
    name = ll_miss;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::LL,
        CacheOpId::Read,
        CacheOpResultId::Miss
    )));
    targets = ll_miss_bench
);
criterion_main!(l1d_access, l1d_miss, ll_access, ll_miss);
