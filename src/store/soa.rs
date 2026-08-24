//! Struct-of-arrays (column-oriented) backend: parallel `Vec<Uuid>` /
//! `Vec<String>` / `Vec<u32>`, tied together by shared array position.
//!
//! Fast column scans by construction (`ages` is already one contiguous
//! `Vec<u32>`); full-record reconstruction and lookup-by-id pay for a
//! linear scan of `ids` plus touching two other arrays at the found
//! position.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use uuid::Uuid;

/// Column-oriented backend: three parallel `Vec`s, one per field.
pub struct SoaStore {
    ids: Vec<Uuid>,
    breeds: Vec<String>,
    ages: Vec<u32>,
}

impl SoaStore {
    /// Build a store from generated records, splitting each record's
    /// fields into the three parallel arrays.
    pub fn new(records: Vec<DogRecord>) -> Self {
        let mut ids = Vec::with_capacity(records.len());
        let mut breeds = Vec::with_capacity(records.len());
        let mut ages = Vec::with_capacity(records.len());
        for record in records {
            ids.push(record.id);
            breeds.push(record.breed);
            ages.push(record.age);
        }
        Self { ids, breeds, ages }
    }

    /// Array position of `id`, or `None` if it isn't present. All three
    /// parallel arrays are the same length, so a position found here is
    /// valid in `breeds` and `ages` too.
    fn position_of(&self, id: Uuid) -> Option<usize> {
        self.ids.iter().position(|&existing| existing == id)
    }
}

impl From<Vec<DogRecord>> for SoaStore {
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records)
    }
}

impl DogStore for SoaStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        let position = self.position_of(id)?;
        Some(DogRecord::new(
            self.ids[position],
            self.breeds[position].clone(),
            self.ages[position],
        ))
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.ages.clone()
    }

    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let position = self.position_of(id).ok_or(StoreError::NotFound(id))?;
        self.ages[position] = age;
        Ok(())
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        let Some(position) = self.position_of(id) else {
            return Vec::new();
        };
        let target_breed = &self.breeds[position];
        let mut result = Vec::new();
        for i in 0..self.ids.len() {
            if self.ids[i] != id && &self.breeds[i] == target_breed {
                result.push(self.ids[i]);
            }
        }
        result
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
        let store = SoaStore::new(sample());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = SoaStore::new(sample());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = SoaStore::new(sample());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn update_age_does_not_disturb_other_records() {
        let mut store = SoaStore::new(sample());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(2)).unwrap().age, 5);
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = SoaStore::new(sample());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = SoaStore::new(sample());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }
}
