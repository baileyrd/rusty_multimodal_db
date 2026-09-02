//! `Order`/`Customer`: the generic library's real reference implementation,
//! not disposable prototype code. Promoted from `generic_spike/order_impl.rs`
//! once every risk the design doc's §4 flagged had been individually
//! spiked and resolved with real data — the associated-type ambiguity fix,
//! the macro-generated per-marker-pair forwarding, and the directed-
//! relation adjacency-index generalization. `Dog` (`crate::record::DogRecord`,
//! its generic trait impls still live in `crate::generic_spike::dog_impl`)
//! stays a historical benchmark fixture, not promoted further — see that
//! module's docs.
//!
//! `Order` is the harder case `Dog` never was: **three** `ScannableField`s
//! (`Amount`, `CreatedAt`, `DiscountCents`) and a directed `ChildOf`
//! relation to `Customer`. [`OrderProductionStack`] below wires **two**
//! of them durably (`STORAGE-017`): `Amount` as [`GenericMmapStore`]'s
//! own mmap-backed field, and `DiscountCents` as an [`MmapScanned`] layer
//! with its own slot file (`<path>.discount_cents.mmap`) on top of it.
//! `CreatedAt` is deliberately left out of the durable stack — it stays
//! in-memory in [`OrderGenericStore`] — as the standing proof that a
//! field which doesn't need durability doesn't pay for it. Both layers
//! forward the fields they don't own through the impls
//! `forward_scannable_pairs!` generates below (one invocation per layer
//! kind); those impls are generic over whatever inner store type they
//! wrap, so the same three-marker list serves both.

use super::mmap_scanned::MmapScanned;
use super::mmap_store::GenericMmapStore;
use super::store::{BaseStore, Indexed, Reversed, Scanned};
use super::traits::{ChildOf, IndexedField, Record, ScannableField, SchemaTag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `Serialize`/`Deserialize` on the three domain types: what
// `GenericMmapStore` needs to persist its companion record blob
// (`STORAGE-015-FR-006`) — nothing else about them changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    Shipped,
    Delivered,
    Cancelled,
    Refunded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub amount_cents: i64,
    pub status: OrderStatus,
    pub created_at_unix_ms: i64,
    pub discount_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub name: String,
}

impl Record for Order {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

// The name written into every `Order` companion blob's header
// (`SCHTAG-FR-007`). Part of the on-disk format: renaming it is a format
// change for every existing `.records`/`.edges` file holding `Order`s.
impl SchemaTag for Order {
    const SCHEMA_TAG: &'static str = "order_customer::Order";
}

impl Record for Customer {
    type Id = Uuid;
    fn id(&self) -> Uuid {
        self.id
    }
}

pub struct Status;
impl IndexedField<Status> for Order {
    type IndexValue = OrderStatus;
    fn indexed_value(&self) -> &OrderStatus {
        &self.status
    }
}

pub struct Amount;
impl ScannableField<Amount> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.amount_cents
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.amount_cents = value;
    }
}

/// A *second* scannable field — the case `Dog` never exercised (it only
/// ever had `Age`).
pub struct CreatedAt;
impl ScannableField<CreatedAt> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.created_at_unix_ms
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.created_at_unix_ms = value;
    }
}

/// A *third* scannable field — validates that adding one costs exactly one
/// entry in the `forward_scannable_pairs!` invocation below, nothing else.
pub struct DiscountCents;
impl ScannableField<DiscountCents> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.discount_cents
    }
    fn set_scannable_value(&mut self, value: i64) {
        self.discount_cents = value;
    }
}

pub struct BelongsToCustomer;
impl ChildOf<BelongsToCustomer> for Order {
    type ParentId = Uuid;
    // Every order has exactly one customer — the mandatory-parent case
    // `ChildOf::parent_id` (now `Option<Self::ParentId>`, see that trait's
    // own doc comment) is allowed to model but doesn't require: this impl
    // simply never returns `None`.
    fn parent_id(&self) -> Option<Uuid> {
        Some(self.customer_id)
    }
}

// One invocation per layer kind, each covering all three scannable
// fields' pairs — see `store.rs`'s `forward_scannable_pairs!` module docs
// for why this can't instead be one generic impl. `MmapScanned` only
// ever owns `DiscountCents` in this module, but listing all three keeps
// the two layers interchangeable per field (`STORAGE-017-FR-008`).
crate::forward_scannable_pairs!(Order; Amount: i64, CreatedAt: i64, DiscountCents: i64);
crate::forward_scannable_pairs!(for MmapScanned; Order; Amount: i64, CreatedAt: i64, DiscountCents: i64);

/// The full in-memory composed stack for `Order`/`Customer`: `BaseStore`
/// (owns `Order` records) -> `Indexed<.., Status>` -> `Scanned<.., Amount>`
/// -> `Scanned<.., CreatedAt>` -> `Scanned<.., DiscountCents>` ->
/// `Reversed<.., Customer, Order, BelongsToCustomer>`. Purely in-memory —
/// see [`OrderProductionStack`] for the durable analogue.
pub type OrderGenericStore = Reversed<
    Scanned<
        Scanned<Scanned<Indexed<BaseStore<Order>, Order, Status>, Order, Amount>, Order, CreatedAt>,
        Order,
        DiscountCents,
    >,
    Customer,
    Order,
    BelongsToCustomer,
>;

pub fn build_order_generic_store(orders: &[Order]) -> OrderGenericStore {
    let base = BaseStore::new(orders.to_vec());
    let indexed = Indexed::<_, Order, Status>::new(base, orders);
    let scanned_amount = Scanned::<_, Order, Amount>::new(indexed, orders);
    let scanned_created_at = Scanned::<_, Order, CreatedAt>::new(scanned_amount, orders);
    let scanned_discount = Scanned::<_, Order, DiscountCents>::new(scanned_created_at, orders);
    Reversed::<_, Customer, Order, BelongsToCustomer>::new(scanned_discount, orders)
}

/// The durable production stack (`STORAGE-017`): [`GenericMmapStore`]
/// (owns records, the `Status` index, and `Amount` in the base slot file
/// at `path`) -> [`MmapScanned<.., Order, DiscountCents>`](MmapScanned)
/// (owns `DiscountCents` in its own slot file at
/// [`discount_cents_path`]`(path)`) -> `Reversed<.., Customer, Order,
/// BelongsToCustomer>` (the directed-relation reverse index, entirely
/// in-memory — relations are rebuilt from the record set at every `open`,
/// same convention every durability variant in this crate already
/// follows). `CreatedAt` is deliberately not part of this stack — see
/// module docs.
pub type OrderProductionStack = Reversed<
    MmapScanned<GenericMmapStore<Order, Status, Amount>, Order, DiscountCents>,
    Customer,
    Order,
    BelongsToCustomer,
>;

/// Where [`OrderProductionStack`] keeps its `DiscountCents` slot file,
/// derived from the base `path`: `<path>.discount_cents.mmap`. Derived
/// rather than supplied so the three constructors below take the same
/// single `path` the `Amount`-only stack took, and so the companion blob
/// (`<path>.records`) and every slot file sit next to each other.
pub fn discount_cents_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".discount_cents.mmap");
    std::path::PathBuf::from(name)
}

fn layer_and_reverse(
    core: GenericMmapStore<Order, Status, Amount>,
    orders: &[Order],
    path: &std::path::Path,
    open: bool,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let discount_path = discount_cents_path(path);
    let layered = if open {
        MmapScanned::<_, Order, DiscountCents>::open(core, orders, &discount_path)?
    } else {
        MmapScanned::<_, Order, DiscountCents>::create(core, orders, &discount_path)?
    };
    Ok(Reversed::<_, Customer, Order, BelongsToCustomer>::new(
        layered, orders,
    ))
}

/// Build a fresh, durable production store for `Order`/`Customer` at
/// `path` — the generic analogue of `ProductionStore::create`. Writes
/// three files: the base slot file at `path`, its companion record blob,
/// and the `DiscountCents` slot file at [`discount_cents_path`]`(path)`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::create`] and [`MmapScanned::create`] do.
pub fn create_order_production_stack(
    orders: Vec<Order>,
    path: &std::path::Path,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Order, Status, Amount>::create(orders.clone(), path)?;
    layer_and_reverse(core, &orders, path, false)
}

/// Reopen an existing durable production store for `Order`/`Customer` at
/// `path` — the generic analogue of `ProductionStore::open`. Each slot
/// file reconciles against `orders` independently (`STORAGE-017-FR-004`).
///
/// # Errors
///
/// Returns whatever [`GenericMmapStore::open`] and [`MmapScanned::open`]
/// return — including
/// [`crate::durability::DurabilityError::SlotWidthMismatch`] when the
/// `DiscountCents` file was written for another record shape.
pub fn open_order_production_stack(
    orders: Vec<Order>,
    path: &std::path::Path,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Order, Status, Amount>::open(orders.clone(), path)?;
    layer_and_reverse(core, &orders, path, true)
}

/// Reopen an existing durable production store for `Order`/`Customer`
/// from its files alone — no `orders` argument — the generic analogue
/// of `ProductionStore::open_portable` (`STORAGE-015-FR-008`,
/// `STORAGE-017-FR-005`). Reads the record set back from the companion
/// blob ([`GenericMmapStore::read_portable_records`]) and then builds the
/// stack exactly as [`open_order_production_stack`] does, so the
/// `Reversed` layer's per-customer child order follows the blob's
/// persisted (creation) order. Only the base slot file has a companion
/// blob; the `DiscountCents` file reconciles against the records the
/// blob yields, like any other `open`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::RecordBlobUnreadable`]
/// if the companion blob is missing or unreadable, plus everything
/// [`open_order_production_stack`] can return.
pub fn open_order_production_stack_portable(
    path: &std::path::Path,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let orders = GenericMmapStore::<Order, Status, Amount>::read_portable_records(path)?;
    open_order_production_stack(orders, path)
}

#[cfg(test)]
mod tests {
    use super::super::query::{Children, FilterEq, GetById, Parent, ScanField, UpdateField};
    use super::*;

    fn sample() -> Vec<Order> {
        vec![
            Order {
                id: Uuid::from_u128(1),
                customer_id: Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
                discount_cents: 50,
            },
            Order {
                id: Uuid::from_u128(2),
                customer_id: Uuid::from_u128(100),
                amount_cents: 4_200,
                status: OrderStatus::Pending,
                created_at_unix_ms: 2_000,
                discount_cents: 0,
            },
            Order {
                id: Uuid::from_u128(3),
                customer_id: Uuid::from_u128(200),
                amount_cents: 999,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 3_000,
                discount_cents: 100,
            },
        ]
    }

    #[test]
    fn full_stack_get_filter_scan_all_fields_parent_and_children_all_work() {
        let store = build_order_generic_store(&sample());

        assert_eq!(
            GetById::<Order>::get(&store, Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );
        assert_eq!(GetById::<Order>::get(&store, Uuid::from_u128(99)), None);

        let mut shipped = FilterEq::<Order, Status>::filter_eq(&store, &OrderStatus::Shipped);
        shipped.sort();
        let mut expected = vec![Uuid::from_u128(1), Uuid::from_u128(3)];
        expected.sort();
        assert_eq!(shipped, expected);

        let mut amounts = ScanField::<Order, Amount>::scan(&store);
        amounts.sort_unstable();
        assert_eq!(amounts, vec![999, 2_500, 4_200]);

        let mut created_ats = ScanField::<Order, CreatedAt>::scan(&store);
        created_ats.sort_unstable();
        assert_eq!(created_ats, vec![1_000, 2_000, 3_000]);

        let mut discounts = ScanField::<Order, DiscountCents>::scan(&store);
        discounts.sort_unstable();
        assert_eq!(discounts, vec![0, 50, 100]);

        assert_eq!(
            Parent::<Order, BelongsToCustomer>::parent(&store, Uuid::from_u128(1)),
            Ok(Some(Uuid::from_u128(100)))
        );

        let mut customer_100_orders =
            Children::<Customer, Order, BelongsToCustomer>::children(&store, Uuid::from_u128(100));
        customer_100_orders.sort();
        let mut expected_children = vec![Uuid::from_u128(1), Uuid::from_u128(2)];
        expected_children.sort();
        assert_eq!(customer_100_orders, expected_children);
    }

    #[test]
    fn update_field_forwards_through_two_layers_and_is_immediately_visible() {
        let mut store = build_order_generic_store(&sample());

        UpdateField::<Order, Amount>::update(&mut store, Uuid::from_u128(1), 12_345).unwrap();
        assert!(ScanField::<Order, Amount>::scan(&store).contains(&12_345));

        let err =
            UpdateField::<Order, Amount>::update(&mut store, Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err.0, Uuid::from_u128(99));
    }

    /// The regression test that should have existed from the first round:
    /// write, then immediately read via `GetById::get`, expect the write
    /// to be visible — against the purely in-memory composed stack
    /// (`build_order_generic_store`), not `GenericMmapStore`. Neither
    /// prior spike (the field-focused round or the directed-relation
    /// round) exercised this combination, which is exactly why the gap
    /// went unnoticed until wiring the durable path's own `get` incidentally
    /// required the same guarantee. Covers both the innermost stacked
    /// `Scanned` layer (`Amount`) and the outermost one (`DiscountCents`)
    /// so a fix that only happens to work for one position in the stack
    /// can't pass silently, and checks that untouched fields on the same
    /// record are unaffected — the fix patches one field at a time as
    /// `get` unwinds through each layer, not the whole record at once.
    #[test]
    fn get_reflects_a_prior_update_through_every_layer_of_the_in_memory_stack() {
        let mut store = build_order_generic_store(&sample());

        // Innermost Scanned layer (Amount) — the write must survive being
        // forwarded back up through the two further Scanned layers stacked
        // on top of it (CreatedAt, DiscountCents) and the outermost
        // Reversed layer.
        UpdateField::<Order, Amount>::update(&mut store, Uuid::from_u128(1), 99_999).unwrap();
        let order = GetById::<Order>::get(&store, Uuid::from_u128(1)).unwrap();
        assert_eq!(order.amount_cents, 99_999);
        assert_eq!(order.created_at_unix_ms, 1_000);
        assert_eq!(order.discount_cents, 50);

        // Outermost Scanned layer (DiscountCents) — proves the fix isn't
        // order-dependent (only catching the innermost layer's writes).
        UpdateField::<Order, DiscountCents>::update(&mut store, Uuid::from_u128(3), 777).unwrap();
        let order = GetById::<Order>::get(&store, Uuid::from_u128(3)).unwrap();
        assert_eq!(order.discount_cents, 777);
        assert_eq!(order.amount_cents, 999);

        // A record nobody wrote to still reads its original values.
        let untouched = GetById::<Order>::get(&store, Uuid::from_u128(2)).unwrap();
        assert_eq!(untouched.amount_cents, 4_200);
        assert_eq!(untouched.discount_cents, 0);
    }

    #[test]
    fn adding_a_third_scannable_field_only_touches_the_macro_invocation() {
        let mut store = build_order_generic_store(&sample());

        UpdateField::<Order, DiscountCents>::update(&mut store, Uuid::from_u128(2), 250).unwrap();
        let mut discounts = ScanField::<Order, DiscountCents>::scan(&store);
        discounts.sort_unstable();
        assert_eq!(discounts, vec![50, 100, 250]);
    }

    #[allow(dead_code)]
    fn _pair_exists<S, Owner, Forwarded>()
    where
        S: ScanField<Order, Forwarded>,
        Scanned<S, Order, Owner>: ScanField<Order, Forwarded>,
        Order: ScannableField<Owner> + ScannableField<Forwarded>,
    {
    }

    #[allow(dead_code)]
    fn _all_six_ordered_pairs_exist<S: ScanField<Order, Amount>>() {
        _pair_exists::<S, CreatedAt, Amount>();
        _pair_exists::<S, DiscountCents, Amount>();
    }

    #[allow(dead_code)]
    fn _all_six_ordered_pairs_exist_2<S: ScanField<Order, CreatedAt>>() {
        _pair_exists::<S, Amount, CreatedAt>();
        _pair_exists::<S, DiscountCents, CreatedAt>();
    }

    #[allow(dead_code)]
    fn _all_six_ordered_pairs_exist_3<S: ScanField<Order, DiscountCents>>() {
        _pair_exists::<S, Amount, DiscountCents>();
        _pair_exists::<S, CreatedAt, DiscountCents>();
    }

    // `STORAGE-017-FR-008`: the same six ordered pairs exist for the
    // durable layer, generated by the macro's `for MmapScanned` arm — no
    // hand-written forwarding impl anywhere in `mmap_scanned.rs`.
    #[allow(dead_code)]
    fn _mmap_pair_exists<S, Owner, Forwarded>()
    where
        S: ScanField<Order, Forwarded> + UpdateField<Order, Forwarded>,
        MmapScanned<S, Order, Owner>: ScanField<Order, Forwarded> + UpdateField<Order, Forwarded>,
        Order: ScannableField<Owner> + ScannableField<Forwarded>,
        <Order as ScannableField<Owner>>::ScanValue: super::super::mmap_field::MmapFieldValue,
    {
    }

    #[allow(dead_code)]
    fn _all_six_mmap_pairs_exist<
        S: ScanField<Order, Amount>
            + UpdateField<Order, Amount>
            + ScanField<Order, CreatedAt>
            + UpdateField<Order, CreatedAt>
            + ScanField<Order, DiscountCents>
            + UpdateField<Order, DiscountCents>,
    >() {
        _mmap_pair_exists::<S, CreatedAt, Amount>();
        _mmap_pair_exists::<S, DiscountCents, Amount>();
        _mmap_pair_exists::<S, Amount, CreatedAt>();
        _mmap_pair_exists::<S, DiscountCents, CreatedAt>();
        _mmap_pair_exists::<S, Amount, DiscountCents>();
        _mmap_pair_exists::<S, CreatedAt, DiscountCents>();
    }

    #[test]
    fn create_then_read_and_write_as_production_stack() {
        let dir = crate::bench_support::fresh_temp_dir("order_production_basic").unwrap();
        let path = dir.join("amount.mmap");
        let mut stack = create_order_production_stack(sample(), &path).unwrap();

        assert_eq!(
            GetById::<Order>::get(&stack, Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            2_500
        );
        UpdateField::<Order, Amount>::update(&mut stack, Uuid::from_u128(1), 8_000).unwrap();
        assert_eq!(
            GetById::<Order>::get(&stack, Uuid::from_u128(1))
                .unwrap()
                .amount_cents,
            8_000
        );
        assert_eq!(
            Children::<Customer, Order, BelongsToCustomer>::children(&stack, Uuid::from_u128(100))
                .len(),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `STORAGE-017` criterion 1 through the real stack: both durable
    /// fields update, flush, and reopen independently; `CreatedAt` reads
    /// back from the caller-supplied records because nothing persists it.
    #[test]
    fn both_durable_fields_survive_flush_and_reopen_through_the_stack() {
        let dir = crate::bench_support::fresh_temp_dir("order_production_two_fields").unwrap();
        let path = dir.join("amount.mmap");

        {
            use super::super::store::Flush;
            let mut stack = create_order_production_stack(sample(), &path).unwrap();
            UpdateField::<Order, Amount>::update(&mut stack, Uuid::from_u128(1), 8_000).unwrap();
            UpdateField::<Order, DiscountCents>::update(&mut stack, Uuid::from_u128(1), 640)
                .unwrap();
            // `CreatedAt` has no layer in this stack, so
            // `UpdateField::<Order, CreatedAt>` doesn't exist for it —
            // a compile error, not a runtime one.
            Flush::flush(&stack).unwrap();
        }
        assert!(discount_cents_path(&path).is_file());

        let reopened = open_order_production_stack(sample(), &path).unwrap();
        let order = GetById::<Order>::get(&reopened, Uuid::from_u128(1)).unwrap();
        assert_eq!(order.amount_cents, 8_000);
        assert_eq!(order.discount_cents, 640);
        assert_eq!(order.created_at_unix_ms, 1_000);

        let mut discounts = ScanField::<Order, DiscountCents>::scan(&reopened);
        discounts.sort_unstable();
        assert_eq!(discounts, vec![0, 100, 640]);
        let mut amounts = ScanField::<Order, Amount>::scan(&reopened);
        amounts.sort_unstable();
        assert_eq!(amounts, vec![999, 4_200, 8_000]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_production_stack_sees_the_written_value() {
        let dir = crate::bench_support::fresh_temp_dir("order_production_roundtrip").unwrap();
        let path = dir.join("amount.mmap");

        {
            use super::super::store::Flush;
            let mut stack = create_order_production_stack(sample(), &path).unwrap();
            UpdateField::<Order, Amount>::update(&mut stack, Uuid::from_u128(3), 55_555).unwrap();
            Flush::flush(&stack).unwrap();
        }

        let reopened = open_order_production_stack(sample(), &path).unwrap();
        assert_eq!(
            GetById::<Order>::get(&reopened, Uuid::from_u128(3))
                .unwrap()
                .amount_cents,
            55_555
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `STORAGE-015-FR-008`: the two files alone are enough to rebuild the
    /// whole stack — the `Reversed` relation layer (parent/children), the
    /// non-durable `created_at`/`discount_cents` fields, and the durable
    /// `Amount` value as last flushed — with no `orders` argument.
    #[test]
    fn open_portable_rebuilds_the_full_stack_from_the_two_files_alone() {
        let dir = crate::bench_support::fresh_temp_dir("order_production_portable").unwrap();
        let path = dir.join("amount.mmap");

        {
            use super::super::store::Flush;
            let mut stack = create_order_production_stack(sample(), &path).unwrap();
            UpdateField::<Order, Amount>::update(&mut stack, Uuid::from_u128(2), 77_777).unwrap();
            Flush::flush(&stack).unwrap();
        }

        let reopened = open_order_production_stack_portable(&path).unwrap();

        // Durable field: the flushed value wins over the blob's snapshot.
        let order_2 = GetById::<Order>::get(&reopened, Uuid::from_u128(2)).unwrap();
        assert_eq!(order_2.amount_cents, 77_777);
        // Non-durable fields come back from the blob exactly as created.
        assert_eq!(order_2.created_at_unix_ms, 2_000);
        assert_eq!(order_2.discount_cents, 0);
        assert_eq!(order_2.status, OrderStatus::Pending);
        assert_eq!(order_2.customer_id, Uuid::from_u128(100));

        // The in-memory relation layer is rebuilt from the blob's records.
        assert_eq!(
            Parent::<Order, BelongsToCustomer>::parent(&reopened, Uuid::from_u128(3)),
            Ok(Some(Uuid::from_u128(200)))
        );
        let mut customer_100_orders = Children::<Customer, Order, BelongsToCustomer>::children(
            &reopened,
            Uuid::from_u128(100),
        );
        customer_100_orders.sort();
        assert_eq!(
            customer_100_orders,
            vec![Uuid::from_u128(1), Uuid::from_u128(2)]
        );

        // And the index over the non-durable `status` field.
        let mut shipped = FilterEq::<Order, Status>::filter_eq(&reopened, &OrderStatus::Shipped);
        shipped.sort();
        assert_eq!(shipped, vec![Uuid::from_u128(1), Uuid::from_u128(3)]);

        // `STORAGE-017` criterion 4: updates to the second durable field
        // survive a portable reopen too, and never touch the blob — the
        // blob is written at `create` and rewritten only by an `open`
        // that found it stale, not by field updates.
        let companion = super::super::record_blob::blob_path(&path);
        let blob_before = std::fs::read(&companion).unwrap();
        {
            use super::super::store::Flush;
            let mut stack = open_order_production_stack_portable(&path).unwrap();
            UpdateField::<Order, DiscountCents>::update(&mut stack, Uuid::from_u128(3), 333)
                .unwrap();
            Flush::flush(&stack).unwrap();
        }
        assert_eq!(std::fs::read(&companion).unwrap(), blob_before);
        let reopened = open_order_production_stack_portable(&path).unwrap();
        let order_3 = GetById::<Order>::get(&reopened, Uuid::from_u128(3)).unwrap();
        assert_eq!(order_3.discount_cents, 333);
        assert_eq!(order_3.amount_cents, 999);

        // Without the companion, the portable path fails naming it; the
        // caller-supplied path still works.
        std::fs::remove_file(&companion).unwrap();
        match open_order_production_stack_portable(&path) {
            Err(crate::durability::DurabilityError::RecordBlobUnreadable { path: p, .. }) => {
                assert_eq!(p, companion)
            }
            Err(other) => panic!("expected RecordBlobUnreadable, got {other:?}"),
            Ok(_) => panic!("expected RecordBlobUnreadable, got a store"),
        }
        assert!(open_order_production_stack(sample(), &path).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
