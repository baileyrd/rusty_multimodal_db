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
//! The real wrinkle was in the schema, not the query side: `Rule`'s parent
//! is *optional* (a root rule has none) — `Order`'s `customer_id` never
//! had this shape (every order has exactly one customer). The first
//! version of this file represented that honestly via `ChildOf::ParentId
//! = Option<Uuid>`, which broke [`crate::generic::query::Children`]'s own
//! bound (`C: ChildOf<Marker, ParentId = P::Id>`, requiring the child's
//! `ParentId` type to *exactly* equal the parent record's `Id` type —
//! `Option<Uuid>` never equals `Uuid`) — a genuine, concrete trait-bound
//! conflict, not a style preference, confirmed directly against `main`
//! before being fixed.
//!
//! **Fixed in a follow-up round, in `crate::generic::traits::ChildOf`
//! itself, not worked around locally here** — the winning redesign kept
//! `ParentId` as the *bare* id type (so it can still equal `P::Id`) and
//! moved optionality onto the method instead: `fn parent_id(&self) ->
//! Option<Self::ParentId>`. `Rule::ChildOf::ParentId` is `Uuid` here now,
//! matching `Rule::Id` exactly, and `parent_id()` returns
//! `self.parent_rule_id` (already `Option<Uuid>`) unchanged. `Order`'s
//! own impl just wraps its always-present `customer_id` in `Some(..)` —
//! "the method never returns `None`," exactly as the redesign intended,
//! not a separate code path. [`crate::generic::store::Reversed::new`]
//! (the `Children` index builder) now skips entries with no parent via a
//! plain `if let Some(parent_id) = ..`, a no-op for `Order`. Real,
//! confirmed-not-assumed cost *at the time*: `Parent::parent` collapsed to
//! a single-level `Option`, so it could no longer distinguish "child not
//! found" from "child found, no parent" — [`chain_to_root`] worked around
//! it locally with an extra `GetById` existence check. **Restored in a
//! second follow-up round**, in `crate::generic::query::Parent` itself:
//! see that trait's own doc comment for the `Result`-returning fix, and
//! [`chain_to_root`]'s doc comment for how the local workaround was then
//! removed rather than kept redundantly. `RuleTreeStore`/[`build_rule_tree_store`] below is `Children` actually
//! implemented for `Rule`, benchmarked against a naive baseline the same
//! way as every other capability in this file — see
//! `benches/rule_trace_spike.rs`'s `rule_children_lookup` group. No
//! trait-set redesign was needed beyond this one fix — no new
//! `OptionalChildOf` trait, no sentinel id.
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

use crate::generic::query::{Children, GetById, Parent};
use crate::generic::store::Reversed;
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
    // `ParentId` is the bare `Uuid` — matching `Rule::Id` exactly, which
    // is what `Children`'s bound (`ParentId = P::Id`) needs. Optionality
    // now lives on `parent_id`'s return type instead (`crate::generic`'s
    // own follow-up fix, motivated directly by this domain) — this impl
    // predates that fix and originally had to work around it by making
    // `ParentId` itself `Option<Uuid>`, which was exactly what made
    // `Children` impossible to implement for `Rule` at all. Now
    // `self.parent_rule_id` (already `Option<Uuid>`) is returned as-is,
    // no wrapping needed.
    type ParentId = Uuid;
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

impl Children<Rule, Rule, ParentOf> for NaiveRuleStore {
    fn children(&self, parent_id: Uuid) -> Vec<Uuid> {
        self.rules
            .iter()
            .filter(|r| r.parent_id() == Some(parent_id))
            .map(|r| r.id)
            .collect()
    }
}

/// The indexed stack for the "list direct children" direction —
/// [`crate::generic::store::Reversed`], self-referential (`Rule` is both
/// the parent record type `P` and the child record type `C`). This is
/// the concrete deliverable the optional-parent `ChildOf` fix (this
/// round's own follow-on) exists to make possible at all: with the
/// pre-fix `ChildOf` (`ParentId = Option<Uuid>`), `Reversed<_, Rule, Rule,
/// ParentOf>` couldn't even name its own type — `ParentId` could never
/// equal `Rule::Id` (`Uuid`). See `crate::generic::traits::ChildOf`'s own
/// doc comment for the fix.
pub type RuleTreeStore = Reversed<IndexedRuleStore, Rule, Rule, ParentOf>;

pub fn build_rule_tree_store(rules: Vec<Rule>) -> RuleTreeStore {
    let base = IndexedRuleStore::new(rules.clone());
    Reversed::<_, Rule, Rule, ParentOf>::new(base, &rules)
}

/// Composes repeated [`Parent`] calls into full-depth traversal — the
/// directed, unbounded-depth analogue of `two_hop_neighbors`'s "compose
/// the same primitive twice" (ADR-0004's precedent), generalized to "as
/// many times as the chain is deep." Returns the full chain, starting
/// with `id` itself and ending at the root (the last id whose own parent
/// is `None`). An unknown `id` yields an empty chain.
///
/// # Only needs `Parent`, not `GetById` too
///
/// This function used to take an extra `GetById<Rule>` bound, needed
/// only to check `id`'s existence up front — `Parent::parent` itself
/// used to collapse "not found" and "found, no parent" to the same
/// `None`, so that check couldn't be done through `Parent` alone. Now
/// that `Parent::parent` returns `Result<Option<Id>, NotFound<Id>>`
/// (see its own doc comment), `Err` *is* that existence check, so the
/// separate bound and upfront call are gone — this composes from
/// `Parent` alone, the way `two_hop_neighbors` composes from `Neighbors`
/// alone.
///
/// **A real, measured cost, not free**: unlike the old upfront-check
/// version (one existence check for `id`, then `Parent::parent` trusted
/// for the rest of the chain), this version checks existence via `Err`
/// on *every* node, `id` included — the only way to keep
/// `chain_to_root_unknown_id_is_empty` honest without the extra bound.
/// That check has to happen, and be resolved, before this node is
/// pushed onto `chain` (not after, the way the old loop checked the
/// *next* node's optionality only once the current one was already
/// pushed) — a real data-dependency shift, not an equivalent rewrite.
/// Measured directly (same-session, back-to-back, `git stash` before/
/// after isolation, `rule_chain_traversal/indexed` at depths 10/100/1000,
/// 100 samples/5s each): roughly 25–35% slower than before this fix,
/// consistent across all three depths. The naive baseline and a single
/// isolated `Parent::parent` call both showed no comparable regression
/// in the same measurements — the cost is specific to this loop's now-
/// required per-node existence check, not to the `Result` return type on
/// its own. This is the accepted cost of the correctness guarantee, not
/// an unexamined regression — see `crate::generic`'s own module docs for
/// the precedent (`Scanned::get`'s write-through fix) of reporting a
/// real measured cost rather than assuming a fix is free.
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
    let mut next = store.parent(current);
    while let Ok(parent) = next {
        if !seen.insert(current) {
            break;
        }
        chain.push(current);
        match parent {
            Some(parent_id) => {
                current = parent_id;
                next = store.parent(current);
            }
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

    fn tree() -> Vec<Rule> {
        // 0 is root; 1,2,3 are its direct children; 4 is 1's own child
        // (so "children of the root" and "children of a non-root" are
        // both exercised, and 4 must NOT show up as a child of 0).
        vec![
            Rule {
                id: Uuid::from_u128(0),
                shall_statement: "root".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: None,
            },
            Rule {
                id: Uuid::from_u128(1),
                shall_statement: "child 1".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(0)),
            },
            Rule {
                id: Uuid::from_u128(2),
                shall_statement: "child 2".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(0)),
            },
            Rule {
                id: Uuid::from_u128(3),
                shall_statement: "child 3".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(0)),
            },
            Rule {
                id: Uuid::from_u128(4),
                shall_statement: "grandchild".into(),
                binding_strength: BindingStrength::Shall,
                parent_rule_id: Some(Uuid::from_u128(1)),
            },
        ]
    }

    #[test]
    fn children_of_the_root_indexed_and_naive_agree() {
        let indexed = build_rule_tree_store(tree());
        let naive = NaiveRuleStore::new(tree());

        let mut expected: Vec<Uuid> = vec![1, 2, 3].into_iter().map(Uuid::from_u128).collect();
        expected.sort();

        let mut indexed_children =
            Children::<Rule, Rule, ParentOf>::children(&indexed, Uuid::from_u128(0));
        indexed_children.sort();
        assert_eq!(indexed_children, expected);

        let mut naive_children =
            Children::<Rule, Rule, ParentOf>::children(&naive, Uuid::from_u128(0));
        naive_children.sort();
        assert_eq!(naive_children, expected);
    }

    #[test]
    fn children_of_a_non_root_node_excludes_siblings_and_the_parent_itself() {
        let indexed = build_rule_tree_store(tree());
        assert_eq!(
            Children::<Rule, Rule, ParentOf>::children(&indexed, Uuid::from_u128(1)),
            vec![Uuid::from_u128(4)]
        );
    }

    #[test]
    fn children_of_a_leaf_is_empty() {
        let indexed = build_rule_tree_store(tree());
        assert!(
            Children::<Rule, Rule, ParentOf>::children(&indexed, Uuid::from_u128(4)).is_empty()
        );
    }

    #[test]
    fn children_of_an_unknown_id_is_empty() {
        let indexed = build_rule_tree_store(tree());
        assert!(
            Children::<Rule, Rule, ParentOf>::children(&indexed, Uuid::from_u128(999)).is_empty()
        );
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
