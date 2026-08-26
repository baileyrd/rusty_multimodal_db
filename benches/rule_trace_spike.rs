//! Spike benchmark for the `Rule`/`RuleRelation` round (`src/generic_spike/rule_trace.rs`):
//! measures the two new stresses this domain introduces that neither
//! `Dog` nor `Order`/`Customer` exercised — recursive parent-chain
//! traversal at a few depths, and a `RuleRelation`-kind-filtered lookup
//! against a naive baseline — plus, from the optional-parent `ChildOf`
//! follow-up fix, `Children` ("list direct children") itself, now that
//! it's implementable for `Rule` at all. Own Criterion group names
//! (`rule_chain_traversal`/`rule_relation_lookup`/`rule_children_lookup`),
//! same convention as every prior spike, so these numbers never mix into
//! any existing benchmark's baseline history.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rusty_multimodal_db::bench_support::SIZES;
use rusty_multimodal_db::generic::query::Children;
use rusty_multimodal_db::generic_spike::rule_bench_support::{
    build_rule_chain, build_rule_children, build_rule_relation_dataset, RoundRobin,
};
use rusty_multimodal_db::generic_spike::rule_trace::{
    build_rule_relation_store, build_rule_tree_store, chain_to_root, IndexedRuleStore,
    NaiveRuleRelationStore, NaiveRuleStore, ParentOf, RelatedTo, Requires, Rule,
};

/// Shallow vs. deep — the depths this round's task explicitly asked for.
/// Not the crate's standard `SIZES` (those size a *dataset*; this sizes
/// one chain's *depth*, a different axis entirely).
const CHAIN_DEPTHS: [usize; 3] = [10, 100, 1_000];

fn bench_rule_chain_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_chain_traversal");
    for &depth in &CHAIN_DEPTHS {
        let rules = build_rule_chain(depth);
        let leaf = rules.last().expect("depth >= 1").id;

        let indexed = IndexedRuleStore::new(rules.clone());
        group.bench_with_input(BenchmarkId::new("indexed", depth), &depth, |b, _| {
            b.iter(|| black_box(chain_to_root(&indexed, black_box(leaf))));
        });

        let naive = NaiveRuleStore::new(rules);
        group.bench_with_input(BenchmarkId::new("naive", depth), &depth, |b, _| {
            b.iter(|| black_box(chain_to_root(&naive, black_box(leaf))));
        });
    }
    group.finish();
}

fn bench_rule_relation_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_relation_lookup");
    for &n in &SIZES {
        let dataset = build_rule_relation_dataset(n);

        let indexed = build_rule_relation_store(
            dataset.rules.clone(),
            &dataset.requires_edges,
            &dataset.implements_edges,
        );
        let mut cursor = RoundRobin::new(dataset.sample_rule_ids.len());
        group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_rule_ids[cursor.advance()];
                let result: Vec<uuid::Uuid> =
                    RelatedTo::<Rule, Requires>::related_to(&indexed, black_box(id));
                black_box(result)
            });
        });

        let naive = NaiveRuleRelationStore::new(
            dataset.requires_edges.clone(),
            dataset.implements_edges.clone(),
        );
        let mut cursor = RoundRobin::new(dataset.sample_rule_ids.len());
        group.bench_with_input(BenchmarkId::new("naive", n), &n, |b, _| {
            b.iter(|| {
                let id = dataset.sample_rule_ids[cursor.advance()];
                let result: Vec<uuid::Uuid> =
                    RelatedTo::<Rule, Requires>::related_to(&naive, black_box(id));
                black_box(result)
            });
        });
    }
    group.finish();
}

/// Same depths as [`bench_rule_chain_traversal`] ("a few sizes," per this
/// round's own instruction), but sweeping fan-out (children of one node)
/// instead of depth.
fn bench_rule_children_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("rule_children_lookup");
    for &n in &CHAIN_DEPTHS {
        let (rules, root_id) = build_rule_children(n);

        let indexed = build_rule_tree_store(rules.clone());
        group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
            b.iter(|| {
                black_box(Children::<Rule, Rule, ParentOf>::children(
                    &indexed,
                    black_box(root_id),
                ))
            });
        });

        let naive = NaiveRuleStore::new(rules);
        group.bench_with_input(BenchmarkId::new("naive", n), &n, |b, _| {
            b.iter(|| {
                black_box(Children::<Rule, Rule, ParentOf>::children(
                    &naive,
                    black_box(root_id),
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_rule_chain_traversal,
    bench_rule_relation_lookup,
    bench_rule_children_lookup
);
criterion_main!(benches);
