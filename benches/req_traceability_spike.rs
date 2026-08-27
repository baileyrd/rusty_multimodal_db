//! Sixth spike round: `Source`/`RuleDerivation`/`SelectionGroup`, the
//! remaining three pieces of the finalized five-entity req-traceability
//! schema `src/generic_spike/rule_trace.rs` describes. Own Criterion group
//! names (`source_root_lookup`/`selection_group_members_lookup`/
//! `derivation_chain_lookup`), same convention as every prior spike, so
//! these numbers never mix into any existing benchmark's baseline history.
//!
//! `source_root_lookup` and `derivation_chain_lookup` are both walk-to-a-
//! terminal-node queries, structurally identical to `rule_trace_spike`'s
//! own `rule_chain_traversal` group (same [`CHAIN_DEPTHS`], same
//! indexed-vs-naive comparison) — run here to check whether the same
//! shallow-vs-deep crossover that benchmark found (naive beats indexed at
//! shallow depths, indexed wins at deep ones) shows up for these two
//! walks too, per this round's own task. `selection_group_members_lookup`
//! is the "list every member" direction, same shape as `Order`'s own
//! `Children` benchmark (`benches/order_relation_spike.rs`'s
//! `order_children` group) and `rule_trace_spike`'s own
//! `rule_children_lookup`.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rusty_multimodal_db::bench_support::SIZES;
use rusty_multimodal_db::generic::query::Children;
use rusty_multimodal_db::generic_spike::rule_bench_support::{
    build_derivation_chain, build_selection_group,
};
use rusty_multimodal_db::generic_spike::rule_derivation::{
    derivation_chain_to_root, IndexedDerivationStore, NaiveDerivationStore,
};
use rusty_multimodal_db::generic_spike::rule_trace::{
    build_selection_group_store, MemberOf, NaiveRuleStore, Rule, SelectionGroup,
};
use rusty_multimodal_db::generic_spike::source::{
    effective_domain_tags, IndexedSourceStore, NaiveSourceStore,
};
use rusty_multimodal_db::generic_spike::source_bench_support::build_source_chain;

/// Shallow vs. deep — same depths `rule_trace_spike`'s own
/// `rule_chain_traversal` group swept, for the same reason: these two
/// walks are structurally the same query shape.
const CHAIN_DEPTHS: [usize; 3] = [10, 100, 1_000];

fn bench_source_root_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("source_root_lookup");
    for &depth in &CHAIN_DEPTHS {
        let sources = build_source_chain(depth, vec!["payments".into()]);
        let leaf_id = sources.last().expect("depth >= 1").id;

        let indexed = IndexedSourceStore::new(sources.clone());
        group.bench_with_input(BenchmarkId::new("indexed", depth), &depth, |b, _| {
            b.iter(|| black_box(effective_domain_tags(&indexed, black_box(leaf_id))));
        });

        let naive = NaiveSourceStore::new(sources);
        group.bench_with_input(BenchmarkId::new("naive", depth), &depth, |b, _| {
            b.iter(|| black_box(effective_domain_tags(&naive, black_box(leaf_id))));
        });
    }
    group.finish();
}

fn bench_derivation_chain_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("derivation_chain_lookup");
    for &depth in &CHAIN_DEPTHS {
        let (rules, edges) = build_derivation_chain(depth);
        let leaf_id = rules.last().expect("depth >= 1").id;

        let indexed = IndexedDerivationStore::new(rules.clone(), &edges);
        group.bench_with_input(BenchmarkId::new("indexed", depth), &depth, |b, _| {
            b.iter(|| black_box(derivation_chain_to_root(&indexed, black_box(leaf_id))));
        });

        let naive = NaiveDerivationStore::new(rules, edges);
        group.bench_with_input(BenchmarkId::new("naive", depth), &depth, |b, _| {
            b.iter(|| black_box(derivation_chain_to_root(&naive, black_box(leaf_id))));
        });
    }
    group.finish();
}

/// Unlike `order_relation_spike`'s own `order_children` benchmark
/// (rotating over many distinct customers, each with a handful of
/// orders), [`build_selection_group`] builds exactly *one* group with `n`
/// members — sweeping membership-set size for a single group, not
/// dataset-wide fan-out — so there's no id pool to round-robin over: the
/// same fixed `group_id` is queried every iteration.
fn bench_selection_group_members_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("selection_group_members_lookup");
    for &n in &SIZES {
        let (rules, group_id) = build_selection_group(n);

        let indexed = build_selection_group_store(rules.clone());
        group.bench_with_input(BenchmarkId::new("indexed", n), &n, |b, _| {
            b.iter(|| {
                black_box(Children::<SelectionGroup, Rule, MemberOf>::children(
                    &indexed,
                    black_box(group_id),
                ))
            });
        });

        let naive = NaiveRuleStore::new(rules);
        group.bench_with_input(BenchmarkId::new("naive", n), &n, |b, _| {
            b.iter(|| {
                black_box(Children::<SelectionGroup, Rule, MemberOf>::children(
                    &naive,
                    black_box(group_id),
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_source_root_lookup,
    bench_derivation_chain_lookup,
    bench_selection_group_members_lookup
);
criterion_main!(benches);
