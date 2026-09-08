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
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, Predicate,
    RecordId, RelationCapabilities, ScanValue, TransactionOp, ValueKind, WriteOp, WriteResult,
};
use super::{
    page_key, predicate_matches, validate_predicate, ConnectionStore, DeleteOutcome, InsertOutcome,
    LinkOutcome, ReplaceIfOutcome, ReplaceOutcome,
};
use crate::generic::entity::{Entity, EntityProductionStack, KindField, MentionCountField};
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{Delete, GetById, Insert, MultiLink, Replace, UpdateField};
use crate::generic::store::valid_relation_label;
use crate::generic::{DeleteError, GuardedReplace, InsertError, LinkError, ReplaceError};
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

    /// `INS-FR-006` (ADR-0046): the whole field list against this
    /// domain's schema, before any write — all four tags exactly once
    /// with a value of its kind, `aliases` as a `StrList` (the first
    /// *write* of one: `ENT4-FR-003`'s read-only rule describes
    /// `UpdateField`, not a whole record set at creation — the blob and
    /// the insert log carry `Vec<Entity>` whole, so aliases are durable
    /// on the same terms as at `create`). `Malformed` for a missing,
    /// repeated, or wrong-kind field; `UnknownField` for a tag this
    /// domain doesn't have. Relations: none — an inserted entity has no
    /// neighbors under either label until the relation-insertion round.
    fn entity_from_fields(
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<Entity, ErrorCode> {
        let mut label = None;
        let mut kind = None;
        let mut mention_count = None;
        let mut aliases = None;
        for (tag, value) in fields {
            match (tag, value) {
                (FIELD_LABEL, ScanValue::Str(v)) if label.is_none() => label = Some(v),
                (FIELD_KIND, ScanValue::Str(v)) if kind.is_none() => kind = Some(v),
                (FIELD_MENTION_COUNT, ScanValue::I64(v)) if mention_count.is_none() => {
                    mention_count = Some(v)
                }
                (FIELD_ALIASES, ScanValue::StrList(v)) if aliases.is_none() => aliases = Some(v),
                (FIELD_LABEL | FIELD_KIND | FIELD_MENTION_COUNT | FIELD_ALIASES, _) => {
                    return Err(ErrorCode::Malformed)
                }
                _ => return Err(ErrorCode::UnknownField),
            }
        }
        let (Some(label), Some(kind), Some(mention_count), Some(aliases)) =
            (label, kind, mention_count, aliases)
        else {
            return Err(ErrorCode::Malformed);
        };
        Ok(Entity {
            id,
            label,
            kind,
            mention_count,
            aliases,
        })
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

impl EntityConnectionStore {
    /// The wire shape of one entity, in tag order — `get` and a
    /// `ReplaceIf` guard's evaluation both go through here.
    fn fields_of(entity: Entity) -> Vec<(FieldRef, ScanValue)> {
        vec![
            (FIELD_LABEL, ScanValue::Str(entity.label)),
            (FIELD_KIND, ScanValue::Str(entity.kind)),
            (FIELD_MENTION_COUNT, ScanValue::I64(entity.mention_count)),
            // `ENT4-FR-002`: raw, stored order, un-normalized.
            (FIELD_ALIASES, ScanValue::StrList(entity.aliases)),
        ]
    }
}

/// `WBT-FR-003` (ADR-0060): a parsed, pre-validated write of an atomic
/// [`WriteOp`] batch on `Entity`.
enum PreparedWrite {
    Insert(Entity),
    Replace(Entity),
    ReplaceIf(Entity, Predicate),
    Delete(RecordId),
    Link {
        left: RecordId,
        right: RecordId,
        relation: String,
    },
}

impl EntityConnectionStore {
    fn prepare_write(schema: &DomainSchema, op: &WriteOp) -> Result<PreparedWrite, ErrorCode> {
        Ok(match op {
            WriteOp::Insert { id, fields } => {
                PreparedWrite::Insert(Self::entity_from_fields(*id, fields.clone())?)
            }
            WriteOp::Replace { id, fields } => {
                PreparedWrite::Replace(Self::entity_from_fields(*id, fields.clone())?)
            }
            WriteOp::ReplaceIf { id, fields, guard } => {
                validate_predicate(schema, guard)?;
                PreparedWrite::ReplaceIf(
                    Self::entity_from_fields(*id, fields.clone())?,
                    guard.clone(),
                )
            }
            WriteOp::Delete { id } => PreparedWrite::Delete(*id),
            WriteOp::Link {
                left,
                right,
                relation,
            } => {
                if !valid_relation_label(relation) {
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

    fn apply_prepared(
        inner: &mut EntityProductionStack,
        prepared: PreparedWrite,
    ) -> Result<WriteResult, ErrorCode> {
        Ok(match prepared {
            PreparedWrite::Insert(entity) => match Insert::insert(inner, entity) {
                Ok(()) => WriteResult::Inserted,
                Err(InsertError::Duplicate(_)) => WriteResult::Duplicate,
                Err(InsertError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::Replace(entity) => match Replace::replace(inner, entity) {
                Ok(()) => WriteResult::Replaced,
                Err(ReplaceError::NotFound(_)) => WriteResult::NotFound,
                Err(ReplaceError::Durability(_)) => return Err(ErrorCode::Storage),
            },
            PreparedWrite::ReplaceIf(entity, guard) => {
                let id = entity.id;
                match GetById::<Entity>::get(inner, id) {
                    None => WriteResult::NotFound,
                    Some(stored) => {
                        if predicate_matches(&Self::fields_of(stored), &guard) {
                            match Replace::replace(inner, entity) {
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
            PreparedWrite::Delete(id) => match Delete::<Entity>::delete(inner, id) {
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

impl ConnectionStore for EntityConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Entity>(id).map(Self::fields_of)
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

    /// `PAG-FR-002` (ADR-0055): the sort key of every record read
    /// straight off [`Entity`], so a page materializes only its own rows.
    /// A field this arm list does not name falls back to the wire shape
    /// — the same key [`page_key`] derives for the trait default.
    fn page_keys(&self, order_by: FieldRef) -> Vec<(RecordId, i128)> {
        self.store
            .all_ids::<Entity>()
            .into_iter()
            .filter_map(|id| {
                let record = self.store.get::<Entity>(id)?;
                let key = match order_by {
                    FIELD_MENTION_COUNT => Some(i128::from(record.mention_count)),
                    _ => None,
                };
                let key = key.unwrap_or_else(|| page_key(&Self::fields_of(record), order_by, id).0);
                Some((id, key))
            })
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

    /// `INS-FR-006` (ADR-0046): validate, then one write under the
    /// store's own lock — through `NameIndex` (its keys added) and
    /// `MultiSymmetric` (no edges) down to the durable core. A duplicate
    /// is the normal outcome; a durability failure is `Storage`.
    fn insert_record(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<InsertOutcome, ErrorCode> {
        let entity = Self::entity_from_fields(id, fields)?;
        match self.store.insert(entity) {
            Ok(()) => Ok(InsertOutcome::Inserted),
            Err(InsertError::Duplicate(_)) => Ok(InsertOutcome::Duplicate),
            Err(InsertError::Durability(_)) => Err(ErrorCode::Storage),
        }
    }

    /// `REP-FR-005` (ADR-0049): the same validation as `insert_record`,
    /// then one whole-record write under the store's own lock — the
    /// name index follows a changed `label`/`aliases`, every relation
    /// edge survives. An unknown id is the normal outcome, not an error.
    fn replace_record(
        &self,
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    ) -> Result<ReplaceOutcome, ErrorCode> {
        let entity = Self::entity_from_fields(id, fields)?;
        match self.store.replace(entity) {
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
        let entity = Self::entity_from_fields(id, fields)?;
        let holds = |stored: &Entity| predicate_matches(&Self::fields_of(stored.clone()), guard);
        match self.store.replace_if(entity, holds) {
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
        match self.store.delete::<Entity>(id) {
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

    /// `LNK-FR-009` (ADR-0047): open labels — any valid label is
    /// accepted and created at first use by the `MultiSymmetric` layer
    /// beneath; `ListRelationKinds`/`DescribeRelations` list it at once.
    fn link_records(
        &self,
        left: RecordId,
        right: RecordId,
        relation: &str,
    ) -> Result<LinkOutcome, ErrorCode> {
        if !valid_relation_label(relation) {
            return Err(ErrorCode::Malformed);
        }
        match self.store.link_by_relation::<Entity>(relation, left, right) {
            Ok(crate::generic::LinkOutcome::Linked) => Ok(LinkOutcome::Linked),
            Ok(crate::generic::LinkOutcome::AlreadyLinked) => Ok(LinkOutcome::AlreadyLinked),
            Err(LinkError::UnknownRecord(_)) => Err(ErrorCode::RecordNotFound),
            Err(LinkError::SelfLoop(_) | LinkError::InvalidLabel(_)) => Err(ErrorCode::Malformed),
            Err(LinkError::Durability(_)) => Err(ErrorCode::Storage),
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

    /// `CNT-FR-002` (ADR-0057): one read under the store's lock; an
    /// unknown label is `Malformed`, as for `neighbors_by_relation`.
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
                                .unwrap_or_else(|| GetById::<Entity>::get(inner, id).is_some())
                        };
                        if !exists(*left) || !exists(*right) {
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
        match self.store.count_edges::<Entity>(relation) {
            Some(count) => Ok(count as u64),
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

    fn table_name(&self) -> &str {
        "entity"
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
    /// `INS-FR-006` (ADR-0046): all four fields, `aliases` as a
    /// `StrList`, validated before any write; the inserted entity is then
    /// found by label and alias through `filter_eq` on `label`; the
    /// repeat is a duplicate; a wrong-kind `aliases` is `Malformed`.
    #[test]
    fn insert_record_takes_aliases_as_a_str_list_and_indexes_them() {
        let adapter = sample_adapter();
        let id = Uuid::from_u128(77);
        let fields = vec![
            (FIELD_LABEL, ScanValue::Str("Grace Hopper".into())),
            (FIELD_KIND, ScanValue::Str("person".into())),
            (FIELD_MENTION_COUNT, ScanValue::I64(1)),
            (
                FIELD_ALIASES,
                ScanValue::StrList(vec!["Amazing Grace".into()]),
            ),
        ];
        assert_eq!(
            adapter.insert_record(id, fields.clone()),
            Ok(InsertOutcome::Inserted)
        );
        assert_eq!(adapter.get(id).unwrap(), fields);
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("amazing grace".into())),
            Ok(vec![id])
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("GRACE HOPPER".into())),
            Ok(vec![id])
        );
        assert_eq!(adapter.neighbors(id), Ok(vec![]));
        assert_eq!(
            adapter.insert_record(id, fields.clone()),
            Ok(InsertOutcome::Duplicate)
        );

        let mut wrong_aliases = fields.clone();
        wrong_aliases[3] = (FIELD_ALIASES, ScanValue::Str("Amazing Grace".into()));
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(78), wrong_aliases),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.insert_record(Uuid::from_u128(78), fields[..3].to_vec()),
            Err(ErrorCode::Malformed),
            "aliases is required, an empty list is the way to say none"
        );
        assert!(adapter.get(Uuid::from_u128(78)).is_none());
    }
    /// `LNK-FR-009` (ADR-0047): open labels through the adapter — a link
    /// under a new label lands, `list_relation_kinds` and
    /// `neighbors_by_relation` see it, the repeat is `AlreadyLinked`, and
    /// every refusal maps to its code with nothing written.
    #[test]
    fn link_records_accepts_any_valid_label_and_maps_every_refusal() {
        let adapter = sample_adapter();
        let (ada, engine) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            adapter.link_records(ada, engine, "invented_by"),
            Ok(LinkOutcome::Linked)
        );
        assert_eq!(
            adapter.link_records(engine, ada, "invented_by"),
            Ok(LinkOutcome::AlreadyLinked)
        );
        assert_eq!(
            adapter.neighbors_by_relation(engine, "invented_by"),
            Ok(vec![ada])
        );
        assert!(adapter
            .list_relation_kinds()
            .contains(&"invented_by".to_string()));
        assert_eq!(
            adapter.link_records(ada, Uuid::from_u128(99), "invented_by"),
            Err(ErrorCode::RecordNotFound)
        );
        assert_eq!(
            adapter.link_records(ada, ada, "relates_to"),
            Err(ErrorCode::Malformed)
        );
        for bad in ["", "no spaces", "../x"] {
            assert_eq!(
                adapter.link_records(ada, engine, bad),
                Err(ErrorCode::Malformed),
                "{bad:?}"
            );
        }
        assert!(!adapter
            .list_relation_kinds()
            .iter()
            .any(|k| k.contains(' ')));
    }

    /// `REP-FR-005` (ADR-0049): a replaced entity's new label and aliases
    /// resolve through `filter_eq` on `label`, the old alias no longer
    /// does, and its neighbors are untouched; an unknown id is `NotFound`.
    #[test]
    fn replace_record_moves_the_name_index_and_keeps_neighbors() {
        let adapter = sample_adapter();
        let id = Uuid::from_u128(1);
        let neighbors_before = adapter.neighbors(id).unwrap();
        assert!(!neighbors_before.is_empty());
        let edited = vec![
            (FIELD_LABEL, ScanValue::Str("Ada King".into())),
            (FIELD_KIND, ScanValue::Str("person".into())),
            (FIELD_MENTION_COUNT, ScanValue::I64(99)),
            (
                FIELD_ALIASES,
                ScanValue::StrList(vec!["Enchantress of Number".into()]),
            ),
        ];
        assert_eq!(
            adapter.replace_record(id, edited.clone()),
            Ok(ReplaceOutcome::Replaced)
        );
        assert_eq!(adapter.get(id).unwrap(), edited);
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("enchantress of number".into())),
            Ok(vec![id])
        );
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("ada lovelace".into())),
            Ok(vec![]),
            "the old label no longer resolves"
        );
        assert_eq!(adapter.neighbors(id), Ok(neighbors_before));
        assert_eq!(
            adapter.replace_record(Uuid::from_u128(4242), edited),
            Ok(ReplaceOutcome::NotFound)
        );
    }

    /// `DEL-FR-006` (ADR-0051): a deleted entity is gone from `get`, from
    /// the name index, and from its neighbors' lists; a repeat is
    /// `NotFound`.
    #[test]
    fn delete_record_removes_the_entity_its_names_and_its_edges() {
        let adapter = sample_adapter();
        let id = Uuid::from_u128(1);
        let neighbors = adapter.neighbors(id).unwrap();
        assert!(!neighbors.is_empty());
        assert_eq!(adapter.delete_record(id), Ok(DeleteOutcome::Deleted));
        assert!(adapter.get(id).is_none());
        assert_eq!(
            adapter.filter_eq(FIELD_LABEL, &ScanValue::Str("ada lovelace".into())),
            Ok(vec![])
        );
        for neighbor in neighbors {
            assert!(!adapter.neighbors(neighbor).unwrap().contains(&id));
        }
        assert_eq!(adapter.delete_record(id), Ok(DeleteOutcome::NotFound));
    }
}
