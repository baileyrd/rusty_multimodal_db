//! [`ConnectionStore`] adapter wrapping
//! [`crate::generic::production::GenericProductionStore<ReminderProductionStack>`]
//! for `Reminder` — this crate's fourth domain, and its first
//! `ConnectionStore` adapter gated by `server` alone, not `server` +
//! `research` (`RMD-FR-006`, ADR-0036): `Reminder` is real, deployable
//! capability, not reference material validating the generic schema
//! library, so unlike `server::order`/`server::employee` it needs no
//! `research` feature to reach.
//!
//! # No relation of either kind
//!
//! `Reminder` has no `ChildOf`/`SymmetricRelation` impl at all — the
//! one combination no existing adapter has (`server::dog`: neighbors
//! only; `server::order`: parent/children only; `server::employee`:
//! both). `parent`/`children`/`neighbors` all report
//! `ErrorCode::Unsupported` unconditionally, the identical shape
//! `server::dog`'s own missing half already uses.
//!
//! # `status`, not a plain number, is the durably-mutable field
//!
//! Every existing domain's `ScannableField` has been a plain number
//! (`Amount`/`SalaryCents`); `Reminder`'s is `status`, an enum
//! discriminant (`RMD-FR-002`). `scan_field`/`update_field` validate
//! the incoming `u32` against [`crate::generic::reminder::status_from_u32`]
//! before ever reaching the store — `ErrorCode::Malformed` on an
//! unrecognized value, nothing applied — the one genuinely new
//! validation path this domain needs beyond the mechanical repetition
//! every existing adapter already has.

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::generic::production::GenericProductionStore;
use crate::generic::query::{GetById, UpdateField};
use crate::generic::reminder::{
    status_from_u32, status_to_u32, DueAtField, Reminder, ReminderProductionStack, StatusField,
};
use std::path::Path;

pub const FIELD_TITLE: FieldRef = 0;
pub const FIELD_DUE_AT: FieldRef = 1;
pub const FIELD_STATUS: FieldRef = 2;

pub struct ReminderConnectionStore {
    store: GenericProductionStore<ReminderProductionStack>,
    /// `JRN-FR-001` (ADR-0025) — see `DogConnectionStore::with_journal`.
    journal: Option<CommitGroup>,
}

impl ReminderConnectionStore {
    pub fn new(store: GenericProductionStore<ReminderProductionStack>) -> Self {
        Self {
            store,
            journal: None,
        }
    }

    /// The crash-atomic variant — see `DogConnectionStore::with_journal`
    /// for the contract; identical here.
    pub fn with_journal(
        store: GenericProductionStore<ReminderProductionStack>,
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

    /// Same validate-then-apply shape `server::dog`'s own uses —
    /// `Reminder`'s only mutable field over this protocol is `status`,
    /// validated against its four known discriminants
    /// (`status_from_u32`) before any write, the same "reject before
    /// any write" posture every existing domain's `validate_batch`
    /// already guarantees, now covering an enum-typed update for the
    /// first time.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_STATUS, ScanValue::U32(raw)) => {
                    if status_from_u32(*raw).is_none() {
                        return Err((i, ErrorCode::Malformed));
                    }
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_STATUS, _) => return Err((i, ErrorCode::Malformed)),
                (FIELD_TITLE | FIELD_DUE_AT, _) => return Err((i, ErrorCode::Unsupported)),
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    fn apply_batch(
        inner: &mut ReminderProductionStack,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::U32(status) = op.value {
                UpdateField::<Reminder, StatusField>::update(inner, op.id, status)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }

    /// `ISO-FR-002`/`ISO-FR-006` — see `DogConnectionStore::check_read_set`
    /// for the full contract; identical shape here.
    fn check_read_set(
        reads: &[(RecordId, FieldRef, ScanValue)],
        get: impl Fn(RecordId) -> Option<Reminder>,
    ) -> Result<(), (usize, ErrorCode)> {
        for (id, field, value) in reads {
            let current = get(*id).and_then(|reminder| match *field {
                FIELD_TITLE => Some(ScanValue::Str(reminder.title)),
                FIELD_DUE_AT => Some(ScanValue::I64(reminder.due_at_unix_ms)),
                FIELD_STATUS => Some(ScanValue::U32(status_to_u32(reminder.status))),
                _ => None,
            });
            if current.as_ref() != Some(value) {
                return Err((0, ErrorCode::Conflict));
            }
        }
        Ok(())
    }
}

impl ConnectionStore for ReminderConnectionStore {
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        self.store.get::<Reminder>(id).map(|reminder| {
            vec![
                (FIELD_TITLE, ScanValue::Str(reminder.title)),
                (FIELD_DUE_AT, ScanValue::I64(reminder.due_at_unix_ms)),
                (FIELD_STATUS, ScanValue::U32(status_to_u32(reminder.status))),
            ]
        })
    }

    /// `SQL-FR-004`/`SQL-FR-005` (ADR-0034): every id from `all_ids`,
    /// each mapped through this adapter's own `get`.
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
        self.store
            .all_ids::<Reminder>()
            .into_iter()
            .filter_map(|id| self.get(id).map(|fields| (id, fields)))
            .collect()
    }

    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        match (field, value) {
            (FIELD_DUE_AT, ScanValue::I64(due_at)) => {
                Ok(self.store.filter_eq::<Reminder, DueAtField>(due_at))
            }
            (FIELD_DUE_AT, _) => Err(ErrorCode::Malformed),
            (FIELD_STATUS | FIELD_TITLE, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_STATUS => Ok(self
                .store
                .scan::<Reminder, StatusField>()
                .into_iter()
                .map(ScanValue::U32)
                .collect()),
            FIELD_TITLE | FIELD_DUE_AT => Err(ErrorCode::Unsupported),
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
            (FIELD_STATUS, ScanValue::U32(raw)) => {
                if status_from_u32(raw).is_none() {
                    return Err(ErrorCode::Malformed);
                }
                match self.store.update::<Reminder, StatusField>(id, raw) {
                    Ok(()) => Ok(true),
                    Err(_not_found) => Ok(false),
                }
            }
            (FIELD_STATUS, _) => Err(ErrorCode::Malformed),
            (FIELD_TITLE | FIELD_DUE_AT, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    /// `RMD-FR-005`: `Reminder` has no relation of either kind.
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
            self.store.get::<Reminder>(id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn describe(&self) -> DomainSchema {
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_TITLE,
                    name: "title".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_DUE_AT,
                    name: "due_at_unix_ms".into(),
                    value_kind: ValueKind::I64,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_STATUS,
                    name: "status".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
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
                Self::validate_batch(updates, |id| GetById::<Reminder>::get(inner, id).is_some())?;
                Self::check_read_set(read_set, |id| GetById::<Reminder>::get(inner, id))?;
                Self::apply_batch(inner, updates)
            }),
            Some(journal) => {
                Self::validate_batch(updates, |id| self.store.get::<Reminder>(id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
                            Self::check_read_set(read_set, |id| {
                                GetById::<Reminder>::get(inner, id)
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
    use crate::generic::reminder::{create_reminder_production_stack, ReminderStatus};
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> ReminderConnectionStore {
        let dir = fresh_temp_dir("server_reminder_adapter").unwrap();
        let path = dir.join("reminders.mmap");
        let reminders = vec![
            Reminder {
                id: Uuid::from_u128(1),
                title: "Pay rent".into(),
                due_at_unix_ms: 1_000,
                status: ReminderStatus::Pending,
            },
            Reminder {
                id: Uuid::from_u128(2),
                title: "Call dentist".into(),
                due_at_unix_ms: 2_000,
                status: ReminderStatus::Snoozed,
            },
        ];
        let stack = create_reminder_production_stack(reminders, &path).unwrap();
        ReminderConnectionStore::new(GenericProductionStore::new(stack))
    }

    #[test]
    fn get_returns_every_field() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap(),
            vec![
                (FIELD_TITLE, ScanValue::Str("Pay rent".into())),
                (FIELD_DUE_AT, ScanValue::I64(1_000)),
                (FIELD_STATUS, ScanValue::U32(0)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn filter_eq_by_due_at_and_unsupported_fields() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_DUE_AT, &ScanValue::I64(2_000)),
            Ok(vec![Uuid::from_u128(2)])
        );
        assert!(adapter
            .filter_eq(FIELD_DUE_AT, &ScanValue::I64(9_999))
            .unwrap()
            .is_empty());
        assert_eq!(
            adapter.filter_eq(FIELD_STATUS, &ScanValue::U32(0)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(FIELD_TITLE, &ScanValue::Str("x".into())),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.filter_eq(99, &ScanValue::I64(0)),
            Err(ErrorCode::UnknownField)
        );
    }

    #[test]
    fn scan_and_update_status_only_with_discriminant_validation() {
        let adapter = sample_adapter();
        let mut statuses = adapter.scan_field(FIELD_STATUS).unwrap();
        statuses.sort_by_key(|v| match v {
            ScanValue::U32(n) => *n,
            _ => 0,
        });
        assert_eq!(
            statuses,
            vec![ScanValue::U32(0), ScanValue::U32(2)],
            "Pending, Snoozed"
        );
        assert_eq!(
            adapter.scan_field(FIELD_DUE_AT),
            Err(ErrorCode::Unsupported)
        );

        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_STATUS, ScanValue::U32(1)),
            Ok(true),
            "Done"
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[2],
            (FIELD_STATUS, ScanValue::U32(1))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_STATUS, ScanValue::U32(9)),
            Err(ErrorCode::Malformed),
            "an unrecognized discriminant is rejected"
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_STATUS, ScanValue::U32(0)),
            Ok(false)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_DUE_AT, ScanValue::I64(0)),
            Err(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn describe_names_all_three_fields_and_reports_no_relations() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 3);
        let status = schema.fields.iter().find(|f| f.name == "status").unwrap();
        assert!(
            status.capabilities.scan
                && status.capabilities.update
                && !status.capabilities.filter_eq
        );
        let due_at = schema
            .fields
            .iter()
            .find(|f| f.name == "due_at_unix_ms")
            .unwrap();
        assert!(
            due_at.capabilities.filter_eq
                && !due_at.capabilities.scan
                && !due_at.capabilities.update
        );
        let title = schema.fields.iter().find(|f| f.name == "title").unwrap();
        assert!(
            !title.capabilities.filter_eq && !title.capabilities.scan && !title.capabilities.update
        );
        assert!(!schema.relations.parent_children);
        assert!(!schema.relations.neighbors);
    }

    #[test]
    fn every_relation_request_is_unsupported() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.parent(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.children(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.neighbors(Uuid::from_u128(1)),
            Err(ErrorCode::Unsupported)
        );
    }
}
