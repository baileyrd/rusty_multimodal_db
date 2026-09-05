//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<EntityProductionStack>`]
//! for `Entity` v2 — `ENT2-FR-006`/`007`, ADR-0039, `server`-gated
//! alone, matching `Reminder`'s own front-door precedent.
//!
//! # Two relation labels, not one
//!
//! `neighbors` (unfiltered) answers the union of both `relates_to`/
//! `mentioned_with`; `neighbors_by_relation`/`list_relation_kinds` are
//! real for the first time in this crate — see
//! [`crate::generic::store::MultiSymmetric`]'s own doc comment for the
//! mechanism.
//!
//! # `kind`, not a plain number, is the equality-filterable field
//!
//! `filter_eq` on `kind` accepts any string now (open-ended, `ENT2-FR-001`)
//! — no discriminant validation, unlike v1's fixed-enum `kind_from_u32`
//! check. `kind` is **not** durably updatable over the wire (unlike v1) —
//! see `crate::generic::entity`'s own module doc for why (`ScannableField::
//! ScanValue: Copy`, and `String` is neither `Copy` nor mmap-fixed-width).
//! `mention_count` fills that role instead, kept unchanged from v1.
//!
//! # `label` is equality-filterable too — through a different index
//!
//! `ENT3-FR-005`/`006` (ADR-0040): `filter_eq` on `label` resolves the
//! query against the stack's `NameIndex` layer, not `GenericMmapStore`'s
//! own `IndexedField` slot (`kind` holds that). It matches `label` *or
//! any alias*, case- and whitespace-insensitively — normalization is the
//! store's, so the raw wire string is passed straight through. Zero, one,
//! or many ids; a miss is `Ok(vec![])`, never an error. Reuses
//! `Request::FilterEq`/`ScanValue::Str`/`Response::RecordList` exactly as
//! they are — **no `PROTOCOL_VERSION` change**; the only thing a client
//! sees differently is `DomainSchema` now reporting `filter_eq: true` for
//! `label`, a data value, not a shape.
//!
//! # `aliases` is readable — protocol 11 — and nothing else
//!
//! `ENT4-FR-002` (ADR-0041): `aliases` has `FIELD_ALIASES = 3`, a
//! `FieldDescriptor` with every capability flag `false`, and rides in
//! `get`/`scan_all` as `ScanValue::StrList` — the raw stored `Vec<String>`
//! in stored order, un-normalized (the `NameIndex` keys are derived from
//! it, never the reverse). Every write/filter/scan path on it is
//! `Unsupported` — a known field that supports nothing, not `UnknownField`.
//! A connection negotiated below 11 never sees the field at all:
//! `downgrade_for_version` in `super` strips the pair from `Record`/`Rows`
//! and the descriptor from `Schema` (rule 3, `ENT4-FR-003`), leaving
//! exactly the three-field shape `FR-042` returned.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::generic::entity::{Entity, EntityProductionStack, KindField, MentionCountField};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{GetById, UpdateField};
use std::path::Path;

pub const FIELD_LABEL: FieldRef = 0;
pub const FIELD_KIND: FieldRef = 1;
pub const FIELD_MENTION_COUNT: FieldRef = 2;
/// `ENT4-FR-002` (ADR-0041, protocol 11): read-only; see module docs.
pub const FIELD_ALIASES: FieldRef = 3;

pub struct EntityConnectionStore {
    store: GenericProductionStore<EntityProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl EntityConnectionStore {
    pub fn new(store: GenericProductionStore<EntityProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<EntityProductionStack>,
        journal_path: &Path,
    ) -> Result<Self, JournalError> {
        let (journal, batches) = CommitGroup::open(journal_path)?;
        store.with_exclusive(|inner| -> Result<(), JournalError> {
            for (batch_index, batch) in batches.iter().enumerate() {
                Self::apply_batch(inner, batch).map_err(|(index, code)| JournalError::Replay {
                    batch: batch_index,
                    index,
                    code,
                })?;
            }
            inner.checkpoint_flush()?;
            journal.truncate()
        })?;
        Ok(Self {
            store,
            journal: Some(journal),
        })
    }

    /// `Entity`'s only mutable field over this protocol is
    /// `mention_count` — `kind` moved to read-only in v2 (see module
    /// docs).
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_MENTION_COUNT, ScanValue::I64(_)) => {
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_MENTION_COUNT, _) => return Err((i, ErrorCode::Malformed)),
                (FIELD_LABEL | FIELD_KIND | FIELD_ALIASES, _) => {
                    return Err((i, ErrorCode::Unsupported))
                }
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut EntityProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::I64(mention_count) = op.value {
                UpdateField::<Entity, MentionCountField>::update(inner, op.id, mention_count)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }

    /// `ISO-FR-002`/`ISO-FR-006` — see `DogConnectionStore::check_read_set`
    /// for the full contract; identical shape here.
    fn check_read_set(
        reads: &[(RecordId, FieldRef, ScanValue)],
        get: impl Fn(RecordId) -> Option<Entity>,
    ) -> Result<(), (usize, ErrorCode)> {
        for (id, field, value) in reads {
            let current = get(*id).and_then(|entity| match *field {
                FIELD_LABEL => Some(ScanValue::Str(entity.label)),
                FIELD_KIND => Some(ScanValue::Str(entity.kind)),
                FIELD_MENTION_COUNT => Some(ScanValue::I64(entity.mention_count)),
                FIELD_ALIASES => Some(ScanValue::StrList(entity.aliases)),
                _ => None,
            });
            if current.as_ref() != Some(value) {
                return Err((0, ErrorCode::Conflict));
            }
        }
        Ok(())
    }
}

impl ConnectionStore for EntityConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Entity>(id).map(|entity| {
            vec![
                (FIELD_LABEL, ScanValue::Str(entity.label)),
                (FIELD_KIND, ScanValue::Str(entity.kind)),
                (FIELD_MENTION_COUNT, ScanValue::I64(entity.mention_count)),
                // `ENT4-FR-002`: raw, stored order, un-normalized.
                (FIELD_ALIASES, ScanValue::StrList(entity.aliases)),
            ]
        })
    }

    /// `SQL-FR-004`/`SQL-FR-005` (ADR-0034): every id from `all_ids`,
    /// each mapped through this adapter's own `get`.
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
        self.store
            .all_ids::<Entity>()
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect()
    }

    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        match (field, value) {
            (FIELD_KIND, ScanValue::Str(kind)) => {
                Ok(self.store.filter_eq::<Entity, KindField>(kind))
            }
            (FIELD_KIND, _) => Err(ErrorCode::Malformed),
            // `ENT3-FR-005`: `label` or any alias, normalized by the store.
            (FIELD_LABEL, ScanValue::Str(name)) => Ok(self.store.find_by_name::<Entity>(name)),
            (FIELD_LABEL, _) => Err(ErrorCode::Malformed),
            (FIELD_MENTION_COUNT | FIELD_ALIASES, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_MENTION_COUNT => Ok(self
                .store
                .scan::<Entity, MentionCountField>()
                .into_iter()
                .map(ScanValue::I64)
                .collect()),
            FIELD_LABEL | FIELD_KIND | FIELD_ALIASES => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn update_field(
        &self,
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    ) -> Result<bool, ErrorCode> {
        match (field, value) {
            (FIELD_MENTION_COUNT, ScanValue::I64(mention_count)) => {
                match self
                    .store
                    .update::<Entity, MentionCountField>(id, mention_count)
                {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_MENTION_COUNT, _) => Err(ErrorCode::Malformed),
            (FIELD_LABEL | FIELD_KIND | FIELD_ALIASES, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    /// `ENT2-FR-006`: `Entity` has no `ChildOf` relation.
    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self.store.all_neighbors::<Entity>(id))
    }

    fn neighbors_by_relation(
        &self,
        id: RecordId,
        relation: &str,
    ) -> Result<Vec<RecordId>, ErrorCode> {
        match self.store.neighbors_by_relation::<Entity>(relation, id) {
            Some(records) => Ok(records),
            None => Err(ErrorCode::Malformed),
        }
    }

    fn list_relation_kinds(&self) -> Vec<String> {
        self.store.relation_kinds::<Entity>()
    }

    /// `STV-FR-002`: `validate_batch` on this one operation, with the
    /// same per-call existence read the journaled path uses.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            self.store.get::<Entity>(id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn describe(&self) -> DomainSchema {
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_LABEL,
                    name: "label".into(),
                    value_kind: ValueKind::Str,
                    // `ENT3-FR-006`: `filter_eq` real since ADR-0040 (via
                    // `NameIndex`, matching aliases too); still read-only.
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_KIND,
                    name: "kind".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_MENTION_COUNT,
                    name: "mention_count".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
                // `ENT4-FR-002` (ADR-0041): read-only — stripped from this
                // schema by `downgrade_for_version` for a connection
                // negotiated below 11.
                FieldDescriptor {
                    tag: FIELD_ALIASES,
                    name: "aliases".into(),
                    value_kind: ValueKind::StrList,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
            ],
            // `Dog`'s own shape: neighbors only, no parent/children.
            relations: RelationCapabilities {
                parent_children: false,
                neighbors: true,
            },
        }
    }

    fn apply_transaction(
        &self,
        updates: &[TransactionOp],
        read_set: &[(RecordId, FieldRef, ScanValue)],
    ) -> Result<(), (usize, ErrorCode)> {
        // See `DogConnectionStore::apply_transaction` for the two paths
        // (`GRP-FR-001`–`005`) and where the read-set check runs in each;
        // identical here.
        match &self.journal {
            None => self.store.with_exclusive(|inner| {
                Self::validate_batch(updates, |id| GetById::<Entity>::get(inner, id).is_some())?;
                Self::check_read_set(read_set, |id| GetById::<Entity>::get(inner, id))?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Entity>(id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
                            Self::check_read_set(read_set, |id| GetById::<Entity>::get(inner, id))?;
                            Self::apply_batch(inner, updates)?;
                            Ok(turn.checkpoint_due && inner.checkpoint_flush().is_ok())
                        })
                    })
                    .map_err(|e| match e {
                        CommitError::Journal(_) => (0, ErrorCode::Journal),
                        CommitError::Apply(e) => e,
                    })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::entity::create_entity_production_stack;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> EntityConnectionStore {
        let dir = fresh_temp_dir("server_entity_v2_adapter").unwrap();
        let path = dir.join("entities.mmap");
        let entities = vec![
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
        ];
        let relates_to = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
        let mentioned_with = vec![(Uuid::from_u128(1), Uuid::from_u128(3))];
        let stack =
            create_entity_production_stack(entities, &relates_to, &mentioned_with, &path).unwrap();
        EntityConnectionStore::new(GenericProductionStore::new(stack))
    }

    #[test]
    fn get_returns_every_field() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap(),
            vec![
                (FIELD_LABEL, ScanValue::Str("Ada Lovelace".into())),
                (FIELD_KIND, ScanValue::Str("person".into())),
                (FIELD_MENTION_COUNT, ScanValue::I64(3)),
                // `ENT4-FR-002`: raw, stored order, un-normalized.
                (
                    FIELD_ALIASES,
                    ScanValue::StrList(vec!["Ada".into(), "Countess of Lovelace".into()])
                ),
            ]
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(2)).unwrap()[3],
            (FIELD_ALIASES, ScanValue::StrList(vec![])),
            "an empty list is still a present field"
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_eq_by_kind_open_ended_and_unsupported_fields() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_KIND, &ScanValue::Str("person".into())),
            Ok(vec![Uuid::from_u128(1)])
        );
        // Open-ended: any string is accepted, no discriminant to fail.
        assert!(adapter
            .filter_eq(FIELD_KIND, &ScanValue::Str("nonexistent-kind".into()))
            .unwrap()
            .is_empty());
        assert_eq!(
            adapter.filter_eq(FIELD_MENTION_COUNT, &ScanValue::I64(0)),
            Err(ErrorCode::Unsupported)
        );
    }

    /// `ENT3-FR-005`: `label` or any alias, case/whitespace-insensitive,
    /// through the raw wire string; a miss is empty; a non-`Str` is
    /// `Malformed` (the same shape `kind`'s own non-`Str` case has).
    #[test]
    fn filter_eq_by_label_matches_label_and_aliases_normalized() {
        let adapter = sample_adapter();
        let ada = Ok(vec![Uuid::from_u128(1)]);
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("Ada Lovelace".into())),
            ada
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("  ada LOVELACE ".into())),
            ada
        );
        // `ENT5-FR-001`: an internal whitespace run is the same name.
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("ada   lovelace".into())),
            ada
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("countess of lovelace".into())),
            ada
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("LONDINIUM".into())),
            Ok(vec![Uuid::from_u128(3)])
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("nobody".into())),
            Ok(vec![])
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::I64(1)),
            Err(ErrorCode::Malformed)
        );
    }

    #[test]
    fn scan_and_update_mention_count_only_kind_is_read_only() {
        let adapter = sample_adapter();
        let mut counts = adapter.scan_field(FIELD_MENTION_COUNT).unwrap();
        counts.sort_by_key(|v| match v {
            ScanValue::I64(n) => *n,
            _ => 0,
        });
        assert_eq!(
            counts,
            vec![ScanValue::I64(1), ScanValue::I64(3), ScanValue::I64(5)]
        );
        assert_eq!(adapter.scan_field(FIELD_KIND), Err(ErrorCode::Unsupported));

        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_MENTION_COUNT, ScanValue::I64(4)),
            Ok(true)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_KIND, ScanValue::Str("x".into())),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn neighbors_by_relation_and_unfiltered_and_list_relation_kinds() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.neighbors_by_relation(Uuid::from_u128(1), "relates_to"),
            Ok(vec![Uuid::from_u128(2)])
        );
        assert_eq!(
            adapter.neighbors_by_relation(Uuid::from_u128(1), "mentioned_with"),
            Ok(vec![Uuid::from_u128(3)])
        );
        assert_eq!(
            adapter.neighbors_by_relation(Uuid::from_u128(1), "unknown"),
            Err(ErrorCode::Malformed)
        );
        let mut unfiltered = adapter.neighbors(Uuid::from_u128(1)).unwrap();
        unfiltered.sort();
        assert_eq!(unfiltered, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        let mut kinds = adapter.list_relation_kinds();
        kinds.sort();
        assert_eq!(
            kinds,
            vec!["mentioned_with".to_string(), "relates_to".to_string()]
        );
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.children(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
    }

    /// `ENT4-FR-002`: `aliases` is a *known* field that supports nothing —
    /// every write/filter/scan path is `Unsupported`, never `UnknownField`
    /// (which tag 3 was before this round) and never `Malformed`.
    #[test]
    fn aliases_is_a_known_field_every_operation_refuses_as_unsupported() {
        let adapter = sample_adapter();
        let list = ScanValue::StrList(vec!["x".into()]);
        assert_eq!(
            adapter.filter_eq(FIELD_ALIASES, &list),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(FIELD_ALIASES, &ScanValue::Str("Ada".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.scan_field(FIELD_ALIASES),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_ALIASES, list.clone()),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.validate_op(&TransactionOp {
                id: Uuid::from_u128(1),
                field: FIELD_ALIASES,
                value: list,
            }),
            Err(ErrorCode::Unsupported)
        );
        // Tag 4 is still unknown — the boundary moved by exactly one.
        assert_eq!(adapter.scan_field(4), Err(ErrorCode::UnknownField));
        // A read-set entry naming `aliases` compares against the raw list.
        assert!(EntityConnectionStore::check_read_set(
            &[(
                Uuid::from_u128(3),
                FIELD_ALIASES,
                ScanValue::StrList(vec!["Londinium".into()])
            )],
            |id| adapter.store.get::<Entity>(id),
        )
        .is_ok());
    }

    #[test]
    fn describe_names_all_four_fields_and_reports_neighbors_only() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        // Four wire fields since protocol 11 — `aliases` gained
        // `FIELD_ALIASES` in `ENT4-FR-002` with every flag `false`.
        assert_eq!(schema.fields.len(), 4);
        let aliases = schema.fields.iter().find(|f| f.name == "aliases").unwrap();
        assert_eq!(aliases.tag, FIELD_ALIASES);
        assert_eq!(aliases.value_kind, ValueKind::StrList);
        assert!(
            !aliases.capabilities.filter_eq
                && !aliases.capabilities.scan
                && !aliases.capabilities.update
        );
        let label = schema.fields.iter().find(|f| f.name == "label").unwrap();
        assert!(
            label.capabilities.filter_eq && !label.capabilities.scan && !label.capabilities.update
        );
        let kind = schema.fields.iter().find(|f| f.name == "kind").unwrap();
        assert!(
            kind.capabilities.filter_eq && !kind.capabilities.scan && !kind.capabilities.update
        );
        let mention_count = schema
            .fields
            .iter()
            .find(|f| f.name == "mention_count")
            .unwrap();
        assert!(
            mention_count.capabilities.scan
                && mention_count.capabilities.update
                && !mention_count.capabilities.filter_eq
        );
        assert!(schema.relations.neighbors);
        assert!(!schema.relations.parent_children);
    }
}
