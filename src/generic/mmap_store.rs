//! The generic, durable storage core — the generic equivalent of
//! `src/durability/mmap_store.rs`'s `MmapAgeStore`, and the piece that
//! makes [`super::production::GenericProductionStore`] real durability
//! rather than a purely in-memory composed stack wrapped in a `RwLock`.
//!
//! # Hand-fused, like `MmapAgeStore`, not built from `BaseStore`/`Indexed`/`Scanned`
//!
//! `MmapAgeStore` doesn't wrap `CanonicalCachedStore` — it rebuilds the
//! same canonical-map/breed-index/position-index architecture directly,
//! with the mutable field backed by `MmapMut` instead of a plain `Vec`
//! (see `src/production.rs`'s own module docs for the full account of why:
//! `CanonicalCachedStore`'s private fields aren't reusable across
//! durability variants without either duplicating them or breaking
//! encapsulation). [`GenericMmapStore`] follows the identical precedent,
//! generically: it does not wrap [`super::store::BaseStore`]/
//! [`super::store::Indexed`]/[`super::store::Scanned`] — it rebuilds their
//! combined shape (one `IndexedField` + one `ScannableField`) directly,
//! with the scannable field's cache backed by `MmapMut`. Composable
//! capability layers (`Symmetric`, `Reversed`) can still be stacked *on
//! top* of a `GenericMmapStore` exactly as they stack on `BaseStore` —
//! their forwarding impls are generic over the inner store type, so they
//! don't care whether what's underneath is in-memory or mmap-backed. This
//! is what lets [`super::order_customer::OrderProductionStack`] reuse
//! `Reversed` completely unchanged.
//!
//! # Scoped to exactly one `IndexedField` and one `ScannableField`, by design
//!
//! `MmapAgeStore` only ever durably tracks one mutable field (`age`) —
//! `docs/decisions/ADR-0009-generic-schema-design-proposal.md` §4.2 is
//! explicit that generalizing mmap durability to more than one mutable
//! field is a real redesign (a string-heap/fixed-layout problem), not an
//! incremental extension, and out of scope for this round. `GenericMmapStore`
//! keeps that same one-durable-field scope, generically: it is parameterized
//! over exactly one `IndexedField` marker (mirroring `breed_index`,
//! immutable after construction) and exactly one `ScannableField` marker
//! (mirroring `age`, the one mutable, mmap-backed field). A domain that
//! wants more than one mutable durable field needs the redesign ADR-0009
//! already flagged as unscoped follow-up work — not attempted here.
//!
//! # A finding from wiring this up: write-through consistency needed a new trait method
//!
//! Building this surfaced something the in-memory spikes never exercised:
//! [`GetById::get`] has to return a record whose scannable field reflects
//! the *latest* `UpdateField::update` write, the same write-through
//! consistency every hand-written backend in this crate already has
//! (`CanonicalCachedStore::update_age` mutates both its canonical record
//! and its cache). The original design's `ScannableField` trait had no way
//! to write a new value back into a record it didn't already own the
//! layout of — `set_scannable_value` (`traits.rs`) was added specifically
//! to close this gap for the durable path. It is **not** yet threaded
//! through the in-memory `Scanned`/`BaseStore` composition (`store.rs`) —
//! see `traits.rs`'s doc comment on `set_scannable_value` for why that's a
//! separate, larger piece of work (the same O(N²) marker-pair problem
//! `forward_scannable_pairs!` already solves for `ScanField`/`UpdateField`
//! would need solving again for a record-mutating capability), not
//! attempted in this round.
//!
//! # Persisted slots are keyed by record identity, not array position
//!
//! **Fixed in a follow-up round** — the schema-evolution diagnosis that
//! motivated it found the real fragility in the original design: each
//! persisted slot held only the scannable value's raw bytes, addressed by
//! `position * BYTE_WIDTH`, where `position` was whatever index the
//! record happened to occupy in the caller-supplied `records: Vec<R>` *at
//! that specific `create`/`open` call* — nothing in the file itself
//! recorded which record a value belonged to. If the caller ever
//! supplied `records` in a different order between `create` and a later
//! `open` (a real possibility this crate's own convention invites,
//! externally-supplied `records` being rebuilt fresh every call — see
//! `crate::durability`'s own module docs), position N's persisted value
//! silently got attributed to whatever record now sat at position N: no
//! error, no panic, just wrong data under a real id.
//!
//! Each slot now holds `(id, value)`, both fixed-width
//! ([`MmapFieldValue`] extended to cover [`uuid::Uuid`] for exactly this
//! purpose): `[R::Id::BYTE_WIDTH bytes][R::ScanValue::BYTE_WIDTH bytes]`,
//! contiguous. [`GenericMmapStore::open`] reads every persisted `(id,
//! value)` pair up front and reconciles it against the caller-supplied
//! `records` **by id**, not position — reordering `records` between calls
//! now has no effect on which value a given id reads, which is exactly
//! the bug this closes. A `HashMap<Uuid, Value>`-shaped index built from
//! the file, same shape as every canonical-store index this project has
//! built from the start, not a novel idea — just finally applied to this
//! one path.
//!
//! ## Explicit behavior for the two mismatch cases
//!
//! The invariant that must hold, unconditionally: **a persisted value is
//! never attributed to an id other than the one it was written under.**
//! Given that, two mismatches between "what's in the file" and "what
//! `records` currently says exists" are possible, and each is handled
//! deliberately, not by accident:
//!
//! - **A persisted id has no matching record in the caller's current
//!   `records`** (stale — removed since the last write that included
//!   it). Its slot is simply never referenced: not added to
//!   [`GetById`]/[`FilterEq`]/[`ScanField`]'s visible state, which are
//!   all built from `records` the same way they always were. The bytes
//!   physically remain in the file (this round doesn't add compaction —
//!   a real, stated cost, not a silent one: the file can only grow, never
//!   shrink, across repeated `open` calls that omit previously-known
//!   ids), but are otherwise inert.
//! - **A record in the caller's current `records` has no persisted
//!   entry** (new — added since the last write). It's treated exactly
//!   the way [`GenericMmapStore::create`] treats every record: seeded
//!   from that record's own [`ScannableField::scannable_value`], and a
//!   new slot is appended to the file for it (growing the file, unmapping
//!   and remapping as needed) so it's durable from this point forward.
//!
//! Both are exercised directly in this module's tests, alongside a
//! reopen-with-reordered-records regression test confirming the original
//! silent-misattribution bug is gone.
//!
//! ## `scan` had to change too, for the same reason
//!
//! [`ScanField::scan`]'s old bulk `chunks_exact` read walked every byte
//! in the file — safe under the old design, where the file's size and
//! `records.len()` were always identical by construction. Once a stale
//! record's slot can outlive it (see above), that's no longer true: a
//! blind full-file scan would leak a removed record's value into
//! [`ScanField::scan`]'s result. `scan` now iterates only the positions
//! [`GenericMmapStore::position_index`] currently maps to a live id,
//! keeping the original bulk `chunks_exact` fast path when every slot in
//! the file is still live (the common case — no record has ever been
//! dropped between an `open` and the `records` now supplied), falling
//! back to per-position reads, sorted for locality, only when it isn't. A
//! real, measured cost of the fix in the general case; see this round's
//! own report for the numbers.

use super::mmap_field::MmapFieldValue;
use super::query::{FilterEq, GetById, ScanField, UpdateField};
use super::store::Flush;
use super::traits::{IndexedField, Record, ScannableField};
use super::NotFound;
use crate::durability::DurabilityError;
use memmap2::MmapMut;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// The generic, durable storage core: owns every record, one equality
/// index (`IndexMarker`), and one mmap-backed scannable field
/// (`ScanMarker`). See module docs for why it's hand-fused, not composed,
/// scoped to exactly one field of each kind, and why each persisted slot
/// now carries its own record id rather than being addressed by array
/// position alone.
pub struct GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
    /// `id` -> that id's *current* slot position in `mmap` — built by
    /// matching persisted ids against `records`, not by array index. See
    /// module docs for the two mismatch cases this reconciliation has to
    /// decide between.
    position_index: HashMap<R::Id, usize>,
    mmap: MmapMut,
    #[allow(dead_code)] // kept for symmetry with MmapAgeStore's Self; not read again
    path: PathBuf,
    _marker: PhantomData<(IndexMarker, ScanMarker)>,
}

/// The caller-derived, file-independent pieces of [`GenericMmapStore`]'s
/// state — everything built purely from the `records: Vec<R>` argument,
/// with no reference to what (if anything) is already on disk. Factored
/// into its own struct (rather than a tuple) purely for readability at
/// the `create`/`open` call sites, mirroring
/// `src/durability/mmap_store.rs`'s own `Indexes` struct. Deliberately
/// does *not* include `position_index` any more — unlike `records`/
/// `index`, that now depends on what's actually persisted (see module
/// docs), so `create`/`open` each compute it themselves.
struct Indexes<R, IndexMarker>
where
    R: IndexedField<IndexMarker>,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
}

/// Every `(id, value)` pair read from an existing file during
/// [`GenericMmapStore::open`]'s reconciliation pass, keyed by id, each
/// paired with the slot position it was found at.
type PersistedSlots<R, ScanMarker> =
    HashMap<<R as Record>::Id, (usize, <R as ScannableField<ScanMarker>>::ScanValue)>;

impl<R, IndexMarker, ScanMarker> GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Bytes per persisted slot: the id prefix plus the scannable value —
    /// see module docs for why a slot is no longer just the bare value.
    fn slot_width() -> usize {
        R::Id::BYTE_WIDTH + R::ScanValue::BYTE_WIDTH
    }

    fn read_value(&self, position: usize) -> R::ScanValue {
        let id_width = R::Id::BYTE_WIDTH;
        let start = position * Self::slot_width() + id_width;
        R::ScanValue::read_le(&self.mmap[start..start + R::ScanValue::BYTE_WIDTH])
    }

    fn write_value(&mut self, position: usize, value: R::ScanValue) {
        let id_width = R::Id::BYTE_WIDTH;
        let start = position * Self::slot_width() + id_width;
        value.write_le(&mut self.mmap[start..start + R::ScanValue::BYTE_WIDTH]);
    }

    /// Write a full `(id, value)` slot at `position` — used when a slot's
    /// identity is being established for the first time (`create`, and
    /// `open`'s handling of a record with no prior persisted entry).
    /// `read_value`/`write_value` above never need to touch the id half
    /// of a slot again once it's written, since `position_index` already
    /// captures the id -> position mapping in memory.
    fn write_slot(&mut self, position: usize, id: R::Id, value: R::ScanValue) {
        let slot_width = Self::slot_width();
        let id_width = R::Id::BYTE_WIDTH;
        let start = position * slot_width;
        id.write_le(&mut self.mmap[start..start + id_width]);
        value.write_le(&mut self.mmap[start + id_width..start + slot_width]);
    }

    /// True once every slot currently in the file maps to a live record in
    /// `position_index` — i.e. no record has ever been dropped between an
    /// `open` and the `records` this store was actually built from. Since
    /// positions are unique and never reused, `position_index.len()`
    /// matching the file's total slot count is sufficient to prove every
    /// slot 0..total is covered (pigeonhole — see module docs' `scan`
    /// section for why this matters).
    fn is_gapless(&self) -> bool {
        self.position_index.len() * Self::slot_width() == self.mmap.len()
    }

    fn build_indexes(records: &[R]) -> Indexes<R, IndexMarker> {
        let mut index: HashMap<R::IndexValue, Vec<R::Id>> = HashMap::new();
        for record in records {
            index
                .entry(record.indexed_value().clone())
                .or_default()
                .push(record.id());
        }
        let records_map = records.iter().cloned().map(|r| (r.id(), r)).collect();
        Indexes {
            records: records_map,
            index,
        }
    }

    /// Build fresh: create a new `slot_width() * records.len()`-byte file
    /// at `path`, one `(id, value)` slot per record in `records`' own
    /// order, and memory-map it. Mirrors `MmapAgeStore::create`'s overall
    /// shape, generically, with the id prefix module docs describe.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized, or the mapping fails.
    pub fn create(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let slot_width = Self::slot_width();
        let indexes = Self::build_indexes(&records);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((records.len() * slot_width) as u64)?;

        // SAFETY: this process holds exclusive read/write access to the
        // freshly-created file at `path` for the lifetime of the mapping;
        // nothing else concurrently truncates or writes to it out from
        // under the mapping — the same single-process-exclusive-access
        // assumption `MmapAgeStore::create` documents.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        let mut position_index = HashMap::with_capacity(records.len());
        for (position, record) in records.iter().enumerate() {
            let start = position * slot_width;
            let id_width = R::Id::BYTE_WIDTH;
            record.id().write_le(&mut mmap[start..start + id_width]);
            record
                .scannable_value()
                .write_le(&mut mmap[start + id_width..start + slot_width]);
            position_index.insert(record.id(), position);
        }
        mmap.flush()?;

        Ok(Self {
            records: indexes.records,
            index: indexes.index,
            position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// Reopen `path`, reconciling its persisted `(id, value)` slots
    /// against the externally-supplied `records` **by id**, not by array
    /// position — see module docs for why, and for the explicit,
    /// deliberate behavior of the two mismatch cases this reconciliation
    /// can hit (a persisted id no longer in `records`; a record in
    /// `records` with no persisted slot yet). A record in the second case
    /// gets a freshly-appended slot, seeded from its own
    /// [`ScannableField::scannable_value`], growing the file exactly the
    /// way [`Self::create`] would have for it.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist, can't be
    /// mapped, or can't be resized to accommodate newly-appended slots.
    pub fn open(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        let indexes = Self::build_indexes(&records);
        let slot_width = Self::slot_width();
        let id_width = R::Id::BYTE_WIDTH;

        let file = OpenOptions::new().read(true).write(true).open(path)?;

        // First pass: read every persisted (id, value) pair, keyed by id
        // — the reconciliation step this fix exists to add. A trailing
        // partial slot (fewer than `slot_width` bytes left) is ignored,
        // the same permissive-truncation convention this crate's WAL
        // reader (`durability::read_wal_entries`) already follows.
        let (persisted, existing_slot_count): (PersistedSlots<R, ScanMarker>, usize) = {
            // SAFETY: see `create` — same single-process exclusive-access
            // assumption. This mapping is read, then dropped before any
            // resize below; `memmap2` unmaps on drop.
            let mmap = unsafe { MmapMut::map_mut(&file)? };
            let existing_slot_count = mmap.len() / slot_width;
            let mut persisted = HashMap::with_capacity(existing_slot_count);
            for position in 0..existing_slot_count {
                let start = position * slot_width;
                let id = R::Id::read_le(&mmap[start..start + id_width]);
                let value = R::ScanValue::read_le(&mmap[start + id_width..start + slot_width]);
                persisted.insert(id, (position, value));
            }
            (persisted, existing_slot_count)
        };

        // Reconcile: every record in `records` either already has a
        // persisted slot (reuse its position) or doesn't (queue it for a
        // freshly-appended one, in `records`' own order — deterministic,
        // not HashMap-iteration-order-dependent). A persisted id with no
        // matching record in `records` is simply never added to
        // `position_index` — see module docs' "stale" case.
        let mut position_index = HashMap::with_capacity(records.len());
        let mut new_slots: Vec<(R::Id, R::ScanValue)> = Vec::new();
        for record in &records {
            match persisted.get(&record.id()) {
                Some(&(position, _)) => {
                    position_index.insert(record.id(), position);
                }
                None => {
                    let position = existing_slot_count + new_slots.len();
                    position_index.insert(record.id(), position);
                    new_slots.push((record.id(), record.scannable_value()));
                }
            }
        }

        let total_slots = existing_slot_count + new_slots.len();
        file.set_len((total_slots * slot_width) as u64)?;
        // SAFETY: see `create`. The previous mapping above was already
        // dropped (block-scoped) before this resize, so there's no stale
        // mapping of the old file length left dangling.
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        let mut store = Self {
            records: indexes.records,
            index: indexes.index,
            position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        };

        if !new_slots.is_empty() {
            for (offset, &(id, value)) in new_slots.iter().enumerate() {
                store.write_slot(existing_slot_count + offset, id, value);
            }
            store.mmap.flush()?;
        }

        Ok(store)
    }
}

impl<R, IndexMarker, ScanMarker> GetById<R> for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Write-through consistent with `UpdateField::update`: the returned
    /// record's scannable field always reflects the live mapped value, not
    /// whatever `records` held at construction time — see this module's
    /// doc comment on why `set_scannable_value` exists.
    fn get(&self, id: R::Id) -> Option<R> {
        let mut record = self.records.get(&id)?.clone();
        let position = *self.position_index.get(&id)?;
        record.set_scannable_value(self.read_value(position));
        Some(record)
    }
}

impl<R, IndexMarker, ScanMarker> FilterEq<R, IndexMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.index.get(value).cloned().unwrap_or_default()
    }
}

impl<R, IndexMarker, ScanMarker> ScanField<R, ScanMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// The bulk `chunks_exact` fast path `MmapAgeStore::scan_ages`'s own
    /// `PRODUCTION-DEFAULT` diagnosis established (25-32x over one
    /// `read_value` call per position, `RESULTS.md`'s `## Production
    /// recommendation` section) still applies whenever every slot in the
    /// file is live (`is_gapless`) — the common case, and the only case
    /// the original benchmark measured. Once a stale record's slot can
    /// outlive it (see module docs), a blind full-file scan would leak
    /// that removed record's value into the result, so the gapped case
    /// falls back to reading only the positions `position_index` says are
    /// actually live, sorted first for some locality rather than following
    /// `HashMap` iteration order — see this round's own report for the
    /// measured cost of that fallback path.
    fn scan(&self) -> Vec<R::ScanValue> {
        let id_width = R::Id::BYTE_WIDTH;
        let slot_width = Self::slot_width();
        if self.is_gapless() {
            return self
                .mmap
                .chunks_exact(slot_width)
                .map(|slot| R::ScanValue::read_le(&slot[id_width..slot_width]))
                .collect();
        }
        let mut positions: Vec<usize> = self.position_index.values().copied().collect();
        positions.sort_unstable();
        positions
            .into_iter()
            .map(|position| self.read_value(position))
            .collect()
    }
}

impl<R, IndexMarker, ScanMarker> UpdateField<R, ScanMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>> {
        let position = *self.position_index.get(&id).ok_or(NotFound(id))?;
        self.write_value(position, value);
        Ok(())
    }
}

impl<R, IndexMarker, ScanMarker> Flush for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Force the mapped scannable field to physical disk (`msync`) —
    /// mirrors `MmapAgeStore::flush` exactly.
    fn flush(&self) -> Result<(), DurabilityError> {
        self.mmap.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic::order_customer::{Amount, Order, OrderStatus, Status};

    fn sample() -> Vec<Order> {
        vec![
            Order {
                id: uuid::Uuid::from_u128(1),
                customer_id: uuid::Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
                discount_cents: 0,
            },
            Order {
                id: uuid::Uuid::from_u128(2),
                customer_id: uuid::Uuid::from_u128(100),
                amount_cents: 4_200,
                status: OrderStatus::Pending,
                created_at_unix_ms: 2_000,
                discount_cents: 0,
            },
        ]
    }

    #[test]
    fn create_then_read_and_write() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_basic").unwrap();
        let path = dir.join("amount.mmap");
        let mut store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();

        assert_eq!(
            GetById::get(&store, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );
        UpdateField::update(&mut store, uuid::Uuid::from_u128(1), 9_999).unwrap();
        assert_eq!(
            GetById::get(&store, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            9_999
        );
        assert!(ScanField::scan(&store).contains(&9_999));

        assert!(matches!(
            UpdateField::update(&mut store, uuid::Uuid::from_u128(99), 1),
            Err(NotFound(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_sees_the_written_value() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_roundtrip").unwrap();
        let path = dir.join("amount.mmap");

        {
            let mut store =
                GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();
            UpdateField::update(&mut store, uuid::Uuid::from_u128(1), 77_000).unwrap();
            Flush::flush(&store).unwrap();
        }

        let reopened = GenericMmapStore::<Order, Status, Amount>::open(sample(), &path).unwrap();
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            77_000
        );
        assert_eq!(
            GetById::get(&reopened, uuid::Uuid::from_u128(2))
                .unwrap()
                .amount_cents,
            4_200
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_eq_works_on_the_index_field() {
        let dir = crate::bench_support::fresh_temp_dir("generic_mmap_index").unwrap();
        let path = dir.join("amount.mmap");
        let store = GenericMmapStore::<Order, Status, Amount>::create(sample(), &path).unwrap();

        assert_eq!(
            FilterEq::filter_eq(&store, &OrderStatus::Shipped),
            vec![uuid::Uuid::from_u128(1)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
