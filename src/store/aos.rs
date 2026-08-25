//! Array-of-structs (row-oriented) backend: `Vec<DogRecord>`.
//!
//! Fast full-record reads by construction (a matching record is already
//! contiguous once found); `scan_ages` and `same_breed` pay for touching
//! every field of every record even though they only need one. `neighbors`
//! is a linear scan of a flat `littermate_of` edge list — the naive
//! baseline for graph traversal, same role AoS plays in every other
//! workload.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use uuid::Uuid;

/// Row-oriented backend: one contiguous `Vec` of full records, plus a flat
/// `littermate_of` edge list scanned linearly by `neighbors`.
pub struct AosStore {
    records: Vec<DogRecord>,
    edges: Vec<(Uuid, Uuid)>,
}

impl AosStore {
    /// Build a store from generated records and littermate edges,
    /// preserving record order.
    pub fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        Self { records, edges }
    }
}

impl From<Vec<DogRecord>> for AosStore {
    /// Convenience for workloads that don't exercise `neighbors` — builds
    /// with no littermate edges.
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records, Vec::new())
    }
}

impl From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)> for AosStore {
    fn from((records, edges): (Vec<DogRecord>, Vec<(Uuid, Uuid)>)) -> Self {
        Self::new(records, edges)
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

    fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.edges
            .iter()
            .filter_map(|&(a, b)| {
                if a == id {
                    Some(b)
                } else if b == id {
                    Some(a)
                } else {
                    None
                }
            })
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

    /// One `littermate_of` edge: dog 1 and dog 2 are littermates; dog 3 has
    /// none.
    fn edges_sample() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }

    #[test]
    fn get_hit_and_miss() {
        let store = AosStore::new(sample(), Vec::new());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = AosStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = AosStore::new(sample(), Vec::new());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = AosStore::new(sample(), Vec::new());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = AosStore::new(sample(), Vec::new());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn same_breed_unknown_id_is_empty() {
        let store = AosStore::new(sample(), Vec::new());
        assert!(store.same_breed(Uuid::from_u128(99)).is_empty());
    }

    #[test]
    fn neighbors_finds_edge_in_either_direction() {
        let store = AosStore::new(sample(), edges_sample());
        assert_eq!(
            store.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            store.neighbors(Uuid::from_u128(2)),
            vec![Uuid::from_u128(1)]
        );
    }

    #[test]
    fn neighbors_no_edges_is_empty() {
        let store = AosStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn neighbors_unknown_id_is_empty() {
        let store = AosStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(99)).is_empty());
    }
}
