//! Tier 1, variant 3: swap the canonical map for `dashmap::DashMap`.
//!
//! `DashMap` is itself internally sharded (a fixed number of independently
//! `RwLock`-guarded segments, chosen by the map at construction based on
//! expected concurrency) and exposes a plain `HashMap`-shaped API on top —
//! this variant exists to test how far an off-the-shelf concurrent map
//! gets versus hand-rolling the sharding in [`super::sharded::ShardedStore`]
//! or a single global lock in [`super::global_rwlock::GlobalRwLockStore`].
//! Per the same scope boundary as the sharded variant, only `age` is
//! tracked (inline on each entry's `DogRecord`) — `same_breed`/`neighbors`
//! aren't part of [`super::ConcurrentStore`] and this crate's concurrency
//! benchmark never exercises them.

use super::{ConcurrencyError, ConcurrentStore};
use crate::record::DogRecord;
use crate::store::StoreError;
use dashmap::DashMap;
use uuid::Uuid;

/// `DashMap`-backed concurrent store. See module docs for the concurrency
/// model.
pub struct DashMapStore {
    records: DashMap<Uuid, DogRecord>,
}

impl ConcurrentStore for DashMapStore {
    fn new(records: Vec<DogRecord>, _edges: Vec<(Uuid, Uuid)>) -> Self {
        let map = DashMap::with_capacity(records.len());
        for record in records {
            map.insert(record.id, record);
        }
        Self { records: map }
    }

    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError> {
        Ok(self.records.get(&id).map(|entry| entry.clone()))
    }

    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError> {
        Ok(self.records.iter().map(|entry| entry.age).collect())
    }

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`StoreError::NotFound`]
    /// if `id` has no record.
    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError> {
        let mut entry = self.records.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        entry.age = age;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concurrency::test_support::run_concurrency_stress_test;

    fn sample() -> Vec<DogRecord> {
        vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
            DogRecord::new(Uuid::from_u128(3), "poodle", 2),
        ]
    }

    #[test]
    fn create_then_read_and_write() {
        let store = DashMapStore::new(sample(), Vec::new());
        assert_eq!(
            store.get(Uuid::from_u128(1)).unwrap().unwrap().breed,
            "labrador"
        );
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().unwrap().age, 42);

        assert!(matches!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(ConcurrencyError::Store(StoreError::NotFound(_)))
        ));
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = DashMapStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages().unwrap();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    /// The flagship correctness property for this variant — see
    /// `run_concurrency_stress_test`'s own doc comment.
    #[test]
    fn concurrent_stress_matches_sequential_replay() {
        run_concurrency_stress_test::<DashMapStore>();
    }
}
