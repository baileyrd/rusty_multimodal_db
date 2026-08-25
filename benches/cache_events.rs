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
//! Covers all six workloads (`get`, `scan_ages`, `update_age`,
//! `same_breed`, `neighbors_one_hop`, `neighbors_two_hop`) across all four
//! backends and three dataset sizes, to match `benches/workloads.rs`. An
//! earlier draft of this file limited coverage to `get`/`same_breed` on the
//! theory that `scan_ages` is dominated by the output `Vec` allocation and
//! `update_age` by the lookup-then-write pattern already exercised by
//! `get` — but that was a guess made without real hardware-counter access
//! to check it against, and the whole point of running on `baileyai` is to
//! test hypotheses like that one directly rather than reason about them
//! from wall-clock noise. See `RESULTS.md`'s cache-miss section for the
//! `scan_ages` finding this coverage was specifically added to check.
//! `neighbors_one_hop`/`neighbors_two_hop` were added alongside the
//! `littermate_of` graph-traversal feature (`STORAGE-006`). Also covers a
//! blended `mixed_workload_write{10,50,90}` group per write ratio (see
//! `MixedWorkloadDriver` in `src/bench_support.rs` and `STORAGE-007`),
//! each again spanning all four backends and three sizes.

#![cfg(target_os = "linux")]

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use criterion_perf_events::Perf;
use perfcnt::linux::HardwareEventType as Hardware;
use perfcnt::linux::PerfCounterBuilderLinux as Builder;
use rusty_multimodal_db::bench_support::{
    build_dataset, two_hop_neighbors, Dataset, MixedWorkloadConfig, MixedWorkloadDriver,
    RoundRobin, MIXED_WRITE_RATIOS, SEED, SIZES,
};
use rusty_multimodal_db::store::{AosStore, CanonicalCachedStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{DogRecord, DogStore};
use uuid::Uuid;

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

fn run_scan_ages<S>(group: &mut BenchmarkGroup<'_, Perf>, name: &str, n: usize, dataset: &Dataset)
where
    S: DogStore + From<Vec<DogRecord>>,
{
    let store = S::from(dataset.records.clone());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| black_box(store.scan_ages()));
    });
}

/// Mirrors `benches/workloads.rs::run_update_age` — store built once and
/// reused across iterations (rotating target IDs) rather than rebuilt per
/// iteration, which would make this impractically slow at 1M records.
fn run_update_age<S>(group: &mut BenchmarkGroup<'_, Perf>, name: &str, n: usize, dataset: &Dataset)
where
    S: DogStore + From<Vec<DogRecord>>,
{
    let mut store = S::from(dataset.records.clone());
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    let mut next_age: u32 = 0;
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            next_age = next_age.wrapping_add(1) % 21;
            store
                .update_age(black_box(id), black_box(next_age))
                .expect("target id is always present");
        });
    });
}

fn run_neighbors_one_hop<S>(
    group: &mut BenchmarkGroup<'_, Perf>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) where
    S: DogStore + From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)>,
{
    let store = S::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(store.neighbors(black_box(id)))
        });
    });
}

/// Mirrors `benches/workloads.rs::run_neighbors_two_hop` — 2-hop is not a
/// trait method (ADR-0004), so this benchmarks
/// `bench_support::two_hop_neighbors` built from two rounds of `neighbors`.
fn run_neighbors_two_hop<S>(
    group: &mut BenchmarkGroup<'_, Perf>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) where
    S: DogStore + From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)>,
{
    let store = S::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(two_hop_neighbors(&store, black_box(id)))
        });
    });
}

/// Mirrors `benches/workloads.rs::run_mixed_workload` — see
/// `MixedWorkloadDriver`'s docs for the blended-sequence design.
fn run_mixed_workload<S>(
    group: &mut BenchmarkGroup<'_, Perf>,
    name: &str,
    n: usize,
    dataset: &Dataset,
    config: MixedWorkloadConfig,
) where
    S: DogStore + From<Vec<DogRecord>>,
{
    let mut store = S::from(dataset.records.clone());
    let mut driver = MixedWorkloadDriver::new(config, SEED, dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let _ = black_box(driver.run_one(&mut store, &dataset.sample_ids));
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
        run_get::<CanonicalCachedStore>(&mut get_group, "canonical_cached", n, &dataset);
    }
    get_group.finish();

    let mut scan_ages_group = c.benchmark_group("scan_ages_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_scan_ages::<AosStore>(&mut scan_ages_group, "aos", n, &dataset);
        run_scan_ages::<SoaStore>(&mut scan_ages_group, "soa", n, &dataset);
        run_scan_ages::<CanonicalStore>(&mut scan_ages_group, "canonical", n, &dataset);
        run_scan_ages::<CanonicalCachedStore>(
            &mut scan_ages_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    scan_ages_group.finish();

    let mut update_age_group = c.benchmark_group("update_age_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_update_age::<AosStore>(&mut update_age_group, "aos", n, &dataset);
        run_update_age::<SoaStore>(&mut update_age_group, "soa", n, &dataset);
        run_update_age::<CanonicalStore>(&mut update_age_group, "canonical", n, &dataset);
        run_update_age::<CanonicalCachedStore>(
            &mut update_age_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    update_age_group.finish();

    let mut same_breed_group = c.benchmark_group("same_breed_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_same_breed::<AosStore>(&mut same_breed_group, "aos", n, &dataset);
        run_same_breed::<SoaStore>(&mut same_breed_group, "soa", n, &dataset);
        run_same_breed::<CanonicalStore>(&mut same_breed_group, "canonical", n, &dataset);
        run_same_breed::<CanonicalCachedStore>(
            &mut same_breed_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    same_breed_group.finish();

    let mut neighbors_one_hop_group = c.benchmark_group("neighbors_one_hop_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_one_hop::<AosStore>(&mut neighbors_one_hop_group, "aos", n, &dataset);
        run_neighbors_one_hop::<SoaStore>(&mut neighbors_one_hop_group, "soa", n, &dataset);
        run_neighbors_one_hop::<CanonicalStore>(
            &mut neighbors_one_hop_group,
            "canonical",
            n,
            &dataset,
        );
        run_neighbors_one_hop::<CanonicalCachedStore>(
            &mut neighbors_one_hop_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    neighbors_one_hop_group.finish();

    let mut neighbors_two_hop_group = c.benchmark_group("neighbors_two_hop_cache_misses");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_two_hop::<AosStore>(&mut neighbors_two_hop_group, "aos", n, &dataset);
        run_neighbors_two_hop::<SoaStore>(&mut neighbors_two_hop_group, "soa", n, &dataset);
        run_neighbors_two_hop::<CanonicalStore>(
            &mut neighbors_two_hop_group,
            "canonical",
            n,
            &dataset,
        );
        run_neighbors_two_hop::<CanonicalCachedStore>(
            &mut neighbors_two_hop_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    neighbors_two_hop_group.finish();

    for &write_ratio in &MIXED_WRITE_RATIOS {
        let Ok(config) = MixedWorkloadConfig::new(write_ratio) else {
            // MIXED_WRITE_RATIOS is a fixed [0.0, 1.0] constant array
            // (bench_support.rs) — this branch is unreachable.
            continue;
        };
        let mut mixed_group = c.benchmark_group(format!(
            "mixed_workload_write{}_cache_misses",
            (write_ratio * 100.0).round() as u32
        ));
        for &n in &SIZES {
            let dataset = build_dataset(n);
            run_mixed_workload::<AosStore>(&mut mixed_group, "aos", n, &dataset, config);
            run_mixed_workload::<SoaStore>(&mut mixed_group, "soa", n, &dataset, config);
            run_mixed_workload::<CanonicalStore>(
                &mut mixed_group,
                "canonical",
                n,
                &dataset,
                config,
            );
            run_mixed_workload::<CanonicalCachedStore>(
                &mut mixed_group,
                "canonical_cached",
                n,
                &dataset,
                config,
            );
        }
        mixed_group.finish();
    }
}

fn cache_references(c: &mut Criterion<Perf>) {
    let mut get_group = c.benchmark_group("get_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get::<AosStore>(&mut get_group, "aos", n, &dataset);
        run_get::<SoaStore>(&mut get_group, "soa", n, &dataset);
        run_get::<CanonicalStore>(&mut get_group, "canonical", n, &dataset);
        run_get::<CanonicalCachedStore>(&mut get_group, "canonical_cached", n, &dataset);
    }
    get_group.finish();

    let mut scan_ages_group = c.benchmark_group("scan_ages_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_scan_ages::<AosStore>(&mut scan_ages_group, "aos", n, &dataset);
        run_scan_ages::<SoaStore>(&mut scan_ages_group, "soa", n, &dataset);
        run_scan_ages::<CanonicalStore>(&mut scan_ages_group, "canonical", n, &dataset);
        run_scan_ages::<CanonicalCachedStore>(
            &mut scan_ages_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    scan_ages_group.finish();

    let mut update_age_group = c.benchmark_group("update_age_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_update_age::<AosStore>(&mut update_age_group, "aos", n, &dataset);
        run_update_age::<SoaStore>(&mut update_age_group, "soa", n, &dataset);
        run_update_age::<CanonicalStore>(&mut update_age_group, "canonical", n, &dataset);
        run_update_age::<CanonicalCachedStore>(
            &mut update_age_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    update_age_group.finish();

    let mut same_breed_group = c.benchmark_group("same_breed_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_same_breed::<AosStore>(&mut same_breed_group, "aos", n, &dataset);
        run_same_breed::<SoaStore>(&mut same_breed_group, "soa", n, &dataset);
        run_same_breed::<CanonicalStore>(&mut same_breed_group, "canonical", n, &dataset);
        run_same_breed::<CanonicalCachedStore>(
            &mut same_breed_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    same_breed_group.finish();

    let mut neighbors_one_hop_group = c.benchmark_group("neighbors_one_hop_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_one_hop::<AosStore>(&mut neighbors_one_hop_group, "aos", n, &dataset);
        run_neighbors_one_hop::<SoaStore>(&mut neighbors_one_hop_group, "soa", n, &dataset);
        run_neighbors_one_hop::<CanonicalStore>(
            &mut neighbors_one_hop_group,
            "canonical",
            n,
            &dataset,
        );
        run_neighbors_one_hop::<CanonicalCachedStore>(
            &mut neighbors_one_hop_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    neighbors_one_hop_group.finish();

    let mut neighbors_two_hop_group = c.benchmark_group("neighbors_two_hop_cache_references");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_two_hop::<AosStore>(&mut neighbors_two_hop_group, "aos", n, &dataset);
        run_neighbors_two_hop::<SoaStore>(&mut neighbors_two_hop_group, "soa", n, &dataset);
        run_neighbors_two_hop::<CanonicalStore>(
            &mut neighbors_two_hop_group,
            "canonical",
            n,
            &dataset,
        );
        run_neighbors_two_hop::<CanonicalCachedStore>(
            &mut neighbors_two_hop_group,
            "canonical_cached",
            n,
            &dataset,
        );
    }
    neighbors_two_hop_group.finish();

    for &write_ratio in &MIXED_WRITE_RATIOS {
        let Ok(config) = MixedWorkloadConfig::new(write_ratio) else {
            // MIXED_WRITE_RATIOS is a fixed [0.0, 1.0] constant array
            // (bench_support.rs) — this branch is unreachable.
            continue;
        };
        let mut mixed_group = c.benchmark_group(format!(
            "mixed_workload_write{}_cache_references",
            (write_ratio * 100.0).round() as u32
        ));
        for &n in &SIZES {
            let dataset = build_dataset(n);
            run_mixed_workload::<AosStore>(&mut mixed_group, "aos", n, &dataset, config);
            run_mixed_workload::<SoaStore>(&mut mixed_group, "soa", n, &dataset, config);
            run_mixed_workload::<CanonicalStore>(
                &mut mixed_group,
                "canonical",
                n,
                &dataset,
                config,
            );
            run_mixed_workload::<CanonicalCachedStore>(
                &mut mixed_group,
                "canonical_cached",
                n,
                &dataset,
                config,
            );
        }
        mixed_group.finish();
    }
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
