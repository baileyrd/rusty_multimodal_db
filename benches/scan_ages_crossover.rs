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
//! narrowly scoped to `scan_ages` on `SoaStore` vs `CanonicalCachedStore`
//! at 100K and 1M — the two sizes that bracket the crossover. Narrow on
//! purpose: this is a follow-up diagnostic, not a rewrite of
//! `benches/cache_events.rs`'s general coverage.
//!
//! Same platform/feature gating as `benches/cache_events.rs` — Linux
//! only, behind the `perf-events` feature, requires real PMU access
//! (`baileyai` or equivalent bare-metal Linux; see
//! `docs/decisions/ADR-0002-cache-miss-instrumentation-platform.md`).
//! This crate's own sessions have never had that access, so this target
//! is verified to compile but not run from here — see `RESULTS.md`'s
//! `scan_ages` 100K crossover entry for what's known so far from
//! measurements this session *could* take (real memory-footprint sizes
//! via `examples/memory_footprint.rs`) and what's still open pending an
//! actual run of this file.
//!
//! Run with: `cargo bench --features perf-events --bench scan_ages_crossover`

#![cfg(target_os = "linux")]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use criterion_perf_events::Perf;
use perfcnt::linux::{CacheId, CacheOpId, CacheOpResultId, PerfCounterBuilderLinux as Builder};
use rusty_multimodal_db::bench_support::build_dataset;
use rusty_multimodal_db::store::{CanonicalCachedStore, SoaStore};
use rusty_multimodal_db::DogStore;

/// The two sizes that bracket the crossover: 100K (SoA had fewer misses)
/// and 1M (Canonical+cache had fewer misses).
const CROSSOVER_SIZES: [usize; 2] = [100_000, 1_000_000];

fn bench_scan_ages(c: &mut Criterion<Perf>) {
    let mut group = c.benchmark_group("scan_ages");
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

criterion_group!(
    name = l1d_read_access;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::L1D,
        CacheOpId::Read,
        CacheOpResultId::Access
    )));
    targets = bench_scan_ages
);
criterion_group!(
    name = l1d_read_miss;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::L1D,
        CacheOpId::Read,
        CacheOpResultId::Miss
    )));
    targets = bench_scan_ages
);
criterion_group!(
    name = ll_read_access;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::LL,
        CacheOpId::Read,
        CacheOpResultId::Access
    )));
    targets = bench_scan_ages
);
criterion_group!(
    name = ll_read_miss;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_cache_event(
        CacheId::LL,
        CacheOpId::Read,
        CacheOpResultId::Miss
    )));
    targets = bench_scan_ages
);
criterion_main!(l1d_read_access, l1d_read_miss, ll_read_access, ll_read_miss);
