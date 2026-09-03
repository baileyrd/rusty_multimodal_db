//! [`ConnectionStore`] adapter wrapping [`crate::production::ProductionStore`]
//! for `Dog` — the front-door validation domain (real symmetric relation,
//! `littermate_of`, via `Neighbors`; no directed relation, so `Parent`/
//! `Children` are unsupported here — see [`super::order`] for the
//! complementary case).

use super::journal::{CheckpointFlush, CommitError, CommitGroup, JournalError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldCapabilities, FieldDescriptor, FieldRef, ParentLookup, RecordId,
    RelationCapabilities, ScanValue, TransactionOp, ValueKind,
};
use super::ConnectionStore;
use crate::concurrency::{ConcurrencyError, ConcurrentStore};
use crate::production::TransactionalStore;
use crate::store::{DogStore, StoreError};
use std::path::Path;

/// `Dog::breed` — read-only over this protocol: no `ScannableField`/
/// `UpdateField` exists for it in-process either (only `age` is mutable,
/// via `update_age`).
pub const FIELD_BREED: FieldRef = 0;
/// `Dog::age` — the one mutable, scannable field.
pub const FIELD_AGE: FieldRef = 1;

/// Wraps any `S: DogStore + ConcurrentStore` (in practice,
/// [`crate::production::ProductionStore`], the only type implementing
/// both). Uses `DogStore`'s `&self` methods (`get`/`scan_ages`/
/// `same_breed`/`neighbors` all take `&self` already) plus
/// `ConcurrentStore::update_age` (the one `&self`-shaped mutator) — never
/// `DogStore::update_age`, which needs `&mut self` and so can't be called
/// through the `Arc<S>` every connection thread shares.
pub struct DogConnectionStore<S> {
    store: S,
    /// `JRN-FR-001` (ADR-0025): the batch journal, when this adapter was
    /// built with [`DogConnectionStore::with_journal`] — behind the
    /// group-commit discipline of `GRP-FR-001`–`004` (ADR-0026), so its
    /// `fsync` never runs inside `with_exclusive`.
    journal: Option<CommitGroup>,
}

impl<S> DogConnectionStore<S> {
    /// An adapter without a batch journal — `apply_transaction` is atomic
    /// with respect to concurrent access only, exactly as at v0.7.0
    /// (`TXN-FR-007`'s named gap stands).
    pub fn new(store: S) -> Self {
        Self {
            store,
            journal: None,
        }
    }
}

impl<S: DogStore + ConcurrentStore + TransactionalStore + Send + Sync> DogConnectionStore<S>
where
    S::Exclusive: CheckpointFlush,
{
    /// An adapter whose batches are crash-atomic (`JRN-FR-001`–`005`,
    /// ADR-0025, `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part
    /// B): every `apply_transaction` batch is appended to the journal at
    /// `journal_path` and `fsync`'d *before* its first write, so a crash
    /// at any instant leaves a batch answered `Ok` either fully applied
    /// or fully replayed by the next `with_journal` open — never partial.
    /// Opening replays every complete entry (an idempotent overwrite per
    /// operation), flushes the store, and truncates the journal; a torn
    /// tail is dropped. Costs one `fsync` per batch on a lone
    /// connection, and one per *group* of concurrent batches (`GRP-FR-002`,
    /// ADR-0026) — the `fsync` runs outside the store's exclusive section,
    /// batches apply in journal order, and a checkpoint waits for a
    /// quiescent moment; single `UpdateField`s are not journaled
    /// (`JRN-FR-007`). Opening the same
    /// files *without* a journal after a crash forgoes the replay — the
    /// one way to lose the guarantee, so open the same way every time.
    pub fn with_journal(store: S, journal_path: &Path) -> Result<Self, JournalError> {
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

    /// Every operation's precondition, checked before any write —
    /// `Dog`'s only mutable field is `age`; existence is the only thing
    /// that can vary at runtime (field/type validity is a pure match
    /// against the request itself, same as `update_field`'s own arms).
    /// Safe under one continuously held lock *and* — on the journaled
    /// path, `GRP-FR-001` — outside any lock at all: this crate never
    /// deletes a record at runtime, so an id `exists` says yes to stays
    /// valid for the apply phase, and field/type validity is a pure
    /// function of the request — see
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`'s own "no runtime
    /// deletion" invariant.
    fn validate_batch(
        updates: &[TransactionOp],
        exists: impl Fn(RecordId) -> bool,
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            match (op.field, &op.value) {
                (FIELD_AGE, ScanValue::U32(_)) => {
                    if !exists(op.id) {
                        return Err((i, ErrorCode::RecordNotFound));
                    }
                }
                (FIELD_AGE, _) => return Err((i, ErrorCode::Malformed)),
                (FIELD_BREED, _) => return Err((i, ErrorCode::Unsupported)),
                _ => return Err((i, ErrorCode::UnknownField)),
            }
        }
        Ok(())
    }

    /// Apply every operation in order — idempotent overwrites, so this
    /// is also the journal's replay step. Reports the first operation
    /// that fails (a missing id), which a validated batch never does.
    fn apply_batch(
        inner: &mut S::Exclusive,
        updates: &[TransactionOp],
    ) -> Result<(), (usize, ErrorCode)> {
        for (i, op) in updates.iter().enumerate() {
            if let ScanValue::U32(age) = op.value {
                DogStore::update_age(inner, op.id, age)
                    .map_err(|_| (i, ErrorCode::RecordNotFound))?;
            }
        }
        Ok(())
    }
}

impl<S: DogStore + ConcurrentStore + TransactionalStore + Send + Sync> ConnectionStore
    for DogConnectionStore<S>
where
    S::Exclusive: CheckpointFlush,
{
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
        DogStore::get(&self.store, id).map(|record| {
            vec![
                (FIELD_BREED, ScanValue::Str(record.breed)),
                (FIELD_AGE, ScanValue::U32(record.age)),
            ]
        })
    }

    fn filter_eq(&self, _field: FieldRef, _value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode> {
        // No IndexedField-shaped "give me every record equal to this
        // value" exists for Dog in-process — `same_breed` filters by
        // *another record's id*, a different shape this protocol's
        // FilterEq (by value) doesn't represent. Named as an out-of-scope
        // gap for v1 (see docs/PROJECT-STATUS.md), not silently faked by
        // reinterpreting FilterEq as something it isn't.
        Err(ErrorCode::Unsupported)
    }

    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
        match field {
            FIELD_AGE => Ok(DogStore::scan_ages(&self.store)
                .into_iter()
                .map(ScanValue::U32)
                .collect()),
            FIELD_BREED => Err(ErrorCode::Unsupported),
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
            (FIELD_AGE, ScanValue::U32(age)) => {
                match ConcurrentStore::update_age(&self.store, id, age) {
                    Ok(()) => Ok(true),
                    Err(ConcurrencyError::Store(StoreError::NotFound(_))) => Ok(false),
                    // A real I/O/durability failure, not a missing record —
                    // surfaced as a server error rather than misreported as
                    // NotFound.
                    Err(_) => Err(ErrorCode::Malformed),
                }
            }
            (FIELD_AGE, _) => Err(ErrorCode::Malformed),
            (FIELD_BREED, _) => Err(ErrorCode::Unsupported),
            _ => Err(ErrorCode::UnknownField),
        }
    }

    fn parent(&self, _id: RecordId) -> Result<ParentLookup, ErrorCode> {
        // Dog has no ChildOf-shaped directed relation.
        Err(ErrorCode::Unsupported)
    }

    fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Err(ErrorCode::Unsupported)
    }

    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
        Ok(DogStore::neighbors(&self.store, id))
    }

    /// `STV-FR-002`: `validate_batch` on this one operation, with the
    /// same per-call existence read the journaled path uses.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode> {
        Self::validate_batch(std::slice::from_ref(op), |id| {
            DogStore::get(&self.store, id).is_some()
        })
        .map_err(|(_, code)| code)
    }

    fn describe(&self) -> DomainSchema {
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: FIELD_BREED,
                    name: "breed".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
                FieldDescriptor {
                    tag: FIELD_AGE,
                    name: "age".into(),
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
                neighbors: true,
            },
        }
    }

    fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)> {
        match &self.journal {
            // The v0.7.0 path, unchanged: validate then apply under one
            // exclusive section.
            None => self.store.with_exclusive(|inner| {
                Self::validate_batch(updates, |id| DogStore::get(inner, id).is_some())?;
                Self::apply_batch(inner, updates)
            }),
            // `GRP-FR-001`: validate with per-call reads, then append,
            // group-`fsync`, and take the apply turn *before* the
            // exclusive section — which now holds only the writes and, at
            // a quiescent checkpoint, the store flush. `JRN-FR-002` still
            // holds by step order: durable before the first write; a
            // journal or `fsync` failure is the batch's failure with
            // nothing applied (`GRP-FR-005`).
            Some(journal) => {
                Self::validate_batch(updates, |id| DogStore::get(&self.store, id).is_some())?;
                journal
                    .commit(updates, |turn| {
                        self.store.with_exclusive(|inner| {
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
    use crate::production::ProductionStore;
    use crate::record::DogRecord;
    use crate::server::journal::BatchJournal;
    use crate::test_support::fresh_temp_dir;
    use uuid::Uuid;

    fn sample_adapter() -> DogConnectionStore<ProductionStore> {
        let dir = fresh_temp_dir("server_dog_adapter").unwrap();
        let path = dir.join("dogs.mmap");
        let records = vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        ];
        let edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
        let store = ProductionStore::create(records, edges, &path).unwrap();
        DogConnectionStore::new(store)
    }

    #[test]
    fn get_returns_breed_and_age() {
        let adapter = sample_adapter();
        let fields = adapter.get(Uuid::from_u128(1)).unwrap();
        assert_eq!(
            fields,
            vec![
                (FIELD_BREED, ScanValue::Str("labrador".into())),
                (FIELD_AGE, ScanValue::U32(3)),
            ]
        );
        assert!(adapter.get(Uuid::from_u128(99)).is_none());
    }

    #[test]
    fn update_field_updates_age_and_reports_missing_ids() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_AGE, ScanValue::U32(9)),
            Ok(true)
        );
        assert_eq!(
            adapter.get(Uuid::from_u128(1)).unwrap()[1],
            (FIELD_AGE, ScanValue::U32(9))
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(99), FIELD_AGE, ScanValue::U32(1)),
            Ok(false)
        );
        assert_eq!(
            adapter.update_field(Uuid::from_u128(1), FIELD_AGE, ScanValue::Bool(true)),
            Err(ErrorCode::Malformed)
        );
    }

    #[test]
    fn neighbors_reflects_the_littermate_edge() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.neighbors(Uuid::from_u128(1)),
            Ok(vec![Uuid::from_u128(2)])
        );
    }

    #[test]
    fn describe_names_both_fields_and_reports_neighbors_only() {
        let adapter = sample_adapter();
        let schema = adapter.describe();
        assert_eq!(schema.fields.len(), 2);
        assert!(schema.fields.iter().any(|f| f.name == "breed"
            && f.value_kind == ValueKind::Str
            && !f.capabilities.scan
            && !f.capabilities.update));
        assert!(schema.fields.iter().any(|f| f.name == "age"
            && f.value_kind == ValueKind::U32
            && f.capabilities.scan
            && f.capabilities.update));
        assert!(schema.relations.neighbors);
        assert!(!schema.relations.parent_children);
    }

    #[test]
    fn filter_eq_and_parent_and_children_are_unsupported() {
        let adapter = sample_adapter();
        assert_eq!(
            adapter.filter_eq(FIELD_BREED, &ScanValue::Str("labrador".into())),
            Err(ErrorCode::Unsupported)
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

    fn op(id: u128, age: u32) -> TransactionOp {
        TransactionOp {
            id: Uuid::from_u128(id),
            field: FIELD_AGE,
            value: ScanValue::U32(age),
        }
    }

    fn age_of(adapter: &DogConnectionStore<ProductionStore>, id: u128) -> u32 {
        match adapter
            .get(Uuid::from_u128(id))
            .unwrap()
            .into_iter()
            .find(|(field, _)| *field == FIELD_AGE)
        {
            Some((_, ScanValue::U32(age))) => age,
            other => panic!("expected an age, got {other:?}"),
        }
    }

    /// `JRN-FR-002`/`JRN-FR-003`/`JRN-FR-005` (design criteria 2–3, the
    /// file-copy replay pair): a journaled batch is on disk after `Ok` and
    /// a rejected one is not; replaying the journal onto *pre-batch* files
    /// yields the full post-batch state, and replaying it onto
    /// *post-batch* files is a no-op — together, replay is correct from
    /// every intermediate state, since each slot is independent.
    #[test]
    fn with_journal_replays_onto_pre_batch_files_and_is_a_no_op_on_post_batch_files() {
        let dir = fresh_temp_dir("server_dog_journal").unwrap();
        let records = || {
            vec![
                DogRecord::new(Uuid::from_u128(1), "labrador", 3),
                DogRecord::new(Uuid::from_u128(2), "labrador", 5),
            ]
        };
        let journal = dir.join("txn.journal");
        let header_len = 12;

        // A journaled adapter applies a batch; the journal holds it.
        let path_a = dir.join("a.mmap");
        let adapter = DogConnectionStore::with_journal(
            ProductionStore::create(records(), Vec::new(), &path_a).unwrap(),
            &journal,
        )
        .unwrap();
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), header_len);
        adapter.apply_transaction(&[op(1, 30), op(2, 40)]).unwrap();
        let after_ok = std::fs::metadata(&journal).unwrap().len();
        assert!(after_ok > header_len);
        assert_eq!(age_of(&adapter, 1), 30);
        // A batch that fails validation journals nothing.
        assert_eq!(
            adapter.apply_transaction(&[op(1, 31), op(99, 1)]),
            Err((1, ErrorCode::RecordNotFound))
        );
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), after_ok);
        assert_eq!(age_of(&adapter, 1), 30);
        drop(adapter);
        let journal_copy = dir.join("txn.journal.copy");
        std::fs::copy(&journal, &journal_copy).unwrap();

        // (i) Pre-batch files + the journal: the whole batch appears, and
        // the journal is checkpointed away after the replay.
        let path_b = dir.join("b.mmap");
        let replayed = DogConnectionStore::with_journal(
            ProductionStore::create(records(), Vec::new(), &path_b).unwrap(),
            &journal,
        )
        .unwrap();
        assert_eq!(age_of(&replayed, 1), 30);
        assert_eq!(age_of(&replayed, 2), 40);
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), header_len);

        // (ii) Post-batch files + the same journal: a no-op, no error.
        let again = DogConnectionStore::with_journal(
            ProductionStore::open(records(), Vec::new(), &path_a).unwrap(),
            &journal_copy,
        )
        .unwrap();
        assert_eq!(age_of(&again, 1), 30);
        assert_eq!(age_of(&again, 2), 40);
        assert_eq!(std::fs::metadata(&journal_copy).unwrap().len(), header_len);

        // A journal belonging to other files is refused, never applied
        // blindly: an id the store does not have is a replay error.
        let foreign = dir.join("foreign.journal");
        {
            let (mut j, _) = BatchJournal::open(&foreign).unwrap();
            j.append(&[op(7, 1)]).unwrap();
        }
        let path_c = dir.join("c.mmap");
        assert!(matches!(
            DogConnectionStore::with_journal(
                ProductionStore::create(records(), Vec::new(), &path_c).unwrap(),
                &foreign,
            )
            .map(|_| ()),
            Err(JournalError::Replay {
                batch: 0,
                index: 0,
                code: ErrorCode::RecordNotFound
            })
        ));
    }

    /// `STV-FR-002`: `validate_op` is `validate_batch` on one operation —
    /// the same codes `apply_transaction` reports by index, with no write.
    #[test]
    fn validate_op_reports_exactly_what_commit_would() {
        let adapter = sample_adapter();
        assert_eq!(adapter.validate_op(&op(1, 30)), Ok(()));
        assert_eq!(
            adapter.validate_op(&op(99, 1)),
            Err(ErrorCode::RecordNotFound)
        );
        assert_eq!(
            adapter.validate_op(&TransactionOp {
                id: Uuid::from_u128(1),
                field: FIELD_BREED,
                value: ScanValue::Str("poodle".into()),
            }),
            Err(ErrorCode::Unsupported)
        );
        assert_eq!(
            adapter.validate_op(&TransactionOp {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::Str("old".into()),
            }),
            Err(ErrorCode::Malformed)
        );
        assert_eq!(
            adapter.validate_op(&TransactionOp {
                id: Uuid::from_u128(1),
                field: 9,
                value: ScanValue::U32(1),
            }),
            Err(ErrorCode::UnknownField)
        );
        assert_eq!(age_of(&adapter, 1), 3, "validation never writes");
    }

    /// `GRP-FR-001` (ADR-0026, design criterion 1): the journal's `fsync`
    /// never runs under the store's write lock — while a leader is held
    /// before its sync, another thread's read and single write on the
    /// same adapter both complete.
    #[test]
    fn the_fsync_never_holds_the_store_lock() {
        use std::sync::{mpsc, Arc, Mutex};
        let dir = fresh_temp_dir("server_dog_journal_lock").unwrap();
        let records = vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        ];
        let adapter = Arc::new(
            DogConnectionStore::with_journal(
                ProductionStore::create(records, Vec::new(), &dir.join("dogs.mmap")).unwrap(),
                &dir.join("txn.journal"),
            )
            .unwrap(),
        );
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        adapter
            .journal
            .as_ref()
            .unwrap()
            .set_sync_hook(Box::new(move || {
                let _ = entered_tx.send(());
                if let Ok(rx) = release_rx.lock() {
                    let _ = rx.recv();
                }
                Ok(())
            }));
        let leader = {
            let adapter = Arc::clone(&adapter);
            std::thread::spawn(move || adapter.apply_transaction(&[op(1, 30)]))
        };
        entered_rx.recv().unwrap();
        // The leader is inside its sync step: the store must be free.
        assert_eq!(age_of(&adapter, 1), 3, "nothing applied before the sync");
        assert_eq!(
            adapter.update_field(Uuid::from_u128(2), FIELD_AGE, ScanValue::U32(7)),
            Ok(true)
        );
        release_tx.send(()).unwrap();
        leader.join().unwrap().unwrap();
        assert_eq!(age_of(&adapter, 1), 30);
        assert_eq!(age_of(&adapter, 2), 7);
    }

    /// `JRN-FR-004` (design criterion 4): past `JOURNAL_CHECKPOINT_BYTES`
    /// the adapter flushes the store and truncates the journal inside the
    /// same exclusive section; a later open replays nothing.
    #[test]
    fn a_journaled_adapter_checkpoints_past_the_threshold() {
        use crate::server::journal::JOURNAL_CHECKPOINT_BYTES;
        let dir = fresh_temp_dir("server_dog_journal_checkpoint").unwrap();
        let records = vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        ];
        let journal = dir.join("txn.journal");
        let path = dir.join("dogs.mmap");
        let adapter = DogConnectionStore::with_journal(
            ProductionStore::create(records.clone(), Vec::new(), &path).unwrap(),
            &journal,
        )
        .unwrap();
        let big: Vec<TransactionOp> = (0..4096)
            .map(|i| op(1 + (i % 2) as u128, i as u32))
            .collect();
        // The checkpoint runs inside the very call whose append crosses
        // the threshold, so the file is never *observed* above it: what is
        // observable is the journal growing by one entry per batch and
        // then dropping back to its header once the next entry would have
        // put it past `JOURNAL_CHECKPOINT_BYTES`.
        let mut seen = Vec::new();
        for _ in 0..16 {
            adapter.apply_transaction(&big).unwrap();
            seen.push(std::fs::metadata(&journal).unwrap().len());
        }
        let entry = seen[0] - 12;
        let peak = *seen.iter().max().unwrap();
        assert!(
            peak + entry > JOURNAL_CHECKPOINT_BYTES,
            "never approached the threshold: {seen:?}"
        );
        assert!(
            peak <= JOURNAL_CHECKPOINT_BYTES,
            "observed above the threshold: {seen:?}"
        );
        assert!(
            seen.windows(2).any(|w| w[0] > w[1] && w[1] == 12),
            "the journal never checkpointed back to its header: {seen:?}"
        );
        assert_eq!(age_of(&adapter, 1), 4094);
        assert_eq!(age_of(&adapter, 2), 4095);
        drop(adapter);
        let reopened = DogConnectionStore::with_journal(
            ProductionStore::open(records, Vec::new(), &path).unwrap(),
            &journal,
        )
        .unwrap();
        assert_eq!(age_of(&reopened, 1), 4094);
    }
}
