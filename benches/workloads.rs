//! Wall-clock Criterion suite: one group per workload, each covering all
//! three backends at three dataset sizes, built from identical generator
//! output within a size. Cross-platform — see
//! `docs/decisions/ADR-0002-cache-miss-instrumentation-platform.md` for
//! the separate, Linux-only cache-miss-counting suite
//! (`benches/cache_events.rs`).

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use rusty_multimodal_db::bench_support::{
    build_dataset, two_hop_neighbors, Dataset, MixedWorkloadConfig, MixedWorkloadDriver,
    RoundRobin, MIXED_WRITE_RATIOS, SEED, SIZES,
};
use rusty_multimodal_db::store::{AosStore, CanonicalCachedStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{DogRecord, DogStore};
use uuid::Uuid;

fn bench_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("get");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get::<AosStore>(&mut group, "aos", n, &dataset);
        run_get::<SoaStore>(&mut group, "soa", n, &dataset);
        run_get::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_get::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

fn run_get<S>(group: &mut BenchmarkGroup<'_, WallTime>, name: &str, n: usize, dataset: &Dataset)
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

fn bench_scan_ages(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_ages");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_scan_ages::<AosStore>(&mut group, "aos", n, &dataset);
        run_scan_ages::<SoaStore>(&mut group, "soa", n, &dataset);
        run_scan_ages::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_scan_ages::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

fn run_scan_ages<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) where
    S: DogStore + From<Vec<DogRecord>>,
{
    let store = S::from(dataset.records.clone());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| black_box(store.scan_ages()));
    });
}

fn bench_update_age(c: &mut Criterion) {
    let mut group = c.benchmark_group("update_age");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_update_age::<AosStore>(&mut group, "aos", n, &dataset);
        run_update_age::<SoaStore>(&mut group, "soa", n, &dataset);
        run_update_age::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_update_age::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

/// `update_age` overwrites `age` in place and never resizes any backing
/// structure or touches the breed index, so its cost doesn't depend on
/// which value a prior iteration wrote — the store is built once and
/// reused across iterations (with rotating target IDs) rather than
/// rebuilt per iteration, which would make this impractically slow at 1M
/// records. See `STORAGE-003`'s data/state notes.
fn run_update_age<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) where
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

fn bench_same_breed(c: &mut Criterion) {
    let mut group = c.benchmark_group("same_breed");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_same_breed::<AosStore>(&mut group, "aos", n, &dataset);
        run_same_breed::<SoaStore>(&mut group, "soa", n, &dataset);
        run_same_breed::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_same_breed::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

fn run_same_breed<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &Dataset,
) where
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

fn bench_neighbors_one_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_one_hop");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_one_hop::<AosStore>(&mut group, "aos", n, &dataset);
        run_neighbors_one_hop::<SoaStore>(&mut group, "soa", n, &dataset);
        run_neighbors_one_hop::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_neighbors_one_hop::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

fn run_neighbors_one_hop<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
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

/// 2-hop traversal is not a trait method (see ADR-0004) — this benchmarks
/// `bench_support::two_hop_neighbors`, which is built entirely out of two
/// rounds of `neighbors` calls, generically over any `DogStore`.
fn bench_neighbors_two_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_two_hop");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_two_hop::<AosStore>(&mut group, "aos", n, &dataset);
        run_neighbors_two_hop::<SoaStore>(&mut group, "soa", n, &dataset);
        run_neighbors_two_hop::<CanonicalStore>(&mut group, "canonical", n, &dataset);
        run_neighbors_two_hop::<CanonicalCachedStore>(&mut group, "canonical_cached", n, &dataset);
    }
    group.finish();
}

fn run_neighbors_two_hop<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
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

/// Blended `get`/`update_age`/`scan_ages` sequence — see
/// `MixedWorkloadDriver`'s docs. One group per write ratio (so the group
/// name carries the swept dimension `BenchmarkId` doesn't have room for
/// alongside backend/size), each covering all four backends at all three
/// sizes, matching every other workload's structure.
fn bench_mixed_workload(c: &mut Criterion) {
    for &write_ratio in &MIXED_WRITE_RATIOS {
        let Ok(config) = MixedWorkloadConfig::new(write_ratio) else {
            // MIXED_WRITE_RATIOS is a fixed [0.0, 1.0] constant array
            // (bench_support.rs) — this branch is unreachable.
            continue;
        };
        let group_name = format!(
            "mixed_workload_write{}",
            (write_ratio * 100.0).round() as u32
        );
        let mut group = c.benchmark_group(group_name);
        for &n in &SIZES {
            let dataset = build_dataset(n);
            run_mixed_workload::<AosStore>(&mut group, "aos", n, &dataset, config);
            run_mixed_workload::<SoaStore>(&mut group, "soa", n, &dataset, config);
            run_mixed_workload::<CanonicalStore>(&mut group, "canonical", n, &dataset, config);
            run_mixed_workload::<CanonicalCachedStore>(
                &mut group,
                "canonical_cached",
                n,
                &dataset,
                config,
            );
        }
        group.finish();
    }
}

fn run_mixed_workload<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
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

criterion_group!(
    benches,
    bench_get,
    bench_scan_ages,
    bench_update_age,
    bench_same_breed,
    bench_neighbors_one_hop,
    bench_neighbors_two_hop,
    bench_mixed_workload
);
criterion_main!(benches);
