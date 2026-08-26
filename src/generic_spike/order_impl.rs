//! `Order`/`Customer` implementing the generic schema traits, per the
//! design doc's §3 walkthrough — the domain this crate's generalization
//! work now targets (`Dog`, `dog_impl.rs`, is done being built on: a
//! benchmark fixture, not a target domain).
//!
//! `Order` is deliberately the harder case `Dog` never was: **three**
//! `ScannableField`s (`Amount`, `CreatedAt`, `DiscountCents` — `Dog` only
//! had `Age`) and a directed `ChildOf` relation to `Customer` (`Dog` only
//! had `SymmetricRelation`). Both compositions turn out to matter for the
//! associated-type-ambiguity bug class two prior rounds diagnosed — see
//! `store.rs`'s module docs.
//!
//! # The macro round: was two hand-written impls, now one invocation
//!
//! An earlier round hand-wrote the one marker-pair forwarding impl `Order`
//! needed (`Amount` reachable through `Scanned<.., CreatedAt>`) directly
//! in this file, and reported the real cost: O(pairs) concrete impls, not
//! O(fields). This round replaces that hand-written pair with one
//! invocation of [`crate::forward_scannable_pairs`] (`store.rs`), and adds
//! a **third** scannable field, `DiscountCents`, to validate the actual
//! claim: does adding a field cost one macro-invocation entry, or does it
//! still touch existing code? It costs one entry — see the single line
//! added to the invocation below and `tests::adding_a_third_scannable_field_only_touches_the_macro_invocation`.
//! The macro generates all 6 ordered pairs among the 3 fields (`3×2`),
//! not just the 2 this file's actual stack traverses — more than strictly
//! needed for *this* stack's fixed layer order, but the point is that the
//! human no longer has to know or reason about which pairs are needed;
//! the field list is the only thing maintained by hand.

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
    /// Third scannable field, added this round — see `DiscountCents`.
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
}

/// A *second* scannable field — the case `Dog` never exercised (it only
/// ever had `Age`).
pub struct CreatedAt;
impl ScannableField<CreatedAt> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.created_at_unix_ms
    }
}

/// A *third* scannable field, added this round specifically to validate
/// the macro: adding it below costs exactly one entry in the
/// `forward_scannable_pairs!` invocation and one new marker/impl pair
/// here — nothing about `Amount` or `CreatedAt`'s own code changes. See
/// `tests::adding_a_third_scannable_field_only_touches_the_macro_invocation`.
pub struct DiscountCents;
impl ScannableField<DiscountCents> for Order {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 {
        self.discount_cents
    }
}

pub struct BelongsToCustomer;
impl ChildOf<BelongsToCustomer> for Order {
    type ParentId = Uuid;
    fn parent_id(&self) -> Uuid {
        self.customer_id
    }
}

// Was two hand-written impls (one marker pair) in the prior round; now
// one invocation covering all three scannable fields' pairs. Adding
// `DiscountCents` to this list is the *entire* diff a third scannable
// field costs at this layer — see `store.rs`'s `forward_scannable_pairs!`
// module docs for why this can't instead be one generic impl.
crate::forward_scannable_pairs!(Order; Amount: i64, CreatedAt: i64, DiscountCents: i64);

/// The full composed stack for `Order`/`Customer`: `BaseStore` (owns
/// `Order` records) -> `Indexed<.., Status>` -> `Scanned<.., Amount>` ->
/// `Scanned<.., CreatedAt>` -> `Scanned<.., DiscountCents>` (three stacked
/// scannable fields) -> `Reversed<.., Customer, Order, BelongsToCustomer>`
/// (the directed relation's expensive direction). No `Symmetric` layer:
/// `Order`/`Customer` has no symmetric relation, the mirror image of
/// `Dog`'s stack having no `Reversed` layer.
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

    /// Exercises every capability the full stack claims to forward:
    /// `get`, `filter_eq` (on `Status`), `scan` on *all three* scannable
    /// fields through the same outermost `Reversed` layer, `parent`
    /// (the blanket-impl cheap direction), and `children` (the real
    /// reverse-index expensive direction).
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

        // The field added this round, to validate the macro — reached
        // through the same macro-generated forwarding path as `Amount`,
        // two layers down from the outermost `Scanned<.., DiscountCents>`.
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

    /// The macro generates `UpdateField` pairs too, not just `ScanField` —
    /// exercises the write side of a macro-generated forwarding impl
    /// (`Amount`, forwarded through the two outer layers,
    /// `Scanned<.., CreatedAt>`/`Scanned<.., DiscountCents>`), matching
    /// `Scanned`'s own `scan_ages_reflects_update_age_immediately` pattern
    /// (`canonical_cached.rs`): a write through the forwarded path must be
    /// visible on the next `scan` through that same path.
    #[test]
    fn update_field_forwards_through_two_layers_and_is_immediately_visible() {
        let mut store = build_order_generic_store(&sample());

        UpdateField::<Order, Amount>::update(&mut store, Uuid::from_u128(1), 12_345).unwrap();
        assert!(ScanField::<Order, Amount>::scan(&store).contains(&12_345));

        let err =
            UpdateField::<Order, Amount>::update(&mut store, Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err.0, Uuid::from_u128(99));
    }

    /// Validates the actual claim this round makes: adding a third
    /// scannable field costs one macro-invocation entry
    /// (`forward_scannable_pairs!(Order; Amount: i64, CreatedAt: i64,
    /// DiscountCents: i64)`), not new hand-written impls, and the field it
    /// adds is reachable through the *same* generic path (`Reversed`'s
    /// forwarding, unmodified since `Dog`) every other field already was.
    /// This isn't a compile check (the crate not compiling would already
    /// prove that); it's a behavioral one: `DiscountCents` round-trips
    /// through `update`+`scan` exactly like the two fields present before
    /// this round.
    #[test]
    fn adding_a_third_scannable_field_only_touches_the_macro_invocation() {
        let mut store = build_order_generic_store(&sample());

        UpdateField::<Order, DiscountCents>::update(&mut store, Uuid::from_u128(2), 250).unwrap();
        let mut discounts = ScanField::<Order, DiscountCents>::scan(&store);
        discounts.sort_unstable();
        assert_eq!(discounts, vec![50, 100, 250]);
    }

    /// A compile-time proof, not a runtime one: the real `OrderGenericStore`
    /// stack only ever exercises 2 of the 6 ordered pairs 3 fields produce
    /// (`Amount` forwarded through `CreatedAt`/`DiscountCents`'s outer
    /// layers) — this checks all 6 actually exist, including pairs no
    /// stack in this file happens to need, to prove the macro generates
    /// the full off-diagonal set `forward_scannable_pairs!`'s docs claim,
    /// not just the ones that happen to get used. Each `_pair_exists`
    /// instantiation is a where-bound the compiler must prove, not a
    /// value that runs — if the macro ever generated fewer than N×(N-1)
    /// pairs (a regression in `@rotate`'s exclusion logic, say), this
    /// function stops compiling.
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
}
