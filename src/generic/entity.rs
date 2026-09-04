//! `Entity` — this crate's fifth domain, and the generic schema
//! library's first appearance of a `SymmetricRelation` outside its own
//! `research`-gated reference material (`ENT-FR-001`, ADR-0037,
//! `docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md`). One self-referential
//! `RelatesTo` relation — the identical shape `Dog`'s own
//! `littermate_of` already has (`Neighbors` only; no `ChildOf`), so
//! `EntityProductionStack` needs exactly one `Symmetric` composition
//! layer over `GenericMmapStore`, nothing more — unlike `Employee`'s
//! `Reversed<Symmetric<..>>` stack, `Entity` has no directed relation
//! to combine it with.
//!
//! `kind` is the equality-filterable `IndexedField` (`ENT-FR-002`) —
//! the usual generic-schema convention (an enum in the index slot),
//! *not* inverted the way `Reminder::status` was: `kind` is expected
//! to change rarely if ever, so there is no forcing function to move
//! it into the scan slot. `mention_count` is the durably-mutable
//! `ScannableField` (`ENT-FR-003`) — the field expected to change over
//! an entity's lifecycle. `label` is read-only over the wire
//! (`ENT-FR-004`).
//!
//! **Not proposed or built here**: a second, independently-named
//! relation type (`relates_to` and `mentions` together, say) —
//! `Symmetric<S, R, Marker>` has no forwarding `Neighbors` impl for a
//! *different* marker from an inner layer, unlike `Reversed`, which
//! gained exactly that forwarding for `FR-012`. See `ADR-0037`'s
//! "Considered options" for the precise gap and why this domain
//! deliberately stays at one relation.

use super::mmap_store::GenericMmapStore;
use super::store::Symmetric;
use super::traits::{IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation};
use crate::durability::DurabilityError;
use crate::generic::edge_blob::edges_path;
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// An entity's category — `ENT-FR-001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind {
    Person,
    Place,
    Organization,
    Concept,
    Event,
}

/// `EntityKind`'s wire/index encoding — a fixed discriminant, the same
/// shape `server::order`/`server::employee`'s own `status_to_u32`-
/// style helpers already established for an enum `IndexedField`.
pub fn kind_to_u32(kind: EntityKind) -> u32 {
    match kind {
        EntityKind::Person => 0,
        EntityKind::Place => 1,
        EntityKind::Organization => 2,
        EntityKind::Concept => 3,
        EntityKind::Event => 4,
    }
}

pub fn kind_from_u32(value: u32) -> Option<EntityKind> {
    match value {
        0 => Some(EntityKind::Person),
        1 => Some(EntityKind::Place),
        2 => Some(EntityKind::Organization),
        3 => Some(EntityKind::Concept),
        4 => Some(EntityKind::Event),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub label: String,
    pub kind: EntityKind,
    pub mention_count: i64,
}

impl Record for Entity {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

// The name written into every `Entity` companion blob's header —
// `<path>.records` and `<path>.edges` alike, part of the on-disk
// format; see `Employee`/`Reminder`'s own impls for the same caveat.
impl SchemaTag for Entity {
    const SCHEMA_TAG: &'static str = "entity::Entity";
}

/// `ENT-FR-002`: the equality-filterable field — `kind`, not inverted
/// the way `Reminder::status` was (see module docs for why).
pub struct KindField;
impl IndexedField<KindField> for Entity {
    type IndexValue = EntityKind;
    fn indexed_value(&self) -> &EntityKind {
        &self.kind
    }
}

/// `ENT-FR-003`: the durably-mutable field.
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

/// `ENT-FR-005`: one self-referential `SymmetricRelation` — the
/// `Dog::littermate_of` shape.
pub struct RelatesTo;
impl SymmetricRelation<RelatesTo> for Entity {}

/// The durable production stack: [`GenericMmapStore`] (owns records,
/// the `KindField` index, and `MentionCountField`) -> `Symmetric<..,
/// RelatesTo>` (the `relates_to` adjacency, persisted to
/// `<path>.edges` via `STORAGE-016`). No `Reversed` layer — `Entity`
/// has no `ChildOf` relation to combine it with.
pub type EntityProductionStack =
    Symmetric<GenericMmapStore<Entity, KindField, MentionCountField>, Entity, RelatesTo>;

/// Build a fresh, durable production store for `Entity` at `path` —
/// the generic analogue of `create_employee_production_stack`/
/// `create_reminder_production_stack`. Writes three files: `path` (the
/// mmap file), `<path>.records` (the record blob), and `<path>.edges`
/// (the `relates_to` edge list).
///
/// # Errors
///
/// Returns [`DurabilityError::Io`] under the same conditions
/// [`GenericMmapStore::create`] does, or if the edge blob can't be
/// written; [`DurabilityError::Serde`] if `entities`/`relates_to_edges`
/// can't be serialized.
pub fn create_entity_production_stack(
    entities: Vec<Entity>,
    relates_to_edges: &[(Uuid, Uuid)],
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let core = GenericMmapStore::<Entity, KindField, MentionCountField>::create(entities, path)?;
    Symmetric::<_, Entity, RelatesTo>::create(core, relates_to_edges, &edges_path(path))
}

/// Reopen an existing durable production store for `Entity` at `path`
/// — the generic analogue of `open_employee_production_stack`/
/// `open_reminder_production_stack`. Keeps both companions current
/// with the caller's arguments; each rewritten only when its content
/// changed.
///
/// # Errors
///
/// Returns [`DurabilityError::Io`]/[`DurabilityError::InvalidMagic`]/
/// [`DurabilityError::SchemaVersionMismatch`] under the same
/// conditions [`GenericMmapStore::open`] does, or if a stale edge blob
/// can't be rewritten; [`DurabilityError::Serde`] if a stale companion
/// can't be serialized.
pub fn open_entity_production_stack(
    entities: Vec<Entity>,
    relates_to_edges: &[(Uuid, Uuid)],
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let core = GenericMmapStore::<Entity, KindField, MentionCountField>::open(entities, path)?;
    Symmetric::<_, Entity, RelatesTo>::open(core, relates_to_edges, &edges_path(path))
}

/// Reopen the whole stack from its three files alone — `path`,
/// `<path>.records`, `<path>.edges` — no `entities`/`relates_to_edges`
/// argument (`STORAGE-016`, the same portability `Employee`'s own
/// `open_employee_production_stack_portable` already provides).
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
    Symmetric::<_, Entity, RelatesTo>::open_portable(core, &edges_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::query::{FilterEq, GetById, Neighbors, UpdateField};
    use crate::test_support::fresh_temp_dir;

    fn sample_entities() -> Vec<Entity> {
        vec![
            Entity {
                id: Uuid::from_u128(1),
                label: "Ada Lovelace".into(),
                kind: EntityKind::Person,
                mention_count: 3,
            },
            Entity {
                id: Uuid::from_u128(2),
                label: "Analytical Engine".into(),
                kind: EntityKind::Concept,
                mention_count: 5,
            },
            Entity {
                id: Uuid::from_u128(3),
                label: "London".into(),
                kind: EntityKind::Place,
                mention_count: 1,
            },
        ]
    }

    fn sample_edges() -> Vec<(Uuid, Uuid)> {
        vec![
            (Uuid::from_u128(1), Uuid::from_u128(2)),
            (Uuid::from_u128(1), Uuid::from_u128(3)),
        ]
    }

    #[test]
    fn kind_to_u32_and_back_round_trips_every_variant() {
        for kind in [
            EntityKind::Person,
            EntityKind::Place,
            EntityKind::Organization,
            EntityKind::Concept,
            EntityKind::Event,
        ] {
            assert_eq!(kind_from_u32(kind_to_u32(kind)), Some(kind));
        }
        assert_eq!(kind_from_u32(5), None, "out of range is rejected");
    }

    #[test]
    fn create_then_get_filter_eq_and_neighbors() {
        let dir = fresh_temp_dir("generic_entity").unwrap();
        let path = dir.join("entities.mmap");
        let store =
            create_entity_production_stack(sample_entities(), &sample_edges(), &path).unwrap();

        let got = GetById::<Entity>::get(&store, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.label, "Ada Lovelace");
        assert_eq!(got.mention_count, 3);

        let matches = FilterEq::<Entity, KindField>::filter_eq(&store, &EntityKind::Person);
        assert_eq!(matches, vec![Uuid::from_u128(1)]);
        assert!(FilterEq::<Entity, KindField>::filter_eq(&store, &EntityKind::Event).is_empty());

        let mut neighbors = Neighbors::<Entity, RelatesTo>::neighbors(&store, Uuid::from_u128(1));
        neighbors.sort();
        assert_eq!(neighbors, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
    }

    #[test]
    fn update_mention_count_is_durable_across_reopen() {
        let dir = fresh_temp_dir("generic_entity_reopen").unwrap();
        let path = dir.join("entities.mmap");
        let entities = sample_entities();
        let edges = sample_edges();
        {
            let mut store =
                create_entity_production_stack(entities.clone(), &edges, &path).unwrap();
            UpdateField::<Entity, MentionCountField>::update(&mut store, Uuid::from_u128(1), 4)
                .unwrap();
        }
        let reopened = open_entity_production_stack(entities, &edges, &path).unwrap();
        let got = GetById::<Entity>::get(&reopened, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.mention_count, 4);
        assert_eq!(
            Neighbors::<Entity, RelatesTo>::neighbors(&reopened, Uuid::from_u128(1)),
            vec![Uuid::from_u128(2), Uuid::from_u128(3)]
        );
    }

    #[test]
    fn portable_reopen_matches_open_neighbors_and_fields() {
        let dir = fresh_temp_dir("generic_entity_portable").unwrap();
        let path = dir.join("entities.mmap");
        create_entity_production_stack(sample_entities(), &sample_edges(), &path).unwrap();

        let portable = open_entity_production_stack_portable(&path).unwrap();
        let got = GetById::<Entity>::get(&portable, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.label, "Ada Lovelace");
        let mut neighbors =
            Neighbors::<Entity, RelatesTo>::neighbors(&portable, Uuid::from_u128(1));
        neighbors.sort();
        assert_eq!(neighbors, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
    }
}
