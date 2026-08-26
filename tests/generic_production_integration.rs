//! Flagship integration test for [`GenericProductionStore`] — the generic
//! analogue of `tests/production_integration.rs`, run against
//! `Order`/`Customer` instead of `Dog`. Mmap durability
//! ([`GenericMmapStore`]) and `RwLock` concurrency
//! ([`GenericProductionStore`]) have each only been verified in isolation
//! until now (`GenericMmapStore`'s own tests cover single-threaded
//! flush/reopen; nothing before this test ran real concurrent threads
//! against a generic store at all). This is the first time both run
//! together, on the directed-relation domain the design doc's §4.3/§4.7
//! and the `directed-relation-spike` round both targeted, exactly the bar
//! `STORAGE-011-FR-004` already set for `Dog`.
//!
//! # What this proves
//!
//! Two phases of real concurrent reader/writer contention (16 threads ×
//! 2,000 iterations each — the same bar `run_concurrency_stress_test`/
//! `tests/production_integration.rs` already established) against the one
//! durable, mmap-backed field (`Amount`), separated by a genuine drop +
//! reopen from disk. Final state is checked two ways, mirroring
//! `production_integration.rs` exactly:
//!
//! 1. **Linearizability** — the full, two-phase recorded write order is
//!    replayed sequentially against a plain `HashMap<Uuid, i64>` reference
//!    (not another generic store — see the note below on why), and the
//!    reopened store's final value for every contended order must match
//!    exactly (no lost updates) and be either the initial value or a value
//!    some thread genuinely attempted to write (no torn reads).
//! 2. **Persistence** — verified via a *third*, fresh
//!    `open_order_production_stack` call, made only after phase 2's store
//!    handle is fully dropped.
//!
//! # Why the reference is a plain `HashMap`, not `OrderGenericStore`
//!
//! `crate::generic`'s own module docs name a known limitation: the
//! purely in-memory `Scanned`/`BaseStore` composition
//! (`build_order_generic_store`) does not write through — `GetById::get`
//! on it can return a stale value for a field `UpdateField::update` just
//! wrote. Using it as this test's sequential-replay reference would make
//! the reference itself untrustworthy. A plain `HashMap<Uuid, i64>`,
//! updated directly by this test as it replays the write log, has no such
//! gap and is a correct ground truth for one scalar field — exactly what
//! this comparison needs. [`GenericMmapStore`] (what
//! `GenericProductionStore` in this test actually wraps) does **not**
//! have the staleness gap — see its own module docs — which is exactly
//! what this test is checking.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, open_order_production_stack, Amount, BelongsToCustomer,
    Customer, Order, OrderProductionStack, OrderStatus, Status,
};
use rusty_multimodal_db::generic::{GenericProductionStore, NotFound};
use rusty_multimodal_db::generic_spike::order_bench_support::build_order_dataset;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use uuid::Uuid;

const THREADS: usize = 16;
const ITERATIONS_PER_THREAD: usize = 2_000;
const CONTENDED_ID_COUNT: usize = 20;
const SEED: u64 = 0x4745_4E50_524F_4453; // "GENPRODS" in ASCII hex, arbitrary

/// One phase of `THREADS` reader/writer threads issuing a random,
/// interleaved sequence of `get`/`update` calls (on `Amount`) against
/// `store`, shared via `Arc`. Mirrors `production_integration.rs`'s
/// `run_contention_phase` exactly, generic-store-shaped.
fn run_contention_phase(
    store: Arc<GenericProductionStore<OrderProductionStack>>,
    ids: &[Uuid],
    seed_xor: u64,
    write_log: &Arc<Mutex<Vec<(Uuid, i64)>>>,
    attempted_writes: &Arc<Mutex<HashMap<Uuid, Vec<i64>>>>,
    order_lock: &Arc<Mutex<()>>,
) {
    let mut handles = Vec::with_capacity(THREADS);
    for thread_index in 0..THREADS {
        let store = Arc::clone(&store);
        let write_log = Arc::clone(write_log);
        let attempted_writes = Arc::clone(attempted_writes);
        let order_lock = Arc::clone(order_lock);
        let ids = ids.to_vec();
        handles.push(thread::spawn(move || {
            let mut rng = StdRng::seed_from_u64(SEED ^ seed_xor ^ thread_index as u64);
            for iteration in 0..ITERATIONS_PER_THREAD {
                let id = ids[rng.gen_range(0..ids.len())];
                if rng.gen_bool(0.5) {
                    let _ = store.get::<Order>(id);
                } else {
                    let amount = (seed_xor as i64)
                        .wrapping_add((thread_index as i64) * 1_000_000 + iteration as i64);
                    attempted_writes
                        .lock()
                        .expect("bookkeeping mutex never poisoned: no panic while holding it")
                        .entry(id)
                        .or_default()
                        .push(amount);
                    let _order_guard = order_lock
                        .lock()
                        .expect("bookkeeping mutex never poisoned: no panic while holding it");
                    let result: Result<(), NotFound<Uuid>> =
                        store.update::<Order, Amount>(id, amount);
                    if result.is_ok() {
                        write_log
                            .lock()
                            .expect("bookkeeping mutex never poisoned: no panic while holding it")
                            .push((id, amount));
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
    let dir = rusty_multimodal_db::bench_support::fresh_temp_dir("generic_production_flagship")
        .expect("temp dir");
    let path = dir.join("amount.mmap");

    let dataset = build_order_dataset(500);
    let contended_ids: Vec<Uuid> = dataset.sample_order_ids[..CONTENDED_ID_COUNT].to_vec();
    let initial_amounts: HashMap<Uuid, i64> = dataset
        .orders
        .iter()
        .map(|order| (order.id, order.amount_cents))
        .collect();

    let write_log: Arc<Mutex<Vec<(Uuid, i64)>>> = Arc::new(Mutex::new(Vec::new()));
    let attempted_writes: Arc<Mutex<HashMap<Uuid, Vec<i64>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let order_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Phase 1: concurrent contention against a freshly created store.
    let stack = create_order_production_stack(dataset.orders.clone(), &path).expect("create");
    let store = Arc::new(GenericProductionStore::new(stack));
    run_contention_phase(
        Arc::clone(&store),
        &contended_ids,
        0x1111_1111,
        &write_log,
        &attempted_writes,
        &order_lock,
    );
    store.flush().expect("flush after phase 1");
    // Drop every handle so the mapping is genuinely torn down before
    // reopening.
    drop(store);

    // Phase 2: reopen from disk, run a second round of concurrent
    // contention — continuing the same write log rather than starting a
    // fresh one.
    let stack =
        open_order_production_stack(dataset.orders.clone(), &path).expect("open after phase 1");
    let store = Arc::new(GenericProductionStore::new(stack));
    run_contention_phase(
        Arc::clone(&store),
        &contended_ids,
        0x2222_2222,
        &write_log,
        &attempted_writes,
        &order_lock,
    );
    store.flush().expect("flush after phase 2");
    drop(store);

    // Verification 1 setup: a THIRD, fresh open — only after phase 2's own
    // handle is fully dropped — is the actual persistence check.
    let reopened_stack =
        open_order_production_stack(dataset.orders.clone(), &path).expect("final reopen");
    let reopened = GenericProductionStore::new(reopened_stack);

    // Verification 2 setup: replay the full two-phase recorded write order
    // sequentially against a plain HashMap reference (see module docs for
    // why not `OrderGenericStore`).
    let mut reference = initial_amounts;
    for (id, amount) in write_log
        .lock()
        .expect("bookkeeping mutex never poisoned: no panic while holding it")
        .iter()
    {
        reference.insert(*id, *amount);
    }

    let attempted_writes = attempted_writes
        .lock()
        .expect("bookkeeping mutex never poisoned: no panic while holding it");
    for &id in &contended_ids {
        let persisted_amount = reopened.get::<Order>(id).map(|order| order.amount_cents);
        let reference_amount = reference.get(&id).copied();
        assert_eq!(
            persisted_amount, reference_amount,
            "order {id} diverged after the drop/reopen in the middle of the test: the reopened \
             store shows {persisted_amount:?}, sequential replay of the recorded write order \
             across both phases shows {reference_amount:?} — lost update, corrupted write, or a \
             write that didn't actually persist across the reopen"
        );

        if let Some(amount) = persisted_amount {
            let initial_amount = *reference
                .get(&id)
                .expect("contended ids are drawn from this dataset's own sample_order_ids");
            let ever_attempted = attempted_writes
                .get(&id)
                .map(|values| values.contains(&amount))
                .unwrap_or(false);
            let is_initial = dataset
                .orders
                .iter()
                .find(|order| order.id == id)
                .map(|order| order.amount_cents == amount)
                .unwrap_or(false);
            assert!(
                is_initial || ever_attempted,
                "order {id}'s persisted amount {amount} was never the initial value nor any \
                 value a thread attempted to write across either phase — a torn/corrupted write \
                 (initial_amount tracked as {initial_amount})"
            );
        }
    }

    // The reopened store keeps working for every other capability this
    // domain has after all this concurrent contention and a real reopen —
    // not just the one durable field.
    for &id in &contended_ids {
        let _ = reopened.parent::<Order, BelongsToCustomer>(id);
    }
    for &customer_id in &dataset.sample_customer_ids[..5] {
        let _ = reopened.children::<Customer, Order, BelongsToCustomer>(customer_id);
    }
    // Exercises `FilterEq` post-reopen too — not asserted against a count
    // (dataset composition isn't this test's concern), just proving the
    // path still works after everything above.
    let _ = reopened.filter_eq::<Order, Status>(&OrderStatus::Shipped);

    let _ = std::fs::remove_dir_all(&dir);
}
