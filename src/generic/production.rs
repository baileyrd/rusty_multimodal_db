//! The generic equivalent of `crate::production::ProductionStore` — the
//! same recipe (a composed store made durable via mmap, made safe for
//! concurrent reader/writer access via one global `RwLock`), generic over
//! any `Record`-implementing type instead of hardcoded to `Dog`. See
//! `crate::production`'s own module docs for why the concrete `Dog`
//! version is `RwLock<MmapAgeStore>`, not three literally nested types —
//! the same reasoning applies here: [`GenericProductionStore<S>`] wraps
//! whatever composed generic stack `S` already is (typically
//! [`super::mmap_store::GenericMmapStore`] with zero or more [`super::store::Reversed`]/
//! [`super::store::Symmetric`]/[`super::store::Indexed`]/[`super::store::Scanned`]
//! layers on top), rather than rebuilding storage internals itself.
//!
//! # Inherent methods, not trait impls — and why
//!
//! `crate::production::ProductionStore` implements two *existing*,
//! `Dog`-shaped traits (`DogStore`, `&mut self`; `ConcurrentStore`, `&self`)
//! because those traits already existed with fixed method names before it
//! was written. There is no such pre-existing pair of traits for a fully
//! generic store, and the generic query traits themselves (`GetById`/
//! `ScanField`/etc., `query.rs`) are deliberately single-owner-shaped —
//! `UpdateField::update` takes `&mut self`, which cannot be implemented by
//! a type meant to be shared across threads via `Arc` (the same reason
//! `DogStore::update_age`'s `&mut self` couldn't be reused for
//! `ConcurrentStore` either — see `src/concurrency/mod.rs`'s own module
//! docs). Rather than inventing a parallel `&self`-shaped trait per query
//! trait (`ConcurrentGetById`, `ConcurrentScanField`, ...), `GenericProductionStore`
//! exposes plain inherent `&self` methods, each generic over whatever
//! capability trait `S` happens to implement — the same effect, without
//! quadrupling the trait surface for a wrapper that only ever has one
//! real implementation strategy (take the lock, delegate).

use super::query::{Children, FilterEq, GetById, Parent, ScanField, UpdateField};
use super::store::Flush;
use super::traits::{ChildOf, IndexedField, Record, ScannableField};
use super::NotFound;
use crate::durability::DurabilityError;
use std::sync::RwLock;

/// Message shared by every `.expect()` in this module — mirrors
/// `crate::production`'s own `LOCK_POISONED` constant and rationale: every
/// operation performed while holding the lock is infallible and never
/// panics under normal operation, so poisoning can only mean a prior
/// holder itself panicked, a genuinely exceptional condition this crate's
/// convention documents rather than propagates as a `Result`.
const LOCK_POISONED: &str =
    "RwLock poisoned: a prior holder panicked, which no operation here should ever do";

/// Wraps a composed generic store `S` in one `RwLock`, safe for sharing
/// across threads via `Arc` — the generic analogue of
/// `crate::production::ProductionStore`. See module docs for why its
/// methods are inherent rather than trait impls.
pub struct GenericProductionStore<S> {
    inner: RwLock<S>,
}

impl<S> GenericProductionStore<S> {
    /// Wrap an already-constructed composed store `S`. Domain-specific
    /// `create`/`open` helpers (e.g.
    /// `order_customer::create_order_production_stack`) build `S` itself
    /// (which needs a filesystem path for its durable layer) and hand the
    /// result here — this type doesn't need to know about paths at all,
    /// only about wrapping whatever `S` already is.
    pub fn new(store: S) -> Self {
        Self {
            inner: RwLock::new(store),
        }
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn get<R>(&self, id: R::Id) -> Option<R>
    where
        R: Record,
        S: GetById<R>,
    {
        self.inner.read().expect(LOCK_POISONED).get(id)
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn filter_eq<R, Marker>(&self, value: &R::IndexValue) -> Vec<R::Id>
    where
        R: IndexedField<Marker>,
        S: FilterEq<R, Marker>,
    {
        self.inner.read().expect(LOCK_POISONED).filter_eq(value)
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn scan<R, Marker>(&self) -> Vec<R::ScanValue>
    where
        R: ScannableField<Marker>,
        S: ScanField<R, Marker>,
    {
        self.inner.read().expect(LOCK_POISONED).scan()
    }

    /// Takes the write lock (not read) — the concurrent-mutation analogue
    /// of `ConcurrentStore::update_age`.
    ///
    /// # Errors
    ///
    /// Returns [`NotFound`] if `id` has no record.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn update<R, Marker>(&self, id: R::Id, value: R::ScanValue) -> Result<(), NotFound<R::Id>>
    where
        R: ScannableField<Marker>,
        S: UpdateField<R, Marker>,
    {
        self.inner.write().expect(LOCK_POISONED).update(id, value)
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn parent<C, Marker>(&self, child_id: C::Id) -> Option<C::ParentId>
    where
        C: ChildOf<Marker>,
        S: Parent<C, Marker>,
    {
        self.inner.read().expect(LOCK_POISONED).parent(child_id)
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn children<P, C, Marker>(&self, parent_id: P::Id) -> Vec<C::Id>
    where
        P: Record,
        C: ChildOf<Marker, ParentId = P::Id>,
        S: Children<P, C, Marker>,
    {
        self.inner.read().expect(LOCK_POISONED).children(parent_id)
    }

    /// Force the durable layer(s) inside `S` to physical disk. Takes the
    /// write lock, same rationale as `ProductionStore::flush`: a
    /// checkpoint wants a quiescent snapshot, not a value racing an
    /// in-flight write.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the flush syscall fails.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn flush(&self) -> Result<(), DurabilityError>
    where
        S: Flush,
    {
        self.inner.write().expect(LOCK_POISONED).flush()
    }
}

#[cfg(test)]
mod tests {
    use super::super::order_customer::{
        create_order_production_stack, open_order_production_stack, Amount, BelongsToCustomer,
        Customer, Order, OrderStatus, Status,
    };
    use super::*;

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
    fn get_filter_scan_update_parent_children_all_work_through_the_lock() {
        let dir = crate::bench_support::fresh_temp_dir("generic_production_basic").unwrap();
        let path = dir.join("amount.mmap");
        let stack = create_order_production_stack(sample(), &path).unwrap();
        let store = GenericProductionStore::new(stack);

        assert_eq!(
            store
                .get::<Order>(uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );
        assert_eq!(
            store.filter_eq::<Order, Status>(&OrderStatus::Shipped),
            vec![uuid::Uuid::from_u128(1)]
        );
        let mut amounts = store.scan::<Order, Amount>();
        amounts.sort_unstable();
        assert_eq!(amounts, vec![2_500, 4_200]);

        store
            .update::<Order, Amount>(uuid::Uuid::from_u128(1), 9_000)
            .unwrap();
        assert_eq!(
            store
                .get::<Order>(uuid::Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            9_000
        );

        assert_eq!(
            store.parent::<Order, BelongsToCustomer>(uuid::Uuid::from_u128(1)),
            Some(uuid::Uuid::from_u128(100))
        );
        let mut children =
            store.children::<Customer, Order, BelongsToCustomer>(uuid::Uuid::from_u128(100));
        children.sort();
        assert_eq!(
            children,
            vec![uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_sees_the_written_value() {
        let dir = crate::bench_support::fresh_temp_dir("generic_production_roundtrip").unwrap();
        let path = dir.join("amount.mmap");

        {
            let stack = create_order_production_stack(sample(), &path).unwrap();
            let store = GenericProductionStore::new(stack);
            store
                .update::<Order, Amount>(uuid::Uuid::from_u128(2), 42_000)
                .unwrap();
            store.flush().unwrap();
        }

        let reopened_stack = open_order_production_stack(sample(), &path).unwrap();
        let reopened = GenericProductionStore::new(reopened_stack);
        assert_eq!(
            reopened
                .get::<Order>(uuid::Uuid::from_u128(2))
                .unwrap()
                .amount_cents,
            42_000
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
