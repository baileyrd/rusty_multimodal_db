//! `MmapScanned`: the durable twin of [`super::store::Scanned`] — one more
//! mutable, durable, scannable field over any inner store, backed by its
//! own `GMMAPST\0` slot file in exactly the format
//! [`super::mmap_store::GenericMmapStore`] writes.
//!
//! This is the answer to `ADR-0006`'s revisit trigger — *"a future record
//! shape needs more than one mutable field persisted"* — as
//! `ADR-0020` / `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`
//! accepted it, and `STORAGE-017` specifies it. `GenericMmapStore` owns
//! every record, one equality index, and *one* mmap-backed scannable
//! field; every other field is durable through the companion record blob
//! but immutable through the store. Stacking one `MmapScanned` per extra
//! field makes each of them mutable and durable too, at the same
//! per-write cost shape (`MFMD-FR-001`: one bounded in-place copy into
//! the mapping, no allocation, no syscall), and the file each one writes
//! is a column of that field alone, so a scan of it walks nothing else
//! (`ADR-0001`'s column-layout result).
//!
//! # How it composes
//!
//! `MmapScanned<S, R, Marker>` mirrors `Scanned<S, R, Marker>` exactly in
//! shape and in every forwarding impl, with the in-memory `cache: Vec`
//! replaced by a `SlotFile` and `new(inner, &records)` replaced by
//! [`MmapScanned::create`]`(inner, &records, path)` /
//! [`MmapScanned::open`]`(inner, &records, path)`. A domain adds a durable
//! field by adding a layer: one type parameter in its stack alias, one
//! constructor line deriving the layer's path from the base path, one
//! entry in a `forward_scannable_pairs!` invocation `for MmapScanned`
//! (`MFMD-FR-008`). `OrderProductionStack` in `order_customer.rs` is the
//! reference: `Amount` stays in `GenericMmapStore`'s own file,
//! `DiscountCents` gains a `<path>.discount_cents.mmap`, and `CreatedAt`
//! deliberately stays in-memory.
//!
//! - `ScanField`/`UpdateField` for its own marker read and write the slot
//!   file; `scan` keeps `GenericMmapStore`'s `chunks_exact` fast path
//!   whenever every slot is live.
//! - `GetById` patches its field into the record on the way up through
//!   `set_scannable_value`, exactly as `Scanned` does, so a stack of
//!   layers returns a record consistent with every durable field
//!   (`MFMD-FR-003`).
//! - `Flush` flushes its own file, then forwards (`MFMD-FR-002`).
//! - `FilterEq`, `Neighbors`, `Children` forward generically; `Parent` is
//!   the blanket impl over `GetById`. Cross-marker `ScanField`/
//!   `UpdateField` forwards are the macro's, for the same `E0119` reason
//!   `store.rs` documents.
//!
//! # Reconciliation, per file
//!
//! `open` reconciles its own file against `records` by id, independently
//! of every other file in the stack (`MFMD-FR-004`): a record with a
//! committed slot reuses that position; a record without one gets a
//! fresh slot appended (seeded from the record's own value); a committed
//! slot whose id is not in `records` is left in place and ignored; a slot
//! whose commit byte is not set is invisible and the record it was for is
//! re-appended. So a stack can be reopened after a crash that landed
//! between two files' appends, and each file heals itself.
//!
//! # What guards a foreign file
//!
//! The layer writes no blob of its own and its file carries no schema
//! tag (`GMMAPST\0` version 2 has no room for one, and a version bump
//! would refuse every existing file — `MFMD-FR-006`). `MFMD-FR-009`'s
//! guarantee therefore rests on two things: the `R: SchemaTag` bound on
//! the file constructors, which makes a layer constructible only for a
//! record type whose stack carries a tagged blob — and the domain's
//! portable-open path reads that blob *first*, before any slot file is
//! touched; and a slot-width check on `open`, which refuses a file whose
//! slot data is not a whole number of this layer's `(Id, ScanValue)`
//! slots ([`DurabilityError::SlotWidthMismatch`], naming the path). The
//! check is weak by design (a foreign width can divide the same body
//! length) and it also refuses a file truncated mid-slot, which
//! `GenericMmapStore` would silently ignore: this layer has no blob to
//! fall back on, so it prefers refusal to a guess. A single-`write_all`
//! append is treated as leaving either a whole slot or nothing, the same
//! residual assumption `GenericMmapStore`'s append path already carries.
//!
//! # Not solved here, deliberately
//!
//! No multi-field atomic update: an `update` on one layer and an `update`
//! on another are two independent in-place writes, and a crash between
//! them is visible as one field new and one old. That is the same
//! invariant `GenericMmapStore` plus a `Scanned` layer already had, now
//! durable on both sides; making the pair atomic is transaction
//! territory (`ADR-0013`), not this layer's.

use super::mmap_field::MmapFieldValue;
use super::query::{Children, FilterEq, GetById, Neighbors, ScanField, UpdateField};
use super::slot_file::SlotFile;
use super::store::Flush;
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SchemaTag, SymmetricRelation};
use super::NotFound;
use crate::durability::DurabilityError;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::Path;

/// One more mutable, durable, scannable field (`Marker`) over an inner
/// store `S` — see the module docs.
pub struct MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    inner: S,
    /// `id` -> that id's *current* slot position in `file`, built by
    /// reconciling persisted ids against the records handed to the
    /// constructor — the same index `GenericMmapStore` keeps for its own
    /// file.
    position_index: HashMap<R::Id, usize>,
    file: SlotFile<R::Id, R::ScanValue>,
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Access to the inner store — what `forward_scannable_pairs!`'s
    /// generated cross-marker impls call through, exactly as for
    /// `Scanned::inner`.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// The slot file this layer owns.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    fn is_gapless(&self) -> bool {
        self.file.is_gapless(self.position_index.len())
    }
}

/// The two file constructors, and only they, add the [`SchemaTag`] bound
/// (`MFMD-FR-009`): this layer writes no blob, so the bound is what ties
/// its file to a record type whose stack carries a tagged one. Every
/// query impl is unaffected.
impl<S, R, Marker> MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker> + SchemaTag,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// Build fresh over `inner`: create a new slot file at `path` holding
    /// one committed `(id, value)` slot per record in `records`' own
    /// order, seeded from each record's own
    /// [`ScannableField::scannable_value`], and memory-map it. Any
    /// existing file at `path` is truncated; `path`'s parent is created
    /// if missing.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the parent can't be created, the
    /// file can't be created/sized/mapped, or the initial flush fails.
    pub fn create(inner: S, records: &[R], path: &Path) -> Result<Self, DurabilityError> {
        let file = SlotFile::create(
            path,
            records
                .iter()
                .map(|record| (record.id(), record.scannable_value())),
        )?;
        let position_index = records
            .iter()
            .enumerate()
            .map(|(position, record)| (record.id(), position))
            .collect();
        Ok(Self {
            inner,
            position_index,
            file,
            _marker: PhantomData,
        })
    }

    /// Reopen the slot file at `path` over `inner`, reconciling its
    /// committed slots against `records` by id — see the module docs for
    /// the four cases. A record with no committed slot gets one appended,
    /// seeded from its own [`ScannableField::scannable_value`], through
    /// the same `O_APPEND` path `GenericMmapStore::open` uses.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist, can't be
    /// mapped, or a slot can't be appended;
    /// [`DurabilityError::InvalidMagic`] or
    /// [`DurabilityError::SchemaVersionMismatch`] if the header doesn't
    /// check out; [`DurabilityError::SlotWidthMismatch`], naming `path`,
    /// if the slot data isn't a whole number of this layer's slots. Every
    /// failure returns before any slot is appended.
    pub fn open(inner: S, records: &[R], path: &Path) -> Result<Self, DurabilityError> {
        let mut file = SlotFile::open(path)?;
        if file.trailing_partial_bytes() != 0 {
            return Err(DurabilityError::SlotWidthMismatch {
                path: path.to_path_buf(),
                body_len: file.slot_bytes().len(),
                slot_width: SlotFile::<R::Id, R::ScanValue>::slot_width(),
            });
        }
        let persisted = file.committed_pairs();

        let mut position_index = HashMap::with_capacity(records.len());
        let mut missing: Vec<&R> = Vec::new();
        for record in records {
            match persisted.get(&record.id()) {
                Some(&(position, _)) => {
                    position_index.insert(record.id(), position);
                }
                None => missing.push(record),
            }
        }
        if !missing.is_empty() {
            let positions = file.append_committed_slots(
                missing
                    .iter()
                    .map(|record| (record.id(), record.scannable_value())),
            )?;
            for (record, position) in missing.iter().zip(positions) {
                position_index.insert(record.id(), position);
            }
        }

        Ok(Self {
            inner,
            position_index,
            file,
            _marker: PhantomData,
        })
    }
}

impl<S, R, Marker> ScanField<R, Marker> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    /// The same two paths as `GenericMmapStore::scan`: a bulk
    /// `chunks_exact` walk of the whole slot column when every slot is
    /// live, otherwise one read per live position in position order.
    fn scan(&self) -> Vec<R::ScanValue> {
        let id_width = R::Id::BYTE_WIDTH;
        let value_width = R::ScanValue::BYTE_WIDTH;
        let slot_width = SlotFile::<R::Id, R::ScanValue>::slot_width();
        if self.is_gapless() {
            return self
                .file
                .slot_bytes()
                .chunks_exact(slot_width)
                .map(|slot| R::ScanValue::read_le(&slot[id_width..id_width + value_width]))
                .collect();
        }
        let mut positions: Vec<usize> = self.position_index.values().copied().collect();
        positions.sort_unstable();
        positions
            .into_iter()
            .map(|position| self.file.read_value(position))
            .collect()
    }
}

impl<S, R, Marker> UpdateField<R, Marker> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    fn update(&mut self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>> {
        let position = *self.position_index.get(&id).ok_or(NotFound(id))?;
        self.file.write_value(position, value);
        Ok(())
    }
}

// Forwarding impl: patch-on-the-way-up, exactly as `Scanned`'s `GetById`
// — this layer can't reach into whatever owns the record, so it patches
// its own field into the record `inner.get` returns (`MFMD-FR-003`).
impl<S, R, Marker> GetById<R> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
    S: GetById<R>,
{
    fn get(&self, id: R::Id) -> Option<R> {
        let mut record = self.inner.get(id)?;
        if let Some(&position) = self.position_index.get(&id) {
            record.set_scannable_value(self.file.read_value(position));
        }
        Some(record)
    }
}

impl<S, R, Marker> Flush for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
    S: Flush,
{
    /// `msync` this layer's own file, then the inner store's
    /// (`MFMD-FR-002`) — the first failure returns, so a caller who sees
    /// `Ok` knows every file in the stack was flushed.
    fn flush(&self) -> Result<(), DurabilityError> {
        self.file.flush()?;
        self.inner.flush()
    }
}

// Forwarding impl: `FilterEq` for any index marker the inner store
// answers — the same two-marker shape as `Scanned`'s.
impl<S, R, Marker, IndexMarker> FilterEq<R, IndexMarker> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker> + IndexedField<IndexMarker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
    S: FilterEq<R, IndexMarker>,
{
    fn filter_eq(&self, value: &R::IndexValue) -> Vec<R::Id> {
        self.inner.filter_eq(value)
    }
}

// Forwarding impl: `Neighbors` for any symmetric relation the inner
// store answers — `Rel`/`RelMarker` are independent of this layer's own
// `R`/`Marker`, as in `Reversed`'s forward.
impl<S, R, Marker, Rel, RelMarker> Neighbors<Rel, RelMarker> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
    Rel: SymmetricRelation<RelMarker>,
    S: Neighbors<Rel, RelMarker>,
{
    fn neighbors(&self, id: Rel::Id) -> Vec<Rel::Id> {
        self.inner.neighbors(id)
    }
}

// Forwarding impl: `Children` for any directed relation the inner store
// answers — so a `Reversed` may sit below this layer as well as above it.
impl<S, R, Marker, P, C, RelMarker> Children<P, C, RelMarker> for MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
    P: Record,
    C: ChildOf<RelMarker, ParentId = P::Id>,
    S: Children<P, C, RelMarker>,
{
    fn children(&self, parent_id: P::Id) -> Vec<C::Id> {
        self.inner.children(parent_id)
    }
}

// Uses `order_customer::Order` as its concrete fixture (the layer under
// test has no domain of its own) — gated behind `research` the same way
// that module is. Numbered against the design's acceptance criteria
// (`docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`, "Acceptance
// criteria"): 1, 2, 3, 5, 8 live here; 4 and 7 in `order_customer.rs`.
#[cfg(all(test, feature = "research"))]
mod tests {
    use super::super::mmap_store::GenericMmapStore;
    use super::super::order_customer::{Amount, DiscountCents, Order, OrderStatus, Status};
    use super::super::slot_file::HEADER_LEN;
    use super::super::store::BaseStore;
    use super::*;
    use crate::bench_support::fresh_temp_dir;
    use uuid::Uuid;

    type Core = GenericMmapStore<Order, Status, Amount>;
    type Layer = MmapScanned<Core, Order, DiscountCents>;

    fn order(n: u128, amount_cents: i64, discount_cents: i64) -> Order {
        Order {
            id: Uuid::from_u128(n),
            customer_id: Uuid::from_u128(100),
            amount_cents,
            status: OrderStatus::Pending,
            created_at_unix_ms: 1_700_000_000_000 + n as i64,
            discount_cents,
        }
    }

    fn sample() -> Vec<Order> {
        vec![order(1, 2_500, 50), order(2, 4_200, 0), order(3, 999, 100)]
    }

    fn create_layer(orders: &[Order], base: &Path, discount: &Path) -> Layer {
        let core = Core::create(orders.to_vec(), base).unwrap();
        Layer::create(core, orders, discount).unwrap()
    }

    fn open_layer(orders: &[Order], base: &Path, discount: &Path) -> Layer {
        let core = Core::open(orders.to_vec(), base).unwrap();
        Layer::open(core, orders, discount).unwrap()
    }

    fn paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = fresh_temp_dir(label).unwrap();
        (
            dir.join("orders.mmap"),
            dir.join("orders.mmap.discount_cents.mmap"),
        )
    }

    /// Criterion 1: two durable fields round-trip independently through
    /// `create` / update both / drop / `open`, and each scan reflects only
    /// its own updates.
    #[test]
    fn two_durable_fields_round_trip_independently() {
        let (base, discount) = paths("mmap_scanned_round_trip");
        let orders = sample();
        {
            let mut layer = create_layer(&orders, &base, &discount);
            UpdateField::<Order, Amount>::update(layer.inner_mut(), orders[1].id, 7_000).unwrap();
            UpdateField::<Order, DiscountCents>::update(&mut layer, orders[1].id, 999).unwrap();
            // Both files updated, no flush: durability through the
            // mapping alone (criterion 1 does not flush on purpose).
        }
        let layer = open_layer(&orders, &base, &discount);
        assert_eq!(
            ScanField::<Order, DiscountCents>::scan(&layer),
            vec![50, 999, 100]
        );
        assert_eq!(
            ScanField::<Order, Amount>::scan(layer.inner()),
            vec![2_500, 7_000, 999]
        );
        // `get` reflects both live files (`MFMD-FR-003`).
        let got = layer.get(orders[1].id).unwrap();
        assert_eq!(got.amount_cents, 7_000);
        assert_eq!(got.discount_cents, 999);
        // And an untouched record reads back its seed values.
        let got = layer.get(orders[0].id).unwrap();
        assert_eq!((got.amount_cents, got.discount_cents), (2_500, 50));
    }

    /// Criterion 2a: a record with no slot in this layer's file gets one
    /// appended on `open`, seeded from the record.
    #[test]
    fn open_appends_a_slot_for_a_record_the_file_never_saw() {
        let (base, discount) = paths("mmap_scanned_append_missing");
        let orders = sample();
        drop(create_layer(&orders, &base, &discount));

        let mut grown = orders.clone();
        grown.push(order(4, 10, 7));
        let layer = open_layer(&grown, &base, &discount);
        assert_eq!(
            ScanField::<Order, DiscountCents>::scan(&layer),
            vec![50, 0, 100, 7]
        );
        assert_eq!(layer.get(grown[3].id).unwrap().discount_cents, 7);
        assert_eq!(layer.position_index[&grown[3].id], 3);
    }

    /// Criterion 2b: a committed slot whose id is no longer in `records`
    /// stays in the file but is invisible to `scan` and `get`.
    #[test]
    fn open_ignores_a_stale_slot() {
        let (base, discount) = paths("mmap_scanned_stale");
        let orders = sample();
        {
            let mut layer = create_layer(&orders, &base, &discount);
            UpdateField::<Order, DiscountCents>::update(&mut layer, orders[2].id, 555).unwrap();
        }
        let kept = vec![orders[0].clone(), orders[2].clone()];
        let layer = open_layer(&kept, &base, &discount);
        assert_eq!(
            ScanField::<Order, DiscountCents>::scan(&layer),
            vec![50, 555]
        );
        assert!(layer.get(orders[1].id).is_none());
        // The stale slot is still physically there: three slots on disk.
        assert_eq!(layer.file.slot_count(), 3);
        assert!(!layer.is_gapless());
    }

    /// Criterion 2c: a slot whose commit byte was cleared (a torn write)
    /// is invisible, and its record is re-appended on `open`.
    #[test]
    fn open_reappends_a_record_whose_slot_is_uncommitted() {
        let (base, discount) = paths("mmap_scanned_torn");
        let orders = sample();
        {
            let mut layer = create_layer(&orders, &base, &discount);
            UpdateField::<Order, DiscountCents>::update(&mut layer, orders[1].id, 321).unwrap();
        }
        // Clear slot 1's trailing commit byte in the layer's file.
        let slot_width = SlotFile::<Uuid, i64>::slot_width();
        let marker = HEADER_LEN + 2 * slot_width - 1;
        let mut bytes = std::fs::read(&discount).unwrap();
        bytes[marker] = 0;
        std::fs::write(&discount, &bytes).unwrap();

        let layer = open_layer(&orders, &base, &discount);
        // The torn slot's 321 is gone; the record came back with its
        // seed value, at a fresh position past the torn one.
        assert_eq!(layer.get(orders[1].id).unwrap().discount_cents, 0);
        assert_eq!(layer.position_index[&orders[1].id], 3);
        assert_eq!(layer.file.slot_count(), 4);
        assert_eq!(
            ScanField::<Order, DiscountCents>::scan(&layer),
            vec![50, 100, 0]
        );
    }

    /// Criterion 3: `flush` msyncs both files — its own first, then the
    /// inner store's — and both hold the updated bytes.
    #[test]
    fn flush_reaches_both_files() {
        let (base, discount) = paths("mmap_scanned_flush");
        let orders = sample();
        let mut layer = create_layer(&orders, &base, &discount);
        UpdateField::<Order, Amount>::update(layer.inner_mut(), orders[0].id, 1).unwrap();
        UpdateField::<Order, DiscountCents>::update(&mut layer, orders[0].id, 2).unwrap();
        layer.flush().unwrap();

        let value_at = |path: &Path| {
            let bytes = std::fs::read(path).unwrap();
            let start = HEADER_LEN + Uuid::BYTE_WIDTH;
            i64::read_le(&bytes[start..start + i64::BYTE_WIDTH])
        };
        assert_eq!(value_at(&base), 1);
        assert_eq!(value_at(&discount), 2);
    }

    /// Criterion 5: a slot file whose body is not a whole number of this
    /// layer's slots is refused by `open`, with the error naming the path.
    #[test]
    fn open_refuses_a_file_with_another_slot_width() {
        // A record with a 4-byte id: 13-byte slots against `Order`'s 25.
        #[derive(Clone)]
        struct Narrow {
            id: u32,
            weight: i64,
        }
        struct Weight;
        impl Record for Narrow {
            type Id = u32;
            fn id(&self) -> u32 {
                self.id
            }
        }
        impl SchemaTag for Narrow {
            const SCHEMA_TAG: &'static str = "mmap_scanned::tests::Narrow";
        }
        impl ScannableField<Weight> for Narrow {
            type ScanValue = i64;
            fn scannable_value(&self) -> i64 {
                self.weight
            }
            fn set_scannable_value(&mut self, value: i64) {
                self.weight = value;
            }
        }

        let (_, path) = paths("mmap_scanned_slot_width");
        let narrow = vec![
            Narrow { id: 1, weight: 10 },
            Narrow { id: 2, weight: 20 },
            Narrow { id: 3, weight: 30 },
        ];
        drop(
            MmapScanned::<_, Narrow, Weight>::create(
                BaseStore::new(narrow.clone()),
                &narrow,
                &path,
            )
            .unwrap(),
        );

        // 3 * 13 = 39 body bytes; 39 % 25 != 0.
        let err = Layer::open(
            Core::create(sample(), &path.with_extension("base")).unwrap(),
            &sample(),
            &path,
        )
        .err()
        .unwrap();
        match err {
            DurabilityError::SlotWidthMismatch {
                path: named,
                body_len,
                slot_width,
            } => {
                assert_eq!(named, path);
                assert_eq!(body_len, 3 * 13);
                assert_eq!(slot_width, 25);
            }
            other => panic!("expected SlotWidthMismatch, got {other:?}"),
        }
        assert!(err_string_names_path(&path));

        // The header is still what the check trusts first: a file that
        // isn't a slot file at all is `InvalidMagic`, not a width error.
        let bogus = path.with_extension("bogus");
        std::fs::write(&bogus, [0u8; HEADER_LEN + 25]).unwrap();
        assert!(matches!(
            Layer::open(
                Core::create(sample(), &path.with_extension("base2")).unwrap(),
                &sample(),
                &bogus
            ),
            Err(DurabilityError::InvalidMagic)
        ));

        fn err_string_names_path(path: &Path) -> bool {
            let err = DurabilityError::SlotWidthMismatch {
                path: path.to_path_buf(),
                body_len: 39,
                slot_width: 25,
            };
            err.to_string().contains(&path.display().to_string())
        }
    }

    /// Criterion 5, the other cause: a file this layer wrote, truncated
    /// mid-slot, is refused too — this layer has no blob to fall back on.
    #[test]
    fn open_refuses_a_file_truncated_mid_slot() {
        let (base, discount) = paths("mmap_scanned_truncated");
        let orders = sample();
        drop(create_layer(&orders, &base, &discount));
        let len = std::fs::metadata(&discount).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&discount)
            .unwrap()
            .set_len(len - 1)
            .unwrap();

        let core = Core::open(orders.clone(), &base).unwrap();
        assert!(matches!(
            Layer::open(core, &orders, &discount),
            Err(DurabilityError::SlotWidthMismatch { slot_width: 25, .. })
        ));
    }

    /// Criterion 8: the layer works over an in-memory inner — a durable
    /// field without a `GenericMmapStore` underneath.
    #[test]
    fn works_over_an_in_memory_inner() {
        type InMemory = MmapScanned<BaseStore<Order>, Order, Amount>;
        let (path, _) = paths("mmap_scanned_in_memory");
        let orders = sample();
        {
            let mut layer =
                InMemory::create(BaseStore::new(orders.clone()), &orders, &path).unwrap();
            assert_eq!(layer.scan(), vec![2_500, 4_200, 999]);
            layer.update(orders[0].id, 1).unwrap();
            assert_eq!(layer.get(orders[0].id).unwrap().amount_cents, 1);
            assert!(matches!(
                layer.update(Uuid::from_u128(99), 1),
                Err(NotFound(id)) if id == Uuid::from_u128(99)
            ));
            layer.flush().unwrap();
        }
        let layer = InMemory::open(BaseStore::new(orders.clone()), &orders, &path).unwrap();
        assert_eq!(layer.scan(), vec![1, 4_200, 999]);
        assert_eq!(layer.path(), path);
    }
}
