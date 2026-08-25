//! Shared dataset-building helpers for the two Criterion bench targets
//! (`benches/workloads.rs` and `benches/cache_events.rs`). Not part of the
//! storage API under test — this exists purely so both bench binaries
//! build the exact same kind of dataset and target-selection logic rather
//! than duplicating it. Also the single home for generic 2-hop graph
//! traversal (see [`two_hop_neighbors`]) — see
//! `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md` for why that
//! logic lives here, in benchmark/test-facing code, rather than as a
//! `DogStore` trait method.

use crate::generator::{generate, generate_littermates, GeneratorConfig};
use crate::record::DogRecord;
use crate::store::DogStore;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use uuid::Uuid;

/// Dataset sizes compared across every benchmark in this crate. 1M rather
/// than 10M as the upper bound for this first pass, to keep iteration
/// time reasonable — see `RESULTS.md`'s open questions for whether it's
/// worth pushing further.
pub const SIZES: [usize; 3] = [1_000, 100_000, 1_000_000];

/// Fixed, deliberately low breed cardinality (representative of "~50 real
/// dog breeds shared across however many dogs"), independent of dataset
/// size — the reuse-heavy case where a breed index is expected to help.
/// Held constant so dataset *size* is the only swept dimension; a
/// cardinality sweep is a candidate follow-up (see `RESULTS.md`).
pub const BREED_CARDINALITY: usize = 50;

/// Seed shared by every benchmark dataset, for reproducibility across
/// runs and across the two bench targets.
pub const SEED: u64 = 20_260_824;

/// Fixed average `littermate_of` out-degree (independent of dataset size),
/// same rationale as `BREED_CARDINALITY`: held constant so dataset *size*
/// is the only swept dimension. `1.5` sits mid-range in the valid `[0.0,
/// 3.0]` band, giving both backends' `neighbors`/two-hop benchmarks a
/// non-trivial (neither empty nor maxed-out) edge list to traverse.
pub const LITTERMATE_AVG_DEGREE: f64 = 1.5;

/// How many distinct target UUIDs to rotate through for point-workload
/// benchmarks (`get`, `update_age`, `same_breed`, `neighbors`).
pub const SAMPLE_TARGET_COUNT: usize = 200;

/// A generated dataset plus a pre-selected pool of target UUIDs for
/// point-workload benchmarks.
pub struct Dataset {
    pub records: Vec<DogRecord>,
    pub edges: Vec<(Uuid, Uuid)>,
    pub sample_ids: Vec<Uuid>,
}

/// Build a benchmark dataset of `n` records using the crate-wide fixed
/// cardinality, littermate degree, and seed.
pub fn build_dataset(n: usize) -> Dataset {
    let config = GeneratorConfig::new(n, BREED_CARDINALITY, LITTERMATE_AVG_DEGREE, SEED)
        .expect("benchmark dataset sizes are all >= BREED_CARDINALITY and degree is in range");
    let records = generate(&config);
    let edges = generate_littermates(&config, &records);

    let mut rng = StdRng::seed_from_u64(SEED ^ 0xA5A5_A5A5);
    let sample_count = SAMPLE_TARGET_COUNT.min(records.len());
    let sample_ids = (0..sample_count)
        .map(|_| records[rng.gen_range(0..records.len())].id)
        .collect();

    Dataset {
        records,
        edges,
        sample_ids,
    }
}

/// Generic 2-hop traversal built once from repeated [`DogStore::neighbors`]
/// calls: the deduplicated union of `store.neighbors(n)` for every `n` in
/// `store.neighbors(id)`. Deliberately lives here rather than as a trait
/// method — see
/// `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md`. No backend
/// is aware this exists; it only ever calls the public one-hop method.
pub fn two_hop_neighbors<S: DogStore>(store: &S, id: Uuid) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    for one_hop in store.neighbors(id) {
        for two_hop in store.neighbors(one_hop) {
            seen.insert(two_hop);
        }
    }
    seen.into_iter().collect()
}

/// Cycles through `0..len` once per [`RoundRobin::advance`] call, so
/// repeated benchmark iterations hit different target UUIDs rather than
/// keeping a single record's cache line artificially hot.
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
    fn round_robin_cycles() {
        let mut cursor = RoundRobin::new(3);
        assert_eq!([0, 1, 2, 0, 1], std::array::from_fn(|_| cursor.advance()));
    }

    #[test]
    fn build_dataset_matches_requested_size() {
        let dataset = build_dataset(500);
        assert_eq!(dataset.records.len(), 500);
        assert_eq!(dataset.sample_ids.len(), SAMPLE_TARGET_COUNT.min(500));
    }

    #[test]
    fn two_hop_neighbors_is_the_dedup_union_of_one_hop_neighbors_of_one_hop_results() {
        use crate::store::AosStore;

        let ids: Vec<Uuid> = (0..5).map(Uuid::from_u128).collect();
        // 0 -> 1, 1 -> 2, 1 -> 3, 2 -> 4: two-hop from 0 should be {2, 3}
        // (1's neighbors other than the path back to 0).
        let edges = vec![
            (ids[0], ids[1]),
            (ids[1], ids[2]),
            (ids[1], ids[3]),
            (ids[2], ids[4]),
        ];
        let store = AosStore::new(Vec::new(), edges);

        let mut result = two_hop_neighbors(&store, ids[0]);
        result.sort();
        let mut expected = vec![ids[0], ids[2], ids[3]];
        expected.sort();
        // ids[0] itself is included: 1's neighbors are {0, 2, 3}, and the
        // literal "neighbors of each one-hop result, deduplicated"
        // definition doesn't exclude the path back to the origin.
        assert_eq!(result, expected);
    }

    #[test]
    fn two_hop_neighbors_of_isolated_node_is_empty() {
        use crate::store::AosStore;

        let store = AosStore::new(Vec::new(), Vec::new());
        assert!(two_hop_neighbors(&store, Uuid::from_u128(0)).is_empty());
    }
}
