//! Deterministic synthetic `Order` dataset generation for
//! `benches/order_relation_spike.rs` — the `Order`/`Customer` analogue of
//! [`crate::generator`] (which is `Dog`-specific and part of the
//! already-merged, benchmarked path this spike stays isolated from; see
//! `mod.rs`'s module docs). Self-contained rather than extending
//! `generator.rs` or `bench_support.rs`, per this round's isolation
//! constraint.

use super::order_impl::{Order, OrderStatus};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use uuid::{Builder, Uuid};

/// Seed for this spike's dataset generation — independent of
/// [`crate::bench_support::SEED`] (the `Dog` benchmark seed) and
/// [`crate::generator`]'s, so this round's numbers can't accidentally
/// entangle with either.
const SEED: u64 = 0x4F52_4445_5253_5049; // "ORDERSPI" in ASCII hex, arbitrary

/// Average orders per customer, held fixed independent of dataset size —
/// same rationale as `bench_support::BREED_CARDINALITY`/
/// `LITTERMATE_AVG_DEGREE`: dataset *size* stays the only swept dimension.
/// `littermate_of` used an average out-degree of 1.5 (deliberately low, a
/// handful of edges per node); this uses a comparably modest 5 orders per
/// customer, the directed relation's analogous "typical fan-out."
const AVG_ORDERS_PER_CUSTOMER: usize = 5;

/// How many distinct target ids to rotate through for point-workload
/// benchmarks — mirrors `bench_support::SAMPLE_TARGET_COUNT`.
pub const SAMPLE_TARGET_COUNT: usize = 200;

/// A generated `Order` dataset plus pre-selected rotation pools: order ids
/// (for `Parent` lookups) and customer ids (for `Children` lookups).
pub struct OrderDataset {
    pub orders: Vec<Order>,
    pub sample_order_ids: Vec<Uuid>,
    pub sample_customer_ids: Vec<Uuid>,
}

/// Build a benchmark dataset of `n` orders, spread across
/// `n / AVG_ORDERS_PER_CUSTOMER` distinct customers (at least one).
pub fn build_order_dataset(n: usize) -> OrderDataset {
    let mut rng = StdRng::seed_from_u64(SEED);
    let customer_count = (n / AVG_ORDERS_PER_CUSTOMER).max(1);
    let customer_ids: Vec<Uuid> = (0..customer_count).map(|_| random_uuid(&mut rng)).collect();

    let orders: Vec<Order> = (0..n)
        .map(|i| {
            let customer_id = customer_ids[rng.gen_range(0..customer_count)];
            Order {
                id: random_uuid(&mut rng),
                customer_id,
                amount_cents: rng.gen_range(0..1_000_000),
                status: STATUSES[rng.gen_range(0..STATUSES.len())],
                created_at_unix_ms: i as i64,
                discount_cents: rng.gen_range(0..500),
            }
        })
        .collect();

    // Independent RNG stream for target selection, same
    // SEED-XOR-a-constant convention `bench_support::build_dataset` and
    // `generator::generate_littermates` both use, so sample selection
    // doesn't perturb (or get perturbed by) the dataset-generation stream
    // above.
    let mut sample_rng = StdRng::seed_from_u64(SEED ^ 0x1234_5678_9ABC_DEF0);
    let sample_order_ids = sample(
        &mut sample_rng,
        &orders.iter().map(|o| o.id).collect::<Vec<_>>(),
    );
    let sample_customer_ids = sample(&mut sample_rng, &customer_ids);

    OrderDataset {
        orders,
        sample_order_ids,
        sample_customer_ids,
    }
}

const STATUSES: [OrderStatus; 5] = [
    OrderStatus::Pending,
    OrderStatus::Shipped,
    OrderStatus::Delivered,
    OrderStatus::Cancelled,
    OrderStatus::Refunded,
];

fn random_uuid(rng: &mut StdRng) -> Uuid {
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    Builder::from_random_bytes(bytes).into_uuid()
}

/// `SAMPLE_TARGET_COUNT` ids drawn (with replacement, for simplicity) from
/// `pool` — `pool` is always non-empty here (at least one customer, and
/// `n >= 1` order for any benchmark size actually used).
fn sample(rng: &mut StdRng, pool: &[Uuid]) -> Vec<Uuid> {
    (0..SAMPLE_TARGET_COUNT)
        .map(|_| pool[rng.gen_range(0..pool.len())])
        .collect()
}

/// Round-robins through a fixed pool size — identical shape to
/// `bench_support::RoundRobin`, duplicated here rather than shared, per
/// this crate's established convention of small explicit duplication
/// across structurally similar, independently-evolving pieces (see
/// `durability`'s `wal_buffered.rs` module docs for the precedent this
/// follows).
pub struct RoundRobin {
    next: usize,
    len: usize,
}

impl RoundRobin {
    pub fn new(len: usize) -> Self {
        Self { next: 0, len }
    }

    pub fn advance(&mut self) -> usize {
        let current = self.next;
        self.next = (self.next + 1) % self.len;
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_dataset_matches_requested_size() {
        let dataset = build_order_dataset(37);
        assert_eq!(dataset.orders.len(), 37);
    }

    #[test]
    fn every_order_references_a_real_customer() {
        let dataset = build_order_dataset(200);
        let customers: std::collections::HashSet<Uuid> =
            dataset.orders.iter().map(|o| o.customer_id).collect();
        // Sanity: at 200 orders / 5-per-customer, expect on the order of
        // 40 distinct customers, not 1 and not 200 (a degenerate
        // generator would produce either extreme).
        assert!(customers.len() > 5 && customers.len() < 200);
    }

    #[test]
    fn same_config_produces_identical_output() {
        let a = build_order_dataset(50);
        let b = build_order_dataset(50);
        assert_eq!(a.orders, b.orders);
    }
}
