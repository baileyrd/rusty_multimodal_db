//! Spike benchmark: measures `Parent`/`Children` lookups on `Order
//! belongs_to Customer` — a directed one-to-many relation — comparing the
//! adjacency-index generic implementation (`OrderGenericStore`, built on
//! `store::Reversed` + the blanket `Parent` impl) against a naive linear
//! scan (`NaiveOrderStore`). This is the design doc §4's last untested
//! risk: whether the adjacency-index pattern that made `littermate_of`
//! traversal ~100,000× faster than a linear scan (`RESULTS.md`'s
//! `neighbors_one_hop`, ~113,000× at 1M) generalizes to a *directed*
//! relation, not just the symmetric one it was originally measured on.
//!
//! Own Criterion group names (`order_parent`/`order_children`), same
//! convention as `benches/generic_spike.rs`, so this spike's numbers never
//! mix into any existing benchmark's baseline history.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use rusty_multimodal_db::bench_support::SIZES;
use rusty_multimodal_db::generic::order_customer::{
    build_order_generic_store, BelongsToCustomer, Customer, Order,
};
use rusty_multimodal_db::generic::query::{Children, Parent};
use rusty_multimodal_db::generic_spike::order_bench_support::{build_order_dataset, RoundRobin};
use rusty_multimodal_db::generic_spike::NaiveOrderStore;

fn bench_order_parent(c: &mut Criterion) {
    let mut group = c.benchmark_group("order_parent");
    for &n in &SIZES {
        let dataset = build_order_dataset(n);

        let adjacency = build_order_generic_store(&dataset.orders);
        run_parent(&mut group, "adjacency_index", n, &dataset, &adjacency);

        let naive = NaiveOrderStore::new(dataset.orders.clone());
        run_parent(&mut group, "naive", n, &dataset, &naive);
    }
    group.finish();
}

fn run_parent<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &rusty_multimodal_db::generic_spike::order_bench_support::OrderDataset,
    store: &S,
) where
    S: Parent<Order, BelongsToCustomer>,
{
    let mut cursor = RoundRobin::new(dataset.sample_order_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_order_ids[cursor.advance()];
            black_box(store.parent(black_box(id)))
        });
    });
}

fn bench_order_children(c: &mut Criterion) {
    let mut group = c.benchmark_group("order_children");
    for &n in &SIZES {
        let dataset = build_order_dataset(n);

        let adjacency = build_order_generic_store(&dataset.orders);
        run_children(&mut group, "adjacency_index", n, &dataset, &adjacency);

        let naive = NaiveOrderStore::new(dataset.orders.clone());
        run_children(&mut group, "naive", n, &dataset, &naive);
    }
    group.finish();
}

fn run_children<S>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    n: usize,
    dataset: &rusty_multimodal_db::generic_spike::order_bench_support::OrderDataset,
    store: &S,
) where
    S: Children<Customer, Order, BelongsToCustomer>,
{
    let mut cursor = RoundRobin::new(dataset.sample_customer_ids.len());
    group.bench_with_input(BenchmarkId::new(name, n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_customer_ids[cursor.advance()];
            black_box(store.children(black_box(id)))
        });
    });
}

criterion_group!(benches, bench_order_parent, bench_order_children);
criterion_main!(benches);
