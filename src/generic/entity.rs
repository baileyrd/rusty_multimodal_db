//! `Entity` v2 — `ENT2-FR-001`–`003`, ADR-0039, `docs/design/
//! SERVER-ENTITY-V2-REDESIGN-DESIGN.md`; plus `aliases` and normalized
//! name lookup — `ENT3-FR-001`–`004`, ADR-0040, `docs/design/
//! SERVER-ENTITY-ALIASES-DESIGN.md`. Revises `ADR-0037`'s own `Entity`
//! in place (no real deployed instance exists, so each change is a
//! straight revision, not a migration) to match `rusty_remind_me`'s real
//! shape found by Unit 41's verification: `name` (a plain field — present
//! in every `GetById`/`Query` result, the closest a `Uuid`-keyed domain
//! can get to real name-based identity without breaking `RecordId`'s
//! crate-wide invariant), `kind` as an open `String` (not a fixed enum)
//! rather than `EntityKind`, and (ADR-0040) real `aliases`.
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
//! # `label` and `aliases` resolve through a *second* index, not the first
//!
//! `kind` occupies the one `IndexedField` slot `GenericMmapStore`
//! structurally admits, so `label` could never be a second one. ADR-0040
//! adds `NameIndex` (see that struct's own doc comment, linked from
//! `EntityProductionStack`'s below) — a separate, runtime-keyed secondary
//! index rebuilt from each record's own `NameIndexed::index_keys` at every
//! `create`/`open`/`open_portable`, normalized (trim + lowercase) at
//! build and query alike. `label` plus every alias all resolve to the
//! entity's id; `aliases` itself has **no wire representation** this
//! round (`ScanValue` has no list variant — a genuinely new "durable but
//! not wire-representable" category, not `label`'s prior "every
//! capability flag `false` but still returned" shape).
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
//! a claimed real `rusty_remind_me` relation name — and, since ADR-0042
//! read that repository's source directly (`docs/design/
//! SERVER-ENTITY-SOURCE-VERIFICATION-DESIGN.md`, Finding F3), a real
//! one could not exist: its `entity_relations.relation` is free-form
//! text per triple, with no vocabulary at all. The fixed, compile-time
//! label set `RELATION_LABELS` models is therefore a known divergence,
//! named for a future design round, not a placeholder awaiting a value.
//!
//! # Identity: `entity_id`, a convention borrowed from the source
//!
//! `rusty_remind_me` keys entities by `sha256(normalized name)` so two
//! machines recording the same name converge on one row (F2/F10 there).
//! `entity_id` (below) offers the same convention here — a `Uuid` from the
//! first 16 bytes of that digest — so `GetById(entity_id(name))` resolves
//! a canonical name in one indexed round trip instead of `FilterEq` then
//! `GetById`. A convention, not a store-enforced constraint: any `Uuid`
//! remains a valid id, and every pre-existing test's hand-picked ids
//! keep working. `ENT5-FR-002`.

use super::mmap_store::GenericMmapStore;
use super::query::NameIndexed;
use super::store::{normalize, MultiSymmetric, NameIndex};
use super::traits::{IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation};
use crate::durability::DurabilityError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

/// The two relation labels this domain's `MultiSymmetric` layer knows —
/// used by every `create`/`open`/`open_portable` call site so the label
/// set is written once, not re-typed at each one.
pub const RELATION_LABELS: [&str; 2] = ["relates_to", "mentioned_with"];

/// The deterministic id for an entity name — `ENT5-FR-002` (ADR-0042).
///
/// A `Uuid` over the first 16 bytes of `sha256(normalize(name))`, where
/// `normalize` is exactly the rule [`NameIndex`] keys on (collapse
/// whitespace, lowercase). So `"Ada Lovelace"`, `"  ada   LOVELACE "`,
/// and `"ada\tlovelace"` all derive the same id, and an entity minted
/// with `id: entity_id(&label)` is found by `GetById(entity_id(query))`
/// for any spelling of its canonical name — one indexed round trip, no
/// `FilterEq`. Aliases are *not* part of the derivation (they resolve
/// through the index, not the id), matching `rusty_remind_me`.
///
/// Full 128 bits of the digest, deliberately not the source's 12-hex
/// (48-bit) truncation — see the design doc's own Non-goals: byte
/// interop with its ids is impossible regardless (`Uuid` vs. a hex
/// `String`), and the narrower domain is one its own author calls
/// "inherited from the reference rather than chosen." A convention for
/// callers, not a constraint the store checks.
pub fn entity_id(name: &str) -> Uuid {
    let digest = Sha256::digest(normalize(name).as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    /// `ENT2-FR-001`: the closest a `Uuid`-keyed domain gets to
    /// `rusty_remind_me`'s real `name`-based identity. Since ADR-0040
    /// (`ENT3-FR-005`) it is equality-filterable over the wire — case-
    /// and whitespace-insensitively, via [`NameIndex`], not via
    /// `GenericMmapStore`'s one `IndexedField` slot, which `kind` holds.
    /// Still not scannable or updatable.
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
    /// `ENT3-FR-001` (ADR-0040): alternate names this entity also
    /// resolves under, each normalized into the same [`NameIndex`] as
    /// `label`. Durable for free — the record blob is `Vec<Entity>`
    /// serialized whole. No `FieldRef`, no `GetById`/`Query` exposure
    /// this round: `ScanValue` has no list variant (see module docs).
    pub aliases: Vec<String>,
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

/// `ENT3-FR-004`: `label` first, then every alias, all un-normalized —
/// [`NameIndex`] owns normalization (`ENT3-FR-003`), so this stays a
/// plain enumeration of the record's own fields.
impl NameIndexed for Entity {
    fn index_keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(1 + self.aliases.len());
        keys.push(self.label.clone());
        keys.extend(self.aliases.iter().cloned());
        keys
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

/// The durable production stack, innermost first: [`GenericMmapStore`]
/// (owns records, the `KindField` index, and `MentionCountField`) ->
/// [`MultiSymmetric`] (both `relates_to`/`mentioned_with` adjacency,
/// each independently durable at `<path>.<label>.edges`) ->
/// [`NameIndex`] (the normalized `label`/`aliases` map, rebuilt from the
/// records beneath it, no file of its own — ADR-0040). No `Reversed`
/// layer — `Entity` has no `ChildOf` relation.
pub type EntityProductionStack = NameIndex<
    MultiSymmetric<GenericMmapStore<Entity, KindField, MentionCountField>, Entity>,
    Entity,
>;

/// Build a fresh, durable production store for `Entity` at `path` — the
/// generic analogue of `create_reminder_production_stack`. Writes four
/// files: `path` (the mmap file), `<path>.records` (the record blob),
/// `<path>.relates_to.edges`, `<path>.mentioned_with.edges`. The
/// `NameIndex` layer writes nothing — it is derived from `<path>.records`.
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
    Ok(NameIndex::new(MultiSymmetric::create(
        core, &relations, path,
    )?))
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
    Ok(NameIndex::new(MultiSymmetric::open(
        core, &relations, path,
    )?))
}

/// Reopen the whole stack from its files alone — `path`,
/// `<path>.records`, both `<path>.<label>.edges` files — no `entities`/
/// edge-list arguments. `RELATION_LABELS` is passed explicitly to
/// `MultiSymmetric::open_portable` since that primitive, unlike
/// `Symmetric::open_portable`, cannot self-discover an open-ended label
/// set from the path alone (see `MultiSymmetric::open_portable`'s own
/// doc comment) — `Entity`'s own two labels are fixed and known here at
/// compile time, so this is a real but narrow limitation, not one that
/// blocks portability in practice. The `NameIndex` layer needs nothing
/// from the path at all — it is rebuilt from the records just read.
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
    Ok(NameIndex::new(MultiSymmetric::open_portable(
        core,
        path,
        &RELATION_LABELS,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::query::{FilterEq, FindByName, GetById, MultiNeighbors, UpdateField};
    use crate::test_support::fresh_temp_dir;

    fn sample_entities() -> Vec<Entity> {
        vec![
            Entity {
                id: Uuid::from_u128(1),
                label: "Ada Lovelace".into(),
                kind: "person".into(),
                mention_count: 3,
                aliases: vec!["Ada".into(), "Countess of Lovelace".into()],
            },
            Entity {
                id: Uuid::from_u128(2),
                label: "Analytical Engine".into(),
                kind: "concept".into(),
                mention_count: 5,
                aliases: vec![],
            },
            Entity {
                id: Uuid::from_u128(3),
                label: "London".into(),
                kind: "place".into(),
                mention_count: 1,
                aliases: vec!["Londinium".into()],
            },
        ]
    }

    fn sample_relates_to() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }

    fn sample_mentioned_with() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(3))]
    }

    /// `ENT5-FR-002`: every spelling of one canonical name derives one
    /// id; a different name derives a different one; the derivation is
    /// stable across calls (no randomness, no timestamp).
    #[test]
    fn entity_id_is_deterministic_over_the_normalized_name() {
        let canonical = entity_id("Bailey Robertson");
        assert_eq!(entity_id("  Bailey   Robertson  "), canonical);
        assert_eq!(entity_id("bailey robertson"), canonical);
        assert_eq!(entity_id("BAILEY\tROBERTSON\n"), canonical);
        assert_eq!(entity_id("Bailey Robertson"), canonical, "stable");
        assert_ne!(entity_id("Bailey Robertson II"), canonical);
        assert_ne!(entity_id("BaileyRobertson"), canonical);
        assert_ne!(entity_id(""), canonical);
        // Never the nil UUID for a real name — the digest is never zero.
        assert_ne!(canonical, Uuid::nil());
    }

    /// `ENT5-FR-002` as a caller would use it: mint with `entity_id`,
    /// fetch by re-deriving from a differently-spelled query — one
    /// `GetById`, no `FilterEq`. Hand-picked ids in the other fixtures
    /// keep working alongside it (a convention, not a constraint).
    #[test]
    fn get_by_id_resolves_a_derived_id_from_any_spelling() {
        let dir = fresh_temp_dir("generic_entity_v5_derived_id").unwrap();
        let path = dir.join("entities.mmap");
        let mut entities = sample_entities();
        entities.push(Entity {
            id: entity_id("Grace Hopper"),
            label: "Grace Hopper".into(),
            kind: "person".into(),
            mention_count: 0,
            aliases: vec!["Amazing Grace".into()],
        });
        let store = create_entity_production_stack(entities, &[], &[], &path).unwrap();

        let got = GetById::<Entity>::get(&store, entity_id("  grace   HOPPER ")).unwrap();
        assert_eq!(got.label, "Grace Hopper");
        // The alias resolves through the index, not the id.
        assert!(GetById::<Entity>::get(&store, entity_id("Amazing Grace")).is_none());
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "amazing   grace"),
            vec![entity_id("Grace Hopper")]
        );
        // Hand-picked ids from the shared fixture are untouched.
        assert!(GetById::<Entity>::get(&store, Uuid::from_u128(1)).is_some());
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
        assert_eq!(got.aliases, vec!["Ada", "Countess of Lovelace"]);

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

    /// `ENT3-FR-003`/`004`/`005`: `label` and every alias resolve, case-
    /// and whitespace-insensitively; a miss is empty, not an error.
    #[test]
    fn find_by_name_resolves_label_and_aliases_normalized() {
        let dir = fresh_temp_dir("generic_entity_v3_names").unwrap();
        let path = dir.join("entities.mmap");
        let store = create_entity_production_stack(
            sample_entities(),
            &sample_relates_to(),
            &sample_mentioned_with(),
            &path,
        )
        .unwrap();

        let ada = vec![Uuid::from_u128(1)];
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "Ada Lovelace"),
            ada
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "ada lovelace"),
            ada
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "  ADA LOVELACE\t"),
            ada
        );
        // `ENT5-FR-001`: internal whitespace runs collapse too.
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "Ada   Lovelace"),
            ada
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "Ada\tLovelace"),
            ada
        );
        assert_eq!(FindByName::<Entity>::find_by_name(&store, "Ada"), ada);
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "countess of lovelace"),
            ada
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&store, "LONDINIUM"),
            vec![Uuid::from_u128(3)]
        );
        assert!(FindByName::<Entity>::find_by_name(&store, "Babbage").is_empty());
        // No substring/prefix matching — exact after normalization only.
        assert!(FindByName::<Entity>::find_by_name(&store, "Ada Love").is_empty());
    }

    /// Two entities sharing a normalized key both come back — collision
    /// handling is the caller's (design doc Non-goals).
    #[test]
    fn find_by_name_returns_every_entity_sharing_a_key() {
        let dir = fresh_temp_dir("generic_entity_v3_collision").unwrap();
        let path = dir.join("entities.mmap");
        let mut entities = sample_entities();
        entities.push(Entity {
            id: Uuid::from_u128(4),
            label: "Ada".into(),
            kind: "person".into(),
            mention_count: 0,
            aliases: vec!["ada".into(), " ADA ".into()],
        });
        let store = create_entity_production_stack(entities, &[], &[], &path).unwrap();
        let mut hits = FindByName::<Entity>::find_by_name(&store, "Ada");
        hits.sort();
        // Entity 4 registers "Ada" three times; it appears once.
        assert_eq!(hits, vec![Uuid::from_u128(1), Uuid::from_u128(4)]);
    }

    #[test]
    fn update_mention_count_is_durable_across_reopen_and_names_survive() {
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
            // An update through the outer layer never touches the name map.
            assert_eq!(
                FindByName::<Entity>::find_by_name(&store, "ada"),
                vec![Uuid::from_u128(1)]
            );
        }
        let reopened =
            open_entity_production_stack(entities, &relates_to, &mentioned_with, &path).unwrap();
        let got = GetById::<Entity>::get(&reopened, Uuid::from_u128(1)).unwrap();
        assert_eq!(got.mention_count, 4);
        assert_eq!(got.aliases, vec!["Ada", "Countess of Lovelace"]);
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &reopened,
                "relates_to",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(2)])
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&reopened, "Countess Of Lovelace"),
            vec![Uuid::from_u128(1)]
        );
    }

    /// `ENT3-FR-002`: the name map needs no file of its own — it comes
    /// back from `<path>.records` alone.
    #[test]
    fn portable_reopen_matches_open_neighbors_fields_and_names() {
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
        assert_eq!(got.aliases, vec!["Ada", "Countess of Lovelace"]);
        assert_eq!(
            MultiNeighbors::<Entity>::neighbors_by_relation(
                &portable,
                "mentioned_with",
                Uuid::from_u128(1)
            ),
            Some(vec![Uuid::from_u128(3)])
        );
        assert_eq!(
            FindByName::<Entity>::find_by_name(&portable, "londinium"),
            vec![Uuid::from_u128(3)]
        );
        // Exactly the four files the docs name — no fifth for the index.
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "entities.mmap",
                "entities.mmap.mentioned_with.edges",
                "entities.mmap.records",
                "entities.mmap.relates_to.edges",
            ]
        );
    }
}
