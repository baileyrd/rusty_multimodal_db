//! Array-of-structs (row-oriented) backend: `Vec<DogRecord>`.
//!
//! Fast full-record reads by construction (a matching record is already
//! contiguous once found); `scan_ages` and `same_breed` pay for touching
//! every field of every record even though they only need one.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use uuid::Uuid;

/// Row-oriented backend: one contiguous `Vec` of full records.
pub struct AosStore {
    records: Vec<DogRecord>,
}

impl AosStore {
    /// Build a store from generated records, preserving their order.
    pub fn new(records: Vec<DogRecord>) -> Self {
        Self { records }
    }
}

impl From<Vec<DogRecord>> for AosStore {
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records)
    }
}

impl DogStore for AosStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.records.iter().find(|r| r.id == id).cloned()
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.records.iter().map(|r| r.age).collect()
    }

    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(StoreError::NotFound(id))?;
        record.age = age;
        Ok(())
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        let Some(target) = self.records.iter().find(|r| r.id == id) else {
            return Vec::new();
        };
        let target_breed = target.breed.clone();
        self.records
            .iter()
            .filter(|r| r.id != id && r.breed == target_breed)
            .map(|r| r.id)
            .collect()
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
        let store = AosStore::new(sample());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = AosStore::new(sample());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = AosStore::new(sample());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = AosStore::new(sample());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = AosStore::new(sample());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn same_breed_unknown_id_is_empty() {
        let store = AosStore::new(sample());
        assert!(store.same_breed(Uuid::from_u128(99)).is_empty());
    }
}
