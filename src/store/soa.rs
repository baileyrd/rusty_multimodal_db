//! Struct-of-arrays (column-oriented) backend: parallel `Vec<Uuid>` /
//! `Vec<String>` / `Vec<u32>`, tied together by shared array position.
//!
//! Fast column scans by construction (`ages` is already one contiguous
//! `Vec<u32>`); full-record reconstruction and lookup-by-id pay for a
//! linear scan of `ids` plus touching two other arrays at the found
//! position. `neighbors` is a linear scan of a flat `littermate_of` edge
//! list — same naive-baseline role as `AosStore`'s; there's no columnar
//! layout that helps an edge-list scan the way parallel arrays help a
//! field scan.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use uuid::Uuid;

/// Column-oriented backend: three parallel `Vec`s, one per field, plus a
/// flat `littermate_of` edge list scanned linearly by `neighbors`.
pub struct SoaStore {
    ids: Vec<Uuid>,
    breeds: Vec<String>,
    ages: Vec<u32>,
    edges: Vec<(Uuid, Uuid)>,
}

impl SoaStore {
    /// Build a store from generated records and littermate edges,
    /// splitting each record's fields into the three parallel arrays.
    pub fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        let mut ids = Vec::with_capacity(records.len());
        let mut breeds = Vec::with_capacity(records.len());
        let mut ages = Vec::with_capacity(records.len());
        for record in records {
            ids.push(record.id);
            breeds.push(record.breed);
            ages.push(record.age);
        }
        Self {
            ids,
            breeds,
            ages,
            edges,
        }
    }

    /// Array position of `id`, or `None` if it isn't present. All three
    /// parallel arrays are the same length, so a position found here is
    /// valid in `breeds` and `ages` too.
    fn position_of(&self, id: Uuid) -> Option<usize> {
        self.ids.iter().position(|&existing| existing == id)
    }
}

impl From<Vec<DogRecord>> for SoaStore {
    /// Convenience for workloads that don't exercise `neighbors` — builds
    /// with no littermate edges.
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records, Vec::new())
    }
}

impl From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)> for SoaStore {
    fn from((records, edges): (Vec<DogRecord>, Vec<(Uuid, Uuid)>)) -> Self {
        Self::new(records, edges)
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
        let store = SoaStore::new(sample(), Vec::new());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = SoaStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = SoaStore::new(sample(), Vec::new());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn update_age_does_not_disturb_other_records() {
        let mut store = SoaStore::new(sample(), Vec::new());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(2)).unwrap().age, 5);
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = SoaStore::new(sample(), Vec::new());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = SoaStore::new(sample(), Vec::new());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn neighbors_finds_edge_in_either_direction() {
        let store = SoaStore::new(sample(), edges_sample());
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
        let store = SoaStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn neighbors_unknown_id_is_empty() {
        let store = SoaStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(99)).is_empty());
    }
}
