//! Tier 2, variant 4: actor / single-writer-thread. One thread owns the
//! store outright (a plain, unsynchronized `CanonicalCachedStore` — no
//! lock, no concurrent map, since only that one thread ever touches it);
//! every other thread talks to it by sending a request over a channel and
//! blocking on a per-call reply channel for the result. Correctness is
//! structural here, not something a lock has to enforce: since exactly one
//! thread ever calls into the store, there is no concurrent access to race
//! on in the first place — the interesting question this variant tests is
//! purely a performance one (does routing every operation through a
//! channel and a dedicated thread cost more than it's worth), not a
//! correctness one.
//!
//! # `std::sync::mpsc`, not `crossbeam-channel`
//!
//! `crossbeam-channel` is generally faster and offers `select!` over
//! multiple channels, neither of which this variant needs: every call is a
//! single request to one fixed destination (the actor thread) followed by
//! blocking on one fixed reply channel — the simplest possible
//! request/response shape, with nothing to select over. `std::sync::mpsc`
//! already in the standard library covers this exactly, so it's the
//! version that doesn't need a new dependency — matching this crate's
//! "justify every addition, keep the list short" dependency discipline
//! (`dashmap`, for variant 3, is the one addition this pass actually
//! needs).
//!
//! # Why a `Mutex` around the request sender
//!
//! `ConcurrentStore` requires `Send + Sync` so a variant can be shared via
//! `Arc` across threads that only ever hold `&Self`. Whether
//! `mpsc::Sender<T>` itself is `Sync` has varied across `std`'s
//! implementation history; rather than depend on that, `request_tx` is
//! wrapped in a `Mutex` here — `Mutex<T>` is unconditionally `Sync` when
//! `T: Send`, regardless of `T`'s own `Sync` status, so `ActorStore`'s
//! `Sync`ness doesn't depend on an implementation detail of the channel
//! type. The lock is held only for the `send` call itself (a single,
//! non-blocking, infallible-in-practice enqueue), not for the reply wait —
//! so it adds a small, fixed enqueue cost per call, not a second point of
//! store-wide serialization on top of the actor thread's own.
//!
//! # The worker thread is detached, not joined
//!
//! `ConcurrentStore::new` spawns the actor thread and doesn't keep its
//! `JoinHandle` — the thread's own loop (`for request in request_rx`) exits
//! naturally once every clone of `request_tx` is dropped (the channel
//! closes, `recv` returns `Err`, the `for` loop ends), which happens when
//! the owning `ActorStore` (and every `Arc` clone of it) is dropped. There
//! is nothing left to explicitly join against at that point that isn't
//! already handled by the channel's own closing behavior.

use super::{ConcurrencyError, ConcurrentStore};
use crate::record::DogRecord;
use crate::store::{CanonicalCachedStore, DogStore, StoreError};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use uuid::Uuid;

enum ActorRequest {
    Get {
        id: Uuid,
        reply: mpsc::Sender<Option<DogRecord>>,
    },
    ScanAges {
        reply: mpsc::Sender<Vec<u32>>,
    },
    UpdateAge {
        id: Uuid,
        age: u32,
        reply: mpsc::Sender<Result<(), StoreError>>,
    },
}

/// Actor/single-writer-thread concurrent store. See module docs for the
/// concurrency model.
pub struct ActorStore {
    request_tx: Mutex<mpsc::Sender<ActorRequest>>,
}

impl ConcurrentStore for ActorStore {
    fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<ActorRequest>();
        let mut store = CanonicalCachedStore::new(records, edges);

        thread::spawn(move || {
            for request in request_rx {
                match request {
                    ActorRequest::Get { id, reply } => {
                        let _ = reply.send(store.get(id));
                    }
                    ActorRequest::ScanAges { reply } => {
                        let _ = reply.send(store.scan_ages());
                    }
                    ActorRequest::UpdateAge { id, age, reply } => {
                        let _ = reply.send(store.update_age(id, age));
                    }
                }
            }
        });

        Self {
            request_tx: Mutex::new(request_tx),
        }
    }

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::ActorDisconnected`] if the worker thread
    /// has already exited — can't happen while this `ActorStore` (and every
    /// `Arc` clone of it) is still alive, since the worker only exits once
    /// every sender is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the sender mutex is poisoned. The only operation performed
    /// while holding it is a single, infallible-in-practice channel
    /// `send`, so poisoning can't happen here in practice — the explicit,
    /// documented exception to "no unwrap/expect outside tests" this
    /// pass's own constraints call for.
    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.request_tx
            .lock()
            .expect(
                "sender mutex poisoned: the only op while holding it (channel send) can't panic",
            )
            .send(ActorRequest::Get {
                id,
                reply: reply_tx,
            })
            .map_err(|_| ConcurrencyError::ActorDisconnected)?;
        reply_rx
            .recv()
            .map_err(|_| ConcurrencyError::ActorDisconnected)
    }

    /// # Errors
    ///
    /// See [`Self::get`].
    ///
    /// # Panics
    ///
    /// See [`Self::get`].
    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.request_tx
            .lock()
            .expect(
                "sender mutex poisoned: the only op while holding it (channel send) can't panic",
            )
            .send(ActorRequest::ScanAges { reply: reply_tx })
            .map_err(|_| ConcurrencyError::ActorDisconnected)?;
        reply_rx
            .recv()
            .map_err(|_| ConcurrencyError::ActorDisconnected)
    }

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`StoreError::NotFound`]
    /// if `id` has no record, or [`ConcurrencyError::ActorDisconnected`] per
    /// [`Self::get`].
    ///
    /// # Panics
    ///
    /// See [`Self::get`].
    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.request_tx
            .lock()
            .expect(
                "sender mutex poisoned: the only op while holding it (channel send) can't panic",
            )
            .send(ActorRequest::UpdateAge {
                id,
                age,
                reply: reply_tx,
            })
            .map_err(|_| ConcurrencyError::ActorDisconnected)?;
        reply_rx
            .recv()
            .map_err(|_| ConcurrencyError::ActorDisconnected)??;
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
        let store = ActorStore::new(sample(), Vec::new());
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
        let store = ActorStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages().unwrap();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    /// The flagship correctness property for this variant — see
    /// `run_concurrency_stress_test`'s own doc comment. Less interesting
    /// here than for the lock-based variants (a single-writer-thread design
    /// can't race with itself), but still confirms the channel plumbing
    /// itself never drops, duplicates, or misroutes a request/reply.
    #[test]
    fn concurrent_stress_matches_sequential_replay() {
        run_concurrency_stress_test::<ActorStore>();
    }
}
