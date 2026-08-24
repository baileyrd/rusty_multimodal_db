//! Deterministic, configurable synthetic dataset generation.
//!
//! Every backend benchmarked in this crate is built from the exact same
//! [`generate`] output for a given [`GeneratorConfig`], so any measured
//! performance difference between backends is attributable to backend
//! design rather than to differences in the input data. See
//! `docs/specifications/storage/STORAGE-001-dataset-generator.md`.

use crate::record::DogRecord;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use thiserror::Error;
use uuid::{Builder, Uuid};

/// Inclusive bounds on generated `age` values, chosen so `scan_ages`'s
/// average is a small, sanity-checkable number.
const MIN_AGE: u32 = 0;
const MAX_AGE: u32 = 20;

/// Configuration error for [`GeneratorConfig::new`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GeneratorConfigError {
    /// Requesting records with no distinct breed to assign them.
    #[error(
        "breed_cardinality must be at least 1 when n > 0 (got n={n}, breed_cardinality={breed_cardinality})"
    )]
    ZeroCardinalityWithRecords { n: usize, breed_cardinality: usize },
}

/// Inputs to [`generate`]. The same config always produces the same
/// dataset (see [`generate`]'s docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratorConfig {
    n: usize,
    breed_cardinality: usize,
    seed: u64,
}

impl GeneratorConfig {
    /// Build a validated config.
    ///
    /// `breed_cardinality` is the number of distinct breed strings records
    /// are drawn from (with replacement) — the reuse/normalization ratio
    /// under test. Low cardinality relative to `n` means breeds repeat
    /// heavily (the case where a breed index is expected to help); raising
    /// `breed_cardinality` toward `n` sharply reduces repetition, though
    /// sampling with replacement still produces some collisions even at
    /// `breed_cardinality == n` (see [`generate`]'s docs).
    ///
    /// # Errors
    ///
    /// Returns [`GeneratorConfigError::ZeroCardinalityWithRecords`] if
    /// `n > 0` and `breed_cardinality == 0` — there would be nowhere to
    /// draw a breed from.
    pub fn new(
        n: usize,
        breed_cardinality: usize,
        seed: u64,
    ) -> Result<Self, GeneratorConfigError> {
        if n > 0 && breed_cardinality == 0 {
            return Err(GeneratorConfigError::ZeroCardinalityWithRecords {
                n,
                breed_cardinality,
            });
        }
        Ok(Self {
            n,
            breed_cardinality,
            seed,
        })
    }

    pub fn n(&self) -> usize {
        self.n
    }

    pub fn breed_cardinality(&self) -> usize {
        self.breed_cardinality
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }
}

/// Generate `config.n()` synthetic [`DogRecord`]s.
///
/// Deterministic: the same `config` always produces the same sequence of
/// records (same IDs, breeds, and ages in the same order), because
/// generation is driven entirely by a [`StdRng`] seeded from
/// `config.seed()` — there is no reliance on OS randomness anywhere in
/// this function, including for the generated UUIDs (built from
/// seeded-RNG bytes rather than `Uuid::new_v4()`, which pulls from OS
/// randomness).
///
/// Each record's breed is drawn uniformly at random *with replacement*
/// from a pool of `config.breed_cardinality()` distinct strings, so even
/// `breed_cardinality == n` doesn't make every breed unique — sampling
/// with replacement from a pool the same size as the draw count still
/// collides (expected distinct count ~63%, `n * (1 - 1/e)`).
///
/// Generated IDs are not deduplicated against each other. With 128-bit
/// random UUIDs, the collision probability across even 1,000,000 records
/// is astronomically small (birthday-bound, on the order of `n^2 / 2^128`)
/// — the same assumption every real-world UUID v4 user relies on. Adding
/// a collision-retry loop would add complexity for a risk this benchmark
/// harness doesn't need to guard against.
pub fn generate(config: &GeneratorConfig) -> Vec<DogRecord> {
    let mut rng = StdRng::seed_from_u64(config.seed);
    let breed_pool = breed_pool(config.breed_cardinality);

    (0..config.n)
        .map(|_| {
            let id = random_uuid(&mut rng);
            let breed_index = rng.gen_range(0..breed_pool.len());
            let age = rng.gen_range(MIN_AGE..=MAX_AGE);
            DogRecord::new(id, breed_pool[breed_index].clone(), age)
        })
        .collect()
}

/// Build a UUID from the seeded RNG rather than `Uuid::new_v4()`, which
/// pulls from OS randomness and would make [`generate`]'s output
/// non-reproducible.
fn random_uuid(rng: &mut StdRng) -> Uuid {
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    Builder::from_random_bytes(bytes).into_uuid()
}

/// `cardinality` distinct, deterministically-named breed strings to draw
/// from. Naming is index-based (not itself randomized) since the
/// randomness under test is *which* pooled breed each record gets, not
/// the pool's contents.
fn breed_pool(cardinality: usize) -> Vec<String> {
    (0..cardinality).map(|i| format!("breed-{i:04}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn same_config_produces_identical_output() {
        let config = GeneratorConfig::new(500, 10, 42).unwrap();
        let a = generate(&config);
        let b = generate(&config);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seed_produces_different_output() {
        let config_a = GeneratorConfig::new(500, 10, 1).unwrap();
        let config_b = GeneratorConfig::new(500, 10, 2).unwrap();
        assert_ne!(generate(&config_a), generate(&config_b));
    }

    #[test]
    fn generated_ids_are_unique() {
        let config = GeneratorConfig::new(50_000, 25, 7).unwrap();
        let records = generate(&config);
        let unique_ids: HashSet<_> = records.iter().map(|r| r.id).collect();
        assert_eq!(unique_ids.len(), records.len());
    }

    #[test]
    fn cardinality_one_gives_a_single_breed() {
        let config = GeneratorConfig::new(200, 1, 3).unwrap();
        let records = generate(&config);
        let unique_breeds: HashSet<_> = records.iter().map(|r| r.breed.as_str()).collect();
        assert_eq!(unique_breeds.len(), 1);
    }

    #[test]
    fn higher_cardinality_yields_more_distinct_breeds() {
        // Each record's breed is drawn uniformly at random *with
        // replacement* from the pool, so even cardinality == n doesn't
        // guarantee every breed is unique (birthday-paradox collisions are
        // expected: for a pool of size n and n draws, the expected number
        // of distinct values is n * (1 - 1/e) =~ 0.63n, not n). This test
        // checks the relative effect of cardinality, not exact uniqueness.
        let n = 2_000;
        let low = GeneratorConfig::new(n, 5, 9).unwrap();
        let high = GeneratorConfig::new(n, n, 9).unwrap();

        let low_unique: HashSet<_> = generate(&low).iter().map(|r| r.breed.clone()).collect();
        let high_unique: HashSet<_> = generate(&high).iter().map(|r| r.breed.clone()).collect();

        assert_eq!(low_unique.len(), 5);
        assert!(high_unique.len() > low_unique.len());
        // Expected distinct count at cardinality == n is ~0.63n; allow
        // generous slack around that for a non-flaky test.
        assert!(high_unique.len() as f64 > n as f64 * 0.5);
    }

    #[test]
    fn ages_are_within_bounds() {
        let config = GeneratorConfig::new(10_000, 5, 11).unwrap();
        let records = generate(&config);
        assert!(records.iter().all(|r| (MIN_AGE..=MAX_AGE).contains(&r.age)));
    }

    #[test]
    fn zero_records_produces_empty_dataset() {
        let config = GeneratorConfig::new(0, 0, 1).unwrap();
        assert!(generate(&config).is_empty());
    }

    #[test]
    fn zero_cardinality_with_records_is_a_config_error() {
        let result = GeneratorConfig::new(10, 0, 1);
        assert_eq!(
            result,
            Err(GeneratorConfigError::ZeroCardinalityWithRecords {
                n: 10,
                breed_cardinality: 0,
            })
        );
    }
}
