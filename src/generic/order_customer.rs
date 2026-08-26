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
//! relation to `Customer`. [`OrderProductionStack`] below wires `Amount`
//! specifically as the one durable, mmap-backed field (mirroring `Dog`'s
//! `age` — see `mmap_store.rs`'s module docs on why exactly one durable
//! field, not three); `CreatedAt`/`DiscountCents` stay in-memory-only
//! `Scanned` layers on top, reusing the very forwarding impls
//! `forward_scannable_pairs!` already generates below, unmodified, since
//! those impls are generic over whatever inner store type they wrap.

use super::mmap_store::GenericMmapStore;
use super::store::{BaseStore, Indexed, Reversed, Scanned};
use super::traits::{ChildOf, IndexedField, Record, ScannableField};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderStatus {
    Pending,
    Shipped,
    Delivered,
    Cancelled,
    Refunded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub amount_cents: i64,
    pub status: OrderStatus,
    pub created_at_unix_ms: i64,
    pub discount_cents: i64,
}

#[derive(Debug, Clone)]
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
    fn parent_id(&self) -> Uuid {
        self.customer_id
    }
}

// One invocation covering all three scannable fields' pairs — see
// `store.rs`'s `forward_scannable_pairs!` module docs for why this can't
// instead be one generic impl.
crate::forward_scannable_pairs!(Order; Amount: i64, CreatedAt: i64, DiscountCents: i64);

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

/// The durable production stack: [`GenericMmapStore`] (owns records, the
/// `Status` index, and `Amount` — the one mmap-backed durable field) ->
/// `Reversed<.., Customer, Order, BelongsToCustomer>` (the directed-
/// relation reverse index, entirely in-memory — relations are rebuilt
/// from the externally-supplied `records` at every `open`, same convention
/// every durability variant in this crate already follows). `CreatedAt`/
/// `DiscountCents` are deliberately not part of this stack — see module
/// docs for why exactly one scannable field is durable.
pub type OrderProductionStack =
    Reversed<GenericMmapStore<Order, Status, Amount>, Customer, Order, BelongsToCustomer>;

/// Build a fresh, durable production store for `Order`/`Customer` at
/// `path` — the generic analogue of `ProductionStore::create`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::create`] does.
pub fn create_order_production_stack(
    orders: Vec<Order>,
    path: &std::path::Path,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Order, Status, Amount>::create(orders.clone(), path)?;
    Ok(Reversed::<_, Customer, Order, BelongsToCustomer>::new(
        core, &orders,
    ))
}

/// Reopen an existing durable production store for `Order`/`Customer` at
/// `path` — the generic analogue of `ProductionStore::open`.
///
/// # Errors
///
/// Returns [`crate::durability::DurabilityError::Io`] under the same
/// conditions [`GenericMmapStore::open`] does.
pub fn open_order_production_stack(
    orders: Vec<Order>,
    path: &std::path::Path,
) -> Result<OrderProductionStack, crate::durability::DurabilityError> {
    let core = GenericMmapStore::<Order, Status, Amount>::open(orders.clone(), path)?;
    Ok(Reversed::<_, Customer, Order, BelongsToCustomer>::new(
        core, &orders,
    ))
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
            Some(Uuid::from_u128(100))
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
}
