//! Deterministic synthetic `Rule` dataset generation for
//! `benches/rule_trace_spike.rs` — the `Rule`/`RuleRelation` analogue of
//! [`super::order_bench_support`], self-contained per this crate's
//! established convention rather than extending `bench_support.rs` or
//! `order_bench_support.rs` (see `rule_trace.rs`'s own module docs for why
//! this domain stays isolated).

use super::rule_trace::{BindingStrength, Rule};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use uuid::{Builder, Uuid};

/// Seed for this spike's relation-dataset generation — independent of
/// every other spike's own seed, per this crate's established convention.
const SEED: u64 = 0x5255_4C45_5350_494B; // "RULESPIK" in ASCII hex, arbitrary

/// Average out-degree per relation kind — a comparably modest fan-out to
/// `littermate_of`'s ~1.5 and `order_bench_support`'s 5-orders-per-customer,
/// not swept.
const AVG_OUT_DEGREE: usize = 2;

pub const SAMPLE_TARGET_COUNT: usize = 200;

/// Build a simple, deliberately linear parent chain of `depth` rules:
/// rule `0` is the root (`parent_rule_id: None`), rule `i` (`i > 0`) has
/// rule `i - 1` as its parent. Purpose-built for the parent-chain-depth
/// benchmark — not a general tree generator (a bushy tree would still
/// have *some* leaf at `depth`, but a straight line makes "how does
/// traversal cost scale with depth" the only variable, with no branching
/// factor to also account for).
pub fn build_rule_chain(depth: usize) -> Vec<Rule> {
    (0..depth)
        .map(|i| Rule {
            id: Uuid::from_u128(i as u128),
            shall_statement: format!("the system shall satisfy rule {i}"),
            binding_strength: BindingStrength::Shall,
            parent_rule_id: if i == 0 {
                None
            } else {
                Some(Uuid::from_u128((i - 1) as u128))
            },
            selection_group_id: None,
        })
        .collect()
}

/// Build one root (id `0`, no parent of its own) with `n` direct
/// children (ids `1..=n`) — purpose-built for the children-of-a-node
/// benchmark, the mirror image of [`build_rule_chain`]'s depth-focused
/// linear generator: this holds depth fixed at 1 and sweeps fan-out
/// instead. Returns `(rules, root_id)`.
pub fn build_rule_children(n: usize) -> (Vec<Rule>, Uuid) {
    let root_id = Uuid::from_u128(0);
    let mut rules = vec![Rule {
        id: root_id,
        shall_statement: "the system shall satisfy the root rule".into(),
        binding_strength: BindingStrength::Shall,
        parent_rule_id: None,
        selection_group_id: None,
    }];
    rules.extend((1..=n).map(|i| Rule {
        id: Uuid::from_u128(i as u128),
        shall_statement: format!("the system shall satisfy rule {i}"),
        binding_strength: BindingStrength::Shall,
        parent_rule_id: Some(root_id),
        selection_group_id: None,
    }));
    (rules, root_id)
}

/// Build `n` rules that all belong to the same [`super::rule_trace::SelectionGroup`]
/// (id `0`) and have no parent-chain structure of their own — the
/// `SelectionGroup` analogue of [`build_rule_children`], sweeping
/// membership-set size instead of parent-tree fan-out. Returns `(rules,
/// group_id)`.
pub fn build_selection_group(n: usize) -> (Vec<Rule>, Uuid) {
    let group_id = Uuid::from_u128(0);
    let rules = (0..n)
        .map(|i| Rule {
            id: Uuid::from_u128(i as u128),
            shall_statement: format!("the system shall satisfy option {i}"),
            binding_strength: BindingStrength::Shall,
            parent_rule_id: None,
            selection_group_id: Some(group_id),
        })
        .collect();
    (rules, group_id)
}

/// Build a linear derivation chain of `depth` rules: rule `0` is the
/// principle root (no outgoing derivation edge of its own), rule `i`
/// (`i > 0`) derives from rule `i - 1` — the [`super::rule_derivation`]
/// analogue of [`build_rule_chain`], same purpose-built straight-line
/// shape for isolating "does traversal cost scale with depth" from any
/// branching factor. Returns `(rules, derivation_edges)`.
pub fn build_derivation_chain(depth: usize) -> (Vec<Rule>, Vec<(Uuid, Uuid)>) {
    let rules: Vec<Rule> = (0..depth)
        .map(|i| Rule {
            id: Uuid::from_u128(i as u128),
            shall_statement: format!("the system shall satisfy rule {i}"),
            binding_strength: BindingStrength::Shall,
            parent_rule_id: None,
            selection_group_id: None,
        })
        .collect();
    let edges = (1..depth)
        .map(|i| (Uuid::from_u128(i as u128), Uuid::from_u128((i - 1) as u128)))
        .collect();
    (rules, edges)
}

/// A generated `RuleRelation` dataset: `n` rules (no parent-chain
/// structure — irrelevant to this benchmark, so every rule is a root),
/// plus a `requires` and an `implements` edge list, each with
/// [`AVG_OUT_DEGREE`] average out-degree, plus a pre-selected rotation
/// pool of ids to query against.
pub struct RuleRelationDataset {
    pub rules: Vec<Rule>,
    pub requires_edges: Vec<(Uuid, Uuid)>,
    pub implements_edges: Vec<(Uuid, Uuid)>,
    pub sample_rule_ids: Vec<Uuid>,
}

pub fn build_rule_relation_dataset(n: usize) -> RuleRelationDataset {
    let mut rng = StdRng::seed_from_u64(SEED);
    let rules: Vec<Rule> = (0..n)
        .map(|i| Rule {
            id: random_uuid(&mut rng),
            shall_statement: format!("the system shall satisfy rule {i}"),
            binding_strength: BindingStrength::Shall,
            parent_rule_id: None,
            selection_group_id: None,
        })
        .collect();
    let ids: Vec<Uuid> = rules.iter().map(|r| r.id).collect();

    let edge_count = n * AVG_OUT_DEGREE;
    let requires_edges = random_edges(&mut rng, &ids, edge_count);
    let implements_edges = random_edges(&mut rng, &ids, edge_count);

    // Independent RNG stream for target selection, same SEED-XOR-a-constant
    // convention every other spike's dataset builder uses.
    let mut sample_rng = StdRng::seed_from_u64(SEED ^ 0x1234_5678_9ABC_DEF0);
    let sample_rule_ids = (0..SAMPLE_TARGET_COUNT)
        .map(|_| ids[sample_rng.gen_range(0..ids.len())])
        .collect();

    RuleRelationDataset {
        rules,
        requires_edges,
        implements_edges,
        sample_rule_ids,
    }
}

fn random_edges(rng: &mut StdRng, ids: &[Uuid], count: usize) -> Vec<(Uuid, Uuid)> {
    (0..count)
        .map(|_| {
            let from = ids[rng.gen_range(0..ids.len())];
            let to = ids[rng.gen_range(0..ids.len())];
            (from, to)
        })
        .collect()
}

fn random_uuid(rng: &mut StdRng) -> Uuid {
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    Builder::from_random_bytes(bytes).into_uuid()
}

/// Round-robins through a fixed pool size — identical shape to
/// `order_bench_support::RoundRobin`, duplicated here rather than shared,
/// per this crate's established convention (see that module's own doc
/// comment, and `durability::wal_buffered`'s module docs for the
/// precedent this follows) of small explicit duplication across
/// structurally similar, independently-evolving pieces — this domain
/// stays isolated from `order_bench_support` too, not just from
/// `Dog`/`Order`/`Customer`'s own real implementations.
pub struct RoundRobin {
    next: usize,
    len: usize,
}

impl RoundRobin {
    pub fn new(len: usize) -> Self {
        Self { next: 0, len }
    }

    pub fn advance(&mut self) -> usize {
        let current = self.next;
        self.next = (self.next + 1) % self.len;
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rule_chain_produces_a_linear_chain_of_the_requested_depth() {
        let chain = build_rule_chain(10);
        assert_eq!(chain.len(), 10);
        assert_eq!(chain[0].parent_rule_id, None);
        for (i, rule) in chain.iter().enumerate().skip(1) {
            assert_eq!(rule.parent_rule_id, Some(Uuid::from_u128((i - 1) as u128)));
        }
    }

    #[test]
    fn build_rule_children_produces_one_root_and_n_direct_children() {
        let (rules, root_id) = build_rule_children(10);
        assert_eq!(rules.len(), 11);
        assert_eq!(rules[0].id, root_id);
        assert_eq!(rules[0].parent_rule_id, None);
        for rule in &rules[1..] {
            assert_eq!(rule.parent_rule_id, Some(root_id));
        }
    }

    #[test]
    fn build_selection_group_produces_n_rules_all_in_the_same_group() {
        let (rules, group_id) = build_selection_group(10);
        assert_eq!(rules.len(), 10);
        for rule in &rules {
            assert_eq!(rule.selection_group_id, Some(group_id));
        }
    }

    #[test]
    fn build_derivation_chain_produces_a_linear_chain_of_the_requested_depth() {
        let (rules, edges) = build_derivation_chain(10);
        assert_eq!(rules.len(), 10);
        assert_eq!(edges.len(), 9);
        for (i, &(from, to)) in edges.iter().enumerate() {
            assert_eq!(from, Uuid::from_u128((i + 1) as u128));
            assert_eq!(to, Uuid::from_u128(i as u128));
        }
    }

    #[test]
    fn build_rule_relation_dataset_matches_requested_size() {
        let dataset = build_rule_relation_dataset(50);
        assert_eq!(dataset.rules.len(), 50);
        assert_eq!(dataset.requires_edges.len(), 50 * AVG_OUT_DEGREE);
        assert_eq!(dataset.implements_edges.len(), 50 * AVG_OUT_DEGREE);
    }

    #[test]
    fn same_config_produces_identical_output() {
        let a = build_rule_relation_dataset(50);
        let b = build_rule_relation_dataset(50);
        assert_eq!(
            a.rules.iter().map(|r| r.id).collect::<Vec<_>>(),
            b.rules.iter().map(|r| r.id).collect::<Vec<_>>()
        );
        assert_eq!(a.requires_edges, b.requires_edges);
    }
}
