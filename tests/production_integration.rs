//! Flagship integration test for `ProductionStore` — the highest-priority
//! test this round adds. Mmap durability (`STORAGE-009`) and global-`RwLock`
//! concurrency (`STORAGE-010`) have each only ever been verified in
//! isolation until now: `MmapAgeStore`'s own tests cover single-threaded
//! flush/reopen, `GlobalRwLockStore`'s own stress test covers concurrency
//! over a purely in-memory store. This is the first time both run together,
//! on top of `CanonicalCachedStore`'s architecture, as the one stack someone
//! deploying `ProductionStore` would actually run. See
//! `docs/decisions/ADR-0008-production-default.md` and
//! `docs/specifications/storage/STORAGE-011-production-default.md`.
//!
//! # What this proves
//!
//! Two phases of real concurrent reader/writer contention (16 threads ×
//! 2,000 iterations each — the same bar
//! `run_concurrency_stress_test`/`STORAGE-010` already established),
//! separated by a genuine drop + reopen from disk (not just an in-process
//! flush) to exercise durability in the middle of the test, not just at the
//! end. The write log spans both phases. Final state is checked two ways:
//!
//! 1. **Linearizability** — the full, two-phase recorded write order is
//!    replayed sequentially against a fresh, single-threaded reference
//!    `CanonicalCachedStore` built from the same initial data; the reopened
//!    store's final value for every contended id must match exactly (no
//!    lost updates), and must be either the initial value or a value some
//!    thread genuinely attempted to write (no torn reads) — the identical
//!    two-part check `run_concurrency_stress_test` already established.
//! 2. **Persistence** — verified via a *third*, fresh `ProductionStore::open`
//!    call, made only after phase 2's store handle has been fully dropped
//!    (unmapping the file, not just idling). If flush/reopen didn't
//!    genuinely persist to disk, this open would see stale or initial
//!    values instead of what phase 2 actually wrote — this is what
//!    distinguishes "durable" from "just still resident in this process's
//!    page cache."

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusty_multimodal_db::concurrency::ConcurrentStore;
use rusty_multimodal_db::store::CanonicalCachedStore;
use rusty_multimodal_db::{bench_support, ProductionStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use uuid::Uuid;

const THREADS: usize = 16;
const ITERATIONS_PER_THREAD: usize = 2_000;
const CONTENDED_ID_COUNT: usize = 20;
const SEED: u64 = 0x5052_4F44_5354_4B49; // "PROD" + "STKI" in ASCII hex, arbitrary

/// Run one phase of `THREADS` reader/writer threads issuing a random,
/// interleaved sequence of `get`/`update_age` calls against `store` (shared
/// via `Arc`, through its `ConcurrentStore` side — this is what makes the
/// RwLock concurrency layer real, not simulated). Mirrors
/// `run_concurrency_stress_test`'s own shape (`src/concurrency/mod.rs`)
/// exactly, factored into a standalone function so it can run twice against
/// the *same* accumulating `write_log`/`attempted_writes`, with a real
/// drop-and-reopen happening between the two calls.
fn run_contention_phase(
    store: Arc<ProductionStore>,
    ids: &[Uuid],
    seed_xor: u64,
    write_log: &Arc<Mutex<Vec<(Uuid, u32)>>>,
    attempted_writes: &Arc<Mutex<HashMap<Uuid, Vec<u32>>>>,
) {
    let mut handles = Vec::with_capacity(THREADS);
    for thread_index in 0..THREADS {
        let store = Arc::clone(&store);
        let write_log = Arc::clone(write_log);
        let attempted_writes = Arc::clone(attempted_writes);
        let ids = ids.to_vec();
        handles.push(thread::spawn(move || {
            let mut rng = StdRng::seed_from_u64(SEED ^ seed_xor ^ thread_index as u64);
            for iteration in 0..ITERATIONS_PER_THREAD {
                let id = ids[rng.gen_range(0..ids.len())];
                if rng.gen_bool(0.5) {
                    // A read: no assertion here, contention makes any
                    // in-flight value valid — the real check happens once
                    // every thread across both phases has finished.
                    let _ = store.get(id);
                } else {
                    let age = (seed_xor as u32)
                        .wrapping_add((thread_index as u32) * 1_000_000 + iteration as u32);
                    attempted_writes
                        .lock()
                        .expect("bookkeeping mutex never poisoned: no panic while holding it")
                        .entry(id)
                        .or_default()
                        .push(age);
                    if store.update_age(id, age).is_ok() {
                        write_log
                            .lock()
                            .expect("bookkeeping mutex never poisoned: no panic while holding it")
                            .push((id, age));
                    }
                }
            }
        }));
    }
    for handle in handles {
        handle
            .join()
            .expect("contention-phase worker thread panicked");
    }
}

#[test]
fn concurrent_writers_survive_a_drop_and_reopen_with_no_lost_updates() {
    let dir = bench_support::fresh_temp_dir("production_flagship").expect("temp dir");
    let path = dir.join("ages.mmap");

    let dataset = bench_support::build_dataset(500);
    let contended_ids: Vec<Uuid> = dataset.sample_ids[..CONTENDED_ID_COUNT].to_vec();

    let write_log: Arc<Mutex<Vec<(Uuid, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let attempted_writes: Arc<Mutex<HashMap<Uuid, Vec<u32>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Phase 1: concurrent contention against a freshly created store.
    let store = Arc::new(
        ProductionStore::create(dataset.records.clone(), dataset.edges.clone(), &path)
            .expect("create"),
    );
    run_contention_phase(
        Arc::clone(&store),
        &contended_ids,
        0x1111_1111,
        &write_log,
        &attempted_writes,
    );
    store.flush().expect("flush after phase 1");
    // Drop every handle so the mapping is genuinely torn down, not just
    // idle, before reopening — the closest this in-process test can get to
    // a real process restart without literally forking a new process.
    drop(store);

    // Phase 2: reopen from disk, run a second round of concurrent
    // contention against the *reopened* store — phase 2's threads race
    // against phase 1's already-persisted state, continuing the same
    // write log rather than starting a fresh one.
    let store = Arc::new(
        ProductionStore::open(dataset.records.clone(), dataset.edges.clone(), &path)
            .expect("open after phase 1"),
    );
    run_contention_phase(
        Arc::clone(&store),
        &contended_ids,
        0x2222_2222,
        &write_log,
        &attempted_writes,
    );
    store.flush().expect("flush after phase 2");
    drop(store);

    // Verification 1 setup: a THIRD, fresh open — only after phase 2's own
    // handle is fully dropped — is the actual persistence check. If
    // flush/reopen hadn't genuinely reached disk, this would see stale
    // values instead of what phase 2 actually wrote.
    let mut reopened = ProductionStore::open(dataset.records.clone(), dataset.edges.clone(), &path)
        .expect("final reopen");

    // Verification 2 setup: replay the full two-phase recorded write order
    // sequentially against a fresh, single-threaded reference store built
    // from the same initial data.
    let mut reference = CanonicalCachedStore::new(dataset.records.clone(), dataset.edges.clone());
    {
        use rusty_multimodal_db::DogStore;
        for (id, age) in write_log
            .lock()
            .expect("bookkeeping mutex never poisoned: no panic while holding it")
            .iter()
        {
            reference.update_age(*id, *age).expect(
                "replaying a write that succeeded during either contention phase must still \
                 succeed against a fresh store built from the same initial data",
            );
        }
    }

    let attempted_writes = attempted_writes
        .lock()
        .expect("bookkeeping mutex never poisoned: no panic while holding it");
    {
        use rusty_multimodal_db::DogStore;
        for &id in &contended_ids {
            let persisted_age = DogStore::get(&reopened, id).map(|record| record.age);
            let reference_age = reference.get(id).map(|record| record.age);
            assert_eq!(
                persisted_age, reference_age,
                "id {id} diverged after the drop/reopen in the middle of the test: the reopened \
                 store shows {persisted_age:?}, sequential replay of the recorded write order \
                 across both phases shows {reference_age:?} — lost update, corrupted write, or a \
                 write that didn't actually persist across the reopen"
            );

            if let Some(age) = persisted_age {
                let initial_age = dataset
                    .records
                    .iter()
                    .find(|record| record.id == id)
                    .map(|record| record.age)
                    .expect("contended ids are drawn from this dataset's own sample_ids");
                let ever_attempted = attempted_writes
                    .get(&id)
                    .map(|values| values.contains(&age))
                    .unwrap_or(false);
                assert!(
                    age == initial_age || ever_attempted,
                    "id {id}'s persisted age {age} was never the initial value ({initial_age}) \
                     nor any value a thread attempted to write across either phase — a \
                     torn/corrupted write"
                );
            }
        }

        // The reopened store also has to keep working as a plain, exclusive-
        // owner DogStore after all this concurrent contention and a real
        // reopen — not just satisfy ConcurrentStore's narrower surface.
        // Exercises same_breed/neighbors (outside ConcurrentStore's scope
        // entirely) and one more update_age/get round-trip.
        for &id in &contended_ids {
            let _ = reopened.same_breed(id);
            let _ = reopened.neighbors(id);
        }
        // Qualified explicitly (not `reopened.update_age(...)`): autoref
        // method resolution would otherwise silently prefer
        // `ConcurrentStore::update_age`'s `&self` over `DogStore`'s `&mut
        // self` at an earlier resolution step, defeating the point of this
        // check — this must genuinely exercise the `DogStore` path.
        DogStore::update_age(&mut reopened, contended_ids[0], 999)
            .expect("update_age on a known id after reopen");
        assert_eq!(DogStore::get(&reopened, contended_ids[0]).unwrap().age, 999);
    }

    let _ = std::fs::remove_dir_all(&dir);
}
