//! Cache-miss / cache-reference counting suite, using Linux's
//! `perf_event_open` via `criterion-perf-events`. Linux-only by
//! construction (the dependency itself is gated to
//! `cfg(target_os = "linux")` in `Cargo.toml`) and behind the
//! `perf-events` Cargo feature — not part of the default `cargo bench`
//! run.
//!
//! Run with: `cargo bench --features perf-events --bench cache_events`
//!
//! Requires real hardware performance-counter access (bare-metal Linux,
//! or a hypervisor that passes through the vPMU). This crate's bootstrap
//! session verified its own environment does **not** have that access —
//! `perf stat` reports `<not supported>` for every counter there — so
//! this target is built and tested for compilation here but not relied
//! on to produce real numbers except when run on hardware that does have
//! counter access (e.g. `baileyai`). See
//! `docs/decisions/ADR-0002-cache-miss-instrumentation-platform.md`.
//!
//! Covers `get` and `same_breed` — the two workloads whose cost is most
//! directly a function of memory-access pattern rather than allocation
//! or hashing overhead (`scan_ages` is dominated by the size of the
//! output `Vec` itself for all three backends, `update_age` by the
//! lookup-then-write pattern already exercised by `get`) — across all
//! three backends and three dataset sizes.

#![cfg(target_os = "linux")]

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use criterion_perf_events::Perf;
use perfcnt::linux::HardwareEventType as Hardware;
use perfcnt::linux::PerfCounterBuilderLinux as Builder;
use rusty_multimodal_db::bench_support::{build_dataset, Dataset, RoundRobin, SIZES};
use rusty_multimodal_db::store::{AosStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{DogRecord, DogStore};

fn run_get<S>(group: &mut BenchmarkGroup<'_, Perf>, name: &str, n: usize, dataset: &Dataset)
where
    S: DogStore + From<Vec<DogRecord>>,
{
    let store = S::from(dataset.records.clone());
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(store.get(black_box(id)))
        });
    });
}

fn run_same_breed<S>(group: &mut BenchmarkGroup<'_, Perf>, name: &str, n: usize, dataset: &Dataset)
where
    S: DogStore + From<Vec<DogRecord>>,
{
    let store = S::from(dataset.records.clone());
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(store.same_breed(black_box(id)))
        });
    });
}

fn cache_misses(c: &mut Criterion<Perf>) {
    let mut get_group = c.benchmark_group("get_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get::<AosStore>(&mut get_group, "aos", n, &dataset);
        run_get::<SoaStore>(&mut get_group, "soa", n, &dataset);
        run_get::<CanonicalStore>(&mut get_group, "canonical", n, &dataset);
    }
    get_group.finish();

    let mut same_breed_group = c.benchmark_group("same_breed_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_same_breed::<AosStore>(&mut same_breed_group, "aos", n, &dataset);
        run_same_breed::<SoaStore>(&mut same_breed_group, "soa", n, &dataset);
        run_same_breed::<CanonicalStore>(&mut same_breed_group, "canonical", n, &dataset);
    }
    same_breed_group.finish();
}

fn cache_references(c: &mut Criterion<Perf>) {
    let mut get_group = c.benchmark_group("get_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get::<AosStore>(&mut get_group, "aos", n, &dataset);
        run_get::<SoaStore>(&mut get_group, "soa", n, &dataset);
        run_get::<CanonicalStore>(&mut get_group, "canonical", n, &dataset);
    }
    get_group.finish();

    let mut same_breed_group = c.benchmark_group("same_breed_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_same_breed::<AosStore>(&mut same_breed_group, "aos", n, &dataset);
        run_same_breed::<SoaStore>(&mut same_breed_group, "soa", n, &dataset);
        run_same_breed::<CanonicalStore>(&mut same_breed_group, "canonical", n, &dataset);
    }
    same_breed_group.finish();
}

criterion_group!(
    name = cache_misses_bench;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_hardware_event(Hardware::CacheMisses)));
    targets = cache_misses
);
criterion_group!(
    name = cache_references_bench;
    config = Criterion::default().with_measurement(Perf::new(Builder::from_hardware_event(Hardware::CacheReferences)));
    targets = cache_references
);
criterion_main!(cache_misses_bench, cache_references_bench);
