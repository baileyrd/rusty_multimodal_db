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
use rusty_multimodal_db::bench_support::{build_dataset, Dataset, RoundRobin, SIZES};
use rusty_multimodal_db::store::{AosStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{DogRecord, DogStore};

fn bench_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("get");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get::<AosStore>(&mut group, "aos", n, &dataset);
        run_get::<SoaStore>(&mut group, "soa", n, &dataset);
        run_get::<CanonicalStore>(&mut group, "canonical", n, &dataset);
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

criterion_group!(
    benches,
    bench_get,
    bench_scan_ages,
    bench_update_age,
    bench_same_breed
);
criterion_main!(benches);
