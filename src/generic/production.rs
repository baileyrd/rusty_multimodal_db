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

use super::query::{Children, FilterEq, GetById, Neighbors, Parent, ScanField, UpdateField};
use super::store::Flush;
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SymmetricRelation};
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
///
/// # Examples
///
/// Building your own domain means implementing [`super::traits::Record`]
/// (an id), plus [`super::traits::IndexedField`] and/or
/// [`super::traits::ScannableField`] for whichever fields need
/// equality-lookup or scan/update access — one zero-sized marker type per
/// field. See `crate::generic::order_customer` (behind the `research`
/// feature) for a larger, real reference domain (`Order`/`Customer`, a
/// directed relation, three scannable fields); this example is the
/// minimal shape, unconditionally available:
///
/// ```
/// use rusty_multimodal_db::generic::mmap_store::GenericMmapStore;
/// use rusty_multimodal_db::generic::production::GenericProductionStore;
/// use rusty_multimodal_db::generic::traits::{IndexedField, Record, ScannableField};
/// use uuid::Uuid;
///
/// #[derive(Clone)]
/// struct Widget {
///     id: Uuid,
///     category: u32,
///     price_cents: i64,
/// }
///
/// // One zero-sized marker per field this domain wants indexed/scannable
/// // access to — see `IndexedField`/`ScannableField`'s own doc comments
/// // for why a marker, not just the field's type, identifies each one.
/// struct Category;
/// struct Price;
///
/// impl Record for Widget {
///     type Id = Uuid;
///     fn id(&self) -> Uuid {
///         self.id
///     }
/// }
///
/// impl IndexedField<Category> for Widget {
///     type IndexValue = u32;
///     fn indexed_value(&self) -> &u32 {
///         &self.category
///     }
/// }
///
/// impl ScannableField<Price> for Widget {
///     type ScanValue = i64;
///     fn scannable_value(&self) -> i64 {
///         self.price_cents
///     }
///     fn set_scannable_value(&mut self, value: i64) {
///         self.price_cents = value;
///     }
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = std::env::temp_dir().join(format!("generic_production_store_doctest_{}", std::process::id()));
/// std::fs::create_dir_all(&dir)?;
/// let path = dir.join("widgets.mmap");
///
/// let a = Uuid::from_u128(1);
/// let b = Uuid::from_u128(2);
/// let widgets = vec![
///     Widget { id: a, category: 10, price_cents: 500 },
///     Widget { id: b, category: 10, price_cents: 900 },
/// ];
///
/// // GenericMmapStore is the durable core; GenericProductionStore adds the
/// // RwLock that makes it safe to share across threads via Arc.
/// let core = GenericMmapStore::<Widget, Category, Price>::create(widgets, &path)?;
/// let store = GenericProductionStore::new(core);
///
/// assert_eq!(store.get::<Widget>(a).unwrap().price_cents, 500);
/// assert_eq!(store.filter_eq::<Widget, Category>(&10).len(), 2);
///
/// store.update::<Widget, Price>(a, 750)?;
/// assert_eq!(store.get::<Widget>(a).unwrap().price_cents, 750);
///
/// # std::fs::remove_dir_all(&dir).ok();
/// # Ok(())
/// # }
/// ```
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

    /// # Errors
    ///
    /// Returns [`NotFound`] if `child_id` has no record.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn parent<C, Marker>(&self, child_id: C::Id) -> Result<Option<C::ParentId>, NotFound<C::Id>>
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

    /// A symmetric relation — the generic analogue of `Dog::neighbors`.
    /// Added alongside the `Employee`-style third-domain validation round
    /// (`SERVER-QUERY-LAYER`): no domain wrapped in `GenericProductionStore`
    /// had ever needed `SymmetricRelation` before, so this method — and
    /// the `Reversed`-forwards-`Neighbors` impl it depends on when a
    /// domain also has a `ChildOf` relation — didn't exist until now.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn neighbors<R, Marker>(&self, id: R::Id) -> Vec<R::Id>
    where
        R: SymmetricRelation<Marker>,
        S: Neighbors<R, Marker>,
    {
        self.inner.read().expect(LOCK_POISONED).neighbors(id)
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

    /// Runs `f` with exclusive access to the wrapped store `S` held for
    /// `f`'s entire duration — the same internal lock every other method
    /// here already acquires and releases per call, exposed here as one
    /// continuous critical section spanning as many logical operations as
    /// `f` performs. The generic analogue of
    /// `crate::production::ProductionStore`'s own `TransactionalStore`
    /// impl — the real mechanism behind the server layer's
    /// `Request::Transaction` atomicity guarantee
    /// (`docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013). A plain
    /// inherent method, not a trait: every `*ConnectionStore` adapter that
    /// wraps `GenericProductionStore<S>` (`OrderConnectionStore`,
    /// `EmployeeConnectionStore`) is concretely typed over one specific
    /// `S`, not generic over it — unlike `server::dog::DogConnectionStore<S>`,
    /// there's no generic caller here needing a trait bound to reach this
    /// method through.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    pub fn with_exclusive<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut guard = self.inner.write().expect(LOCK_POISONED);
        f(&mut guard)
    }
}

// Uses `order_customer::{Order, ...}` as its concrete test fixture — see
// `mmap_store.rs`'s identical gating and comment for why.
#[cfg(all(test, feature = "research"))]
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
            Ok(Some(uuid::Uuid::from_u128(100)))
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
