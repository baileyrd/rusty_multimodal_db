//! Sixth spike round, first of three pieces: `Source` — where a `Rule`
//! (or, in the full external design, any requirement) comes from. Part of
//! the finalized five-entity req-traceability schema `rule_trace.rs`'s own
//! module docs describe (`Source`/`Rule`/`RuleRelation`/`RuleDerivation`/
//! `SelectionGroup`); this file is the `Source` piece. **Still a spike, not
//! a migration decision** — same discipline as every prior round: nothing
//! here is wired into `crate::generic`, `GENERIC-SCHEMA-DESIGN.md`, or
//! ADR-0009.
//!
//! # Shape: the same nested hierarchy as `Rule`'s parent-chain
//!
//! A `Source` can nest under a parent `Source` (e.g. a specific
//! implementation detail nested under the org-wide standard it came from)
//! — structurally identical to `Rule`'s own optional-parent tree, so this
//! reuses the exact same [`ChildOf`]/[`Parent`] pattern `rule_trace`
//! established, fixed signature included (`ParentId` the bare `Uuid`,
//! optionality on `parent_id`'s return, not the associated type; see
//! `crate::generic::traits::ChildOf`'s own doc comment for why).
//!
//! # `domain_tags`: only the root populates it directly
//!
//! Per this round's own task: `domain_tags: Vec<String>` is only ever
//! populated on a *root* `Source` (`parent_source_id.is_none()`); a
//! nested `Source`'s *effective* domain tags are whatever its root has,
//! found by walking up the parent chain — not accumulated along the way
//! (a mid-tree `Source` carries an empty `domain_tags` of its own; the
//! walk doesn't merge anything it passes through, it just keeps going
//! until there's nowhere further up to go, then reads that node's tags).
//! [`source_root_id`] is the walk, shaped exactly like `rule_trace`'s own
//! [`chain_to_root`](super::rule_trace::chain_to_root) loop, but tracking
//! only the *last* node reached instead of accumulating a `Vec` — since
//! the caller only ever wants the root, not the path to it.
//! [`effective_domain_tags`] is `source_root_id` plus one `GetById` read.

use crate::generic::query::{GetById, Parent};
use crate::generic::traits::{ChildOf, Record};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    PractitionerImplementation,
    OrgImplementation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: Uuid,
    pub kind: SourceKind,
    /// `None` for a root source — see this module's docs on why this is
    /// `Option<Uuid>`, the same shape `Rule::parent_rule_id` already
    /// established.
    pub parent_source_id: Option<Uuid>,
    /// Only meaningful (and only ever populated by a well-formed dataset)
    /// on a root `Source` — see this module's docs. A nested `Source`
    /// leaves this empty; its *effective* tags come from
    /// [`effective_domain_tags`], not this field directly.
    pub domain_tags: Vec<String>,
}

impl Record for Source {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

pub struct SourceOf;
impl ChildOf<SourceOf> for Source {
    type ParentId = Uuid;
    fn parent_id(&self) -> Option<Uuid> {
        self.parent_source_id
    }
}

/// Owns `Source` records in a `HashMap` — the indexed side of the
/// root-lookup benchmark comparison, same shape as `rule_trace`'s own
/// `IndexedRuleStore` (redefined here rather than imported for the same
/// reason: this spike deliberately doesn't depend on any composable layer
/// from `crate::generic::store` beyond `GetById`, via which `Parent`
/// falls out for free through the blanket impl already there).
pub struct IndexedSourceStore {
    records: HashMap<Uuid, Source>,
}

impl IndexedSourceStore {
    pub fn new(sources: Vec<Source>) -> Self {
        Self {
            records: sources.into_iter().map(|s| (s.id, s)).collect(),
        }
    }
}

impl GetById<Source> for IndexedSourceStore {
    fn get(&self, id: Uuid) -> Option<Source> {
        self.records.get(&id).cloned()
    }
}

/// The naive baseline: a linear scan, no index — same role
/// `NaiveRuleStore` played for the parent-chain-traversal round.
pub struct NaiveSourceStore {
    sources: Vec<Source>,
}

impl NaiveSourceStore {
    pub fn new(sources: Vec<Source>) -> Self {
        Self { sources }
    }
}

impl GetById<Source> for NaiveSourceStore {
    fn get(&self, id: Uuid) -> Option<Source> {
        self.sources.iter().find(|s| s.id == id).cloned()
    }
}

/// Walks from `id` up through successive `parent_id()` links to the root
/// (the first node whose own parent is `None`) and returns *that node's*
/// id — not the path to it, unlike
/// [`chain_to_root`](super::rule_trace::chain_to_root), which this
/// mirrors structurally (same `Parent`-only composition, same cycle
/// guard, same "stop where the trail goes cold" behavior for a dangling
/// parent reference — see `chain_to_root`'s own doc comment for why that
/// isn't distinguished from a legitimate root by this walk alone).
/// `None` only when `id` itself isn't a real `Source`.
pub fn source_root_id<S>(store: &S, id: Uuid) -> Option<Uuid>
where
    S: Parent<Source, SourceOf>,
{
    let mut root = None;
    let mut seen = HashSet::new();
    let mut current = id;
    let mut next = store.parent(current);
    while let Ok(parent) = next {
        if !seen.insert(current) {
            break; // cycle guard — `root` already holds the last-seen node
        }
        root = Some(current);
        match parent {
            Some(parent_id) => {
                current = parent_id;
                next = store.parent(current);
            }
            None => break, // reached the real root
        }
    }
    root
}

/// [`source_root_id`] plus one `GetById` read of the root's own
/// `domain_tags` — the actual "effective domain tags for a (possibly
/// nested) `Source`" query this module exists to answer. Empty for an
/// unknown `id`, or for a well-formed root whose own `domain_tags`
/// happens to be empty — this function doesn't distinguish the two, same
/// as `GetById::get` on a missing id and `Vec::new()` are indistinguishable
/// once both are just "empty."
pub fn effective_domain_tags<S>(store: &S, id: Uuid) -> Vec<String>
where
    S: Parent<Source, SourceOf> + GetById<Source>,
{
    match source_root_id(store, id) {
        Some(root_id) => store
            .get(root_id)
            .map(|source| source.domain_tags)
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(depth: usize, root_tags: Vec<String>) -> Vec<Source> {
        (0..depth)
            .map(|i| Source {
                id: Uuid::from_u128(i as u128),
                kind: SourceKind::OrgImplementation,
                parent_source_id: if i == 0 {
                    None
                } else {
                    Some(Uuid::from_u128((i - 1) as u128))
                },
                domain_tags: if i == 0 {
                    root_tags.clone()
                } else {
                    Vec::new()
                },
            })
            .collect()
    }

    #[test]
    fn source_root_id_of_the_root_itself_is_itself() {
        let store = IndexedSourceStore::new(chain(5, vec!["payments".into()]));
        assert_eq!(
            source_root_id(&store, Uuid::from_u128(0)),
            Some(Uuid::from_u128(0))
        );
    }

    #[test]
    fn source_root_id_of_a_leaf_walks_to_the_root_indexed_and_naive_agree() {
        let indexed = IndexedSourceStore::new(chain(5, vec!["payments".into()]));
        let naive = NaiveSourceStore::new(chain(5, vec!["payments".into()]));
        assert_eq!(
            source_root_id(&indexed, Uuid::from_u128(4)),
            Some(Uuid::from_u128(0))
        );
        assert_eq!(
            source_root_id(&naive, Uuid::from_u128(4)),
            Some(Uuid::from_u128(0))
        );
    }

    #[test]
    fn source_root_id_of_an_unknown_id_is_none() {
        let store = IndexedSourceStore::new(chain(5, vec!["payments".into()]));
        assert_eq!(source_root_id(&store, Uuid::from_u128(999)), None);
    }

    #[test]
    fn source_root_id_stops_on_a_cycle_instead_of_looping_forever() {
        let cyclic = vec![
            Source {
                id: Uuid::from_u128(0),
                kind: SourceKind::OrgImplementation,
                parent_source_id: Some(Uuid::from_u128(1)),
                domain_tags: Vec::new(),
            },
            Source {
                id: Uuid::from_u128(1),
                kind: SourceKind::OrgImplementation,
                parent_source_id: Some(Uuid::from_u128(0)),
                domain_tags: Vec::new(),
            },
        ];
        let store = IndexedSourceStore::new(cyclic);
        // Must terminate — the exact root chosen among a cycle's own
        // members isn't a meaningful guarantee, only termination is.
        assert!(source_root_id(&store, Uuid::from_u128(0)).is_some());
    }

    #[test]
    fn effective_domain_tags_of_the_root_is_its_own_tags() {
        let store = IndexedSourceStore::new(chain(5, vec!["payments".into(), "billing".into()]));
        assert_eq!(
            effective_domain_tags(&store, Uuid::from_u128(0)),
            vec!["payments".to_string(), "billing".to_string()]
        );
    }

    #[test]
    fn effective_domain_tags_of_a_nested_source_is_inherited_from_the_root_not_accumulated() {
        let indexed = IndexedSourceStore::new(chain(5, vec!["payments".into()]));
        let naive = NaiveSourceStore::new(chain(5, vec!["payments".into()]));
        // Every non-root node's own `domain_tags` is empty (see `chain`'s
        // own construction) — if the walk were accumulating instead of
        // reading only the root, this would still be `vec!["payments"]`
        // for the wrong reason (concatenating empties), so this alone
        // doesn't distinguish the two behaviors; the real proof is that a
        // nested node's *own* `domain_tags` field is never consulted,
        // which `chain`'s construction (empty for every non-root) already
        // guarantees: a bug that accidentally read the leaf's own field
        // instead of walking to the root would return `vec![]` here, not
        // `vec!["payments"]`.
        assert_eq!(
            effective_domain_tags(&indexed, Uuid::from_u128(4)),
            vec!["payments".to_string()]
        );
        assert_eq!(
            effective_domain_tags(&naive, Uuid::from_u128(4)),
            vec!["payments".to_string()]
        );
    }

    #[test]
    fn effective_domain_tags_of_an_unknown_id_is_empty() {
        let store = IndexedSourceStore::new(chain(5, vec!["payments".into()]));
        assert!(effective_domain_tags(&store, Uuid::from_u128(999)).is_empty());
    }
}
