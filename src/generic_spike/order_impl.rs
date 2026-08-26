//! `Order`/`Customer` implementing the generic schema traits, per the
//! design doc's §3 walkthrough — the domain this file's motivating task
//! uses to diagnose the real scope of the associated-type ambiguity
//! pattern (`R::Value` ambiguous between two marker-parameterized traits)
//! that showed up once already in the `Dog` spike's `Scanned`/`FilterEq`
//! forwarding impl. `Dog` (`dog_impl.rs`) is left as historical reference,
//! not extended further — this crate's generalization work now targets
//! `Order`/`Customer` (or whatever comes after it), per the task.
//!
//! `Order` is deliberately the harder case `Dog` never was: **two**
//! `ScannableField`s (`Amount`, `CreatedAt`, where `Dog` only had `Age`)
//! and a directed `ChildOf` relation to `Customer` (where `Dog` only had
//! `SymmetricRelation`). Both of those are exactly the compositions that
//! turn out to matter for this bug class — see the module docs on
//! `store.rs`'s new forwarding impls and this file's tests.

use super::query::{ScanField, UpdateField};
use super::store::{BaseStore, Indexed, Reversed, Scanned};
use super::traits::{ChildOf, IndexedField, Record, ScannableField};
use super::NotFound;
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
}

/// A *second* scannable field — the case `Dog` never exercised (it only
/// ever had `Age`) and the one that turns out to matter most for this
/// bug class: see `store.rs`'s `Scanned`-forwarding-`ScanField`-for-
/// another-marker impls and this file's tests.
pub struct CreatedAt;
impl ScannableField<CreatedAt> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.created_at_unix_ms
    }
}

pub struct BelongsToCustomer;
impl ChildOf<BelongsToCustomer> for Order {
    type ParentId = Uuid;
    fn parent_id(&self) -> Uuid {
        self.customer_id
    }
}

// Stacking two `Scanned` layers (`Amount` inner, `CreatedAt` outer) needs
// the outer layer to also answer `ScanField<Order, Amount>`, so it can be
// reached through `Reversed`'s own generic forwarding on top. This can't
// be a generic "any other marker" impl — see `store.rs`'s module docs on
// why (`E0119`, not fixable by disambiguating an associated type at all)
// — so it's one concrete, hand-written impl for this exact marker pair,
// living here rather than in `store.rs` since it's domain-specific.
impl<S> ScanField<Order, Amount> for Scanned<S, Order, CreatedAt>
where
    S: ScanField<Order, Amount>,
{
    fn scan(&self) -> Vec<i64> {
        self.inner().scan()
    }
}

impl<S> UpdateField<Order, Amount> for Scanned<S, Order, CreatedAt>
where
    S: UpdateField<Order, Amount>,
{
    fn update(&mut self, id: Uuid, value: i64) -> Result<(), NotFound<Uuid>> {
        self.inner_mut().update(id, value)
    }
}

/// The full composed stack for `Order`/`Customer`: `BaseStore` (owns
/// `Order` records) -> `Indexed<.., Status>` -> `Scanned<.., Amount>` ->
/// `Scanned<.., CreatedAt>` (the second scannable field, stacked — this is
/// the new case) -> `Reversed<.., Customer, Order, BelongsToCustomer>`
/// (the directed relation's expensive direction). No `Symmetric` layer:
/// `Order`/`Customer` has no symmetric relation, the mirror image of
/// `Dog`'s stack having no `Reversed` layer.
pub type OrderGenericStore = Reversed<
    Scanned<Scanned<Indexed<BaseStore<Order>, Order, Status>, Order, Amount>, Order, CreatedAt>,
    Customer,
    Order,
    BelongsToCustomer,
>;

pub fn build_order_generic_store(orders: &[Order]) -> OrderGenericStore {
    let base = BaseStore::new(orders.to_vec());
    let indexed = Indexed::<_, Order, Status>::new(base, orders);
    let scanned_amount = Scanned::<_, Order, Amount>::new(indexed, orders);
    let scanned_created_at = Scanned::<_, Order, CreatedAt>::new(scanned_amount, orders);
    Reversed::<_, Customer, Order, BelongsToCustomer>::new(scanned_created_at, orders)
}

#[cfg(test)]
mod tests {
    use super::super::query::{Children, FilterEq, GetById, Parent};
    use super::*;

    fn sample() -> Vec<Order> {
        vec![
            Order {
                id: Uuid::from_u128(1),
                customer_id: Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
            },
            Order {
                id: Uuid::from_u128(2),
                customer_id: Uuid::from_u128(100),
                amount_cents: 4_200,
                status: OrderStatus::Pending,
                created_at_unix_ms: 2_000,
            },
            Order {
                id: Uuid::from_u128(3),
                customer_id: Uuid::from_u128(200),
                amount_cents: 999,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 3_000,
            },
        ]
    }

    /// Exercises every capability the full stack claims to forward:
    /// `get`, `filter_eq` (on `Status`), `scan` on *both* scannable
    /// fields through the same outermost `Reversed` layer, `parent`
    /// (the blanket-impl cheap direction), and `children` (the real
    /// reverse-index expensive direction).
    #[test]
    fn full_stack_get_filter_scan_both_fields_parent_and_children_all_work() {
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
}
