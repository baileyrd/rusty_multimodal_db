//! Deterministic, configurable synthetic dataset generation.
//!
//! Every backend benchmarked in this crate is built from the exact same
//! [`generate`] output for a given [`GeneratorConfig`], so any measured
//! performance difference between backends is attributable to backend
//! design rather than to differences in the input data. See
//! `docs/specifications/storage/STORAGE-001-dataset-generator.md` (record
//! generation) and `STORAGE-006-...md` (the `littermate_of` edge
//! generation added alongside it, via [`generate_littermates`]).

use crate::record::DogRecord;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use thiserror::Error;
use uuid::{Builder, Uuid};

/// Inclusive bounds on generated `age` values, chosen so `scan_ages`'s
/// average is a small, sanity-checkable number.
const MIN_AGE: u32 = 0;
const MAX_AGE: u32 = 20;

/// Valid inclusive range for [`GeneratorConfig::littermate_avg_degree`] —
/// each dog gets 0-3 outgoing `littermate_of` edges (see
/// [`generate_littermates`]), so the configurable average can't exceed the
/// per-dog maximum.
const MAX_LITTERMATE_DEGREE: f64 = 3.0;

/// XORed into [`GeneratorConfig::seed`] to derive [`generate_littermates`]'s
/// RNG stream, so edge generation draws from a stream independent of
/// [`generate`]'s record-generation stream even though both come from the
/// same seed. Mirrors `bench_support`'s `SEED ^ 0xA5A5_A5A5` pattern for its
/// own independent (target-UUID-sampling) stream.
const LITTERMATE_SEED_XOR: u64 = 0x1177_3355_9911_7733;

/// Configuration error for [`GeneratorConfig::new`].
#[derive(Debug, Error, PartialEq)]
pub enum GeneratorConfigError {
    /// Requesting records with no distinct breed to assign them.
    #[error(
        "breed_cardinality must be at least 1 when n > 0 (got n={n}, breed_cardinality={breed_cardinality})"
    )]
    ZeroCardinalityWithRecords { n: usize, breed_cardinality: usize },
    /// Requesting an average littermate degree outside the range each dog's
    /// individual out-degree can actually take (0-3, see
    /// [`generate_littermates`]).
    #[error(
        "littermate_avg_degree must be within [0.0, {MAX_LITTERMATE_DEGREE}] (got {littermate_avg_degree})"
    )]
    InvalidLittermateAvgDegree { littermate_avg_degree: f64 },
}

/// Inputs to [`generate`] and [`generate_littermates`]. The same config
/// always produces the same dataset (see their docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeneratorConfig {
    n: usize,
    breed_cardinality: usize,
    littermate_avg_degree: f64,
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
    /// `littermate_avg_degree` is the expected number of outgoing
    /// `littermate_of` edges [`generate_littermates`] draws per dog (each
    /// dog gets 0-3, see that function's docs) — must be within `[0.0,
    /// 3.0]` since it can't exceed the per-dog maximum.
    ///
    /// # Errors
    ///
    /// Returns [`GeneratorConfigError::ZeroCardinalityWithRecords`] if
    /// `n > 0` and `breed_cardinality == 0` — there would be nowhere to
    /// draw a breed from. Returns
    /// [`GeneratorConfigError::InvalidLittermateAvgDegree`] if
    /// `littermate_avg_degree` is outside `[0.0, 3.0]`.
    pub fn new(
        n: usize,
        breed_cardinality: usize,
        littermate_avg_degree: f64,
        seed: u64,
    ) -> Result<Self, GeneratorConfigError> {
        if n > 0 && breed_cardinality == 0 {
            return Err(GeneratorConfigError::ZeroCardinalityWithRecords {
                n,
                breed_cardinality,
            });
        }
        if !(0.0..=MAX_LITTERMATE_DEGREE).contains(&littermate_avg_degree) {
            return Err(GeneratorConfigError::InvalidLittermateAvgDegree {
                littermate_avg_degree,
            });
        }
        Ok(Self {
            n,
            breed_cardinality,
            littermate_avg_degree,
            seed,
        })
    }

    pub fn n(&self) -> usize {
        self.n
    }

    pub fn breed_cardinality(&self) -> usize {
        self.breed_cardinality
    }

    pub fn littermate_avg_degree(&self) -> f64 {
        self.littermate_avg_degree
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

/// Generate the synthetic `littermate_of` relationship: each dog in
/// `records` gets 0-3 outgoing edges to other dogs in the same slice.
///
/// Deterministic given `config` and `records`: driven entirely by a
/// [`StdRng`] seeded from `config.seed()` XORed with a fixed constant, so
/// this draws from a stream independent of [`generate`]'s own RNG stream
/// (calling both against the same `config` doesn't have one call's draws
/// perturb the other's) while still being fully reproducible.
///
/// Each dog's out-degree is drawn as 3 independent Bernoulli trials with
/// success probability `config.littermate_avg_degree() / 3.0`, so the
/// number of edges a given dog originates is always 0, 1, 2, or 3, and the
/// expected out-degree across the dataset equals
/// `config.littermate_avg_degree()`. Each successful trial picks a
/// uniformly random *other* dog in `records` (never `records[i]` itself —
/// no self-loops) as the edge's other endpoint.
///
/// `littermate_of` is a symmetric relationship (if A is B's littermate, B
/// is A's), but this returns each generated edge once, as an unordered
/// pair — it is the caller/backend's job to treat `(a, b)` as connecting
/// both `a` to `b` and `b` to `a` (see [`crate::store::DogStore::neighbors`]
/// and each backend's adjacency handling), not this function's job to
/// double the list.
///
/// Returns an empty `Vec` if `records` has fewer than 2 dogs — there is no
/// valid "other" dog to connect to.
pub fn generate_littermates(config: &GeneratorConfig, records: &[DogRecord]) -> Vec<(Uuid, Uuid)> {
    let n = records.len();
    if n < 2 {
        return Vec::new();
    }

    let mut rng = StdRng::seed_from_u64(config.seed ^ LITTERMATE_SEED_XOR);
    let success_probability = config.littermate_avg_degree / MAX_LITTERMATE_DEGREE;

    let mut edges = Vec::new();
    for (i, record) in records.iter().enumerate() {
        for _ in 0..3 {
            if rng.gen_bool(success_probability) {
                // Pick a uniformly random index other than `i` in one draw
                // (no self-loop retry loop needed): drawing from the n-1
                // other positions and shifting past `i` maps evenly onto
                // `0..n` excluding `i`.
                let mut other = rng.gen_range(0..n - 1);
                if other >= i {
                    other += 1;
                }
                edges.push((record.id, records[other].id));
            }
        }
    }
    edges
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

    /// Arbitrary valid `littermate_avg_degree` for tests that don't care
    /// about edge generation specifically.
    const DEGREE: f64 = 1.5;

    #[test]
    fn same_config_produces_identical_output() {
        let config = GeneratorConfig::new(500, 10, DEGREE, 42).unwrap();
        let a = generate(&config);
        let b = generate(&config);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seed_produces_different_output() {
        let config_a = GeneratorConfig::new(500, 10, DEGREE, 1).unwrap();
        let config_b = GeneratorConfig::new(500, 10, DEGREE, 2).unwrap();
        assert_ne!(generate(&config_a), generate(&config_b));
    }

    #[test]
    fn generated_ids_are_unique() {
        let config = GeneratorConfig::new(50_000, 25, DEGREE, 7).unwrap();
        let records = generate(&config);
        let unique_ids: HashSet<_> = records.iter().map(|r| r.id).collect();
        assert_eq!(unique_ids.len(), records.len());
    }

    #[test]
    fn cardinality_one_gives_a_single_breed() {
        let config = GeneratorConfig::new(200, 1, DEGREE, 3).unwrap();
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
        let low = GeneratorConfig::new(n, 5, DEGREE, 9).unwrap();
        let high = GeneratorConfig::new(n, n, DEGREE, 9).unwrap();

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
        let config = GeneratorConfig::new(10_000, 5, DEGREE, 11).unwrap();
        let records = generate(&config);
        assert!(records.iter().all(|r| (MIN_AGE..=MAX_AGE).contains(&r.age)));
    }

    #[test]
    fn zero_records_produces_empty_dataset() {
        let config = GeneratorConfig::new(0, 0, DEGREE, 1).unwrap();
        assert!(generate(&config).is_empty());
    }

    #[test]
    fn zero_cardinality_with_records_is_a_config_error() {
        let result = GeneratorConfig::new(10, 0, DEGREE, 1);
        assert_eq!(
            result,
            Err(GeneratorConfigError::ZeroCardinalityWithRecords {
                n: 10,
                breed_cardinality: 0,
            })
        );
    }

    #[test]
    fn littermate_degree_below_zero_is_a_config_error() {
        let result = GeneratorConfig::new(10, 5, -0.1, 1);
        assert_eq!(
            result,
            Err(GeneratorConfigError::InvalidLittermateAvgDegree {
                littermate_avg_degree: -0.1,
            })
        );
    }

    #[test]
    fn littermate_degree_above_three_is_a_config_error() {
        let result = GeneratorConfig::new(10, 5, 3.1, 1);
        assert_eq!(
            result,
            Err(GeneratorConfigError::InvalidLittermateAvgDegree {
                littermate_avg_degree: 3.1,
            })
        );
    }

    #[test]
    fn littermate_degree_bounds_are_inclusive() {
        assert!(GeneratorConfig::new(10, 5, 0.0, 1).is_ok());
        assert!(GeneratorConfig::new(10, 5, 3.0, 1).is_ok());
    }

    #[test]
    fn same_config_produces_identical_littermate_edges() {
        let config = GeneratorConfig::new(500, 10, DEGREE, 42).unwrap();
        let records = generate(&config);
        let a = generate_littermates(&config, &records);
        let b = generate_littermates(&config, &records);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seed_produces_different_littermate_edges() {
        let config_a = GeneratorConfig::new(500, 10, DEGREE, 1).unwrap();
        let config_b = GeneratorConfig::new(500, 10, DEGREE, 2).unwrap();
        let records_a = generate(&config_a);
        let records_b = generate(&config_b);
        assert_ne!(
            generate_littermates(&config_a, &records_a),
            generate_littermates(&config_b, &records_b)
        );
    }

    #[test]
    fn zero_degree_produces_no_edges() {
        let config = GeneratorConfig::new(500, 10, 0.0, 5).unwrap();
        let records = generate(&config);
        assert!(generate_littermates(&config, &records).is_empty());
    }

    #[test]
    fn max_degree_gives_every_dog_exactly_three_out_edges() {
        let config = GeneratorConfig::new(200, 10, MAX_LITTERMATE_DEGREE, 6).unwrap();
        let records = generate(&config);
        let edges = generate_littermates(&config, &records);
        assert_eq!(edges.len(), records.len() * 3);
        for record in &records {
            let out_degree = edges.iter().filter(|&&(a, _)| a == record.id).count();
            assert_eq!(out_degree, 3);
        }
    }

    #[test]
    fn littermate_edges_have_no_self_loops() {
        let config = GeneratorConfig::new(1_000, 20, MAX_LITTERMATE_DEGREE, 8).unwrap();
        let records = generate(&config);
        let edges = generate_littermates(&config, &records);
        assert!(edges.iter().all(|&(a, b)| a != b));
    }

    #[test]
    fn fewer_than_two_records_produces_no_littermate_edges() {
        let config = GeneratorConfig::new(1, 1, MAX_LITTERMATE_DEGREE, 9).unwrap();
        let records = generate(&config);
        assert!(generate_littermates(&config, &records).is_empty());

        let empty_config = GeneratorConfig::new(0, 0, MAX_LITTERMATE_DEGREE, 9).unwrap();
        let empty_records = generate(&empty_config);
        assert!(generate_littermates(&empty_config, &empty_records).is_empty());
    }
}
