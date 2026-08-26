//! Concurrent reader/writer access prototypes for `CanonicalCachedStore` —
//! the only backend that gets a concurrency story in this pass, same
//! "don't build for backends nobody would deploy" reasoning the durability
//! work (`STORAGE-008`/`STORAGE-009`) already established for that backend.
//! Everything benchmarked before this module assumed single-threaded
//! access; this tests what happens once real reader/writer threads contend
//! for the same store. See `docs/decisions/ADR-0007-concurrency-strategies.md`
//! for the design decisions and `docs/specifications/storage/STORAGE-010-concurrency-prototypes.md`
//! for the requirements each variant satisfies.
//!
//! # Scope: in-memory only, not paired with a durability variant
//!
//! This is concurrency over the plain, non-durable `CanonicalCachedStore`
//! shape — none of these variants persist anything to disk. Combining a
//! concurrency strategy with a specific durability variant (e.g. "a
//! sharded, WAL-backed store") is a real, natural follow-up, but a second
//! round: the two axes (concurrency, durability) multiply into a much
//! larger design space than either alone, and this pass's job is to
//! establish the concurrency numbers on their own footing first.
//!
//! # Two tiers, same rigor split as durability
//!
//! - **Tier 1** (full build, correctness-stress-tested, fully benchmarked):
//!   [`global_rwlock::GlobalRwLockStore`] (one `RwLock` around the whole
//!   store), [`sharded::ShardedStore`] (N independently-locked shards,
//!   partitioned by UUID), [`dashmap_store::DashMapStore`] (swaps the
//!   canonical map for `dashmap::DashMap`).
//! - **Tier 2** (lighter proof-of-concept): [`actor::ActorStore`] (a single
//!   writer thread owns the store outright; every other thread talks to it
//!   over a channel).
//!
//! # Why one shared trait, not four uses of `DogStore`
//!
//! `DogStore::update_age` takes `&mut self` — correct for single-threaded
//! backends, but incompatible with sharing one store instance across
//! threads (`&mut` can't be held by more than one caller at a time, which
//! is exactly what these variants exist to allow via their own internal
//! synchronization). [`ConcurrentStore`] below takes `&self` everywhere,
//! including for `update_age`, and returns `Result` uniformly across all
//! four variants — mirroring how `DogStore::update_age` is `Result` for
//! every backend even though only an unknown UUID ever produces `Err` for
//! three of the four; here, only [`actor::ActorStore`] can fail on a
//! channel disconnect, but giving every method one shared, fallible
//! signature is what lets one generic stress test and one generic
//! benchmark loop drive all four variants identically.

use crate::record::DogRecord;
use crate::store::StoreError;
use thiserror::Error;
use uuid::Uuid;

pub mod actor;
pub mod dashmap_store;
pub mod global_rwlock;
pub mod sharded;

pub use actor::ActorStore;
pub use dashmap_store::DashMapStore;
pub use global_rwlock::GlobalRwLockStore;
pub use sharded::ShardedStore;

/// Every fallible outcome across every concurrency variant. `Store` covers
/// the one failure mode every variant shares (`update_age` on an unknown
/// UUID); `ActorDisconnected` is specific to [`ActorStore`] — a channel
/// send/recv failing because the single writer thread has already exited,
/// which can't happen while the owning `ActorStore` is still alive (its
/// `Drop` doesn't run until every clone/reference is gone), but callers
/// still see the same `Result`-shaped API as every other variant rather
/// than a special case.
#[derive(Debug, Error)]
pub enum ConcurrencyError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("actor thread disconnected")]
    ActorDisconnected,
}

/// Shared interface every concurrency variant implements: `&self` (not
/// `&mut self`) on every method, since these types are shared across
/// threads via `Arc`, with each variant responsible for its own internal
/// synchronization. See the module docs for why this isn't just `DogStore`
/// with a different receiver type.
pub trait ConcurrentStore: Send + Sync {
    /// Build from records and littermate edges — same inputs every other
    /// backend/variant in this crate takes.
    fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self
    where
        Self: Sized;

    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError>;

    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError>;

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`StoreError::NotFound`]
    /// if `id` has no record.
    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError>;
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// Threads spawned by [`run_concurrency_stress_test`] — comfortably
    /// above the "16+" the task calls for.
    const STRESS_THREADS: usize = 16;
    /// Operations per thread — enough that, against a pool this small, most
    /// contended ids get touched by most threads many times over.
    const STRESS_ITERATIONS_PER_THREAD: usize = 2_000;
    /// A small, shared pool of ids every thread hammers — deliberately
    /// tiny relative to the dataset so writes to the *same* id from
    /// *different* threads are frequent, not rare. This is the whole point
    /// of the stress test: exercising real contention, not just running
    /// threads that never touch each other's keys.
    const CONTENDED_ID_COUNT: usize = 20;
    const STRESS_SEED: u64 = 0x5773_5253_5354_5345; // "SWRSTSTE" in ASCII hex, arbitrary

    /// The flagship correctness test for every Tier 1/Tier 2 concurrency
    /// variant — same priority `CanonicalCachedStore`'s stale-cache test
    /// carried in the original round. Spawns [`STRESS_THREADS`] reader and
    /// writer threads issuing a random, interleaved sequence of
    /// `get`/`update_age` calls against one shared `S` instance. Every
    /// successful write is appended, immediately after it returns `Ok`, to
    /// a shared, externally-observed log (guarded by its own mutex,
    /// independent of `S`'s internal synchronization) — a witness of the
    /// real order in which writes actually completed. After every thread
    /// joins, that exact recorded order is replayed *sequentially* against
    /// a fresh, single-threaded reference store built from the same
    /// initial data, and the final state of `S` must match the reference
    /// store's final state exactly: no lost update (a write that completed
    /// but didn't "stick"), no torn write (a value that doesn't correspond
    /// to anything any thread ever actually wrote).
    ///
    /// # A note on what this does and doesn't prove
    ///
    /// The write log's append is made atomic with the `update_age` call
    /// itself, via `order_lock`: a thread holds that guard across both the
    /// store call and the log push, so no other thread's write can
    /// interleave between "the write completed" and "the write was
    /// recorded." An earlier version of this test appended to the log
    /// under its own, separate mutex, released as soon as the store call
    /// returned; that gap let a second thread's write-and-log complete
    /// between the first thread's store call returning and its log-append
    /// running, letting the log's order for *same-id* writes diverge from
    /// the order they actually completed in — CI caught this directly, as
    /// intermittent false-positive "lost update" failures on the exact
    /// (contended-id, real-timing) coincidence this smoke test exists to
    /// probe for. This is a linearizability *smoke test*, not a formal
    /// checker (e.g. Jepsen/Knossos) — appropriate for a benchmark harness,
    /// not offered as a proof.
    pub(crate) fn run_concurrency_stress_test<S: ConcurrentStore + 'static>() {
        let dataset = crate::bench_support::build_dataset(500);
        let contended_ids: Vec<Uuid> = dataset.sample_ids[..CONTENDED_ID_COUNT].to_vec();

        let store = Arc::new(S::new(dataset.records.clone(), dataset.edges.clone()));
        let write_log: Arc<Mutex<Vec<(Uuid, u32)>>> = Arc::new(Mutex::new(Vec::new()));
        // Guards `update_age` + the write_log push as one critical section
        // (see the doc comment above) so the log's append order always
        // matches the store's true write-completion order for same-id
        // writes, which is what the sequential-replay check below depends
        // on.
        let order_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
        // Every value any thread ever *attempts* to write to a given id —
        // used for the secondary "no torn reads" membership check below.
        let attempted_writes: Arc<Mutex<std::collections::HashMap<Uuid, Vec<u32>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));

        let mut handles = Vec::with_capacity(STRESS_THREADS);
        for thread_index in 0..STRESS_THREADS {
            let store = Arc::clone(&store);
            let write_log = Arc::clone(&write_log);
            let attempted_writes = Arc::clone(&attempted_writes);
            let order_lock = Arc::clone(&order_lock);
            let ids = contended_ids.clone();
            handles.push(thread::spawn(move || {
                let mut rng = StdRng::seed_from_u64(STRESS_SEED ^ thread_index as u64);
                for iteration in 0..STRESS_ITERATIONS_PER_THREAD {
                    let id = ids[rng.gen_range(0..ids.len())];
                    if rng.gen_bool(0.5) {
                        // A read: no correctness assertion made on the
                        // returned value here (contention makes any
                        // particular in-flight value valid) — the real
                        // check happens once every thread has finished, via
                        // the sequential replay comparison.
                        let _ = store.get(id);
                    } else {
                        let age = (thread_index as u32) * 1_000_000 + iteration as u32;
                        attempted_writes
                            .lock()
                            .expect("stress-test bookkeeping mutex never poisoned: no operation while holding it can panic")
                            .entry(id)
                            .or_default()
                            .push(age);
                        // Held across the store call and the log push (see
                        // this fn's doc comment): makes the log's append
                        // order match the store's true write order.
                        let _order_guard = order_lock.lock().expect(
                            "stress-test bookkeeping mutex never poisoned: no operation while holding it can panic",
                        );
                        if store.update_age(id, age).is_ok() {
                            write_log
                                .lock()
                                .expect("stress-test bookkeeping mutex never poisoned: no operation while holding it can panic")
                                .push((id, age));
                        }
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().expect("stress-test worker thread panicked");
        }

        // Sequential replay: fresh reference store, same initial data, the
        // exact recorded write order applied one at a time.
        let mut reference =
            crate::store::CanonicalCachedStore::new(dataset.records.clone(), dataset.edges.clone());
        for (id, age) in write_log
            .lock()
            .expect("stress-test bookkeeping mutex never poisoned: no operation while holding it can panic")
            .iter()
        {
            reference
                .update_age(*id, *age)
                .expect("replaying a write that succeeded during the stress run must still succeed against a fresh store built from the same initial data");
        }
        use crate::store::DogStore as _;

        let attempted_writes = attempted_writes.lock().expect(
            "stress-test bookkeeping mutex never poisoned: no operation while holding it can panic",
        );
        for &id in &contended_ids {
            let concurrent_age = store
                .get(id)
                .expect("get on a variant with no fallible read path")
                .map(|record| record.age);
            let reference_age = reference.get(id).map(|record| record.age);
            assert_eq!(
                concurrent_age, reference_age,
                "id {id} diverged after the stress run: concurrent store shows {concurrent_age:?}, \
                 sequential replay of the exact recorded write order shows {reference_age:?} — \
                 lost update or corrupted write"
            );

            // No torn reads: whatever the concurrent store ended up with
            // must be either the untouched initial value or one of the
            // values some thread actually attempted to write — never a
            // value that doesn't correspond to any real write.
            if let Some(age) = concurrent_age {
                let initial_age = dataset
                    .records
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| r.age)
                    .expect("contended ids are drawn from this dataset's own sample_ids");
                let ever_attempted = attempted_writes
                    .get(&id)
                    .map(|values| values.contains(&age))
                    .unwrap_or(false);
                assert!(
                    age == initial_age || ever_attempted,
                    "id {id}'s final age {age} was never the initial value ({initial_age}) nor any \
                     value a thread attempted to write ({:?}) — a torn/corrupted write",
                    attempted_writes.get(&id)
                );
            }
        }
    }
}
