//! The `Relation` domain's [`ConnectionStore`] adapter — `REL-FR-004`,
//! ADR-0058: the consumer's `entity_relations` table over the wire as a
//! record table. Seven fields in tag order — `subject` (indexed:
//! `FilterEq` is "every edge out of this subject"), `relation`, `object`,
//! `created_at_unix_ms`, `updated_at_unix_ms` (scannable and updatable:
//! `Page` orders by it, last-writer-wins guards on it), `node_id` and
//! `deleted_at_unix_ms` (`ADR-0056`'s sentinels). `Insert`/`Replace`/
//! `ReplaceIf`/`Delete`/`Compact` as every front-door domain; no relation
//! layer of its own, so every edge request is `Unsupported`. Served by
//! `memory_server` as its third table, `relation`.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, Predicate,
    RecordId, RelationCapabilities, ScanValue, TransactionOp, ValueKind, WriteOp, WriteResult,
};
use super::{
    page_by_scan, page_key, predicate_matches, validate_predicate, ConnectionStore, DeleteOutcome,
    InsertOutcome, PageRow, ReplaceIfOutcome, ReplaceOutcome,
};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{Delete, GetById, Insert, Replace, UpdateField};
use crate::generic::relation::{Relation, RelationProductionStack, SubjectField, UpdatedAtField};
use crate::generic::{DeleteError, GuardedReplace, InsertError, ReplaceError};
use std::path::Path;

pub const FIELD_SUBJECT: FieldRef = 0;
pub const FIELD_RELATION: FieldRef = 1;
pub const FIELD_OBJECT: FieldRef = 2;
pub const FIELD_CREATED_AT: FieldRef = 3;
pub const FIELD_UPDATED_AT: FieldRef = 4;
/// `ADR-0056`'s sentinel: `""` is unattributed.
pub const FIELD_NODE_ID: FieldRef = 5;
/// `ADR-0056`'s sentinel: `0` is live; validated non-negative.
pub const FIELD_DELETED_AT: FieldRef = 6;

/// Every field but the scannable `updated_at_unix_ms`: read-only over
/// `UpdateField`, changed by whole-record `Replace`.
const READ_ONLY_FIELDS: [FieldRef; 6] = [
    FIELD_SUBJECT,
    FIELD_RELATION,
    FIELD_OBJECT,
    FIELD_CREATED_AT,
    FIELD_NODE_ID,
    FIELD_DELETED_AT,
];

pub struct RelationConnectionStore {
    store: GenericProductionStore<RelationProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl RelationConnectionStore {
    pub fn new(store: GenericProductionStore<RelationProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<RelationProductionStack>,
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

    /// The validate-then-apply shape every adapter uses — the one
    /// updatable field over this protocol is `updated_at_unix_ms`.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_UPDATED_AT, ScanValue::I64(_)) => {
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_UPDATED_AT, _) => return Err((i, ErrorCode::Malformed)),
                (field, _) if READ_ONLY_FIELDS.contains(&field) => {
                    return Err((i, ErrorCode::Unsupported))
                }
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut RelationProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::I64(stamp) = op.value {
                UpdateField::<Relation, UpdatedAtField>::update(inner, op.id, stamp)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }

    /// `INS-FR-006` (ADR-0046): the whole field list against this
    /// domain's schema, before any write — all seven tags exactly once
    /// with a value of its kind; `subject`, `relation`, and `object`
    /// non-empty; `deleted_at_unix_ms` non-negative. `Malformed` for a
    /// missing, repeated, wrong-kind, or out-of-rule field;
    /// `UnknownField` for a tag this domain doesn't have.
    fn relation_from_fields(
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<Relation, ErrorCode> {
        let mut subject = None;
        let mut relation = None;
        let mut object = None;
        let mut created_at = None;
        let mut updated_at = None;
        let mut node_id = None;
        let mut deleted_at = None;
        for (tag, value) in fields {
            match (tag, value) {
                (FIELD_SUBJECT, ScanValue::Str(v)) if subject.is_none() && !v.is_empty() => {
                    subject = Some(v)
                }
                (FIELD_RELATION, ScanValue::Str(v)) if relation.is_none() && !v.is_empty() => {
                    relation = Some(v)
                }
                (FIELD_OBJECT, ScanValue::Str(v)) if object.is_none() && !v.is_empty() => {
                    object = Some(v)
                }
                (FIELD_CREATED_AT, ScanValue::I64(v)) if created_at.is_none() => {
                    created_at = Some(v)
                }
                (FIELD_UPDATED_AT, ScanValue::I64(v)) if updated_at.is_none() => {
                    updated_at = Some(v)
                }
                (FIELD_NODE_ID, ScanValue::Str(v)) if node_id.is_none() => node_id = Some(v),
                (FIELD_DELETED_AT, ScanValue::I64(v)) if deleted_at.is_none() && v >= 0 => {
                    deleted_at = Some(v)
                }
                (tag, _) if tag <= FIELD_DELETED_AT => return Err(ErrorCode::Malformed),
                _ => return Err(ErrorCode::UnknownField),
            }
        }
        let (
            Some(subject),
            Some(relation),
            Some(object),
            Some(created_at_unix_ms),
            Some(updated_at_unix_ms),
            Some(node_id),
            Some(deleted_at_unix_ms),
        ) = (
            subject, relation, object, created_at, updated_at, node_id, deleted_at,
        )
        else {
            return Err(ErrorCode::Malformed);
        };
        Ok(Relation {
            id,
            subject,
            relation,
            object,
            created_at_unix_ms,
            updated_at_unix_ms,
            node_id,
            deleted_at_unix_ms,
        })
    }

    fn check_read_set(
        reads: &[(RecordId, FieldRef, ScanValue)],
        get: impl Fn(RecordId) -> Option<Relation>,
    ) -> Result<(), (usize, ErrorCode)> {
        for (id, field, value) in reads {
            let current = get(*id).and_then(|relation| {
                Self::fields_of(relation)
                    .into_iter()
                    .find(|(tag, _)| tag == field)
                    .map(|(_, value)| value)
            });
            if current.as_ref() != Some(value) {
                return Err((0, ErrorCode::Conflict));
            }
        }
        Ok(())
    }
}

impl RelationConnectionStore {
    /// The wire shape of one relation, in tag order — `get`, the read-set
    /// check, and a `ReplaceIf` guard's evaluation all go through here.
    fn fields_of(relation: Relation) -> Vec<(FieldRef, ScanValue)> {
        vec![
            (FIELD_SUBJECT, ScanValue::Str(relation.subject)),
            (FIELD_RELATION, ScanValue::Str(relation.relation)),
            (FIELD_OBJECT, ScanValue::Str(relation.object)),
            (
                FIELD_CREATED_AT,
                ScanValue::I64(relation.created_at_unix_ms),
            ),
            (
                FIELD_UPDATED_AT,
                ScanValue::I64(relation.updated_at_unix_ms),
            ),
            (FIELD_NODE_ID, ScanValue::Str(relation.node_id)),
            (
                FIELD_DELETED_AT,
                ScanValue::I64(relation.deleted_at_unix_ms),
            ),
        ]
    }
}

/// `WBT-FR-003` (ADR-0060): a parsed, pre-validated write of an atomic
/// [`WriteOp`] batch on `Relation`. `Relation` is a record table with no
/// edge layer, so `Link` is not representable here — an atomic batch
/// carrying one aborts `Unsupported` in `prepare_write`.
enum PreparedWrite {
    Insert(Relation),
    Replace(Relation),
    ReplaceIf(Relation, Predicate),
    Delete(RecordId),
}

impl RelationConnectionStore {
    fn prepare_write(schema: &DomainSchema, op: &WriteOp) -> Result<PreparedWrite, ErrorCode> {
        Ok(match op {
            WriteOp::Insert { id, fields } => {
                PreparedWrite::Insert(Self::relation_from_fields(*id, fields.clone())?)
            }
            WriteOp::Replace { id, fields } => {
                PreparedWrite::Replace(Self::relation_from_fields(*id, fields.clone())?)
            }
            WriteOp::ReplaceIf { id, fields, guard } => {
                validate_predicate(schema, guard)?;
                PreparedWrite::ReplaceIf(
                    Self::relation_from_fields(*id, fields.clone())?,
                    guard.clone(),
                )
            }
            WriteOp::Delete { id } => PreparedWrite::Delete(*id),
            WriteOp::Link { .. } => return Err(ErrorCode::Unsupported),
        })
    }

    fn apply_prepared(
        inner: &mut RelationProductionStack,
        prepared: PreparedWrite,
    ) -> Result<WriteResult, ErrorCode> {
        Ok(match prepared {
            PreparedWrite::Insert(relation) => match Insert::insert(inner, relation) {
                Ok(()) => WriteResult::Inserted,
                Err(InsertError::Duplicate(_)) => WriteResult::Duplicate,
                Err(InsertError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::Replace(relation) => match Replace::replace(inner, relation) {
                Ok(()) => WriteResult::Replaced,
                Err(ReplaceError::NotFound(_)) => WriteResult::NotFound,
                Err(ReplaceError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::ReplaceIf(relation, guard) => {
                let id = relation.id;
                match GetById::<Relation>::get(inner, id) {
                    None => WriteResult::NotFound,
                    Some(stored) => {
                        if predicate_matches(&Self::fields_of(stored), &guard) {
                            match Replace::replace(inner, relation) {
                                Ok(()) => WriteResult::Replaced,
                                Err(ReplaceError::NotFound(_)) => WriteResult::NotFound,
                                Err(ReplaceError::Durability(_)) => return Err(ErrorCode::Storage),
                            }
                        } else {
                            WriteResult::GuardFailed
                        }
                    }
                }
            }
            PreparedWrite::Delete(id) => match Delete::<Relation>::delete(inner, id) {
                Ok(()) => WriteResult::Deleted,
                Err(DeleteError::NotFound(_)) => WriteResult::NotFound,
                Err(DeleteError::Durability(_)) => return Err(ErrorCode::Storage),
            },
        })
    }
}

impl ConnectionStore for RelationConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Relation>(id).map(Self::fields_of)
    }

    fn write_batch(
        &self,
        ops: &[WriteOp],
        atomic: bool,
    ) -> Result<Vec<WriteResult>, (usize, ErrorCode)> {
        if !atomic {
            return Ok(ops.iter().map(|op| self.apply_write_op(op)).collect());
        }
        self.write_batch_checked(ops, &|_| Ok(()))
    }

    fn write_batch_checked(
        &self,
        ops: &[WriteOp],
        check: &dyn Fn(usize) -> Result<(), ErrorCode>,
    ) -> Result<Vec<WriteResult>, (usize, ErrorCode)> {
        let schema = self.describe();
        self.store.with_exclusive(|inner| {
            let mut prepared = Vec::with_capacity(ops.len());
            for (i, op) in ops.iter().enumerate() {
                check(i).map_err(|code| (i, code))?;
                let p = Self::prepare_write(&schema, op).map_err(|code| (i, code))?;
                prepared.push(p);
            }
            let mut results = Vec::with_capacity(prepared.len());
            for (i, p) in prepared.into_iter().enumerate() {
                results.push(Self::apply_prepared(inner, p).map_err(|code| (i, code))?);
            }
            Ok(results)
        })
    }

    /// `SQL-FR-004`/`SQL-FR-005` (ADR-0034): every id from `all_ids`,
    /// each mapped through this adapter's own `get`.
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
        self.store
            .all_ids::<Relation>()
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect()
    }

    /// `ORD-FR-005` (ADR-0059): a page ordered by `updated_at_unix_ms`
    /// is a range walk of the stack's sorted index; any other orderable
    /// field takes the scan path. A cursor here is `I64` or absent
    /// (`validate_page`).
    fn page(
        &self,
        order_by: FieldRef,
        after: Option<(ScanValue, RecordId)>,
        limit: usize,
    ) -> Result<Vec<PageRow>, ErrorCode> {
        if order_by != FIELD_UPDATED_AT {
            return Ok(page_by_scan(self, order_by, after, limit));
        }
        let cursor = match after {
            None => None,
            Some((ScanValue::I64(stamp), id)) => Some((stamp, id)),
            Some(_) => return Err(ErrorCode::Malformed),
        };
        Ok(self
            .store
            .page_by::<Relation, UpdatedAtField>(cursor, limit)
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect())
    }

    /// `PAG-FR-002` (ADR-0055): the sort key of every record read
    /// straight off [`Relation`], so a page materializes only its own rows.
    /// A field this arm list does not name falls back to the wire shape
    /// — the same key [`page_key`] derives for the trait default.
    fn page_keys(&self, order_by: FieldRef) -> Vec<(RecordId, i128)> {
        self.store
            .all_ids::<Relation>()
            .into_iter()
            .filter_map(|id| {
                let record = self.store.get::<Relation>(id)?;
                let key = match order_by {
                    FIELD_CREATED_AT => Some(i128::from(record.created_at_unix_ms)),
                    FIELD_UPDATED_AT => Some(i128::from(record.updated_at_unix_ms)),
                    FIELD_DELETED_AT => Some(i128::from(record.deleted_at_unix_ms)),
                    _ => None,
                };
                let key = key.unwrap_or_else(|| page_key(&Self::fields_of(record), order_by, id).0);
                Some((id, key))
            })
            .collect()
    }

    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        match (field, value) {
            (FIELD_SUBJECT, ScanValue::Str(subject)) => {
                Ok(self.store.filter_eq::<Relation, SubjectField>(subject))
            }
            (FIELD_SUBJECT, _) => Err(ErrorCode::Malformed),
            (field, _) if field <= FIELD_DELETED_AT => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_UPDATED_AT => Ok(self
                .store
                .scan::<Relation, UpdatedAtField>()
                .into_iter()
                .map(ScanValue::I64)
                .collect()),
            field if READ_ONLY_FIELDS.contains(&field) => Err(ErrorCode::Unsupported),
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
            (FIELD_UPDATED_AT, ScanValue::I64(stamp)) => {
                match self.store.update::<Relation, UpdatedAtField>(id, stamp) {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_UPDATED_AT, _) => Err(ErrorCode::Malformed),
            (field, _) if READ_ONLY_FIELDS.contains(&field) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    /// `INS-FR-006` (ADR-0046): validate, then one write under the
    /// store's own lock. A duplicate is the normal outcome, not an
    /// error; a durability failure is `Storage` (see the trait's docs).
    fn insert_record(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<InsertOutcome, ErrorCode> {
        let relation = Self::relation_from_fields(id, fields)?;
        match self.store.insert(relation) {
            Ok(()) => Ok(InsertOutcome::Inserted),
            Err(InsertError::Duplicate(_)) => Ok(InsertOutcome::Duplicate),
            Err(InsertError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `REP-FR-005` (ADR-0049): the same validation as `insert_record`,
    /// then one whole-record write under the store's own lock. An
    /// unknown id is the normal outcome, not an error.
    fn replace_record(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<ReplaceOutcome, ErrorCode> {
        let relation = Self::relation_from_fields(id, fields)?;
        match self.store.replace(relation) {
            Ok(()) => Ok(ReplaceOutcome::Replaced),
            Err(ReplaceError::NotFound(_)) => Ok(ReplaceOutcome::NotFound),
            Err(ReplaceError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `GRD-FR-003` (ADR-0054): `replace_record`'s validation, then the
    /// read, the guard over this adapter's own wire shape of the stored
    /// record, and the write under one acquisition of the store's lock.
    fn replace_record_if(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
        guard: &Predicate,
    ) -> Result<ReplaceIfOutcome, ErrorCode> {
        let relation = Self::relation_from_fields(id, fields)?;
        let holds = |stored: &Relation| predicate_matches(&Self::fields_of(stored.clone()), guard);
        match self.store.replace_if(relation, holds) {
            Ok(GuardedReplace::Replaced) => Ok(ReplaceIfOutcome::Replaced),
            Ok(GuardedReplace::Refused) => Ok(ReplaceIfOutcome::GuardFailed),
            Err(ReplaceError::NotFound(_)) => Ok(ReplaceIfOutcome::NotFound),
            Err(ReplaceError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `DEL-FR-006` (ADR-0051): one whole-record delete under the store's
    /// own lock — the record and, within this table, every edge touching
    /// it. An unknown id is the normal outcome, not an error.
    fn delete_record(&self, id: RecordId) -> Result<DeleteOutcome, ErrorCode> {
        match self.store.delete::<Relation>(id) {
            Ok(()) => Ok(DeleteOutcome::Deleted),
            Err(DeleteError::NotFound(_)) => Ok(DeleteOutcome::NotFound),
            Err(DeleteError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `CMP-FR-006` (ADR-0052): the stack compacted under the store's own
    /// write lock; a file that could not be rewritten is `Storage`.
    fn compact(&self) -> Result<crate::generic::CompactionReport, ErrorCode> {
        self.store.compact().map_err(|_| ErrorCode::Storage)
    }

    /// `REL-FR-003`: a *table of edges* has no edge layer of its own —
    /// out-edges are `FilterEq subject`, in-edges `Query object = …`.
    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors_by_relation(
        &self,
        _id: RecordId,
        _relation: &str,
    ) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn list_relation_kinds(&self) -> Vec<String> {
        Vec::new()
    }

    /// `STV-FR-002`: `validate_batch` on this one operation, with the
    /// same per-call existence read the journaled path uses.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            self.store.get::<Relation>(id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn table_name(&self) -> &str {
        "relation"
    }

    fn describe(&self) -> DomainSchema {
        let read_only = FieldCapabilities {
            filter_eq: false,
            scan: false,
            update: false,
        };
        let field = |tag: FieldRef, name: &str, value_kind: ValueKind| FieldDescriptor {
            tag,
            name: name.into(),
            value_kind,
            capabilities: read_only,
        };
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_SUBJECT,
                    name: "subject".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                field(FIELD_RELATION, "relation", ValueKind::Str),
                field(FIELD_OBJECT, "object", ValueKind::Str),
                field(FIELD_CREATED_AT, "created_at_unix_ms", ValueKind::I64),
                FieldDescriptor {
                    tag: FIELD_UPDATED_AT,
                    name: "updated_at_unix_ms".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
                field(FIELD_NODE_ID, "node_id", ValueKind::Str),
                field(FIELD_DELETED_AT, "deleted_at_unix_ms", ValueKind::I64),
            ],
            relations: RelationCapabilities {
                parent_children: false,
                neighbors: false,
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
                Self::validate_batch(updates, |id| GetById::<Relation>::get(inner, id).is_some())?;
                Self::check_read_set(read_set, |id| GetById::<Relation>::get(inner, id))?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Relation>(id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
                            Self::check_read_set(read_set, |id| {
                                GetById::<Relation>::get(inner, id)
                            })?;
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
    use crate::generic::relation::create_relation_production_stack;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample() -> Vec<Relation> {
        let relation = |n: u128, subject: &str, label: &str, object: &str| Relation {
            id: Uuid::from_u128(n),
            subject: subject.into(),
            relation: label.into(),
            object: object.into(),
            created_at_unix_ms: 1_000 * n as i64,
            updated_at_unix_ms: 1_000 * n as i64,
            node_id: String::new(),
            deleted_at_unix_ms: 0,
        };
        vec![
            relation(1, "aaa", "works_with", "bbb"),
            relation(2, "aaa", "located_in", "ccc"),
            relation(3, "bbb", "works_with", "aaa"),
        ]
    }

    fn sample_adapter() -> RelationConnectionStore {
        let dir = fresh_temp_dir("server_relation_adapter").unwrap();
        let stack =
            create_relation_production_stack(sample(), &dir.join("relations.mmap")).unwrap();
        RelationConnectionStore::new(GenericProductionStore::new(stack))
    }

    /// `REL-FR-004`: seven fields in tag order, `subject` the one
    /// `filter_eq`, `updated_at_unix_ms` the one scan/update, no relations.
    #[test]
    fn describe_get_filter_scan_and_update_match_the_shape() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 7);
        let names: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "subject",
                "relation",
                "object",
                "created_at_unix_ms",
                "updated_at_unix_ms",
                "node_id",
                "deleted_at_unix_ms"
            ]
        );
        assert!(schema.fields[0].capabilities.filter_eq);
        assert!(schema.fields[4].capabilities.scan && schema.fields[4].capabilities.update);
        assert!(!schema.relations.neighbors && !schema.relations.parent_children);

        let fields = adapter.get(Uuid::from_u128(1)).unwrap();
        assert_eq!(fields[0], (FIELD_SUBJECT, ScanValue::Str("aaa".into())));
        assert_eq!(fields[6], (FIELD_DELETED_AT, ScanValue::I64(0)));
        let mut out = adapter
            .filter_eq(FIELD_SUBJECT, &ScanValue::Str("aaa".into()))
            .unwrap();
        out.sort();
        assert_eq!(out, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
        assert_eq!(
            adapter.filter_eq(FIELD_OBJECT, &ScanValue::Str("aaa".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(adapter.scan_field(FIELD_UPDATED_AT).unwrap().len(), 3);
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_UPDATED_AT, ScanValue::I64(9_000)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[4],
            (FIELD_UPDATED_AT, ScanValue::I64(9_000))
        );
        assert_eq!(
            adapter.neighbors(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(adapter.table_name(), "relation");
    }

    /// `REL-FR-004`: insert validates every field and the three rules
    /// (non-empty endpoints and label, non-negative stamp); replace,
    /// guarded replace, and delete behave as every front-door domain.
    #[test]
    fn insert_replace_replace_if_and_delete_apply_the_rules() {
        use crate::server::protocol::{CompareOp, Predicate};
        let adapter = sample_adapter();
        let id = Uuid::from_u128(4);
        let full = |subject: &str, updated_at: i64| {
            vec![
                (FIELD_SUBJECT, ScanValue::Str(subject.into())),
                (FIELD_RELATION, ScanValue::Str("part_of".into())),
                (FIELD_OBJECT, ScanValue::Str("aaa".into())),
                (FIELD_CREATED_AT, ScanValue::I64(4_000)),
                (FIELD_UPDATED_AT, ScanValue::I64(updated_at)),
                (FIELD_NODE_ID, ScanValue::Str("laptop".into())),
                (FIELD_DELETED_AT, ScanValue::I64(0)),
            ]
        };
        assert_eq!(
            adapter.insert_record(id, full("ccc", 4_000)),
            Ok(InsertOutcome::Inserted)
        );
        assert_eq!(adapter.get(id).unwrap(), full("ccc", 4_000));
        assert_eq!(
            adapter.insert_record(id, full("ccc", 4_000)),
            Ok(InsertOutcome::Duplicate)
        );
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), full("", 5_000)),
            Err(ErrorCode::Malformed),
            "an empty subject"
        );
        let mut negative = full("ccc", 5_000);
        negative[6] = (FIELD_DELETED_AT, ScanValue::I64(-1));
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), negative),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), full("ccc", 5_000)[..6].to_vec()),
            Err(ErrorCode::Malformed),
            "a missing field"
        );
        assert!(adapter.get(Uuid::from_u128(5)).is_none(), "nothing written");

        assert_eq!(
            adapter.replace_record(id, full("ddd", 6_000)),
            Ok(ReplaceOutcome::Replaced)
        );
        assert_eq!(
            adapter.filter_eq(FIELD_SUBJECT, &ScanValue::Str("ddd".into())),
            Ok(vec![id]),
            "the index moved"
        );
        let lww = |mine: i64| Predicate {
            field: FIELD_UPDATED_AT,
            op: CompareOp::Lt,
            value: ScanValue::I64(mine),
        };
        assert_eq!(
            adapter.replace_record_if(id, full("ddd", 5_000), &lww(5_000)),
            Ok(ReplaceIfOutcome::GuardFailed)
        );
        assert_eq!(
            adapter.replace_record_if(id, full("ddd", 7_000), &lww(7_000)),
            Ok(ReplaceIfOutcome::Replaced)
        );
        assert_eq!(adapter.delete_record(id), Ok(DeleteOutcome::Deleted));
        assert_eq!(adapter.delete_record(id), Ok(DeleteOutcome::NotFound));
    }
}
