//! Cross-backend equivalence tests.
//!
//! All four backends must agree on the same input, since the whole point
//! of comparing them behind one trait is that they're interchangeable for
//! correctness and differ only in performance. See
//! `docs/specifications/storage/STORAGE-002-dogstore-backends.md`'s,
//! `STORAGE-005-canonical-cached-backend.md`'s, and
//! `STORAGE-006-littermate-graph-traversal.md`'s verification plans.

use rusty_multimodal_db::bench_support::two_hop_neighbors;
use rusty_multimodal_db::store::{AosStore, CanonicalCachedStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{generate, generate_littermates, DogStore, GeneratorConfig, StoreError};
use std::collections::HashSet;
use uuid::Uuid;

fn build_all(
    records: Vec<rusty_multimodal_db::DogRecord>,
    edges: Vec<(Uuid, Uuid)>,
) -> (AosStore, SoaStore, CanonicalStore, CanonicalCachedStore) {
    (
        AosStore::from((records.clone(), edges.clone())),
        SoaStore::from((records.clone(), edges.clone())),
        CanonicalStore::from((records.clone(), edges.clone())),
        CanonicalCachedStore::from((records, edges)),
    )
}

#[test]
fn all_backends_agree_on_get() {
    let config = GeneratorConfig::new(300, 15, 1.5, 21).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical, canonical_cached) = build_all(records.clone(), Vec::new());

    for record in &records {
        assert_eq!(aos.get(record.id).as_ref(), Some(record));
        assert_eq!(soa.get(record.id).as_ref(), Some(record));
        assert_eq!(canonical.get(record.id).as_ref(), Some(record));
        assert_eq!(canonical_cached.get(record.id).as_ref(), Some(record));
    }

    let unknown = Uuid::from_u128(u128::MAX);
    assert_eq!(aos.get(unknown), None);
    assert_eq!(soa.get(unknown), None);
    assert_eq!(canonical.get(unknown), None);
    assert_eq!(canonical_cached.get(unknown), None);
}

#[test]
fn all_backends_agree_on_scan_ages_as_a_multiset() {
    let config = GeneratorConfig::new(300, 15, 1.5, 22).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical, canonical_cached) = build_all(records.clone(), Vec::new());

    let mut expected: Vec<u32> = records.iter().map(|r| r.age).collect();
    expected.sort_unstable();

    let mut aos_ages = aos.scan_ages();
    let mut soa_ages = soa.scan_ages();
    let mut canonical_ages = canonical.scan_ages();
    let mut canonical_cached_ages = canonical_cached.scan_ages();
    aos_ages.sort_unstable();
    soa_ages.sort_unstable();
    canonical_ages.sort_unstable();
    canonical_cached_ages.sort_unstable();

    assert_eq!(aos_ages, expected);
    assert_eq!(soa_ages, expected);
    assert_eq!(canonical_ages, expected);
    assert_eq!(canonical_cached_ages, expected);
}

#[test]
fn all_backends_agree_on_same_breed() {
    let config = GeneratorConfig::new(300, 5, 1.5, 23).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical, canonical_cached) = build_all(records.clone(), Vec::new());

    for record in &records {
        let expected: HashSet<Uuid> = records
            .iter()
            .filter(|r| r.id != record.id && r.breed == record.breed)
            .map(|r| r.id)
            .collect();

        let aos_result: HashSet<Uuid> = aos.same_breed(record.id).into_iter().collect();
        let soa_result: HashSet<Uuid> = soa.same_breed(record.id).into_iter().collect();
        let canonical_result: HashSet<Uuid> = canonical.same_breed(record.id).into_iter().collect();
        let canonical_cached_result: HashSet<Uuid> =
            canonical_cached.same_breed(record.id).into_iter().collect();

        assert_eq!(aos_result, expected);
        assert_eq!(soa_result, expected);
        assert_eq!(canonical_result, expected);
        assert_eq!(canonical_cached_result, expected);
    }
}

#[test]
fn all_backends_agree_on_update_age_success_and_failure() {
    let config = GeneratorConfig::new(50, 5, 1.5, 24).unwrap();
    let records = generate(&config);
    let target_id = records[0].id;
    let unknown_id = Uuid::from_u128(u128::MAX);
    let (mut aos, mut soa, mut canonical, mut canonical_cached) = build_all(records, Vec::new());

    assert!(aos.update_age(target_id, 99).is_ok());
    assert!(soa.update_age(target_id, 99).is_ok());
    assert!(canonical.update_age(target_id, 99).is_ok());
    assert!(canonical_cached.update_age(target_id, 99).is_ok());
    assert_eq!(aos.get(target_id).unwrap().age, 99);
    assert_eq!(soa.get(target_id).unwrap().age, 99);
    assert_eq!(canonical.get(target_id).unwrap().age, 99);
    assert_eq!(canonical_cached.get(target_id).unwrap().age, 99);

    assert_eq!(
        aos.update_age(unknown_id, 1),
        Err(StoreError::NotFound(unknown_id))
    );
    assert_eq!(
        soa.update_age(unknown_id, 1),
        Err(StoreError::NotFound(unknown_id))
    );
    assert_eq!(
        canonical.update_age(unknown_id, 1),
        Err(StoreError::NotFound(unknown_id))
    );
    assert_eq!(
        canonical_cached.update_age(unknown_id, 1),
        Err(StoreError::NotFound(unknown_id))
    );
}

/// Cross-backend version of `CanonicalCachedStore`'s own staleness test:
/// after `update_age`, every backend's `scan_ages` — not just the cached
/// one — must reflect the new value immediately.
#[test]
fn all_backends_reflect_update_age_in_scan_ages_immediately() {
    let config = GeneratorConfig::new(200, 10, 1.5, 25).unwrap();
    let records = generate(&config);
    let target_id = records[0].id;
    let old_age = records[0].age;
    let new_age = if old_age == 20 { 0 } else { old_age + 1 };
    let (mut aos, mut soa, mut canonical, mut canonical_cached) = build_all(records, Vec::new());

    aos.update_age(target_id, new_age).unwrap();
    soa.update_age(target_id, new_age).unwrap();
    canonical.update_age(target_id, new_age).unwrap();
    canonical_cached.update_age(target_id, new_age).unwrap();

    assert!(aos.scan_ages().contains(&new_age));
    assert!(soa.scan_ages().contains(&new_age));
    assert!(canonical.scan_ages().contains(&new_age));
    assert!(canonical_cached.scan_ages().contains(&new_age));
}

/// The highest-priority test in this file, per this feature's task: the
/// equivalent of `CanonicalCachedStore`'s stale-cache test, but for graph
/// traversal. `AosStore`'s `neighbors` is a direct linear scan of the raw
/// edge list — ground truth by construction — so every other backend's
/// adjacency-index-based `neighbors` must return the exact same set for
/// every id, or that backend's index was built wrong (missing a direction,
/// double-inserting, using the wrong key, etc.).
#[test]
fn all_backends_agree_on_neighbors_one_hop() {
    let config = GeneratorConfig::new(300, 10, 2.0, 26).unwrap();
    let records = generate(&config);
    let edges = generate_littermates(&config, &records);
    assert!(
        !edges.is_empty(),
        "test needs a non-trivial edge list to be meaningful"
    );
    let (aos, soa, canonical, canonical_cached) = build_all(records.clone(), edges);

    for record in &records {
        let ground_truth: HashSet<Uuid> = aos.neighbors(record.id).into_iter().collect();

        let soa_result: HashSet<Uuid> = soa.neighbors(record.id).into_iter().collect();
        let canonical_result: HashSet<Uuid> = canonical.neighbors(record.id).into_iter().collect();
        let canonical_cached_result: HashSet<Uuid> =
            canonical_cached.neighbors(record.id).into_iter().collect();

        assert_eq!(
            soa_result, ground_truth,
            "soa disagreed for id {}",
            record.id
        );
        assert_eq!(
            canonical_result, ground_truth,
            "canonical disagreed for id {}",
            record.id
        );
        assert_eq!(
            canonical_cached_result, ground_truth,
            "canonical_cached disagreed for id {}",
            record.id
        );
    }

    let unknown = Uuid::from_u128(u128::MAX);
    assert!(aos.neighbors(unknown).is_empty());
    assert!(soa.neighbors(unknown).is_empty());
    assert!(canonical.neighbors(unknown).is_empty());
    assert!(canonical_cached.neighbors(unknown).is_empty());
}

/// 2-hop isn't a trait method (ADR-0004) — `bench_support::two_hop_neighbors`
/// is built generically from `neighbors`, so once every backend agrees on
/// one-hop `neighbors` (previous test), they must also agree on its 2-hop
/// composition. Verifies the shared helper itself, not per-backend logic.
#[test]
fn all_backends_agree_on_neighbors_two_hop() {
    let config = GeneratorConfig::new(300, 10, 2.0, 27).unwrap();
    let records = generate(&config);
    let edges = generate_littermates(&config, &records);
    let (aos, soa, canonical, canonical_cached) = build_all(records.clone(), edges);

    for record in &records {
        let ground_truth: HashSet<Uuid> = two_hop_neighbors(&aos, record.id).into_iter().collect();

        let soa_result: HashSet<Uuid> = two_hop_neighbors(&soa, record.id).into_iter().collect();
        let canonical_result: HashSet<Uuid> = two_hop_neighbors(&canonical, record.id)
            .into_iter()
            .collect();
        let canonical_cached_result: HashSet<Uuid> =
            two_hop_neighbors(&canonical_cached, record.id)
                .into_iter()
                .collect();

        assert_eq!(
            soa_result, ground_truth,
            "soa disagreed for id {}",
            record.id
        );
        assert_eq!(
            canonical_result, ground_truth,
            "canonical disagreed for id {}",
            record.id
        );
        assert_eq!(
            canonical_cached_result, ground_truth,
            "canonical_cached disagreed for id {}",
            record.id
        );
    }
}
