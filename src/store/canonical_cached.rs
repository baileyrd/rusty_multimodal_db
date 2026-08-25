//! Hybrid backend: `CanonicalStore`'s `HashMap<Uuid, DogRecord>` as source
//! of truth, plus a materialized, packed `Vec<u32>` age cache (SoA-style)
//! for `scan_ages`.
//!
//! This is the fourth backend ADR-0001 flagged as a likely follow-up once
//! `RESULTS.md` showed `CanonicalStore` losing `scan_ages` to both
//! baselines: it exists specifically to test whether a small, targeted
//! materialized cache can close that gap without giving up the
//! point-lookup wins. See
//! `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for
//! why the cache is kept in sync eagerly (write-through on every
//! `update_age`) rather than lazily.

use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::collections::HashMap;
use uuid::Uuid;

/// UUID-canonical backend with a materialized age cache: the `HashMap` is
/// still the only copy of `breed` and full-record data, but `age` is
/// additionally held in a packed `Vec<u32>`, kept in sync on every write.
/// `neighbors` uses the same `littermate_of` adjacency-index pattern as
/// [`CanonicalStore`](crate::store::CanonicalStore) — this backend doesn't
/// change that pattern, just inherits it alongside the age cache.
pub struct CanonicalCachedStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    /// Packed ages, in the same order as `position_index` assigns.
    age_cache: Vec<u32>,
    /// UUID -> index into `age_cache`, so `update_age` can write through
    /// in O(1) instead of scanning the cache to find its own record.
    position_index: HashMap<Uuid, usize>,
}

impl CanonicalCachedStore {
    /// Build a store from generated records and littermate edges: the
    /// canonical map, breed index, and adjacency index (identical to
    /// [`CanonicalStore`](crate::store::CanonicalStore)), plus a packed age
    /// cache and the position index needed to write through to it.
    pub fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
        let mut age_cache = Vec::with_capacity(records.len());
        let mut position_index = HashMap::with_capacity(records.len());

        for (position, record) in records.iter().enumerate() {
            breed_index
                .entry(record.breed.clone())
                .or_default()
                .push(record.id);
            age_cache.push(record.age);
            position_index.insert(record.id, position);
        }

        let mut adjacency_index: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (a, b) in edges {
            adjacency_index.entry(a).or_default().push(b);
            adjacency_index.entry(b).or_default().push(a);
        }

        let records = records.into_iter().map(|r| (r.id, r)).collect();

        Self {
            records,
            breed_index,
            adjacency_index,
            age_cache,
            position_index,
        }
    }
}

impl From<Vec<DogRecord>> for CanonicalCachedStore {
    /// Convenience for workloads that don't exercise `neighbors` — builds
    /// with no littermate edges.
    fn from(records: Vec<DogRecord>) -> Self {
        Self::new(records, Vec::new())
    }
}

impl From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)> for CanonicalCachedStore {
    fn from((records, edges): (Vec<DogRecord>, Vec<(Uuid, Uuid)>)) -> Self {
        Self::new(records, edges)
    }
}

impl DogStore for CanonicalCachedStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.records.get(&id).cloned()
    }

    /// Reads the materialized cache directly — this is the whole point of
    /// this backend: no `HashMap` traversal, no per-record heap chase.
    fn scan_ages(&self) -> Vec<u32> {
        self.age_cache.clone()
    }

    /// Write-through: updates the canonical record *and* the cache in the
    /// same call, so `scan_ages` can never observe a stale age. See
    /// ADR-0003 for why eager was chosen over a lazy/dirty-flag strategy.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let record = self.records.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        record.age = age;

        let position = *self
            .position_index
            .get(&id)
            .expect("every id in `records` has a position in `age_cache` by construction");
        self.age_cache[position] = age;

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

    fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.adjacency_index.get(&id).cloned().unwrap_or_default()
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

    /// The one bug this backend can introduce that the other three can't:
    /// a stale cache. `scan_ages` must reflect an `update_age` immediately,
    /// with no separate "flush" step. Highest-priority test in this file.
    #[test]
    fn scan_ages_reflects_update_age_immediately() {
        let mut store = CanonicalCachedStore::new(sample(), Vec::new());
        store.update_age(Uuid::from_u128(1), 99).unwrap();

        let ages = store.scan_ages();
        assert!(
            ages.contains(&99),
            "scan_ages returned {ages:?}, which doesn't include the just-written age 99 — the cache went stale"
        );
        assert!(
            !ages.contains(&3),
            "scan_ages returned {ages:?}, which still includes the old age 3 — the cache wasn't overwritten, just appended to"
        );
    }

    #[test]
    fn get_hit_and_miss() {
        let store = CanonicalCachedStore::new(sample(), Vec::new());
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(store.get(Uuid::from_u128(99)), None);
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = CanonicalCachedStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    #[test]
    fn update_age_success_and_not_found() {
        let mut store = CanonicalCachedStore::new(sample(), Vec::new());
        store.update_age(Uuid::from_u128(1), 10).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 10);

        let err = store.update_age(Uuid::from_u128(99), 1).unwrap_err();
        assert_eq!(err, StoreError::NotFound(Uuid::from_u128(99)));
    }

    #[test]
    fn update_age_on_unknown_id_does_not_touch_the_cache() {
        let mut store = CanonicalCachedStore::new(sample(), Vec::new());
        let before = {
            let mut ages = store.scan_ages();
            ages.sort_unstable();
            ages
        };

        assert!(store.update_age(Uuid::from_u128(99), 42).is_err());

        let after = {
            let mut ages = store.scan_ages();
            ages.sort_unstable();
            ages
        };
        assert_eq!(before, after);
    }

    #[test]
    fn same_breed_finds_shared_and_excludes_self() {
        let store = CanonicalCachedStore::new(sample(), Vec::new());
        let mut result = store.same_breed(Uuid::from_u128(1));
        result.sort();
        assert_eq!(result, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn same_breed_unique_breed_is_empty() {
        let store = CanonicalCachedStore::new(sample(), Vec::new());
        assert!(store.same_breed(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn same_breed_unknown_id_is_empty() {
        let store = CanonicalCachedStore::new(sample(), Vec::new());
        assert!(store.same_breed(Uuid::from_u128(99)).is_empty());
    }

    #[test]
    fn neighbors_finds_edge_in_either_direction() {
        let store = CanonicalCachedStore::new(sample(), edges_sample());
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
        let store = CanonicalCachedStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(3)).is_empty());
    }

    #[test]
    fn neighbors_unknown_id_is_empty() {
        let store = CanonicalCachedStore::new(sample(), edges_sample());
        assert!(store.neighbors(Uuid::from_u128(99)).is_empty());
    }
}
