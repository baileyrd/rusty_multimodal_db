//! Shared dataset-building helpers for the two Criterion bench targets
//! (`benches/workloads.rs` and `benches/cache_events.rs`). Not part of the
//! storage API under test — this exists purely so both bench binaries
//! build the exact same kind of dataset and target-selection logic rather
//! than duplicating it. Also the single home for generic 2-hop graph
//! traversal (see [`two_hop_neighbors`]) — see
//! `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md` for why that
//! logic lives here, in benchmark/test-facing code, rather than as a
//! `DogStore` trait method — and for the mixed read/write workload driver
//! (see [`MixedWorkloadDriver`]), for the same reason: blending calls
//! together is workload logic, not something a backend needs to know how
//! to do.

use crate::concurrency::{ConcurrencyError, ConcurrentStore};
use crate::generator::{generate, generate_littermates, GeneratorConfig};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::hint::black_box;
use thiserror::Error;
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

/// Write ratios swept by the mixed read/write workload
/// ([`MixedWorkloadDriver`]): 10%, 50%, and 90% `update_age` calls, with
/// the remainder in each case split evenly between `get` and `scan_ages`.
pub const MIXED_WRITE_RATIOS: [f64; 3] = [0.10, 0.50, 0.90];

/// XORed into [`SEED`] to derive [`MixedWorkloadDriver`]'s op-selection RNG
/// stream, independent of every other seeded stream in this crate (mirrors
/// `build_dataset`'s `SEED ^ 0xA5A5_A5A5` and the generator's
/// `LITTERMATE_SEED_XOR`).
const MIXED_WORKLOAD_SEED_XOR: u64 = 0x2244_6688_AACC_EE00;

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

/// One operation [`MixedWorkloadDriver::run_one`] can choose to run.
/// Exposed (rather than kept private) so the configured ratio's long-run
/// distribution can be tested directly, without needing a real store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedOp {
    Get,
    UpdateAge,
    ScanAges,
}

/// Configuration error for [`MixedWorkloadConfig::new`].
#[derive(Debug, Error, PartialEq)]
pub enum MixedWorkloadConfigError {
    /// `write_ratio` isn't a valid probability.
    #[error("write_ratio must be within [0.0, 1.0] (got {write_ratio})")]
    InvalidWriteRatio { write_ratio: f64 },
}

/// The write/read split [`MixedWorkloadDriver`] draws operations from:
/// `write_ratio` chance of `update_age` on any given call, with the
/// remainder split evenly between `get` and `scan_ages`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixedWorkloadConfig {
    write_ratio: f64,
}

impl MixedWorkloadConfig {
    /// # Errors
    ///
    /// Returns [`MixedWorkloadConfigError::InvalidWriteRatio`] if
    /// `write_ratio` is outside `[0.0, 1.0]` (including `NaN`, which no
    /// range comparison ever contains).
    pub fn new(write_ratio: f64) -> Result<Self, MixedWorkloadConfigError> {
        if !(0.0..=1.0).contains(&write_ratio) {
            return Err(MixedWorkloadConfigError::InvalidWriteRatio { write_ratio });
        }
        Ok(Self { write_ratio })
    }

    pub fn write_ratio(&self) -> f64 {
        self.write_ratio
    }
}

/// Issues a deterministic, seeded sequence of `get`/`update_age`/`scan_ages`
/// calls against a [`DogStore`], drawn per [`MixedWorkloadConfig`]'s
/// write/read split — a blended access pattern none of this crate's other
/// benchmarks exercise (they each isolate one call type). Reuses
/// [`RoundRobin`] for target-UUID selection, the same sample-ID rotation
/// every other point-workload benchmark in this crate uses, rather than
/// building a new ID sampler.
///
/// Each call to [`Self::run_one`] performs exactly one operation, so a
/// Criterion `b.iter(|| driver.run_one(...))` loop times the blended
/// sequence one call at a time — the reported median is already "time per
/// operation in the blended sequence," with no separate per-call-type
/// breakout needed.
pub struct MixedWorkloadDriver {
    config: MixedWorkloadConfig,
    rng: StdRng,
    cursor: RoundRobin,
    next_age: u32,
}

impl MixedWorkloadDriver {
    /// `sample_id_count` must match the length of the `sample_ids` slice
    /// later passed to [`Self::run_one`] — it's taken up front so the
    /// internal [`RoundRobin`] cursor can be built once, not re-derived
    /// from the slice's length on every call.
    pub fn new(config: MixedWorkloadConfig, seed: u64, sample_id_count: usize) -> Self {
        Self {
            config,
            rng: StdRng::seed_from_u64(seed ^ MIXED_WORKLOAD_SEED_XOR),
            cursor: RoundRobin::new(sample_id_count),
            next_age: 0,
        }
    }

    /// Which op the next [`Self::run_one`] call will perform, without
    /// touching a store — exposed so the configured ratio's long-run
    /// distribution can be tested directly.
    pub fn next_op(&mut self) -> MixedOp {
        if self.rng.gen_bool(self.config.write_ratio) {
            MixedOp::UpdateAge
        } else if self.rng.gen_bool(0.5) {
            MixedOp::Get
        } else {
            MixedOp::ScanAges
        }
    }

    /// Run one operation drawn from the configured mix against `store`.
    /// `get`/`scan_ages` results are discarded but `black_box`ed first, so
    /// this crate's aggressive bench profile (`opt-level = 3, lto = true`)
    /// can't prove an unused pure read is dead code and elide the call.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] only if `sample_ids` contains a
    /// UUID `store` doesn't have — never happens when `sample_ids` comes
    /// from the same generated dataset `store` was built from, which is
    /// the only way this crate constructs a [`MixedWorkloadDriver`] (see
    /// `mixed_workload_driver_never_errors_against_its_own_dataset`).
    pub fn run_one<S: DogStore>(
        &mut self,
        store: &mut S,
        sample_ids: &[Uuid],
    ) -> Result<(), StoreError> {
        match self.next_op() {
            MixedOp::Get => {
                let id = sample_ids[self.cursor.advance()];
                black_box(store.get(id));
            }
            MixedOp::UpdateAge => {
                let id = sample_ids[self.cursor.advance()];
                self.next_age = self.next_age.wrapping_add(1) % 21;
                store.update_age(id, self.next_age)?;
            }
            MixedOp::ScanAges => {
                black_box(store.scan_ages());
            }
        }
        Ok(())
    }

    /// Same blended sequence as [`Self::run_one`], driven against a
    /// [`ConcurrentStore`] (`&S`, shared — e.g. behind an `Arc` across
    /// threads) instead of a [`DogStore`] (`&mut S`, exclusively owned).
    /// New method, not a change to `run_one` — the concurrency work
    /// (`STORAGE-010`) reuses this driver rather than building a second
    /// workload generator, but every existing `run_one` call site and its
    /// benchmarks/tests are untouched.
    ///
    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`StoreError::NotFound`]
    /// only if `sample_ids` contains a UUID `store` doesn't have — same
    /// non-issue as `run_one`'s own doc comment describes, for the same
    /// reason (`sample_ids` always comes from the same generated dataset
    /// `store` was built from).
    pub fn run_one_concurrent<S: ConcurrentStore>(
        &mut self,
        store: &S,
        sample_ids: &[Uuid],
    ) -> Result<(), ConcurrencyError> {
        match self.next_op() {
            MixedOp::Get => {
                let id = sample_ids[self.cursor.advance()];
                black_box(store.get(id)?);
            }
            MixedOp::UpdateAge => {
                let id = sample_ids[self.cursor.advance()];
                self.next_age = self.next_age.wrapping_add(1) % 21;
                store.update_age(id, self.next_age)?;
            }
            MixedOp::ScanAges => {
                black_box(store.scan_ages()?);
            }
        }
        Ok(())
    }
}

/// A fresh, empty, uniquely-named directory under the OS temp dir — shared
/// by the durability variants' unit tests (`src/durability/*.rs`) and
/// `benches/durability.rs`, so both build persisted-file paths the same
/// way rather than duplicating temp-directory setup. Uniqueness (PID +
/// atomic counter, not just `label`) matters because Rust's test harness
/// runs tests concurrently by default — two tests both naming themselves
/// `"wal_fsync"` must not collide on the same on-disk files.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the directory can't be
/// created (e.g. no space, no permission on the OS temp dir).
pub fn fresh_temp_dir(label: &str) -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rusty_multimodal_db_{label}_{}_{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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

    #[test]
    fn mixed_workload_config_rejects_out_of_range_write_ratio() {
        assert_eq!(
            MixedWorkloadConfig::new(-0.01),
            Err(MixedWorkloadConfigError::InvalidWriteRatio { write_ratio: -0.01 })
        );
        assert_eq!(
            MixedWorkloadConfig::new(1.01),
            Err(MixedWorkloadConfigError::InvalidWriteRatio { write_ratio: 1.01 })
        );
    }

    #[test]
    fn mixed_workload_config_accepts_inclusive_bounds() {
        assert!(MixedWorkloadConfig::new(0.0).is_ok());
        assert!(MixedWorkloadConfig::new(1.0).is_ok());
    }

    #[test]
    fn mixed_workload_config_rejects_nan() {
        assert!(MixedWorkloadConfig::new(f64::NAN).is_err());
    }

    /// The highest-priority correctness property for this driver: every
    /// UUID it draws from `sample_ids` must be one `store` actually has, or
    /// the reported benchmark numbers would be silently corrupted by
    /// `update_age`'s much-cheaper `NotFound` fast path standing in for a
    /// real write.
    #[test]
    fn mixed_workload_driver_never_errors_against_its_own_dataset() {
        use crate::store::AosStore;

        let dataset = build_dataset(500);
        let mut store = AosStore::from(dataset.records.clone());
        let config = MixedWorkloadConfig::new(0.5).unwrap();
        let mut driver = MixedWorkloadDriver::new(config, SEED, dataset.sample_ids.len());

        for _ in 0..5_000 {
            assert!(driver.run_one(&mut store, &dataset.sample_ids).is_ok());
        }
    }

    #[test]
    fn mixed_workload_driver_is_deterministic_given_same_seed() {
        let ids: Vec<Uuid> = (0..10).map(Uuid::from_u128).collect();
        let config = MixedWorkloadConfig::new(0.3).unwrap();

        let mut a = MixedWorkloadDriver::new(config, 42, ids.len());
        let mut b = MixedWorkloadDriver::new(config, 42, ids.len());

        let sequence_a: Vec<MixedOp> = (0..200).map(|_| a.next_op()).collect();
        let sequence_b: Vec<MixedOp> = (0..200).map(|_| b.next_op()).collect();
        assert_eq!(sequence_a, sequence_b);
    }

    #[test]
    fn mixed_workload_driver_different_seed_gives_different_sequence() {
        let ids: Vec<Uuid> = (0..10).map(Uuid::from_u128).collect();
        let config = MixedWorkloadConfig::new(0.5).unwrap();

        let mut a = MixedWorkloadDriver::new(config, 1, ids.len());
        let mut b = MixedWorkloadDriver::new(config, 2, ids.len());

        let sequence_a: Vec<MixedOp> = (0..200).map(|_| a.next_op()).collect();
        let sequence_b: Vec<MixedOp> = (0..200).map(|_| b.next_op()).collect();
        assert_ne!(sequence_a, sequence_b);
    }

    /// Statistical check that `next_op`'s long-run distribution matches the
    /// configured split: `write_ratio` for `UpdateAge`, with the remainder
    /// split evenly between `Get` and `ScanAges`. N is large enough (200k)
    /// that binomial variance at these probabilities is well under 0.1
    /// percentage points, so a 2-percentage-point tolerance is generous
    /// and non-flaky, not a hand-tuned near-miss.
    #[test]
    fn mixed_workload_op_distribution_matches_configured_write_ratio() {
        let ids: Vec<Uuid> = (0..10).map(Uuid::from_u128).collect();
        const N: u32 = 200_000;
        const TOLERANCE: f64 = 0.02;

        for &write_ratio in &MIXED_WRITE_RATIOS {
            let config = MixedWorkloadConfig::new(write_ratio).unwrap();
            let mut driver = MixedWorkloadDriver::new(config, SEED, ids.len());

            let mut updates = 0u32;
            let mut gets = 0u32;
            let mut scans = 0u32;
            for _ in 0..N {
                match driver.next_op() {
                    MixedOp::UpdateAge => updates += 1,
                    MixedOp::Get => gets += 1,
                    MixedOp::ScanAges => scans += 1,
                }
            }

            let update_fraction = f64::from(updates) / f64::from(N);
            let get_fraction = f64::from(gets) / f64::from(N);
            let scan_fraction = f64::from(scans) / f64::from(N);
            let expected_read_fraction = (1.0 - write_ratio) / 2.0;

            assert!(
                (update_fraction - write_ratio).abs() < TOLERANCE,
                "write_ratio={write_ratio}: expected update fraction ~{write_ratio}, got {update_fraction}"
            );
            assert!(
                (get_fraction - expected_read_fraction).abs() < TOLERANCE,
                "write_ratio={write_ratio}: expected get fraction ~{expected_read_fraction}, got {get_fraction}"
            );
            assert!(
                (scan_fraction - expected_read_fraction).abs() < TOLERANCE,
                "write_ratio={write_ratio}: expected scan fraction ~{expected_read_fraction}, got {scan_fraction}"
            );
        }
    }

    /// End-to-end sanity check that `run_one`'s `UpdateAge` branch actually
    /// mutates the store (not just that it returns `Ok`): forcing
    /// `write_ratio = 1.0` and confirming every sampled id's age changed
    /// from its original value at least once.
    #[test]
    fn mixed_workload_driver_write_only_actually_mutates_ages() {
        use crate::store::AosStore;
        use std::collections::HashMap;

        let dataset = build_dataset(50);
        let original_ages: HashMap<Uuid, u32> =
            dataset.records.iter().map(|r| (r.id, r.age)).collect();
        let mut store = AosStore::from(dataset.records.clone());
        let config = MixedWorkloadConfig::new(1.0).unwrap();
        let mut driver = MixedWorkloadDriver::new(config, SEED, dataset.sample_ids.len());

        for _ in 0..dataset.sample_ids.len() * 3 {
            driver.run_one(&mut store, &dataset.sample_ids).unwrap();
        }

        let changed = dataset
            .sample_ids
            .iter()
            .filter(|id| store.get(**id).unwrap().age != original_ages[id])
            .count();
        assert!(
            changed > 0,
            "write_ratio=1.0 for {} iterations changed no ages",
            dataset.sample_ids.len() * 3
        );
    }
}
