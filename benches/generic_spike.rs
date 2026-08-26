//! Spike benchmark: measures `get`/`scan_ages` through the generic-schema
//! path (`src/generic_spike/`) at the same three sizes `benches/workloads.rs`
//! already established `CanonicalCachedStore`'s numbers at, so the two are
//! directly comparable against `RESULTS.md`. Deliberately its own bench
//! target (own Criterion group names, `generic_get`/`generic_scan_ages`,
//! rather than reusing `get`/`scan_ages`) so this spike's throwaway numbers
//! never mix into the existing benchmark's own baseline history.
//!
//! This measures **only** `get` and `scan_ages` — the two operations the
//! design doc's §4.1 named as the biggest risk (the packed-`Vec` cache).
//! `update_age`/`same_breed`/`neighbors` through the generic path are not
//! benchmarked here — out of scope per the task that motivated this spike.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use rusty_multimodal_db::bench_support::{build_dataset, RoundRobin, SIZES};
use rusty_multimodal_db::generic_spike::query::{GetById, ScanField};
use rusty_multimodal_db::generic_spike::{build_dog_generic_store, dog_impl::Age};
use rusty_multimodal_db::DogRecord;

fn bench_generic_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_get");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_generic_get(&mut group, n, &dataset);
    }
    group.finish();
}

fn run_generic_get(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &rusty_multimodal_db::bench_support::Dataset,
) {
    let store = build_dog_generic_store(&dataset.records, &dataset.edges);
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("generic", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let result: Option<DogRecord> = GetById::get(&store, black_box(id));
            black_box(result)
        });
    });
}

fn bench_generic_scan_ages(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_scan_ages");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_generic_scan_ages(&mut group, n, &dataset);
    }
    group.finish();
}

fn run_generic_scan_ages(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &rusty_multimodal_db::bench_support::Dataset,
) {
    let store = build_dog_generic_store(&dataset.records, &dataset.edges);
    group.bench_with_input(BenchmarkId::new("generic", n), &n, |b, _| {
        b.iter(|| {
            let ages: Vec<u32> = ScanField::<DogRecord, Age>::scan(&store);
            black_box(ages)
        });
    });
}

criterion_group!(benches, bench_generic_get, bench_generic_scan_ages);
criterion_main!(benches);
