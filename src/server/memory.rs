//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<MemoryProductionStack>`]
//! for `Memory` — this crate's sixth domain and third front-door one
//! (`MEM-FR-006`, ADR-0048, `docs/design/SERVER-MEMORY-DOMAIN-DESIGN.md`),
//! `server`-gated alone like `reminder`/`entity`. Eleven fields:
//! `category` is equality-filterable, `access_count` scannable and
//! updatable (non-negative — the one domain rule), everything else
//! refused by `UpdateField` and changed only whole through
//! `Request::Replace` (`REP-FR-005`, ADR-0049), reachable through
//! `Query`/`Aggregate` like any field. `tags` is a `StrList`. One
//! relation since `ADR-0050` (`TBL-FR-008`): `mentions`, whose rows are
//! the `entity` table's — `describe_relations` says so, a same-table
//! `Join` over it is `Unsupported`, and a `Link` under it has its far
//! end checked by the server against that table (`TBL-FR-007`); no
//! `ChildOf` relation.
//!
//! See `crate::generic::memory`'s own module docs for what the
//! consumer's table holds that this record does not, and why.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, JoinRelation,
    ParentLookup, Predicate, RecordId, RelationCapabilities, RelationDescriptor, ScanValue,
    TransactionOp, ValueKind, WriteOp, WriteResult,
};
use super::{
    page_by_scan, page_key, predicate_matches, validate_predicate, ConnectionStore, DeleteOutcome,
    InsertOutcome, LinkOutcome, PageRow, ReplaceIfOutcome, ReplaceOutcome,
};
use crate::generic::memory::{
    AccessCountField, CategoryField, Memory, MemoryProductionStack, UpdatedAtOrder,
    MEMORY_FOREIGN_TABLE, MEMORY_RELATION_LABELS,
};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{Delete, GetById, Insert, MultiLink, Replace, UpdateField};
use crate::generic::{DeleteError, GuardedReplace, InsertError, LinkError, ReplaceError};
use std::path::Path;

pub const FIELD_CONTENT: FieldRef = 0;
pub const FIELD_CATEGORY: FieldRef = 1;
pub const FIELD_TAGS: FieldRef = 2;
pub const FIELD_SOURCE: FieldRef = 3;
pub const FIELD_METADATA_JSON: FieldRef = 4;
pub const FIELD_CREATED_AT: FieldRef = 5;
pub const FIELD_UPDATED_AT: FieldRef = 6;
pub const FIELD_MEMORY_TYPE: FieldRef = 7;
pub const FIELD_STATUS: FieldRef = 8;
pub const FIELD_SENSITIVE: FieldRef = 9;
pub const FIELD_ACCESS_COUNT: FieldRef = 10;
/// `SYN-FR-001` (ADR-0056): the soft-delete stamp; `0` is live.
pub const FIELD_DELETED_AT: FieldRef = 11;
/// `SYN-FR-001` (ADR-0056): the writing node; `""` is unattributed.
pub const FIELD_NODE_ID: FieldRef = 12;

/// Every field but `access_count`: refused by `UpdateField`
/// (`MEM-FR-004`) — changed only whole, with every other field, through
/// `replace_record` (`REP-FR-005`, ADR-0049).
const READ_ONLY_FIELDS: [FieldRef; 12] = [
    FIELD_CONTENT,
    FIELD_CATEGORY,
    FIELD_TAGS,
    FIELD_SOURCE,
    FIELD_METADATA_JSON,
    FIELD_CREATED_AT,
    FIELD_UPDATED_AT,
    FIELD_MEMORY_TYPE,
    FIELD_STATUS,
    FIELD_SENSITIVE,
    FIELD_DELETED_AT,
    FIELD_NODE_ID,
];

/// `MEM-FR-003`: `access_count` is a counter — a negative value is
/// `Malformed` before any write, the one domain rule this adapter adds.
fn valid_access_count(value: i64) -> bool {
    value >= 0
}

pub struct MemoryConnectionStore {
    store: GenericProductionStore<MemoryProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl MemoryConnectionStore {
    pub fn new(store: GenericProductionStore<MemoryProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<MemoryProductionStack>,
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
    /// mutable field is `access_count`, checked non-negative.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_ACCESS_COUNT, ScanValue::I64(count)) => {
                    if !valid_access_count(*count) {
                        return Err((i, ErrorCode::Malformed));
                    }
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_ACCESS_COUNT, _) => return Err((i, ErrorCode::Malformed)),
                (field, _) if READ_ONLY_FIELDS.contains(&field) => {
                    return Err((i, ErrorCode::Unsupported))
                }
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut MemoryProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::I64(count) = op.value {
                UpdateField::<Memory, AccessCountField>::update(inner, op.id, count)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }

    /// `INS-FR-006` (ADR-0046): the whole field list against this
    /// domain's schema, before any write — all thirteen tags exactly once
    /// with a value of its kind, `tags` as a `StrList`, `access_count`
    /// non-negative. `Malformed` for a missing, repeated, or wrong-kind
    /// field; `UnknownField` for a tag this domain doesn't have.
    fn memory_from_fields(
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<Memory, ErrorCode> {
        let mut content = None;
        let mut category = None;
        let mut tags = None;
        let mut source = None;
        let mut metadata_json = None;
        let mut created_at = None;
        let mut updated_at = None;
        let mut memory_type = None;
        let mut status = None;
        let mut sensitive = None;
        let mut access_count = None;
        let mut deleted_at = None;
        let mut node_id = None;
        for (tag, value) in fields {
            match (tag, value) {
                (FIELD_CONTENT, ScanValue::Str(v)) if content.is_none() => content = Some(v),
                (FIELD_CATEGORY, ScanValue::Str(v)) if category.is_none() => category = Some(v),
                (FIELD_TAGS, ScanValue::StrList(v)) if tags.is_none() => tags = Some(v),
                (FIELD_SOURCE, ScanValue::Str(v)) if source.is_none() => source = Some(v),
                (FIELD_METADATA_JSON, ScanValue::Str(v)) if metadata_json.is_none() => {
                    metadata_json = Some(v)
                }
                (FIELD_CREATED_AT, ScanValue::I64(v)) if created_at.is_none() => {
                    created_at = Some(v)
                }
                (FIELD_UPDATED_AT, ScanValue::I64(v)) if updated_at.is_none() => {
                    updated_at = Some(v)
                }
                (FIELD_MEMORY_TYPE, ScanValue::Str(v)) if memory_type.is_none() => {
                    memory_type = Some(v)
                }
                (FIELD_STATUS, ScanValue::Str(v)) if status.is_none() => status = Some(v),
                (FIELD_SENSITIVE, ScanValue::Bool(v)) if sensitive.is_none() => sensitive = Some(v),
                (FIELD_ACCESS_COUNT, ScanValue::I64(v))
                    if access_count.is_none() && valid_access_count(v) =>
                {
                    access_count = Some(v)
                }
                (FIELD_DELETED_AT, ScanValue::I64(v)) if deleted_at.is_none() && v >= 0 => {
                    deleted_at = Some(v)
                }
                (FIELD_NODE_ID, ScanValue::Str(v)) if node_id.is_none() => node_id = Some(v),
                (tag, _) if tag <= FIELD_NODE_ID => return Err(ErrorCode::Malformed),
                _ => return Err(ErrorCode::UnknownField),
            }
        }
        let (
            Some(content),
            Some(category),
            Some(tags),
            Some(source),
            Some(metadata_json),
            Some(created_at_unix_ms),
            Some(updated_at_unix_ms),
            Some(memory_type),
            Some(status),
            Some(sensitive),
            Some(access_count),
            Some(deleted_at_unix_ms),
            Some(node_id),
        ) = (
            content,
            category,
            tags,
            source,
            metadata_json,
            created_at,
            updated_at,
            memory_type,
            status,
            sensitive,
            access_count,
            deleted_at,
            node_id,
        )
        else {
            return Err(ErrorCode::Malformed);
        };
        Ok(Memory {
            id,
            content,
            category,
            tags,
            source,
            metadata_json,
            created_at_unix_ms,
            updated_at_unix_ms,
            memory_type,
            status,
            sensitive,
            access_count,
            deleted_at_unix_ms,
            node_id,
        })
    }

    /// `ISO-FR-002`/`ISO-FR-006` — see `DogConnectionStore::check_read_set`
    /// for the full contract; identical shape here.
    fn check_read_set(
        reads: &[(RecordId, FieldRef, ScanValue)],
        get: impl Fn(RecordId) -> Option<Memory>,
    ) -> Result<(), (usize, ErrorCode)> {
        for (id, field, value) in reads {
            let current = get(*id).and_then(|memory| {
                Self::fields_of(memory)
                    .into_iter()
                    .find(|(tag, _)| tag == field)
                    .map(|(_, v)| v)
            });
            if current.as_ref() != Some(value) {
                return Err((0, ErrorCode::Conflict));
            }
        }
        Ok(())
    }

    /// The wire shape of one memory, in tag order — `get`, `scan_all`,
    /// and the read-set check all go through here so they cannot drift.
    fn fields_of(memory: Memory) -> Vec<(FieldRef, ScanValue)> {
        vec![
            (FIELD_CONTENT, ScanValue::Str(memory.content)),
            (FIELD_CATEGORY, ScanValue::Str(memory.category)),
            (FIELD_TAGS, ScanValue::StrList(memory.tags)),
            (FIELD_SOURCE, ScanValue::Str(memory.source)),
            (FIELD_METADATA_JSON, ScanValue::Str(memory.metadata_json)),
            (FIELD_CREATED_AT, ScanValue::I64(memory.created_at_unix_ms)),
            (FIELD_UPDATED_AT, ScanValue::I64(memory.updated_at_unix_ms)),
            (FIELD_MEMORY_TYPE, ScanValue::Str(memory.memory_type)),
            (FIELD_STATUS, ScanValue::Str(memory.status)),
            (FIELD_SENSITIVE, ScanValue::Bool(memory.sensitive)),
            (FIELD_ACCESS_COUNT, ScanValue::I64(memory.access_count)),
            (FIELD_DELETED_AT, ScanValue::I64(memory.deleted_at_unix_ms)),
            (FIELD_NODE_ID, ScanValue::Str(memory.node_id)),
        ]
    }
}

/// `WBT-FR-003` (ADR-0060): one parsed, pre-validated write of an atomic
/// [`WriteOp`] batch — the field lists decoded to a [`Memory`] and the
/// guard/label checked during the exclusive section's preflight pass.
enum PreparedWrite {
    Insert(Memory),
    Replace(Memory),
    ReplaceIf(Memory, Predicate),
    Delete(RecordId),
    Link {
        left: RecordId,
        right: RecordId,
        relation: String,
    },
}

impl MemoryConnectionStore {
    /// Parse and pre-validate one op during atomic preflight (`WBT-FR-003`):
    /// the field list to a `Memory`, the `ReplaceIf` guard as a `Query`
    /// predicate (`GRD-FR-004`), the `Link` label against this table's
    /// relations. Any failure aborts the atomic batch before a write.
    fn prepare_write(schema: &DomainSchema, op: &WriteOp) -> Result<PreparedWrite, ErrorCode> {
        Ok(match op {
            WriteOp::Insert { id, fields } => {
                PreparedWrite::Insert(Self::memory_from_fields(*id, fields.clone())?)
            }
            WriteOp::Replace { id, fields } => {
                PreparedWrite::Replace(Self::memory_from_fields(*id, fields.clone())?)
            }
            WriteOp::ReplaceIf { id, fields, guard } => {
                validate_predicate(schema, guard)?;
                PreparedWrite::ReplaceIf(
                    Self::memory_from_fields(*id, fields.clone())?,
                    guard.clone(),
                )
            }
            WriteOp::Delete { id } => PreparedWrite::Delete(*id),
            WriteOp::Link {
                left,
                right,
                relation,
            } => {
                if !MEMORY_RELATION_LABELS.contains(&relation.as_str()) {
                    return Err(ErrorCode::Malformed);
                }
                PreparedWrite::Link {
                    left: *left,
                    right: *right,
                    relation: relation.clone(),
                }
            }
        })
    }

    /// Apply one prepared write to the locked stack (`WBT-FR-003`).
    /// Every soft outcome is a [`WriteResult`]; only a storage I/O error
    /// (or a link to a missing own-table endpoint) is a hard `Err` — and
    /// the batch's own-endpoint existence was checked before this ran.
    fn apply_prepared(
        inner: &mut MemoryProductionStack,
        prepared: PreparedWrite,
    ) -> Result<WriteResult, ErrorCode> {
        Ok(match prepared {
            PreparedWrite::Insert(memory) => match Insert::insert(inner, memory) {
                Ok(()) => WriteResult::Inserted,
                Err(InsertError::Duplicate(_)) => WriteResult::Duplicate,
                Err(InsertError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::Replace(memory) => match Replace::replace(inner, memory) {
                Ok(()) => WriteResult::Replaced,
                Err(ReplaceError::NotFound(_)) => WriteResult::NotFound,
                Err(ReplaceError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::ReplaceIf(memory, guard) => {
                let id = memory.id;
                match GetById::<Memory>::get(inner, id) {
                    None => WriteResult::NotFound,
                    Some(stored) => {
                        if predicate_matches(&Self::fields_of(stored), &guard) {
                            match Replace::replace(inner, memory) {
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
            PreparedWrite::Delete(id) => match Delete::<Memory>::delete(inner, id) {
                Ok(()) => WriteResult::Deleted,
                Err(DeleteError::NotFound(_)) => WriteResult::NotFound,
                Err(DeleteError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::Link {
                left,
                right,
                relation,
            } => match MultiLink::link(inner, &relation, left, right) {
                Ok(crate::generic::LinkOutcome::Linked) => WriteResult::Linked,
                Ok(crate::generic::LinkOutcome::AlreadyLinked) => WriteResult::AlreadyLinked,
                Err(LinkError::UnknownRecord(_)) => return Err(ErrorCode::RecordNotFound),
                Err(LinkError::SelfLoop(_) | LinkError::InvalidLabel(_)) => {
                    return Err(ErrorCode::Malformed)
                }
                Err(LinkError::Durability(_)) => return Err(ErrorCode::Storage),
            },
        })
    }
}

impl ConnectionStore for MemoryConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Memory>(id).map(Self::fields_of)
    }

    /// `SQL-FR-004`/`SQL-FR-005` (ADR-0034): every id from `all_ids`,
    /// each mapped through this adapter's own `get`.
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
        self.store
            .all_ids::<Memory>()
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect()
    }

    /// `ORD-FR-005` (ADR-0059): a page ordered by `updated_at_unix_ms`
    /// is a range walk of the stack's sorted index — the page's cost,
    /// not the table's. Any other orderable field takes the scan path.
    /// `validate_page` has already matched the cursor's kind to the
    /// field's, so a cursor here is `I64` or absent.
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
            .page_by::<Memory, UpdatedAtOrder>(cursor, limit)
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect())
    }

    /// `PAG-FR-002` (ADR-0055): the sort key of every record read
    /// straight off [`Memory`], so a page materializes only its own rows.
    /// A field this arm list does not name falls back to the wire shape
    /// — the same key [`page_key`] derives for the trait default.
    fn page_keys(&self, order_by: FieldRef) -> Vec<(RecordId, i128)> {
        self.store
            .all_ids::<Memory>()
            .into_iter()
            .filter_map(|id| {
                let record = self.store.get::<Memory>(id)?;
                let key = match order_by {
                    FIELD_CREATED_AT => Some(i128::from(record.created_at_unix_ms)),
                    FIELD_UPDATED_AT => Some(i128::from(record.updated_at_unix_ms)),
                    FIELD_ACCESS_COUNT => Some(i128::from(record.access_count)),
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
            (FIELD_CATEGORY, ScanValue::Str(category)) => {
                Ok(self.store.filter_eq::<Memory, CategoryField>(category))
            }
            (FIELD_CATEGORY, _) => Err(ErrorCode::Malformed),
            (field, _) if field <= FIELD_NODE_ID => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_ACCESS_COUNT => Ok(self
                .store
                .scan::<Memory, AccessCountField>()
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
            (FIELD_ACCESS_COUNT, ScanValue::I64(count)) => {
                if !valid_access_count(count) {
                    return Err(ErrorCode::Malformed);
                }
                match self.store.update::<Memory, AccessCountField>(id, count) {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_ACCESS_COUNT, _) => Err(ErrorCode::Malformed),
            (field, _) if READ_ONLY_FIELDS.contains(&field) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    /// `INS-FR-006` (ADR-0046): validate, then one write under the
    /// store's own lock.
    fn insert_record(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<InsertOutcome, ErrorCode> {
        let memory = Self::memory_from_fields(id, fields)?;
        match self.store.insert(memory) {
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
        let memory = Self::memory_from_fields(id, fields)?;
        match self.store.replace(memory) {
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
        let memory = Self::memory_from_fields(id, fields)?;
        let holds = |stored: &Memory| predicate_matches(&Self::fields_of(stored.clone()), guard);
        match self.store.replace_if(memory, holds) {
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
        match self.store.delete::<Memory>(id) {
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

    /// `MEM-FR-005`: `Memory` has no `ChildOf` relation.
    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    /// `TBL-FR-008` (ADR-0050): the entity ids a memory mentions — or,
    /// given an entity id, the memories that mention it, since the edge
    /// is stored in both directions. Every id on the far side is an
    /// `Entity` (another table's record); none is a `Memory`.
    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(self.store.all_neighbors::<Memory>(id))
    }

    fn neighbors_by_relation(
        &self,
        id: RecordId,
        relation: &str,
    ) -> Result<Vec<RecordId>, ErrorCode> {
        match self.store.neighbors_by_relation::<Memory>(relation, id) {
            Some(records) => Ok(records),
            None => Err(ErrorCode::Malformed),
        }
    }

    /// `CNT-FR-002` (ADR-0057): one read under the store's lock; an
    /// unknown label is `Malformed`, as for `neighbors_by_relation`.
    /// `WBT-FR-002`/`003` (ADR-0060): pipelined is the trait default
    /// (each op through its single-shot method); atomic parses and
    /// pre-validates every op under one exclusive section, including the
    /// server's foreign checks and prior writes to own-table endpoints,
    /// then applies every op, so
    /// a precondition failure aborts with nothing applied and the batch
    /// is isolated from other connections. Not crash-atomic: a storage
    /// I/O error mid-apply is not rolled back (the named follow-on).
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
            let mut existence = std::collections::HashMap::new();
            for (i, op) in ops.iter().enumerate() {
                check(i).map_err(|code| (i, code))?;
                let p = Self::prepare_write(&schema, op).map_err(|code| (i, code))?;
                match &p {
                    PreparedWrite::Insert(record) => {
                        existence.insert(record.id, true);
                    }
                    PreparedWrite::Delete(id) => {
                        existence.insert(*id, false);
                    }
                    PreparedWrite::Link { left, right, .. } => {
                        let exists = |id| {
                            existence
                                .get(&id)
                                .copied()
                                .unwrap_or_else(|| GetById::<Memory>::get(inner, id).is_some())
                        };
                        if !exists(*left) {
                            return Err((i, ErrorCode::RecordNotFound));
                        }
                        if left == right {
                            return Err((i, ErrorCode::Malformed));
                        }
                    }
                    _ => {}
                }
                prepared.push(p);
            }
            let mut results = Vec::with_capacity(prepared.len());
            for (i, p) in prepared.into_iter().enumerate() {
                results.push(Self::apply_prepared(inner, p).map_err(|code| (i, code))?);
            }
            Ok(results)
        })
    }

    fn count_edges(&self, relation: &str) -> Result<u64, ErrorCode> {
        match self.store.count_edges::<Memory>(relation) {
            Some(count) => Ok(count as u64),
            None => Err(ErrorCode::Malformed),
        }
    }

    fn list_relation_kinds(&self) -> Vec<String> {
        self.store.relation_kinds::<Memory>()
    }

    /// `TBL-FR-008`: the one relation, `mentions`, with its rows in the
    /// `entity` table — so a same-table `Join` over it is `Unsupported`
    /// and a cross-table one needs `right_table: Some("entity")`. The
    /// unfiltered `neighbors` is deliberately *not* listed: its far side
    /// is never this table's rows.
    fn describe_relations(&self) -> Vec<RelationDescriptor> {
        MEMORY_RELATION_LABELS
            .iter()
            .map(|label| RelationDescriptor {
                name: label.to_string(),
                kind: JoinRelation::Neighbors(Some(label.to_string())),
                target_table: Some(MEMORY_FOREIGN_TABLE.to_string()),
            })
            .collect()
    }

    /// `TBL-FR-008`: a fixed label set — `mentions` only; any other label
    /// is `Malformed`. `left` must be a memory (`RecordNotFound`); `right`
    /// is an entity id this adapter cannot see — the server checks it
    /// against the `entity` table before this is called (`TBL-FR-007`).
    fn link_records(
        &self,
        left: RecordId,
        right: RecordId,
        relation: &str,
    ) -> Result<LinkOutcome, ErrorCode> {
        if !MEMORY_RELATION_LABELS.contains(&relation) {
            return Err(ErrorCode::Malformed);
        }
        match self.store.link_by_relation::<Memory>(relation, left, right) {
            Ok(crate::generic::LinkOutcome::Linked) => Ok(LinkOutcome::Linked),
            Ok(crate::generic::LinkOutcome::AlreadyLinked) => Ok(LinkOutcome::AlreadyLinked),
            Err(LinkError::UnknownRecord(_)) => Err(ErrorCode::RecordNotFound),
            Err(LinkError::SelfLoop(_) | LinkError::InvalidLabel(_)) => Err(ErrorCode::Malformed),
            Err(LinkError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    fn table_name(&self) -> &str {
        "memory"
    }

    /// `DEL-FR-005` (ADR-0051): the `entity` table deleted `id` — every
    /// `mentions` edge to it goes (the consumer's `DELETE FROM
    /// memory_entities WHERE entity_id = ?`). A label this domain lacks
    /// is `Malformed`.
    fn detach_record(&self, relation: &str, id: RecordId) -> Result<usize, ErrorCode> {
        if !MEMORY_RELATION_LABELS.contains(&relation) {
            return Err(ErrorCode::Malformed);
        }
        match self.store.detach::<Memory>(relation, id) {
            Ok(n) => Ok(n),
            Err(DeleteError::NotFound(_)) => Err(ErrorCode::Malformed),
            Err(DeleteError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `STV-FR-002`: `validate_batch` on this one operation.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            self.store.get::<Memory>(id).is_some()
        })
        .map_err(|(_, code)| code)
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
                field(FIELD_CONTENT, "content", ValueKind::Str),
                FieldDescriptor {
                    tag: FIELD_CATEGORY,
                    name: "category".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                field(FIELD_TAGS, "tags", ValueKind::StrList),
                field(FIELD_SOURCE, "source", ValueKind::Str),
                field(FIELD_METADATA_JSON, "metadata_json", ValueKind::Str),
                field(FIELD_CREATED_AT, "created_at_unix_ms", ValueKind::I64),
                field(FIELD_UPDATED_AT, "updated_at_unix_ms", ValueKind::I64),
                field(FIELD_MEMORY_TYPE, "memory_type", ValueKind::Str),
                field(FIELD_STATUS, "status", ValueKind::Str),
                field(FIELD_SENSITIVE, "sensitive", ValueKind::Bool),
                FieldDescriptor {
                    tag: FIELD_ACCESS_COUNT,
                    name: "access_count".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
                field(FIELD_DELETED_AT, "deleted_at_unix_ms", ValueKind::I64),
                field(FIELD_NODE_ID, "node_id", ValueKind::Str),
            ],
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
        // (`GRP-FR-001`–`005`) and where the read-set check runs in each.
        match &self.journal {
            None => self.store.with_exclusive(|inner| {
                Self::validate_batch(updates, |id| GetById::<Memory>::get(inner, id).is_some())?;
                Self::check_read_set(read_set, |id| GetById::<Memory>::get(inner, id))?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Memory>(id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
                            Self::check_read_set(read_set, |id| GetById::<Memory>::get(inner, id))?;
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
    use crate::generic::memory::create_memory_production_stack;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn memory(n: u128, category: &str, sensitive: bool) -> Memory {
        Memory {
            id: Uuid::from_u128(n),
            content: format!("memory {n}"),
            category: category.into(),
            tags: vec!["t".into()],
            source: "manual".into(),
            metadata_json: "{}".into(),
            created_at_unix_ms: 1_000 * n as i64,
            updated_at_unix_ms: 1_000 * n as i64,
            memory_type: "unclassified".into(),
            status: "active".into(),
            sensitive,
            access_count: 0,
            deleted_at_unix_ms: 0,
            node_id: String::new(),
        }
    }

    fn sample_adapter() -> MemoryConnectionStore {
        let dir = fresh_temp_dir("server_memory_adapter").unwrap();
        let path = dir.join("memories.mmap");
        let stack = create_memory_production_stack(
            vec![
                memory(1, "general", false),
                memory(2, "preference", false),
                memory(3, "general", true),
            ],
            &[],
            &path,
        )
        .unwrap();
        MemoryConnectionStore::new(GenericProductionStore::new(stack))
    }

    fn full_fields(n: u128) -> Vec<(FieldRef, ScanValue)> {
        MemoryConnectionStore::fields_of(memory(n, "decision", false))
    }

    #[test]
    fn get_returns_every_field_in_tag_order_and_describe_matches() {
        let adapter = sample_adapter();
        let fields = adapter.get(Uuid::from_u128(3)).unwrap();
        assert_eq!(fields.len(), 13);
        assert_eq!(
            fields[2],
            (FIELD_TAGS, ScanValue::StrList(vec!["t".into()]))
        );
        assert_eq!(fields[9], (FIELD_SENSITIVE, ScanValue::Bool(true)));
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 13);
        for (i, f) in schema.fields.iter().enumerate() {
            assert_eq!(f.tag as usize, i, "tags are dense and in order");
            assert_eq!(fields[i].0, f.tag);
        }
        let category = &schema.fields[FIELD_CATEGORY as usize];
        assert!(category.capabilities.filter_eq && !category.capabilities.update);
        let count = &schema.fields[FIELD_ACCESS_COUNT as usize];
        assert!(
            count.capabilities.scan && count.capabilities.update && !count.capabilities.filter_eq
        );
        assert_eq!(
            schema.fields[FIELD_CONTENT as usize].capabilities,
            FieldCapabilities {
                filter_eq: false,
                scan: false,
                update: false
            }
        );
        assert!(schema.relations.neighbors && !schema.relations.parent_children);
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_by_category_scan_and_update_access_count_with_the_domain_rule() {
        let adapter = sample_adapter();
        let mut general = adapter
            .filter_eq(FIELD_CATEGORY, &ScanValue::Str("general".into()))
            .unwrap();
        general.sort();
        assert_eq!(general, vec![Uuid::from_u128(1), Uuid::from_u128(3)]);
        assert_eq!(
            adapter.filter_eq(FIELD_CONTENT, &ScanValue::Str("x".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(FIELD_CATEGORY, &ScanValue::I64(1)),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.filter_eq(99, &ScanValue::I64(1)),
            Err(ErrorCode::UnknownField)
        );

        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_ACCESS_COUNT, ScanValue::I64(5)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[10],
            (FIELD_ACCESS_COUNT, ScanValue::I64(5))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_ACCESS_COUNT, ScanValue::I64(-1)),
            Err(ErrorCode::Malformed),
            "a counter is never negative"
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_ACCESS_COUNT, ScanValue::I64(1)),
            Ok(false)
        );
        assert_eq!(
            adapter.update_field(
                Uuid::from_u128(1),
                FIELD_CONTENT,
                ScanValue::Str("x".into())
            ),
            Err(ErrorCode::Unsupported)
        );
        let mut counts = adapter.scan_field(FIELD_ACCESS_COUNT).unwrap();
        counts.sort_by_key(|v| if let ScanValue::I64(n) = v { *n } else { 0 });
        assert_eq!(
            counts,
            vec![ScanValue::I64(0), ScanValue::I64(0), ScanValue::I64(5)]
        );
        assert_eq!(adapter.scan_field(FIELD_TAGS), Err(ErrorCode::Unsupported));
    }

    #[test]
    fn insert_record_takes_all_thirteen_fields_and_refuses_every_malformed_list() {
        let adapter = sample_adapter();
        let id = Uuid::from_u128(4);
        assert_eq!(
            adapter.insert_record(id, full_fields(4)),
            Ok(InsertOutcome::Inserted)
        );
        assert_eq!(adapter.get(id).unwrap(), full_fields(4));
        assert_eq!(
            adapter.insert_record(id, full_fields(4)),
            Ok(InsertOutcome::Duplicate)
        );
        let mut negative = full_fields(5);
        negative[10] = (FIELD_ACCESS_COUNT, ScanValue::I64(-3));
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), negative),
            Err(ErrorCode::Malformed)
        );
        let mut wrong_tags = full_fields(5);
        wrong_tags[2] = (FIELD_TAGS, ScanValue::Str("t".into()));
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), wrong_tags),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), full_fields(5)[..12].to_vec()),
            Err(ErrorCode::Malformed)
        );
        let mut extra = full_fields(5);
        extra.push((42, ScanValue::U32(0)));
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(5), extra),
            Err(ErrorCode::UnknownField)
        );
        assert!(
            adapter.get(Uuid::from_u128(5)).is_none(),
            "nothing inserted"
        );
    }

    #[test]
    fn mentions_is_the_one_relation_foreign_to_entity_and_links_without_seeing_the_far_end() {
        let adapter = sample_adapter();
        let (one, ada) = (Uuid::from_u128(1), Uuid::from_u128(0xada));
        assert_eq!(adapter.parent(one), Err(ErrorCode::Unsupported));
        assert_eq!(adapter.children(one), Err(ErrorCode::Unsupported));
        assert!(adapter.describe().relations.neighbors);
        assert_eq!(adapter.table_name(), "memory");
        assert_eq!(
            adapter.describe_relations(),
            vec![RelationDescriptor {
                name: "mentions".into(),
                kind: JoinRelation::Neighbors(Some("mentions".into())),
                target_table: Some("entity".into()),
            }]
        );
        assert_eq!(adapter.list_relation_kinds(), vec!["mentions".to_string()]);
        assert_eq!(adapter.neighbors(one), Ok(vec![]));
        // The far end is an entity this adapter never sees (`TBL-FR-007`).
        assert_eq!(
            adapter.link_records(one, ada, "mentions"),
            Ok(LinkOutcome::Linked)
        );
        assert_eq!(
            adapter.link_records(one, ada, "mentions"),
            Ok(LinkOutcome::AlreadyLinked)
        );
        assert_eq!(
            adapter.neighbors_by_relation(one, "mentions"),
            Ok(vec![ada])
        );
        assert_eq!(
            adapter.neighbors(ada),
            Ok(vec![one]),
            "read from the entity's side"
        );
        assert_eq!(
            adapter.link_records(one, ada, "x"),
            Err(ErrorCode::Malformed),
            "a fixed label set"
        );
        assert_eq!(
            adapter.link_records(Uuid::from_u128(77), ada, "mentions"),
            Err(ErrorCode::RecordNotFound),
            "the near end must be a memory"
        );
        assert_eq!(
            adapter.neighbors_by_relation(one, "x"),
            Err(ErrorCode::Malformed)
        );
    }

    /// `REP-FR-005` (ADR-0049): the same validation as `insert_record`,
    /// then the whole record replaced — every read sees the new version
    /// at once, the old category bucket no longer lists the id; an
    /// unknown id is `NotFound` with nothing written; a malformed list
    /// is refused before any write.
    #[test]
    fn replace_record_validates_like_insert_then_swaps_the_whole_record() {
        let adapter = sample_adapter();
        let id = Uuid::from_u128(1);
        let mut edited = full_fields(1);
        edited[0] = (FIELD_CONTENT, ScanValue::Str("memory 1, revised".into()));
        edited[1] = (FIELD_CATEGORY, ScanValue::Str("decision".into()));
        edited[10] = (FIELD_ACCESS_COUNT, ScanValue::I64(5));
        assert_eq!(
            adapter.replace_record(id, edited.clone()),
            Ok(ReplaceOutcome::Replaced)
        );
        assert_eq!(adapter.get(id).unwrap(), edited);
        assert_eq!(
            adapter.filter_eq(FIELD_CATEGORY, &ScanValue::Str("decision".into())),
            Ok(vec![id])
        );
        assert!(!adapter
            .filter_eq(FIELD_CATEGORY, &ScanValue::Str("general".into()))
            .unwrap()
            .contains(&id));
        assert_eq!(
            adapter.replace_record(Uuid::from_u128(9), full_fields(9)),
            Ok(ReplaceOutcome::NotFound)
        );
        assert!(adapter.get(Uuid::from_u128(9)).is_none());
        let mut negative = full_fields(1);
        negative[10] = (FIELD_ACCESS_COUNT, ScanValue::I64(-1));
        assert_eq!(
            adapter.replace_record(id, negative),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.replace_record(id, full_fields(1)[..12].to_vec()),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.get(id).unwrap(),
            edited,
            "nothing written on refusal"
        );
        assert_eq!(adapter.scan_all().len(), 3, "no new record");
    }

    /// `DEL-FR-006`/`005` (ADR-0051): a deleted memory is gone from every
    /// read and its `mentions` edge with it; a repeat is `NotFound`;
    /// `detach_record` for an entity id drops every memory's edge to it
    /// and refuses a label the domain lacks.
    #[test]
    fn delete_record_and_detach_record_drop_the_memory_and_its_edges() {
        let adapter = sample_adapter();
        let (one, two, ada) = (
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            Uuid::from_u128(0xada),
        );
        adapter.link_records(one, ada, "mentions").unwrap();
        adapter.link_records(two, ada, "mentions").unwrap();
        assert_eq!(adapter.delete_record(one), Ok(DeleteOutcome::Deleted));
        assert!(adapter.get(one).is_none());
        assert_eq!(adapter.scan_all().len(), 2);
        assert!(!adapter
            .filter_eq(FIELD_CATEGORY, &ScanValue::Str("general".into()))
            .unwrap()
            .contains(&one));
        assert_eq!(
            adapter.neighbors_by_relation(ada, "mentions"),
            Ok(vec![two])
        );
        assert_eq!(adapter.delete_record(one), Ok(DeleteOutcome::NotFound));
        assert_eq!(adapter.detach_record("mentions", ada), Ok(1));
        assert_eq!(adapter.neighbors_by_relation(two, "mentions"), Ok(vec![]));
        assert!(adapter.get(two).is_some());
        assert_eq!(adapter.detach_record("mentions", ada), Ok(0));
        assert_eq!(adapter.detach_record("x", ada), Err(ErrorCode::Malformed));
    }

    /// `CMP-FR-006` (ADR-0052): the adapter compacts its stack and reports
    /// it; every read afterwards is what it was.
    #[test]
    fn compact_reports_what_it_reclaimed_and_changes_no_read() {
        let adapter = sample_adapter();
        let (one, ada) = (Uuid::from_u128(1), Uuid::from_u128(0xada));
        adapter.link_records(one, ada, "mentions").unwrap();
        assert_eq!(
            adapter.delete_record(Uuid::from_u128(3)),
            Ok(DeleteOutcome::Deleted)
        );
        let before = adapter.scan_all();
        let report = adapter.compact().unwrap();
        assert_eq!(report.records, 2);
        assert_eq!(report.slots_reclaimed, 1);
        assert_eq!(report.log_entries_folded, 1);
        assert_eq!(report.edge_logs_folded, 1);
        assert_eq!(adapter.scan_all(), before);
        assert_eq!(
            adapter.neighbors_by_relation(one, "mentions"),
            Ok(vec![ada])
        );
    }

    /// `GRD-FR-003` (ADR-0054): the guard is evaluated over this adapter's
    /// own wire shape of the stored record — `updated_at < mine` holds
    /// for a newer version and replaces, fails for an older one with
    /// nothing written; an unknown id is `NotFound`; `replace_record`'s
    /// validation still runs first.
    #[test]
    fn replace_record_if_is_last_writer_wins_over_the_stored_updated_at() {
        use crate::server::protocol::{CompareOp, Predicate};
        let adapter = sample_adapter();
        let id = Uuid::from_u128(1);
        let stored_updated_at = 1_000;
        let mut newer = full_fields(1);
        newer[0] = (FIELD_CONTENT, ScanValue::Str("newer".into()));
        newer[6] = (FIELD_UPDATED_AT, ScanValue::I64(5_000));
        let guard = |mine: i64| Predicate {
            field: FIELD_UPDATED_AT,
            op: CompareOp::Lt,
            value: ScanValue::I64(mine),
        };
        assert_eq!(
            adapter.replace_record_if(id, newer.clone(), &guard(5_000)),
            Ok(ReplaceIfOutcome::Replaced)
        );
        assert_eq!(adapter.get(id).unwrap(), newer);

        let mut older = newer.clone();
        older[0] = (FIELD_CONTENT, ScanValue::Str("older".into()));
        older[6] = (FIELD_UPDATED_AT, ScanValue::I64(stored_updated_at));
        assert_eq!(
            adapter.replace_record_if(id, older, &guard(stored_updated_at)),
            Ok(ReplaceIfOutcome::GuardFailed)
        );
        assert_eq!(adapter.get(id).unwrap(), newer, "nothing written");

        assert_eq!(
            adapter.replace_record_if(Uuid::from_u128(99), newer.clone(), &guard(9_000)),
            Ok(ReplaceIfOutcome::NotFound)
        );
        let mut negative = newer.clone();
        negative[10] = (FIELD_ACCESS_COUNT, ScanValue::I64(-1));
        assert_eq!(
            adapter.replace_record_if(id, negative, &guard(9_000)),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(adapter.get(id).unwrap(), newer);
    }
}
