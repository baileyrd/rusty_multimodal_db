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

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use rusty_multimodal_db::bench_support::{fresh_temp_dir, SIZES};
use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, open_order_production_stack, Amount, BelongsToCustomer,
    Customer, Order, OrderStatus, Status,
};
use rusty_multimodal_db::generic::GenericProductionStore;
use rusty_multimodal_db::generic_spike::order_bench_support::{
    build_order_dataset, OrderDataset, RoundRobin,
};
use uuid::Uuid;

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

/// New this round (the record-identity-keying fix) — `create`/`open`
/// weren't previously in this suite (only measured *indirectly*, via
/// `build_store`'s untimed setup for the groups above). Added specifically
/// to measure the fix's real cost: each persisted slot is now wider (an
/// id prefix alongside the value), and `open` does a full reconciliation
/// pass (`HashMap` build keyed by id) instead of trusting array position.
fn bench_create(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_create");
    for &n in &SIZES {
        let dataset = build_order_dataset(n);
        let dir = fresh_temp_dir(&format!("generic_production_bench_create_{n}"))
            .expect("fresh temp dir for generic production create bench");
        let path = dir.join("amount.mmap");
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                // `create` truncates unconditionally, so reusing one path
                // across iterations measures just the create cost itself,
                // not per-iteration temp-directory setup.
                let stack = create_order_production_stack(black_box(dataset.orders.clone()), &path)
                    .expect("create generic production stack for bench");
                black_box(stack)
            });
        });
    }
    group.finish();
}

/// The common, no-mismatch reopen case: the same records supplied at
/// `create` time, unchanged — the shape every existing test in this crate
/// already uses, and the case `is_gapless`'s fast `scan` path is named
/// for. Mismatch-case costs (a record added or removed since the last
/// write) aren't swept here — the fix's own tests
/// (`tests/mmap_record_identity_keying.rs`) cover their *correctness*;
/// this bench is about the cost of the reconciliation every `open` now
/// does, even in the everyday case where it changes nothing.
fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_open");
    for &n in &SIZES {
        let dataset = build_order_dataset(n);
        let dir = fresh_temp_dir(&format!("generic_production_bench_open_{n}"))
            .expect("fresh temp dir for generic production open bench");
        let path = dir.join("amount.mmap");
        {
            let stack = create_order_production_stack(dataset.orders.clone(), &path)
                .expect("create generic production stack for bench");
            drop(stack);
        }
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let stack = open_order_production_stack(black_box(dataset.orders.clone()), &path)
                    .expect("open generic production stack for bench");
                black_box(stack)
            });
        });
    }
    group.finish();
}

/// Also new this round — `update`'s cost was never separately isolated in
/// this suite before (only exercised indirectly via `build_store`'s
/// untimed setup). `write_value` itself is a single indexed byte-slice
/// write either way; the fix doesn't add per-call cost to an *existing*
/// record's update (only `create`/`open` changed), so this establishes
/// the first baseline for this operation on the generic path rather than
/// comparing against a pre-fix number that never existed.
fn bench_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_update");
    for &n in &SIZES {
        let (dataset, store) = build_store(n, "update");
        let mut cursor = RoundRobin::new(dataset.sample_order_ids.len());
        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_order_ids[cursor.advance()];
                store
                    .update::<Order, Amount>(black_box(id), black_box(12_345))
                    .expect("update generic production stack for bench");
            });
        });
    }
    group.finish();
}

/// Records appended on top of each base size in [`bench_open_append`] —
/// modest, matching the "typical fan-out" scale this suite's other fixed
/// constants (`AVG_ORDERS_PER_CUSTOMER`, `SAMPLE_TARGET_COUNT`) use, not
/// picked to stress-test extreme batch sizes.
const APPEND_COUNT: usize = 100;

/// The cost of `open`'s append/slot-claiming path specifically — the
/// exact code the multi-process append-race fix round changed (see
/// `src/generic/mmap_store.rs`'s own "next free slot" doc section).
/// [`bench_open`] above deliberately never exercises this path: it
/// always reopens with the identical records the store was created
/// with, so every record already has a persisted slot and nothing is
/// ever appended. This group instead opens with [`APPEND_COUNT`] records
/// beyond what's on disk, isolating the code path that fix touched (each
/// missing record's slot position now comes from an `O_APPEND` write's
/// own resulting file offset, not a locally-computed
/// `existing_slot_count`) from the unrelated, unchanged reconciliation
/// cost `bench_open` already measures.
///
/// Uses `iter_batched`/`PerIteration`, not plain `b.iter`: unlike the
/// gapless reopen case, this operation isn't repeatable against the same
/// file — once one call appends its `APPEND_COUNT` new slots, a second
/// call against that same file would find them already persisted and
/// measure the gapless path instead. Each iteration's (untimed) setup
/// rebuilds a fresh base file of `n` records from scratch, so this
/// group's `sample_size` is lowered to Criterion's minimum (10) — that
/// rebuild is real cost at the 1,000,000 size (see `bench_create`'s own
/// numbers) — trading some statistical precision, versus this suite's
/// other groups' default 100 samples, for a tractable total run time.
fn bench_open_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("generic_production_open_append");
    group.sample_size(10);
    for &n in &SIZES {
        let dataset = build_order_dataset(n);
        let extra: Vec<Order> = (0..APPEND_COUNT)
            .map(|i| Order {
                id: Uuid::from_u128(u128::MAX - i as u128),
                customer_id: dataset.orders[0].customer_id,
                amount_cents: 1,
                status: OrderStatus::Pending,
                created_at_unix_ms: 0,
                discount_cents: 0,
            })
            .collect();
        let dir = fresh_temp_dir(&format!("generic_production_bench_open_append_{n}"))
            .expect("fresh temp dir for generic production open-append bench");
        let path = dir.join("amount.mmap");

        group.bench_with_input(BenchmarkId::new("generic_production", n), &n, |b, _| {
            b.iter_batched(
                || {
                    let stack = create_order_production_stack(dataset.orders.clone(), &path)
                        .expect("create the base store for an open-append bench iteration");
                    drop(stack);
                    let mut records = dataset.orders.clone();
                    records.extend(extra.iter().cloned());
                    records
                },
                |records| {
                    let stack = open_order_production_stack(black_box(records), &path)
                        .expect("open generic production stack with new records for bench");
                    black_box(stack)
                },
                BatchSize::PerIteration,
            );
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
    bench_children,
    bench_create,
    bench_open,
    bench_open_append,
    bench_update
);
criterion_main!(benches);
