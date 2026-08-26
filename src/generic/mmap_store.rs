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

use super::mmap_field::MmapFieldValue;
use super::query::{FilterEq, GetById, ScanField, UpdateField};
use super::store::Flush;
use super::traits::{IndexedField, ScannableField};
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
/// and scoped to exactly one field of each kind.
pub struct GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker>,
    R::ScanValue: MmapFieldValue,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
    position_index: HashMap<R::Id, usize>,
    mmap: MmapMut,
    #[allow(dead_code)] // kept for symmetry with MmapAgeStore's Self; not read again
    path: PathBuf,
    _marker: PhantomData<(IndexMarker, ScanMarker)>,
}

/// The immutable-after-construction pieces of [`GenericMmapStore`]'s
/// state — everything except the mapped scannable-field region itself.
/// Factored into its own struct (rather than a 3-tuple) purely for
/// readability at the `create`/`open` call sites, mirroring
/// `src/durability/mmap_store.rs`'s own `Indexes` struct.
struct Indexes<R, IndexMarker>
where
    R: IndexedField<IndexMarker>,
{
    records: HashMap<R::Id, R>,
    index: HashMap<R::IndexValue, Vec<R::Id>>,
    position_index: HashMap<R::Id, usize>,
}

impl<R, IndexMarker, ScanMarker> GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
    R::ScanValue: MmapFieldValue,
{
    fn read_value(&self, position: usize) -> R::ScanValue {
        let width = R::ScanValue::BYTE_WIDTH;
        let start = position * width;
        R::ScanValue::read_le(&self.mmap[start..start + width])
    }

    fn write_value(&mut self, position: usize, value: R::ScanValue) {
        let width = R::ScanValue::BYTE_WIDTH;
        let start = position * width;
        value.write_le(&mut self.mmap[start..start + width]);
    }

    fn build_indexes(records: &[R]) -> Indexes<R, IndexMarker> {
        let mut index: HashMap<R::IndexValue, Vec<R::Id>> = HashMap::new();
        let mut position_index = HashMap::with_capacity(records.len());
        for (position, record) in records.iter().enumerate() {
            index
                .entry(record.indexed_value().clone())
                .or_default()
                .push(record.id());
            position_index.insert(record.id(), position);
        }
        let records_map = records.iter().cloned().map(|r| (r.id(), r)).collect();
        Indexes {
            records: records_map,
            index,
            position_index,
        }
    }

    /// Build fresh: create a new `BYTE_WIDTH * records.len()`-byte file at
    /// `path`, initialized from each record's starting scannable value, and
    /// memory-map it. Mirrors `MmapAgeStore::create` exactly, generically.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized, or the mapping fails.
    pub fn create(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let width = R::ScanValue::BYTE_WIDTH;
        let initial_values: Vec<R::ScanValue> =
            records.iter().map(|r| r.scannable_value()).collect();
        let indexes = Self::build_indexes(&records);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((initial_values.len() * width) as u64)?;

        // SAFETY: this process holds exclusive read/write access to the
        // freshly-created file at `path` for the lifetime of the mapping;
        // nothing else concurrently truncates or writes to it out from
        // under the mapping — the same single-process-exclusive-access
        // assumption `MmapAgeStore::create` documents.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        for (position, value) in initial_values.iter().enumerate() {
            value.write_le(&mut mmap[position * width..position * width + width]);
        }
        mmap.flush()?;

        Ok(Self {
            records: indexes.records,
            index: indexes.index,
            position_index: indexes.position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }

    /// Rebuild indexes from the externally-supplied `records` (their
    /// scannable field is ignored — the mapped file is the source of
    /// truth), and memory-map the existing file at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist or can't be
    /// mapped.
    pub fn open(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError> {
        let indexes = Self::build_indexes(&records);

        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: see `create` — same single-process exclusive-access
        // assumption.
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            records: indexes.records,
            index: indexes.index,
            position_index: indexes.position_index,
            mmap,
            path: path.to_path_buf(),
            _marker: PhantomData,
        })
    }
}

impl<R, IndexMarker, ScanMarker> GetById<R> for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
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
    R::ScanValue: MmapFieldValue,
{
    /// Bulk-reads via `chunks_exact(BYTE_WIDTH)` rather than one
    /// `read_value` call per position — the same fix `MmapAgeStore::scan_ages`
    /// needed after the `PRODUCTION-DEFAULT` follow-up diagnosis found its
    /// original per-position loop 25-32x slower (`RESULTS.md`'s
    /// `## Production recommendation` section), applied here from the
    /// start rather than rediscovered under this round's own benchmark.
    fn scan(&self) -> Vec<R::ScanValue> {
        let width = R::ScanValue::BYTE_WIDTH;
        self.mmap
            .chunks_exact(width)
            .map(R::ScanValue::read_le)
            .collect()
    }
}

impl<R, IndexMarker, ScanMarker> UpdateField<R, ScanMarker>
    for GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone,
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
