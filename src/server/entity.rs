//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<EntityProductionStack>`]
//! for `Entity` — this crate's fifth domain, and its second
//! `ConnectionStore` adapter gated by `server` alone (`ENT-FR-006`,
//! ADR-0037), matching `Reminder`'s own front-door precedent.
//!
//! # One relation only: `neighbors`, not `parent`/`children`
//!
//! `Entity` has a `SymmetricRelation` (`relates_to`) but no `ChildOf`
//! — the same shape `server::dog`'s own missing half already uses.
//! `parent`/`children` report `ErrorCode::Unsupported` unconditionally.
//!
//! # `kind`, not a plain number, is the equality-filterable field
//!
//! `filter_eq`/`update_field` validate an incoming `kind` discriminant
//! against [`crate::generic::entity::kind_from_u32`] before ever
//! reaching the store — `ErrorCode::Malformed` on an unrecognized
//! value — the same "validate before write" shape `server::reminder`'s
//! own `status` handling already established, here applied to
//! `FilterEq` since `kind` is the `IndexedField`, not the
//! `ScannableField`.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::generic::entity::{
    kind_from_u32, kind_to_u32, Entity, EntityProductionStack, KindField, MentionCountField,
    RelatesTo,
};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{GetById, UpdateField};
use std::path::Path;

pub const FIELD_LABEL: FieldRef = 0;
pub const FIELD_KIND: FieldRef = 1;
pub const FIELD_MENTION_COUNT: FieldRef = 2;

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

    /// Same validate-then-apply shape `server::reminder`'s own uses —
    /// `Entity`'s only mutable field over this protocol is
    /// `mention_count`.
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
                (FIELD_LABEL | FIELD_KIND, _) => return Err((i, ErrorCode::Unsupported)),
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
                FIELD_KIND => Some(ScanValue::U32(kind_to_u32(entity.kind))),
                FIELD_MENTION_COUNT => Some(ScanValue::I64(entity.mention_count)),
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
                (FIELD_KIND, ScanValue::U32(kind_to_u32(entity.kind))),
                (FIELD_MENTION_COUNT, ScanValue::I64(entity.mention_count)),
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
            (FIELD_KIND, ScanValue::U32(raw)) => match kind_from_u32(*raw) {
                Some(kind) => Ok(self.store.filter_eq::<Entity, KindField>(&kind)),
                None => Err(ErrorCode::Malformed),
            },
            (FIELD_KIND, _) => Err(ErrorCode::Malformed),
            (FIELD_LABEL | FIELD_MENTION_COUNT, _) => Err(ErrorCode::Unsupported),
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
            FIELD_LABEL | FIELD_KIND => Err(ErrorCode::Unsupported),
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
            (FIELD_LABEL | FIELD_KIND, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    /// `ENT-FR-005`: `Entity` has no `ChildOf` relation.
    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self.store.neighbors::<Entity, RelatesTo>(id))
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
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_KIND,
                    name: "kind".into(),
                    value_kind: ValueKind::U32,
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
    use crate::generic::entity::{create_entity_production_stack, EntityKind};
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> EntityConnectionStore {
        let dir = fresh_temp_dir("server_entity_adapter").unwrap();
        let path = dir.join("entities.mmap");
        let entities = vec![
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
        ];
        let edges = vec![
            (Uuid::from_u128(1), Uuid::from_u128(2)),
            (Uuid::from_u128(1), Uuid::from_u128(3)),
        ];
        let stack = create_entity_production_stack(entities, &edges, &path).unwrap();
        EntityConnectionStore::new(GenericProductionStore::new(stack))
    }

    #[test]
    fn get_returns_every_field() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap(),
            vec![
                (FIELD_LABEL, ScanValue::Str("Ada Lovelace".into())),
                (FIELD_KIND, ScanValue::U32(0)),
                (FIELD_MENTION_COUNT, ScanValue::I64(3)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_eq_by_kind_with_discriminant_validation_and_unsupported_fields() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_KIND, &ScanValue::U32(0)),
            Ok(vec![Uuid::from_u128(1)])
        );
        assert!(adapter
            .filter_eq(FIELD_KIND, &ScanValue::U32(4))
            .unwrap()
            .is_empty());
        assert_eq!(
            adapter.filter_eq(FIELD_KIND, &ScanValue::U32(99)),
            Err(ErrorCode::Malformed),
            "an unrecognized discriminant is rejected"
        );
        assert_eq!(
            adapter.filter_eq(FIELD_MENTION_COUNT, &ScanValue::I64(0)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("x".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(99, &ScanValue::I64(0)),
            Err(ErrorCode::UnknownField)
        );
    }

    #[test]
    fn scan_and_update_mention_count_only() {
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
            adapter.get(Uuid::from_u128(1)).unwrap()[2],
            (FIELD_MENTION_COUNT, ScanValue::I64(4))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_MENTION_COUNT, ScanValue::I64(1)),
            Ok(false)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_LABEL, ScanValue::Str("x".into())),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn neighbors_reflects_relates_to_and_parent_children_are_unsupported() {
        let adapter = sample_adapter();
        let mut neighbors = adapter.neighbors(Uuid::from_u128(1)).unwrap();
        neighbors.sort();
        assert_eq!(neighbors, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.children(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn describe_names_all_three_fields_and_reports_neighbors_only() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 3);
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
