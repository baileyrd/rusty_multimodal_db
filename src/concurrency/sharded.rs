//! Tier 1, variant 2: sharded locking — partition records across N
//! independently-locked shards, so writers to different shards proceed in
//! parallel instead of all serializing behind one lock.
//!
//! # Scope: `get`/`update_age`/`scan_ages` only, not the full `DogStore` shape
//!
//! [`ConcurrentStore`] doesn't include `same_breed`/`neighbors` — the
//! concurrency benchmark (`MixedWorkloadDriver`, reused from the
//! mixed-workload round) only ever exercises `get`/`update_age`/`scan_ages`,
//! so those are the only operations any of these four variants need to
//! support. That scope boundary is what keeps sharding tractable in one
//! pass: a real sharded `same_breed`/`neighbors` would need the breed and
//! adjacency indexes sharded too, each shard boundary chosen so a query
//! doesn't have to lock (or, worse, only *sometimes* have to lock) every
//! shard — a genuinely bigger design problem than this pass's scope calls
//! for. `age` lives inline on each shard's own `DogRecord`, not in a
//! separate packed cache: `CanonicalCachedStore`'s packed `Vec<u32>` +
//! position-index trick is a single-threaded, contiguous-array
//! optimization that doesn't shard cleanly (positions aren't globally
//! contiguous once records are partitioned across N independent maps), and
//! isn't what this variant is trying to measure anyway — the interesting
//! question here is lock-contention shape under concurrent access, not
//! `scan_ages`'s single-threaded micro-cost.
//!
//! # Shard count and routing
//!
//! Records are routed to shard `id.as_u128() % SHARD_COUNT` — UUIDs in
//! this crate are generated from a seeded RNG (`Builder::from_random_bytes`,
//! see `src/generator.rs`), so they're already high-entropy and distribute
//! evenly across shards without needing a separate hash function. Shard
//! count is fixed at construction time (no resharding/resizing — this
//! prototype doesn't need to grow) and deliberately set well above the
//! largest thread count this crate's benchmarks sweep (`SHARD_COUNT = 64`
//! vs. a max of 16 threads in `benches/concurrency.rs`), so at the
//! measured thread counts, two threads landing on the *same* shard by
//! chance is the exception, not the rule — the benchmark should mostly be
//! measuring genuine parallelism, not shard-collision contention. A
//! shard-count sweep of its own is unexplored — see `RESULTS.md`'s open
//! questions.
//!
//! # `scan_ages` needs every shard
//!
//! Unlike `get`/`update_age` (which only ever touch the one shard `id`
//! routes to), `scan_ages` has to read every record — it acquires each
//! shard's read lock in turn, one at a time, not all simultaneously. A
//! concurrent writer can freely proceed against any shard `scan_ages`
//! hasn't reached yet, or any shard it's already passed; only the *one*
//! shard currently being scanned is blocked from taking a write lock, and
//! only for as long as that shard's `HashMap` values take to clone. This
//! makes `scan_ages` this variant's own worst case for read/write overlap
//! — never fully blocking the whole store the way the global-lock variant
//! does, but also never a single atomic snapshot across shards (a
//! `scan_ages` call can observe some shards' pre-update state and others'
//! post-update state if a write lands mid-scan) — the same kind of
//! eventual-consistency-during-a-scan tradeoff sharded systems generally
//! accept in exchange for not blocking everything at once.

use super::{ConcurrencyError, ConcurrentStore};
use crate::record::DogRecord;
use crate::store::StoreError;
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

/// Fixed shard count — see module docs for why 64.
const SHARD_COUNT: usize = 64;

fn shard_index(id: Uuid) -> usize {
    (id.as_u128() % SHARD_COUNT as u128) as usize
}

/// Sharded-locking concurrent store. See module docs for the concurrency
/// model and its scope boundary.
pub struct ShardedStore {
    shards: Vec<RwLock<HashMap<Uuid, DogRecord>>>,
}

impl ConcurrentStore for ShardedStore {
    fn new(records: Vec<DogRecord>, _edges: Vec<(Uuid, Uuid)>) -> Self {
        let mut shard_maps: Vec<HashMap<Uuid, DogRecord>> =
            (0..SHARD_COUNT).map(|_| HashMap::new()).collect();
        for record in records {
            let shard = shard_index(record.id);
            shard_maps[shard].insert(record.id, record);
        }
        let shards = shard_maps.into_iter().map(RwLock::new).collect();
        Self { shards }
    }

    /// # Panics
    ///
    /// Panics if a shard's lock is poisoned. Every operation performed
    /// while holding a shard lock (plain `HashMap` reads/inserts) is
    /// infallible and never panics under normal operation, so poisoning
    /// can't happen here in practice — the explicit, documented exception
    /// to "no unwrap/expect outside tests" this pass's own constraints
    /// call for.
    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError> {
        let shard = &self.shards[shard_index(id)];
        Ok(shard
            .read()
            .expect("shard RwLock poisoned: a prior holder panicked, which no operation here should ever do")
            .get(&id)
            .cloned())
    }

    /// # Panics
    ///
    /// See [`Self::get`].
    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError> {
        let mut ages = Vec::new();
        for shard in &self.shards {
            let guard = shard
                .read()
                .expect("shard RwLock poisoned: a prior holder panicked, which no operation here should ever do");
            ages.extend(guard.values().map(|record| record.age));
        }
        Ok(ages)
    }

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`StoreError::NotFound`]
    /// if `id` has no record.
    ///
    /// # Panics
    ///
    /// See [`Self::get`].
    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError> {
        let shard = &self.shards[shard_index(id)];
        let mut guard = shard
            .write()
            .expect("shard RwLock poisoned: a prior holder panicked, which no operation here should ever do");
        let record = guard.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        record.age = age;
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
        let store = ShardedStore::new(sample(), Vec::new());
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
    fn scan_ages_returns_every_age_across_all_shards() {
        let store = ShardedStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages().unwrap();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    /// Records that hash to different shards must still all be reachable —
    /// the defining correctness property of sharding itself (as opposed to
    /// contention behavior, which the stress test below covers): nothing
    /// about routing an id to shard N should ever lose or misplace it.
    #[test]
    fn records_across_different_shards_are_all_reachable() {
        let records: Vec<DogRecord> = (0..(SHARD_COUNT as u128 * 3))
            .map(|n| DogRecord::new(Uuid::from_u128(n), "labrador", n as u32))
            .collect();
        let store = ShardedStore::new(records.clone(), Vec::new());
        for record in &records {
            assert_eq!(
                store.get(record.id).unwrap().map(|r| r.age),
                Some(record.age)
            );
        }
        assert_eq!(store.scan_ages().unwrap().len(), records.len());
    }

    /// The flagship correctness property for this variant — see
    /// `run_concurrency_stress_test`'s own doc comment.
    #[test]
    fn concurrent_stress_matches_sequential_replay() {
        run_concurrency_stress_test::<ShardedStore>();
    }
}
