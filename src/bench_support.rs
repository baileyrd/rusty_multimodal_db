//! Shared dataset-building helpers for the two Criterion bench targets
//! (`benches/workloads.rs` and `benches/cache_events.rs`). Not part of the
//! storage API under test — this exists purely so both bench binaries
//! build the exact same kind of dataset and target-selection logic rather
//! than duplicating it.

use crate::generator::{generate, GeneratorConfig};
use crate::record::DogRecord;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
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

/// How many distinct target UUIDs to rotate through for point-workload
/// benchmarks (`get`, `update_age`, `same_breed`).
pub const SAMPLE_TARGET_COUNT: usize = 200;

/// A generated dataset plus a pre-selected pool of target UUIDs for
/// point-workload benchmarks.
pub struct Dataset {
    pub records: Vec<DogRecord>,
    pub sample_ids: Vec<Uuid>,
}

/// Build a benchmark dataset of `n` records using the crate-wide fixed
/// cardinality and seed.
pub fn build_dataset(n: usize) -> Dataset {
    let config = GeneratorConfig::new(n, BREED_CARDINALITY, SEED)
        .expect("benchmark dataset sizes are all >= BREED_CARDINALITY");
    let records = generate(&config);

    let mut rng = StdRng::seed_from_u64(SEED ^ 0xA5A5_A5A5);
    let sample_count = SAMPLE_TARGET_COUNT.min(records.len());
    let sample_ids = (0..sample_count)
        .map(|_| records[rng.gen_range(0..records.len())].id)
        .collect();

    Dataset {
        records,
        sample_ids,
    }
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
}
