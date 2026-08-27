//! Sixth spike round, second of three pieces: `RuleDerivation` — traces a
//! specific/detailed [`Rule`] back to a more abstract "principle" `Rule`
//! it elaborates on. Part of the finalized five-entity req-traceability
//! schema `rule_trace.rs`'s own module docs describe. **Still a spike,
//! not a migration decision** — same discipline as every prior round.
//!
//! # Deliberately *not* another `RuleRelation` kind — a type-level firewall
//!
//! `rule_trace`'s [`DirectedRelation`](super::rule_trace::DirectedRelation)/
//! [`RelatedTo`](super::rule_trace::RelatedTo)/
//! [`DirectedRelated`](super::rule_trace::DirectedRelated) triad already
//! models directed `Rule`-to-`Rule` edges (`requires`/`implements`) from
//! an externally-supplied edge list — structurally, a derivation link
//! ("elaborates on") is the same shape. The task motivating this round is
//! explicit that a derivation link must **not** be foldable into that
//! triad as a third relation kind: derivation is non-authoritative
//! (informational lineage), while `requires`/`implements` are binding
//! dependencies a real consumer of this schema would treat as gating
//! logic — conflating the two, even by accident, would let a mere "this
//! elaborates on that" note be mistaken for "this requires that."
//!
//! This file defines a **completely separate** trait triad —
//! [`DerivationRelation`]/[`DerivesFrom`]/[`Derived`] — that shares no
//! supertrait, blanket impl, or marker type with `RuleRelation`'s. `Rule`
//! implements `DerivationRelation<PrincipleOf>` here; it does **not**
//! implement `DirectedRelation<PrincipleOf>` anywhere, so
//! `PrincipleOf` can never be used to instantiate a `RelatedTo`/
//! `DirectedRelated` — the compiler rejects it, not just convention. See
//! this module's `firewall` doctest (a `compile_fail` doc-test, the
//! standard-library, zero-dependency way to assert "this must not
//! compile") for the mechanical proof, and the `tests` module below for
//! the runtime-observable half (an indexed `Derived` store and a
//! `DirectedRelated` store built over the *same* `Rule` data produce
//! independent, non-interchangeable results).
//!
//! # Chains: `derives_from` can be walked to a principle root, like `Rule`'s own parent chain
//!
//! A detailed rule's principle can itself be a derivation of a still more
//! abstract principle, forming a chain — the same shape as `Source`'s
//! nesting and `Rule`'s own `ParentOf` tree. [`derivation_chain_to_root`]
//! walks it, structurally identical to
//! [`chain_to_root`](super::rule_trace::chain_to_root): `derives_from`
//! returns `Vec<Uuid>` (mirroring `RelatedTo::related_to`'s shape, for
//! consistency with the rest of this file's own triad), but a derivation
//! chain in practice is single-parent — this walk follows the *first*
//! edge found at each step and stops once none remain, documented here
//! rather than silently assumed: a `Rule` with more than one recorded
//! principle only ever has its first-found one walked.

use super::rule_trace::Rule;
use crate::generic::query::GetById;
use crate::generic::traits::Record;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use uuid::Uuid;

/// `R` participates in a directed *derivation* relation, identified by
/// `Marker` — structurally mirrors
/// [`DirectedRelation`](super::rule_trace::DirectedRelation) but is a
/// wholly distinct trait: implementing this for some `Marker` says
/// nothing about `DirectedRelation<Marker>`, and vice versa. That
/// independence *is* the firewall — see this module's own docs.
pub trait DerivationRelation<Marker>: Record {}

/// "What `id` derives from under this derivation kind" — the derivation
/// analogue of [`RelatedTo`](super::rule_trace::RelatedTo). Distinct
/// trait, not a blanket reuse of `RelatedTo`, for the same reason
/// [`DerivationRelation`] is distinct from `DirectedRelation`.
pub trait DerivesFrom<R, Marker>
where
    R: DerivationRelation<Marker>,
{
    fn derives_from(&self, id: R::Id) -> Vec<R::Id>;
}

/// Adds one `DerivesFrom` capability over an inner store, from an
/// externally-supplied directed edge list — the derivation analogue of
/// [`DirectedRelated`](super::rule_trace::DirectedRelated). Distinct
/// type, not a reuse, for the same reason as above.
pub struct Derived<S, R, Marker>
where
    R: DerivationRelation<Marker>,
{
    inner: S,
    outgoing: HashMap<R::Id, Vec<R::Id>>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> Derived<S, R, Marker>
where
    R: DerivationRelation<Marker>,
{
    pub fn new(inner: S, edges: &[(R::Id, R::Id)]) -> Self {
        let mut outgoing: HashMap<R::Id, Vec<R::Id>> = HashMap::new();
        for &(from, to) in edges {
            outgoing.entry(from).or_default().push(to);
        }
        Self {
            inner,
            outgoing,
            _marker: PhantomData,
        }
    }
}

impl<S, R, Marker> DerivesFrom<R, Marker> for Derived<S, R, Marker>
where
    R: DerivationRelation<Marker>,
{
    fn derives_from(&self, id: R::Id) -> Vec<R::Id> {
        self.outgoing.get(&id).cloned().unwrap_or_default()
    }
}

// Forwarding impl: `Derived<S, ..>` re-exposing `GetById` from its inner
// store — same shape as every forwarding impl in `crate::generic::store`
// and `rule_trace::DirectedRelated`'s own.
impl<S, R, Marker> GetById<R> for Derived<S, R, Marker>
where
    R: DerivationRelation<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

pub struct PrincipleOf;
impl DerivationRelation<PrincipleOf> for Rule {}

/// Owns `Rule` records in a `HashMap` — the base layer `Derived` wraps
/// for the indexed side of the derivation-chain benchmark comparison.
/// Not `pub`: only exists to give `Derived` something providing
/// `GetById<Rule>` to sit on top of, same role `rule_trace::IndexedRuleStore`
/// plays for `DirectedRelated`, kept private here since nothing outside
/// this file needs to construct one directly.
struct RuleRecords {
    records: HashMap<Uuid, Rule>,
}

impl GetById<Rule> for RuleRecords {
    fn get(&self, id: Uuid) -> Option<Rule> {
        self.records.get(&id).cloned()
    }
}

/// The indexed side of the derivation-chain benchmark comparison:
/// `RuleRecords` (owns records, `HashMap`-backed `GetById`) ->
/// `Derived<.., PrincipleOf>` — same two-layer shape as
/// `rule_trace::RuleRelationStore`.
pub struct IndexedDerivationStore {
    inner: Derived<RuleRecords, Rule, PrincipleOf>,
}

impl IndexedDerivationStore {
    pub fn new(rules: Vec<Rule>, edges: &[(Uuid, Uuid)]) -> Self {
        let records = RuleRecords {
            records: rules.into_iter().map(|r| (r.id, r)).collect(),
        };
        Self {
            inner: Derived::new(records, edges),
        }
    }
}

impl GetById<Rule> for IndexedDerivationStore {
    fn get(&self, id: Uuid) -> Option<Rule> {
        self.inner.get(id)
    }
}

impl DerivesFrom<Rule, PrincipleOf> for IndexedDerivationStore {
    fn derives_from(&self, id: Uuid) -> Vec<Uuid> {
        self.inner.derives_from(id)
    }
}

/// The naive baseline: a linear scan over the edge list, no index — same
/// role `NaiveRuleRelationStore` played for the relation-kind round.
pub struct NaiveDerivationStore {
    rules: Vec<Rule>,
    edges: Vec<(Uuid, Uuid)>,
}

impl NaiveDerivationStore {
    pub fn new(rules: Vec<Rule>, edges: Vec<(Uuid, Uuid)>) -> Self {
        Self { rules, edges }
    }
}

impl GetById<Rule> for NaiveDerivationStore {
    fn get(&self, id: Uuid) -> Option<Rule> {
        self.rules.iter().find(|r| r.id == id).cloned()
    }
}

impl DerivesFrom<Rule, PrincipleOf> for NaiveDerivationStore {
    fn derives_from(&self, id: Uuid) -> Vec<Uuid> {
        self.edges
            .iter()
            .filter(|&&(from, _)| from == id)
            .map(|&(_, to)| to)
            .collect()
    }
}

/// Walks from `id` through successive `derives_from` links to the
/// ultimate principle root (the first node with no recorded principle of
/// its own), following the *first* edge at each step — see this module's
/// own docs for why. Returns the full chain, `id` first, root last. An
/// unknown `id` yields an empty chain — same convention as
/// [`chain_to_root`](super::rule_trace::chain_to_root).
pub fn derivation_chain_to_root<S>(store: &S, id: Uuid) -> Vec<Uuid>
where
    S: DerivesFrom<Rule, PrincipleOf> + GetById<Rule>,
{
    if store.get(id).is_none() {
        return Vec::new();
    }
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = id;
    loop {
        if !seen.insert(current) {
            break;
        }
        chain.push(current);
        match store.derives_from(current).first().copied() {
            Some(principle_id) => current = principle_id,
            None => break, // reached the principle root
        }
    }
    chain
}

/// A `RuleDerivation` marker (e.g. [`PrincipleOf`]) can never be used
/// where a `RuleRelation` marker is expected, or vice versa — not just by
/// convention, but because [`DerivationRelation`]/[`DerivesFrom`] and
/// [`super::rule_trace::DirectedRelation`]/[`super::rule_trace::RelatedTo`]
/// are two entirely separate trait hierarchies with no shared supertrait
/// or blanket impl connecting them. This doesn't compile, proving the
/// firewall at the type level, not just in how the two happen to be used
/// today:
///
/// ```compile_fail
/// use rusty_multimodal_db::generic_spike::rule_derivation::{IndexedDerivationStore, PrincipleOf};
/// use rusty_multimodal_db::generic_spike::rule_trace::{RelatedTo, Rule};
///
/// // `Rule` implements `DerivationRelation<PrincipleOf>`, not
/// // `DirectedRelation<PrincipleOf>` — so this bound can never be
/// // satisfied for `PrincipleOf`, regardless of which store is passed.
/// fn use_as_a_rule_relation<S: RelatedTo<Rule, PrincipleOf>>(_: &S) {}
///
/// fn call_it(store: IndexedDerivationStore) {
///     use_as_a_rule_relation(&store);
/// }
/// ```
#[allow(dead_code)]
fn _firewall_doctest_anchor() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic_spike::rule_trace::{BindingStrength, DirectedRelated, RelatedTo};

    fn rule(i: u128) -> Rule {
        Rule {
            id: Uuid::from_u128(i),
            shall_statement: format!("rule {i}"),
            binding_strength: BindingStrength::Shall,
            parent_rule_id: None,
            selection_group_id: None,
        }
    }

    fn chain(depth: usize) -> (Vec<Rule>, Vec<(Uuid, Uuid)>) {
        let rules: Vec<Rule> = (0..depth as u128).map(rule).collect();
        // rule i (i > 0) derives from rule i - 1; rule 0 is the principle
        // root, with no outgoing derivation edge of its own.
        let edges: Vec<(Uuid, Uuid)> = (1..depth as u128)
            .map(|i| (Uuid::from_u128(i), Uuid::from_u128(i - 1)))
            .collect();
        (rules, edges)
    }

    #[test]
    fn derivation_chain_to_root_walks_every_principle_indexed_and_naive_agree() {
        let (rules, edges) = chain(5);
        let indexed = IndexedDerivationStore::new(rules.clone(), &edges);
        let naive = NaiveDerivationStore::new(rules, edges);
        let leaf = Uuid::from_u128(4);

        let expected: Vec<Uuid> = vec![4, 3, 2, 1, 0]
            .into_iter()
            .map(Uuid::from_u128)
            .collect();
        assert_eq!(derivation_chain_to_root(&indexed, leaf), expected);
        assert_eq!(derivation_chain_to_root(&naive, leaf), expected);
    }

    #[test]
    fn derivation_chain_to_root_from_the_principle_root_itself_is_a_single_element_chain() {
        let (rules, edges) = chain(5);
        let store = IndexedDerivationStore::new(rules, &edges);
        assert_eq!(
            derivation_chain_to_root(&store, Uuid::from_u128(0)),
            vec![Uuid::from_u128(0)]
        );
    }

    #[test]
    fn derivation_chain_to_root_unknown_id_is_empty() {
        let (rules, edges) = chain(5);
        let store = IndexedDerivationStore::new(rules, &edges);
        assert!(derivation_chain_to_root(&store, Uuid::from_u128(999)).is_empty());
    }

    #[test]
    fn derivation_chain_to_root_stops_on_a_cycle_instead_of_looping_forever() {
        let rules = vec![rule(0), rule(1)];
        let edges = vec![
            (Uuid::from_u128(0), Uuid::from_u128(1)),
            (Uuid::from_u128(1), Uuid::from_u128(0)),
        ];
        let store = IndexedDerivationStore::new(rules, &edges);
        let result = derivation_chain_to_root(&store, Uuid::from_u128(0));
        assert_eq!(result.len(), 2, "must terminate, not loop forever");
    }

    /// The runtime-observable half of the firewall: an indexed
    /// `DerivesFrom<Rule, PrincipleOf>` store and a `RelatedTo<Rule,
    /// Requires>` store built over the same underlying `Rule` data behave
    /// completely independently — populating one's edge list has no
    /// effect on the other, because they're backed by unrelated types
    /// with unrelated storage, not two views of one shared relation.
    #[test]
    fn derivation_edges_and_relation_edges_over_the_same_rules_are_fully_independent() {
        let rules = vec![rule(1), rule(2)];
        let derivation_edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
        let derivation_store = IndexedDerivationStore::new(rules.clone(), &derivation_edges);

        let relation_store = DirectedRelated::<_, Rule, super::super::rule_trace::Requires>::new(
            super::super::rule_trace::IndexedRuleStore::new(rules),
            &[], // no `requires` edges recorded at all
        );

        assert_eq!(
            derivation_store.derives_from(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert!(
            RelatedTo::<Rule, super::super::rule_trace::Requires>::related_to(
                &relation_store,
                Uuid::from_u128(1)
            )
            .is_empty()
        );
    }

    #[test]
    fn firewall_doctest_anchor_is_reachable() {
        _firewall_doctest_anchor();
    }
}
