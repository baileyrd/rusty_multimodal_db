//! The naive baseline for `Order belongs_to Customer`: linear scan, no
//! index at all — the directed-relation analogue of [`AosStore`]'s own
//! `neighbors` (`src/store/aos.rs`), which scans `edges: Vec<(Uuid,
//! Uuid)>` linearly rather than consulting an adjacency index. This
//! exists purely as this round's speedup baseline (per the task: "the
//! equivalent of what AoS/SoA did before any index existed") — it isn't
//! meant to be a real alternative, any more than `AosStore` itself is
//! recommended for production use.
//!
//! # Why a linear scan over `orders`, not a separate edge list
//!
//! `littermate_of`'s naive baseline scans a *separate* `edges: Vec<(Uuid,
//! Uuid)>` list, because `littermate_of` is symmetric and ad-hoc — the
//! relationship isn't data any single record carries. `Order belongs_to
//! Customer` is structurally different from the start: the foreign key
//! (`customer_id`) already lives directly on every `Order` record (see
//! `order_impl.rs`'s `ChildOf` impl). So the naive baseline here scans
//! `orders` itself, not a parallel edge list — there is no edge list to
//! build in the first place, index or no index. This asymmetry between
//! the two lookups' naive costs (`parent`: find one record by id and read
//! a field; `children`: scan every record checking one field) mirrors the
//! adjacency-index side's own asymmetry (see `store.rs`'s blanket `Parent`
//! impl vs. `Reversed`'s real index) — it isn't introduced by this file,
//! it's inherent to the relation shape, and both variants have to live
//! with it.

use super::order_impl::{BelongsToCustomer, Customer, Order};
use super::query::{Children, Parent};
use super::traits::Record;
use uuid::Uuid;

/// Owns `orders` with no index of any kind — every lookup is a full,
/// unindexed pass over the slice.
pub struct NaiveOrderStore {
    orders: Vec<Order>,
}

impl NaiveOrderStore {
    pub fn new(orders: Vec<Order>) -> Self {
        Self { orders }
    }
}

/// The cheap direction, naively: still just "find the record, read the
/// field" — no adjacency structure was ever going to make *this* side
/// cheaper (see `store.rs`'s blanket `Parent` impl, which is exactly this
/// same shape, one `HashMap` lookup instead of a linear scan). What the
/// naive baseline actually costs here is the *linear scan* to find the
/// record at all — `BaseStore`'s `HashMap` avoids that; `NaiveOrderStore`
/// doesn't.
impl Parent<Order, BelongsToCustomer> for NaiveOrderStore {
    fn parent(&self, child_id: Uuid) -> Option<Uuid> {
        self.orders
            .iter()
            .find(|order| order.id() == child_id)
            .map(|order| order.customer_id)
    }
}

/// The expensive direction, naively: a full scan of every order, checking
/// one field — no better and no worse, asymptotically, than the adjacency-
/// index side's own `Reversed::children` before that index existed (this
/// is the "no `children_of` reverse index was ever built" case).
impl Children<Customer, Order, BelongsToCustomer> for NaiveOrderStore {
    fn children(&self, parent_id: Uuid) -> Vec<Uuid> {
        self.orders
            .iter()
            .filter(|order| order.customer_id == parent_id)
            .map(|order| order.id())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::order_impl::OrderStatus;
    use super::*;

    fn sample() -> Vec<Order> {
        vec![
            Order {
                id: Uuid::from_u128(1),
                customer_id: Uuid::from_u128(100),
                amount_cents: 2_500,
                status: OrderStatus::Shipped,
                created_at_unix_ms: 1_000,
                discount_cents: 0,
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
                discount_cents: 0,
            },
        ]
    }

    #[test]
    fn parent_finds_the_owning_customer() {
        let store = NaiveOrderStore::new(sample());
        assert_eq!(store.parent(Uuid::from_u128(1)), Some(Uuid::from_u128(100)));
        assert_eq!(store.parent(Uuid::from_u128(3)), Some(Uuid::from_u128(200)));
    }

    #[test]
    fn parent_unknown_id_is_none() {
        let store = NaiveOrderStore::new(sample());
        assert_eq!(store.parent(Uuid::from_u128(99)), None);
    }

    #[test]
    fn children_finds_every_order_for_a_customer() {
        let store = NaiveOrderStore::new(sample());
        let mut children = store.children(Uuid::from_u128(100));
        children.sort();
        let mut expected = vec![Uuid::from_u128(1), Uuid::from_u128(2)];
        expected.sort();
        assert_eq!(children, expected);
    }

    #[test]
    fn children_unknown_customer_is_empty() {
        let store = NaiveOrderStore::new(sample());
        assert!(store.children(Uuid::from_u128(999)).is_empty());
    }

    #[test]
    fn children_customer_with_one_order() {
        let store = NaiveOrderStore::new(sample());
        assert_eq!(
            store.children(Uuid::from_u128(200)),
            vec![Uuid::from_u128(3)]
        );
    }
}
