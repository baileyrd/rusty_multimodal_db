//! Benchmark suite for [`GenericProductionStore`] on `Order`/`Customer` —
//! `get`/`scan`/`filter_eq`/`parent`/`children` through the real, durable,
//! `RwLock`-guarded generic production store, at the same three sizes
//! `benches/workloads.rs`/`benches/order_relation_spike.rs` already used.
//! Own Criterion group names (`generic_production_*`) so these numbers
//! never mix into any existing benchmark's baseline history. See
//! `RESULTS.md`'s `## Generic schema library` section for the comparison
//! against the `directed-relation-spike`/`generic-schema-macro-forwarding`
//! rounds' in-memory numbers — the point of this suite is confirming
//! nothing regressed in the move from scratch/spike code to this real,
//! `RwLock`+mmap-backed implementation, not establishing a new verdict.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rusty_multimodal_db::bench_support::{fresh_temp_dir, SIZES};
use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Amount, BelongsToCustomer, Customer, Order, OrderStatus, Status,
};
use rusty_multimodal_db::generic::GenericProductionStore;
use rusty_multimodal_db::generic_spike::order_bench_support::{
    build_order_dataset, OrderDataset, RoundRobin,
};

fn build_store(
    n: usize,
    label: &str,
) -> (
    OrderDataset,
    GenericProductionStore<rusty_multimodal_db::generic::OrderProductionStack>,
) {
    let dataset = build_order_dataset(n);
    let dir = fresh_temp_dir(&format!("generic_production_bench_{label}_{n}"))
        .expect("fresh temp dir for generic production bench");
    let path = dir.join("amount.mmap");
    let stack = create_order_production_stack(dataset.orders.clone(), &path)
        .expect("create generic production stack for bench");
    (dataset, GenericProductionStore::new(stack))
}

fn bench_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_get");
    for &n in &SIZES {
        let (dataset, store) = build_store(n, "get");
        let mut cursor = RoundRobin::new(dataset.sample_order_ids.len());
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_order_ids[cursor.advance()];
                let result: Option<Order> = store.get(black_box(id));
                black_box(result)
            });
        });
    }
    group.finish();
}

fn bench_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_scan");
    for &n in &SIZES {
        let (_dataset, store) = build_store(n, "scan");
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let amounts: Vec<i64> = store.scan::<Order, Amount>();
                black_box(amounts)
            });
        });
    }
    group.finish();
}

fn bench_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_filter");
    for &n in &SIZES {
        let (_dataset, store) = build_store(n, "filter");
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let ids = store.filter_eq::<Order, Status>(black_box(&OrderStatus::Shipped));
                black_box(ids)
            });
        });
    }
    group.finish();
}

fn bench_parent(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_parent");
    for &n in &SIZES {
        let (dataset, store) = build_store(n, "parent");
        let mut cursor = RoundRobin::new(dataset.sample_order_ids.len());
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_order_ids[cursor.advance()];
                let result = store.parent::<Order, BelongsToCustomer>(black_box(id));
                black_box(result)
            });
        });
    }
    group.finish();
}

fn bench_children(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_children");
    for &n in &SIZES {
        let (dataset, store) = build_store(n, "children");
        let mut cursor = RoundRobin::new(dataset.sample_customer_ids.len());
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_customer_ids[cursor.advance()];
                let result = store.children::<Customer, Order, BelongsToCustomer>(black_box(id));
                black_box(result)
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_get,
    bench_scan,
    bench_filter,
    bench_parent,
    bench_children
);
criterion_main!(benches);
