//! Fifth spike round: `Rule`/`RuleRelation`, a narrow slice of a real,
//! external requirements-traceability spec-tree design (currently planned
//! for SQLite + optional `petgraph`, five entities in full —
//! `Source`/`Rule`/`RuleRelation`/`RuleDerivation`/`SelectionGroup`). Scoped
//! to just `Rule` and `RuleRelation`, per the task that motivated this
//! round — the core spec-tree-with-traceability structure, not the full
//! schema. **This is a spike, not a migration decision** — same
//! discipline as every prior round: nothing here is wired into
//! `crate::generic`, `GENERIC-SCHEMA-DESIGN.md`, or ADR-0009, and none of
//! them are touched by this round.
//!
//! `Order`/`Customer` validated fields and a single directed one-to-many
//! relation. This domain stresses two things neither `Dog` nor
//! `Order`/`Customer` ever exercised:
//!
//! 1. **Recursive parent-chain traversal.** A `Rule` can have a parent
//!    `Rule`, forming an arbitrarily deep tree (the original SQL design
//!    needs `WITH RECURSIVE` for this) — not a single-hop lookup.
//! 2. **Multiple distinct relation kinds** (`requires`, `implements`, ...)
//!    between `Rule`s — not one relation type.
//!
//! # Finding 1: parent-chain traversal composes cleanly, with one real wrinkle
//!
//! Repeated [`crate::generic::query::Parent`] calls compose into arbitrary-
//! depth traversal exactly the way `two_hop_neighbors` composed two
//! `Neighbors` calls (ADR-0004's precedent) — see [`chain_to_root`], a
//! plain generic loop, **no new trait needed**. This is a genuine "it just
//! works" outcome for the composition question.
//!
//! The real wrinkle is in the schema, not the query side: `Rule`'s parent
//! is *optional* (a root rule has none) — `Order`'s `customer_id` never
//! had this shape (every order has exactly one customer). Representing
//! that honestly means `ChildOf::ParentId = Option<Uuid>`, not `Uuid`.
//! `Parent<C, Marker>` itself has no trouble with this — it just returns
//! `Option<C::ParentId>` (i.e. `Option<Option<Uuid>>`, structurally fine).
//! But [`crate::generic::query::Children`]'s own bound,
//! `C: ChildOf<Marker, ParentId = P::Id>`, requires the child's
//! `ParentId` type to be *exactly* the parent record type's `Id` — which
//! `Option<Uuid>` is not, since `Rule::Id = Uuid`. **This means `Children`
//! cannot be implemented for `Rule` at all with an honest optional
//! `ParentId`, a genuine, concrete trait-bound conflict, not a style
//! preference.** Not fixed or worked around here (this round only needs
//! `Parent`, for chain-to-root traversal, not the reverse "list direct
//! children" direction) — flagged as a real, narrow gap for the report.
//! The two ways out, if a future round needed `Children` too: a sentinel
//! `ParentId = Uuid` (e.g. a root self-referencing its own id, or
//! `Uuid::nil()`) trading honesty for trait-fit, or relaxing `Children`'s
//! bound to something like `Into<P::Id>`/a projection instead of type
//! equality — a real design change, not attempted here.
//!
//! Also unlike `two_hop_neighbors` (fixed at exactly 2 hops, so cycles are
//! structurally irrelevant), unbounded recursion needs its own cycle
//! guard — the trait set provides no protection against a malformed
//! (cyclic) parent chain, so [`chain_to_root`] carries one explicitly.
//!
//! # Finding 2: multiple relation kinds hit the *exact* same coherence wall multiple `ScannableField`s did — and the same macro fix extends
//!
//! No existing trait fits `RuleRelation`: it's directed (unlike
//! `SymmetricRelation`) and many-to-many with *externally-supplied* edges,
//! not a foreign key living on the record (unlike `ChildOf`, which is
//! exactly what makes `Order belongs_to Customer`'s `Parent` a free
//! blanket impl — `RuleRelation` has no such single field to read). This
//! round prototypes a new, spike-local shape to test it: [`DirectedRelation`]
//! (marker trait, mirrors `SymmetricRelation`), [`RelatedTo`] (query trait,
//! mirrors `Neighbors`), [`DirectedRelated`] (store wrapper, mirrors
//! `Symmetric` but one-directional). **Deliberately kept local to this
//! spike file, not added to `crate::generic::{traits,query,store}`** — per
//! this round's own instruction to stop for review before any design-doc
//! change; if this shape were ever promoted, the natural home is
//! alongside `Neighbors`/`Symmetric` in the real library, not decided here.
//!
//! With two relation kinds (`Requires`, `Implements`) stacked
//! (`DirectedRelated<DirectedRelated<BaseStore<Rule>, Rule, Requires>, Rule, Implements>`),
//! a first, naive attempt at a generic forwarding impl —
//! `impl<S, R, Marker, OtherMarker> RelatedTo<R, OtherMarker> for DirectedRelated<S, R, Marker>` —
//! was actually compiled (in an isolated `rustc` probe, not left in this
//! file) to confirm, not assume, the failure mode. It hit the *identical*
//! `E0119` conflicting-implementations error `ScanField`'s own naive
//! attempt hit in the macro-forwarding round:
//!
//! ```text
//! error[E0119]: conflicting implementations of trait `RelatedTo<_, _>` for type `DirectedRelated<_, _, _>`
//! ```
//!
//! Same root cause: Rust's coherence checker can't prove `OtherMarker !=
//! Marker`, so the generic impl and `DirectedRelated`'s own direct impl
//! for its own marker are seen as potentially the same impl. **The same
//! `forward_scannable_pairs!` rotating-accumulator macro pattern extends
//! directly** — see [`forward_related_to_pairs`]'s invocation below,
//! generating the one ordered pair two markers produce
//! (`Requires`-forwarded-through-`Implements` and vice versa). This is a
//! clean, positive "targeted fix, same shape as before" outcome, not a
//! new problem needing new design work.

use crate::generic::query::{GetById, Parent};
use crate::generic::traits::{ChildOf, Record};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingStrength {
    Shall,
    Should,
    May,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: Uuid,
    pub shall_statement: String,
    pub binding_strength: BindingStrength,
    /// `None` for a root rule — see this module's docs on why this is
    /// `Option<Uuid>`, not a bare `Uuid`, and what that costs.
    pub parent_rule_id: Option<Uuid>,
}

impl Record for Rule {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

pub struct ParentOf;
impl ChildOf<ParentOf> for Rule {
    type ParentId = Option<Uuid>;
    fn parent_id(&self) -> Option<Uuid> {
        self.parent_rule_id
    }
}

/// Owns `Rule` records in a `HashMap` — the indexed side of the parent-
/// chain benchmark comparison, reusing `crate::generic::store::BaseStore`'s
/// exact shape but redefined here rather than imported, since this spike
/// deliberately doesn't depend on any other composable layer from
/// `crate::generic::store` (only `GetById`, via which `Parent` falls out
/// for free through the blanket impl already in `crate::generic::store`).
pub struct IndexedRuleStore {
    records: HashMap<Uuid, Rule>,
}

impl IndexedRuleStore {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            records: rules.into_iter().map(|r| (r.id, r)).collect(),
        }
    }
}

impl GetById<Rule> for IndexedRuleStore {
    fn get(&self, id: Uuid) -> Option<Rule> {
        self.records.get(&id).cloned()
    }
}

/// The naive baseline: a linear scan, no index — same role
/// `NaiveOrderStore` played for the directed-relation-spike round.
pub struct NaiveRuleStore {
    rules: Vec<Rule>,
}

impl NaiveRuleStore {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }
}

impl GetById<Rule> for NaiveRuleStore {
    fn get(&self, id: Uuid) -> Option<Rule> {
        self.rules.iter().find(|r| r.id == id).cloned()
    }
}

/// Composes repeated [`Parent`] calls into full-depth traversal — the
/// directed, unbounded-depth analogue of `two_hop_neighbors`'s "compose
/// the same primitive twice" (ADR-0004's precedent), generalized to "as
/// many times as the chain is deep." Returns the full chain, starting
/// with `id` itself and ending at the root (the last id whose own parent
/// is `None`).
///
/// # Cycle guard
///
/// Unlike `two_hop_neighbors` (fixed at exactly 2 hops, so a cycle in the
/// underlying relation can't cause unbounded work), this loop has no
/// structural bound — a malformed (cyclic) parent chain would loop
/// forever without one. Stops and returns the chain built so far if a
/// previously-seen id is encountered again, rather than looping forever.
/// The trait set itself provides no such protection; it isn't a design
/// gap in `Parent`, just a real, easy-to-miss cost of recursion that a
/// single fixed hop count never had to think about.
pub fn chain_to_root<S>(store: &S, id: Uuid) -> Vec<Uuid>
where
    S: Parent<Rule, ParentOf>,
{
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = id;
    // `store.parent(current)` is `Option<Option<Uuid>>` — the outer
    // `Option` is `Parent`'s own "does this id even exist" (`None` if
    // not, matching `GetById::get`'s "unknown id" convention, and the
    // loop's own exit for an unknown id); the inner `Option` is
    // `Rule::ParentId` itself, `None` at the root (handled inside the
    // loop body below). Checked before pushing `current`, so an unknown
    // id yields an empty chain rather than a one-element chain containing
    // an id that was never actually a real record.
    while let Some(parent) = store.parent(current) {
        if !seen.insert(current) {
            break;
        }
        chain.push(current);
        match parent {
            Some(parent_id) => current = parent_id,
            None => break, // reached a root
        }
    }
    chain
}

/// `R` participates in a directed relation, identified by `Marker` —
/// mirrors [`crate::generic::traits::SymmetricRelation`]'s shape but
/// one-directional. Spike-local: see this module's docs on why this
/// isn't (yet) part of `crate::generic::traits`.
pub trait DirectedRelation<Marker>: Record {}

/// "Everything `id` relates to under this relation kind" — the directed
/// analogue of [`crate::generic::query::Neighbors`].
pub trait RelatedTo<R, Marker>
where
    R: DirectedRelation<Marker>,
{
    fn related_to(&self, id: R::Id) -> Vec<R::Id>;
}

/// Adds one `RelatedTo` capability over an inner store, from an
/// externally-supplied directed edge list — the directed analogue of
/// [`crate::generic::store::Symmetric`] (which builds a bidirectional
/// adjacency map from an undirected edge list; this builds a one-directional
/// one).
pub struct DirectedRelated<S, R, Marker>
where
    R: DirectedRelation<Marker>,
{
    inner: S,
    outgoing: HashMap<R::Id, Vec<R::Id>>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> DirectedRelated<S, R, Marker>
where
    R: DirectedRelation<Marker>,
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

    /// Needed by `forward_related_to_pairs!`'s generated pairs — same
    /// rationale as `crate::generic::store::Scanned::inner`.
    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S, R, Marker> RelatedTo<R, Marker> for DirectedRelated<S, R, Marker>
where
    R: DirectedRelation<Marker>,
{
    fn related_to(&self, id: R::Id) -> Vec<R::Id> {
        self.outgoing.get(&id).cloned().unwrap_or_default()
    }
}

// Forwarding impl: `DirectedRelated<S, ..>` re-exposing `GetById` from
// its inner store — same shape as every forwarding impl in
// `crate::generic::store`.
impl<S, R, Marker> GetById<R> for DirectedRelated<S, R, Marker>
where
    R: DirectedRelation<Marker>,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        self.inner.get(id)
    }
}

/// Generates the concrete, non-generic `RelatedTo` forwarding impl per
/// *ordered pair* of relation-kind markers — the exact same
/// rotating-accumulator technique as `crate::forward_scannable_pairs!`,
/// applied to `RelatedTo`/`DirectedRelated` instead of
/// `ScanField`/`Scanned`. See this module's docs for the confirmed
/// `E0119` this works around. `$record; Marker1, Marker2, ...` — no
/// per-marker value type needed here (unlike `forward_scannable_pairs!`,
/// which needs each field's concrete `ScanValue` type to write a
/// `Vec<Value>` return type; `RelatedTo::related_to` always returns
/// `Vec<R::Id>`, so there's nothing analogous to look up).
#[macro_export]
macro_rules! forward_related_to_pairs {
    ($record:ty; $($marker:ident),+ $(,)?) => {
        $crate::forward_related_to_pairs!(@rotate $record; []; [$($marker),+]);
    };
    (@rotate $record:ty; [$($prefix:ident),*]; [$owner:ident $(, $rest:ident)*]) => {
        $crate::forward_related_to_pairs!(@pairs $record; $owner; [$($prefix,)* $($rest),*]);
        $crate::forward_related_to_pairs!(@rotate $record; [$($prefix,)* $owner]; [$($rest),*]);
    };
    (@rotate $record:ty; [$($prefix:ident),*]; []) => {};
    (@pairs $record:ty; $owner:ident; [$($forwarded:ident),* $(,)?]) => {
        $(
            $crate::forward_related_to_pairs!(@impl_pair $record; $owner; $forwarded);
        )*
    };
    (@impl_pair $record:ty; $owner:ident; $forwarded:ident) => {
        impl<S> $crate::generic_spike::rule_trace::RelatedTo<$record, $forwarded>
            for $crate::generic_spike::rule_trace::DirectedRelated<S, $record, $owner>
        where
            S: $crate::generic_spike::rule_trace::RelatedTo<$record, $forwarded>,
        {
            fn related_to(
                &self,
                id: <$record as $crate::generic::traits::Record>::Id,
            ) -> Vec<<$record as $crate::generic::traits::Record>::Id> {
                $crate::generic_spike::rule_trace::DirectedRelated::inner(self).related_to(id)
            }
        }
    };
}

pub struct Requires;
impl DirectedRelation<Requires> for Rule {}

pub struct Implements;
impl DirectedRelation<Implements> for Rule {}

forward_related_to_pairs!(Rule; Requires, Implements);

/// The indexed stack: `IndexedRuleStore` (owns records, `HashMap`-backed
/// `GetById`) -> `DirectedRelated<.., Requires>` -> `DirectedRelated<.., Implements>`.
pub type RuleRelationStore =
    DirectedRelated<DirectedRelated<IndexedRuleStore, Rule, Requires>, Rule, Implements>;

pub fn build_rule_relation_store(
    rules: Vec<Rule>,
    requires_edges: &[(Uuid, Uuid)],
    implements_edges: &[(Uuid, Uuid)],
) -> RuleRelationStore {
    let base = IndexedRuleStore::new(rules);
    let with_requires = DirectedRelated::<_, Rule, Requires>::new(base, requires_edges);
    DirectedRelated::<_, Rule, Implements>::new(with_requires, implements_edges)
}

/// The naive baseline: a linear scan over each kind's own edge list, no
/// index of any kind — mirrors `NaiveOrderStore`'s role for the
/// directed-relation-spike round.
pub struct NaiveRuleRelationStore {
    requires_edges: Vec<(Uuid, Uuid)>,
    implements_edges: Vec<(Uuid, Uuid)>,
}

impl NaiveRuleRelationStore {
    pub fn new(requires_edges: Vec<(Uuid, Uuid)>, implements_edges: Vec<(Uuid, Uuid)>) -> Self {
        Self {
            requires_edges,
            implements_edges,
        }
    }
}

impl RelatedTo<Rule, Requires> for NaiveRuleRelationStore {
    fn related_to(&self, id: Uuid) -> Vec<Uuid> {
        self.requires_edges
            .iter()
            .filter(|&&(from, _)| from == id)
            .map(|&(_, to)| to)
            .collect()
    }
}

impl RelatedTo<Rule, Implements> for NaiveRuleRelationStore {
    fn related_to(&self, id: Uuid) -> Vec<Uuid> {
        self.implements_edges
            .iter()
            .filter(|&&(from, _)| from == id)
            .map(|&(_, to)| to)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(depth: usize) -> Vec<Rule> {
        (0..depth)
            .map(|i| Rule {
                id: Uuid::from_u128(i as u128),
                shall_statement: format!("rule {i}"),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: if i == 0 {
                    None
                } else {
                    Some(Uuid::from_u128((i - 1) as u128))
                },
            })
            .collect()
    }

    #[test]
    fn chain_to_root_walks_every_ancestor_indexed_and_naive_agree() {
        let indexed = IndexedRuleStore::new(chain(5));
        let naive = NaiveRuleStore::new(chain(5));
        let leaf = Uuid::from_u128(4);

        let expected: Vec<Uuid> = vec![4, 3, 2, 1, 0]
            .into_iter()
            .map(Uuid::from_u128)
            .collect();
        assert_eq!(chain_to_root(&indexed, leaf), expected);
        assert_eq!(chain_to_root(&naive, leaf), expected);
    }

    #[test]
    fn chain_to_root_from_the_root_itself_is_a_single_element_chain() {
        let store = IndexedRuleStore::new(chain(5));
        assert_eq!(
            chain_to_root(&store, Uuid::from_u128(0)),
            vec![Uuid::from_u128(0)]
        );
    }

    #[test]
    fn chain_to_root_unknown_id_is_empty() {
        let store = IndexedRuleStore::new(chain(5));
        assert!(chain_to_root(&store, Uuid::from_u128(999)).is_empty());
    }

    #[test]
    fn chain_to_root_stops_on_a_cycle_instead_of_looping_forever() {
        // A malformed dataset: 0 -> 1 -> 0 (a 2-cycle), unreachable through
        // any real ChildOf construction path but not something the trait
        // set itself prevents — this is exactly the guard's reason to exist.
        let cyclic = vec![
            Rule {
                id: Uuid::from_u128(0),
                shall_statement: "a".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(1)),
            },
            Rule {
                id: Uuid::from_u128(1),
                shall_statement: "b".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(0)),
            },
        ];
        let store = IndexedRuleStore::new(cyclic);
        let result = chain_to_root(&store, Uuid::from_u128(0));
        assert_eq!(result.len(), 2, "must terminate, not loop forever");
    }

    fn sample_relation_rules() -> Vec<Rule> {
        (1..=3)
            .map(|i| Rule {
                id: Uuid::from_u128(i),
                shall_statement: format!("rule {i}"),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: None,
            })
            .collect()
    }

    #[test]
    fn related_to_forwards_through_the_stacked_layer_indexed_and_naive_agree() {
        let requires_edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
        let implements_edges = vec![(Uuid::from_u128(1), Uuid::from_u128(3))];

        let store =
            build_rule_relation_store(sample_relation_rules(), &requires_edges, &implements_edges);
        // Requires is forwarded through the outer Implements layer.
        assert_eq!(
            RelatedTo::<Rule, Requires>::related_to(&store, Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        // Implements is the outer layer's own direct impl.
        assert_eq!(
            RelatedTo::<Rule, Implements>::related_to(&store, Uuid::from_u128(1)),
            vec![Uuid::from_u128(3)]
        );
        assert!(RelatedTo::<Rule, Requires>::related_to(&store, Uuid::from_u128(2)).is_empty());

        let naive = NaiveRuleRelationStore::new(requires_edges, implements_edges);
        assert_eq!(
            RelatedTo::<Rule, Requires>::related_to(&naive, Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            RelatedTo::<Rule, Implements>::related_to(&naive, Uuid::from_u128(1)),
            vec![Uuid::from_u128(3)]
        );
    }

    /// A compile-time proof, not a runtime one — mirrors
    /// `order_customer.rs`'s `_all_six_ordered_pairs_exist` for the
    /// `forward_scannable_pairs!` macro: with 2 markers, the macro must
    /// generate both ordered pairs (`Requires` forwarded through
    /// `Implements`, and `Implements` forwarded through `Requires`), not
    /// just the one this file's own `RuleRelationStore` stack happens to
    /// need.
    #[allow(dead_code)]
    fn _pair_exists<S, Owner, Forwarded>()
    where
        S: RelatedTo<Rule, Forwarded>,
        DirectedRelated<S, Rule, Owner>: RelatedTo<Rule, Forwarded>,
        Rule: DirectedRelation<Owner> + DirectedRelation<Forwarded>,
    {
    }

    #[allow(dead_code)]
    fn _both_ordered_pairs_exist<S: RelatedTo<Rule, Requires> + RelatedTo<Rule, Implements>>() {
        _pair_exists::<S, Implements, Requires>();
        _pair_exists::<S, Requires, Implements>();
    }
}
