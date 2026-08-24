//! UUID-canonical backend: `HashMap<Uuid, DogRecord>` as the *only*
//! physical copy of record data, with `scan_ages` and `same_breed`
//! implemented as views/derived indexes over it rather than as separate
//! physical copies.
//!
//! This is the design under test in
//! `docs/decisions/ADR-0001-three-backend-empirical-comparison.md`. The
//! boundary that makes it a fair test: `scan_ages` iterates the map's
//! values directly (no cached `Vec<u32>`); the breed index stores only
//! UUIDs (an index of keys, not a duplicate of breed data), the minimum
//! structure a one-hop lookup needs to be possible at all.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::collections::HashMap;
use uuid::Uuid;

/// UUID-canonical backend: one `HashMap` of full records, plus a
/// breed-name → UUIDs index built once at construction to serve
/// `same_breed` without a linear scan.
pub struct CanonicalStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
}

impl CanonicalStore {
    /// Build a store from generated records: the canonical map plus a
    /// derived breed index. `update_age` never changes a record's breed,
    /// so the index doesn't need to be kept in sync by any operation in
    /// this crate — if a future breed-mutating operation is added, the
    /// index must be updated alongside it.
    pub fn new(records: Vec<DogRecord>) -> Self {
        let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
        for record in &records {
            breed_index
                .entry(record.breed.clone())
                .or_default()
                .push(record.id);
        }
        let records = records.into_iter().map(|r| (r.id, r)).collect();
        Self {
            records,
            breed_index,
        }
    }
}

impl From<Vec<DogRecord>> for CanonicalStore {
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records)
    }
}

impl DogStore for CanonicalStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.records.get(&id).cloned()
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.records.values().map(|r| r.age).collect()
    }

    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let record = self.records.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        record.age = age;
        Ok(())
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        let Some(target) = self.records.get(&id) else {
            return Vec::new();
        };
        match self.breed_index.get(&target.breed) {
            Some(ids) => ids.iter().copied().filter(|&other| other != id).collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<DogRecord> {
        vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
            DogRecord::new(Uuid::from_u128(3), "poodle", 2),
        ]
    }

    #[test]
    fn get_hit_and_miss() {
        let store = CanonicalStore::new(sample());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = CanonicalStore::new(sample());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = CanonicalStore::new(sample());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = CanonicalStore::new(sample());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = CanonicalStore::new(sample());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn same_breed_unknown_id_is_empty() {
        let store = CanonicalStore::new(sample());
        assert!(store.same_breed(Uuid::from_u128(99)).is_empty());
    }
}
