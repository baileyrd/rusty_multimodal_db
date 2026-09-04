//! `Entity` v2 — `ENT2-FR-001`–`003`, ADR-0039, `docs/design/
//! SERVER-ENTITY-V2-REDESIGN-DESIGN.md`. Revises `ADR-0037`'s own
//! `Entity` in place (no real deployed instance exists, so this is a
//! straight revision, not a migration) to match `rusty_remind_me`'s real
//! shape found by Unit 41's verification: `name` (a new, plain field —
//! present in every `GetById`/`Query` result, no independent capability,
//! the closest a `Uuid`-keyed domain can get to real name-based lookup
//! without breaking `RecordId`'s crate-wide invariant), `kind` as an open
//! `String` (not a fixed enum) rather than `EntityKind`.
//!
//! # `kind` stays the `IndexedField`; `mention_count` stays the
//! `ScannableField` — `kind` could not become both
//!
//! `ADR-0039`'s own original proposal had `kind` filling both
//! `GenericMmapStore`'s mandatory roles (`IndexedField` *and*
//! `ScannableField`), retiring `mention_count` entirely. That does not
//! compile: `ScannableField::ScanValue: Copy`, and `GenericMmapStore`'s
//! own mmap slot mechanism (`mmap_field.rs`) is fixed-width only by
//! `ADR-0009`'s own design (§4.2) — `String` is neither `Copy` nor
//! fixed-width. Caught during implementation, not assumed away:
//! `mention_count` stays exactly as `ADR-0037` had it, satisfying the
//! structural constraint; `kind` stays the equality-filterable
//! `IndexedField` it already was, now open-ended rather than a fixed
//! enum, but **not** durably updatable over the wire (the same
//! always-read-only shape every other domain's `IndexedField` already
//! has — `Order::status`, `Employee::department`) — a real, named
//! narrowing from `ADR-0039`'s own original text, not hidden.
//!
//! # Two relations: `RelatesTo` and `MentionedWith`
//!
//! `RelatesTo` (kept, unchanged meaning) and `MentionedWith` (new) prove
//! `MultiSymmetric` (see that struct's own doc comment, linked from
//! `MentionedWith`'s below) — a genuinely new
//! multi-relation primitive, not a `Symmetric`-forwarding patch; see
//! that type's own doc comment for the real, `rustc`-confirmed reason
//! the originally-proposed generic-forwarding-impl mechanism doesn't
//! compile. `MentionedWith` is an honest mechanism-validation label, not
//! a claimed real `rusty_remind_me` relation name (still unverified,
//! that repository's own source never having been read this session).

use super::mmap_store::GenericMmapStore;
use super::store::MultiSymmetric;
use super::traits::{IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation};
use crate::durability::DurabilityError;
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// The two relation labels this domain's `MultiSymmetric` layer knows —
/// used by every `create`/`open`/`open_portable` call site so the label
/// set is written once, not re-typed at each one.
pub const RELATION_LABELS: [&str; 2] = ["relates_to", "mentioned_with"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    /// `ENT2-FR-001`: new — the closest a `Uuid`-keyed domain gets to
    /// `rusty_remind_me`'s real `name`-based identity. Plain field, no
    /// independent wire capability (every capability flag `false`, the
    /// same shape `Reminder::title`/`Order::created_at_unix_ms` already
    /// have) — `FilterEq` could not also be given to `name` without a
    /// second `GenericMmapStore`-native index slot, which `kind` already
    /// occupies (see module docs).
    pub label: String,
    /// `ENT2-FR-001`: was `EntityKind` (a fixed 5-variant enum), now an
    /// open `String` — matching Unit 41 Finding 2 directly. Stays the
    /// equality-filterable `IndexedField`; no longer durably updatable
    /// (see module docs for why).
    pub kind: String,
    /// Unchanged from `ADR-0037` — kept, not removed, since
    /// `GenericMmapStore` structurally requires one `Copy`-typed
    /// `ScannableField` and neither `label` nor `kind` (both `String`)
    /// can satisfy it.
    pub mention_count: i64,
}

impl Record for Entity {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

impl SchemaTag for Entity {
    const SCHEMA_TAG: &'static str = "entity::Entity";
}

/// `ENT2-FR-001`: the equality-filterable field — open-ended now, not a
/// fixed enum.
pub struct KindField;
impl IndexedField<KindField> for Entity {
    type IndexValue = String;
    fn indexed_value(&self) -> &String {
        &self.kind
    }
}

/// Unchanged from `ADR-0037`.
pub struct MentionCountField;
impl ScannableField<MentionCountField> for Entity {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.mention_count
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.mention_count = value;
    }
}

/// `ENT2-FR-002`: kept, unchanged meaning — `Dog::littermate_of`'s own
/// shape.
pub struct RelatesTo;
impl SymmetricRelation<RelatesTo> for Entity {}

/// `ENT2-FR-002`: new — an honest mechanism-validation relation, proving
/// [`MultiSymmetric`] with a second, real label; not a claimed
/// `rusty_remind_me` term (see module docs).
pub struct MentionedWith;
impl SymmetricRelation<MentionedWith> for Entity {}

/// The durable production stack: [`GenericMmapStore`] (owns records, the
/// `KindField` index, and `MentionCountField`) -> `MultiSymmetric<..>`
/// (both `relates_to`/`mentioned_with` adjacency, each independently
/// durable at `<path>.<label>.edges`). No `Reversed` layer — `Entity`
/// has no `ChildOf` relation.
pub type EntityProductionStack =
    MultiSymmetric<GenericMmapStore<Entity, KindField, MentionCountField>, Entity>;

/// Build a fresh, durable production store for `Entity` at `path` — the
/// generic analogue of `create_reminder_production_stack`. Writes four
/// files: `path` (the mmap file), `<path>.records` (the record blob),
/// `<path>.relates_to.edges`, `<path>.mentioned_with.edges`.
///
/// # Errors
///
/// Returns [`DurabilityError::Io`] under the same conditions
/// [`GenericMmapStore::create`] does, or if an edge blob can't be
/// written; [`DurabilityError::Serde`] if `entities`/either edge list
/// can't be serialized.
pub fn create_entity_production_stack(
    entities: Vec<Entity>,
    relates_to_edges: &[(Uuid, Uuid)],
    mentioned_with_edges: &[(Uuid, Uuid)],
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let core = GenericMmapStore::<Entity, KindField, MentionCountField>::create(entities, path)?;
    let relations = [
        (RELATION_LABELS[0].to_string(), relates_to_edges.to_vec()),
        (
            RELATION_LABELS[1].to_string(),
            mentioned_with_edges.to_vec(),
        ),
    ];
    MultiSymmetric::create(core, &relations, path)
}

/// Reopen an existing durable production store for `Entity` at `path` —
/// the generic analogue of `open_reminder_production_stack`. Each edge
/// blob is rewritten only when its own content changed.
///
/// # Errors
///
/// Returns [`DurabilityError::Io`]/[`DurabilityError::InvalidMagic`]/
/// [`DurabilityError::SchemaVersionMismatch`] under the same conditions
/// [`GenericMmapStore::open`] does, or if a stale edge blob can't be
/// rewritten; [`DurabilityError::Serde`] if a stale companion can't be
/// serialized.
pub fn open_entity_production_stack(
    entities: Vec<Entity>,
    relates_to_edges: &[(Uuid, Uuid)],
    mentioned_with_edges: &[(Uuid, Uuid)],
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let core = GenericMmapStore::<Entity, KindField, MentionCountField>::open(entities, path)?;
    let relations = [
        (RELATION_LABELS[0].to_string(), relates_to_edges.to_vec()),
        (
            RELATION_LABELS[1].to_string(),
            mentioned_with_edges.to_vec(),
        ),
    ];
    MultiSymmetric::open(core, &relations, path)
}

/// Reopen the whole stack from its files alone — `path`,
/// `<path>.records`, both `<path>.<label>.edges` files — no `entities`/
/// edge-list arguments. `RELATION_LABELS` is passed explicitly to
/// `MultiSymmetric::open_portable` since that primitive, unlike
/// `Symmetric::open_portable`, cannot self-discover an open-ended label
/// set from the path alone (see `MultiSymmetric::open_portable`'s own
/// doc comment) — `Entity`'s own two labels are fixed and known here at
/// compile time, so this is a real but narrow limitation, not one that
/// blocks portability in practice.
///
/// # Errors
///
/// Returns [`DurabilityError::RecordBlobUnreadable`] naming whichever
/// companion is missing or invalid. Otherwise everything
/// [`GenericMmapStore::open`] can return.
pub fn open_entity_production_stack_portable(
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let entities =
        GenericMmapStore::<Entity, KindField, MentionCountField>::read_portable_records(path)?;
    let core =
        GenericMmapStore::<Entity, KindField, MentionCountField>::open(entities.clone(), path)?;
    MultiSymmetric::open_portable(core, path, &RELATION_LABELS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::query::{FilterEq, GetById, MultiNeighbors, UpdateField};
    use crate::test_support::fresh_temp_dir;

    fn sample_entities() -> Vec<Entity> {
        vec![
            Entity {
                id: Uuid::from_u128(1),
                label: "Ada Lovelace".into(),
                kind: "person".into(),
                mention_count: 3,
            },
            Entity {
                id: Uuid::from_u128(2),
                label: "Analytical Engine".into(),
                kind: "concept".into(),
                mention_count: 5,
            },
            Entity {
                id: Uuid::from_u128(3),
                label: "London".into(),
                kind: "place".into(),
                mention_count: 1,
            },
        ]
    }

    fn sample_relates_to() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }

    fn sample_mentioned_with() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(3))]
    }

    #[test]
    fn create_then_get_filter_eq_and_multi_relation_neighbors() {
        let dir = fresh_temp_dir("generic_entity_v2").unwrap();
        let path = dir.join("entities.mmap");
        let store = create_entity_production_stack(
            sample_entities(),
            &sample_relates_to(),
            &sample_mentioned_with(),
            &path,
        )
        .unwrap();

        let got = GetById::<Entity>::get(&store, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.label, "Ada Lovelace");
        assert_eq!(got.mention_count, 3);

        let matches = FilterEq::<Entity, KindField>::filter_eq(&store, &"person".to_string());
        assert_eq!(matches, vec![Uuid::from_u128(1)]);
        assert!(FilterEq::<Entity, KindField>::filter_eq(&store, &"event".to_string()).is_empty());

        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &store,
                "relates_to",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(2)])
        );
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &store,
                "mentioned_with",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(3)])
        );
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(&store, "unknown", Uuid::from_u128(1)),
            None
        );
        let mut all = MultiNeighbors::<Entity>::all_neighbors(&store, Uuid::from_u128(1));
        all.sort();
        assert_eq!(all, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        let mut kinds = MultiNeighbors::<Entity>::relation_kinds(&store);
        kinds.sort();
        assert_eq!(
            kinds,
            vec!["mentioned_with".to_string(), "relates_to".to_string()]
        );
    }

    #[test]
    fn update_mention_count_is_durable_across_reopen() {
        let dir = fresh_temp_dir("generic_entity_v2_reopen").unwrap();
        let path = dir.join("entities.mmap");
        let entities = sample_entities();
        let relates_to = sample_relates_to();
        let mentioned_with = sample_mentioned_with();
        {
            let mut store = create_entity_production_stack(
                entities.clone(),
                &relates_to,
                &mentioned_with,
                &path,
            )
            .unwrap();
            UpdateField::<Entity, MentionCountField>::update(&mut store, Uuid::from_u128(1), 4)
                .unwrap();
        }
        let reopened =
            open_entity_production_stack(entities, &relates_to, &mentioned_with, &path).unwrap();
        let got = GetById::<Entity>::get(&reopened, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.mention_count, 4);
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &reopened,
                "relates_to",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(2)])
        );
    }

    #[test]
    fn portable_reopen_matches_open_neighbors_and_fields() {
        let dir = fresh_temp_dir("generic_entity_v2_portable").unwrap();
        let path = dir.join("entities.mmap");
        create_entity_production_stack(
            sample_entities(),
            &sample_relates_to(),
            &sample_mentioned_with(),
            &path,
        )
        .unwrap();

        let portable = open_entity_production_stack_portable(&path).unwrap();
        let got = GetById::<Entity>::get(&portable, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.label, "Ada Lovelace");
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &portable,
                "mentioned_with",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(3)])
        );
    }
}
