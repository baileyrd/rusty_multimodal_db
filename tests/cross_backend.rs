//! Cross-backend equivalence tests.
//!
//! All three backends must agree on the same input, since the whole point
//! of comparing them behind one trait is that they're interchangeable for
//! correctness and differ only in performance. See
//! `docs/specifications/storage/STORAGE-002-dogstore-backends.md`'s
//! verification plan.

use rusty_multimodal_db::store::{AosStore, CanonicalStore, SoaStore};
use rusty_multimodal_db::{generate, DogStore, GeneratorConfig, StoreError};
use std::collections::HashSet;
use uuid::Uuid;

fn build_all(records: Vec<rusty_multimodal_db::DogRecord>) -> (AosStore, SoaStore, CanonicalStore) {
    (
        AosStore::from(records.clone()),
        SoaStore::from(records.clone()),
        CanonicalStore::from(records),
    )
}

#[test]
fn all_backends_agree_on_get() {
    let config = GeneratorConfig::new(300, 15, 21).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical) = build_all(records.clone());

    for record in &records {
        assert_eq!(aos.get(record.id).as_ref(), Some(record));
        assert_eq!(soa.get(record.id).as_ref(), Some(record));
        assert_eq!(canonical.get(record.id).as_ref(), Some(record));
    }

    let unknown = Uuid::from_u128(u128::MAX);
    assert_eq!(aos.get(unknown), None);
    assert_eq!(soa.get(unknown), None);
    assert_eq!(canonical.get(unknown), None);
}

#[test]
fn all_backends_agree_on_scan_ages_as_a_multiset() {
    let config = GeneratorConfig::new(300, 15, 22).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical) = build_all(records.clone());

    let mut expected: Vec<u32> = records.iter().map(|r| r.age).collect();
    expected.sort_unstable();

    let mut aos_ages = aos.scan_ages();
    let mut soa_ages = soa.scan_ages();
    let mut canonical_ages = canonical.scan_ages();
    aos_ages.sort_unstable();
    soa_ages.sort_unstable();
    canonical_ages.sort_unstable();

    assert_eq!(aos_ages, expected);
    assert_eq!(soa_ages, expected);
    assert_eq!(canonical_ages, expected);
}

#[test]
fn all_backends_agree_on_same_breed() {
    let config = GeneratorConfig::new(300, 5, 23).unwrap();
    let records = generate(&config);
    let (aos, soa, canonical) = build_all(records.clone());

    for record in &records {
        let expected: HashSet<Uuid> = records
            .iter()
            .filter(|r| r.id != record.id && r.breed == record.breed)
            .map(|r| r.id)
            .collect();

        let aos_result: HashSet<Uuid> = aos.same_breed(record.id).into_iter().collect();
        let soa_result: HashSet<Uuid> = soa.same_breed(record.id).into_iter().collect();
        let canonical_result: HashSet<Uuid> = canonical.same_breed(record.id).into_iter().collect();

        assert_eq!(aos_result, expected);
        assert_eq!(soa_result, expected);
        assert_eq!(canonical_result, expected);
    }
}

#[test]
fn all_backends_agree_on_update_age_success_and_failure() {
    let config = GeneratorConfig::new(50, 5, 24).unwrap();
    let records = generate(&config);
    let target_id = records[0].id;
    let unknown_id = Uuid::from_u128(u128::MAX);
    let (mut aos, mut soa, mut canonical) = build_all(records);

    assert!(aos.update_age(target_id, 99).is_ok());
    assert!(soa.update_age(target_id, 99).is_ok());
    assert!(canonical.update_age(target_id, 99).is_ok());
    assert_eq!(aos.get(target_id).unwrap().age, 99);
    assert_eq!(soa.get(target_id).unwrap().age, 99);
    assert_eq!(canonical.get(target_id).unwrap().age, 99);

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
}
