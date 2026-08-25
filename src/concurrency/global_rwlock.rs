//! Tier 1, variant 1: one `RwLock` around the whole store.
//!
//! The simplest possible strategy, and the reference point the other three
//! variants are measured against: wraps the existing, closed
//! `CanonicalCachedStore` (`src/store/canonical_cached.rs`) directly —
//! nothing about that backend needed to change or be duplicated, since
//! `RwLock<T>` only needs `&self`/`&mut self` access to `T`, which
//! `CanonicalCachedStore`'s own `DogStore` impl already provides. Multiple
//! readers can hold the lock concurrently; a writer needs exclusive access,
//! blocking every other reader and writer until it's done — the whole
//! store is the unit of contention, which is exactly the tradeoff this
//! variant exists to measure against sharded/lock-free alternatives.
//!
//! # `std::sync::RwLock`, not `parking_lot`
//!
//! `parking_lot::RwLock` is generally faster (no poisoning overhead,
//! smaller/faster fast-path locking) and is a common choice for real
//! concurrent Rust code. This prototype uses the standard library's own
//! `RwLock` instead: it needs no new dependency (this crate's dependency
//! list is short and every addition has been individually justified — see
//! `Cargo.toml`), and the whole point of *this* variant is to be the
//! simplest possible baseline; reaching for a specialized lock crate before
//! measuring whether the standard one is even a bottleneck would be
//! optimizing before there's a number to justify it. If the benchmark
//! numbers in `RESULTS.md` show std's `RwLock` overhead is actually
//! significant, swapping in `parking_lot` here is a small, contained
//! change — noted as a candidate follow-up, not built speculatively now.

use super::{ConcurrencyError, ConcurrentStore};
use crate::record::DogRecord;
use crate::store::{CanonicalCachedStore, DogStore};
use std::sync::RwLock;
use uuid::Uuid;

/// Global-`RwLock`-backed concurrent store. See module docs for the
/// concurrency model.
pub struct GlobalRwLockStore {
    inner: RwLock<CanonicalCachedStore>,
}

impl ConcurrentStore for GlobalRwLockStore {
    fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        Self {
            inner: RwLock::new(CanonicalCachedStore::new(records, edges)),
        }
    }

    /// # Panics
    ///
    /// Panics if the lock is poisoned. Every operation performed while
    /// holding this lock (`DogStore`'s plain `HashMap`/`Vec` reads and
    /// writes) is infallible and never panics under normal operation, so
    /// poisoning can't happen here in practice; this is the explicit,
    /// documented exception to "no unwrap/expect outside tests" this
    /// pass's own constraints call for, not an oversight.
    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError> {
        Ok(self
            .inner
            .read()
            .expect(
                "RwLock poisoned: a prior holder panicked, which no operation here should ever do",
            )
            .get(id))
    }

    /// # Panics
    ///
    /// See [`Self::get`].
    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError> {
        Ok(self
            .inner
            .read()
            .expect(
                "RwLock poisoned: a prior holder panicked, which no operation here should ever do",
            )
            .scan_ages())
    }

    /// # Errors
    ///
    /// Returns [`ConcurrencyError::Store`] wrapping [`crate::store::StoreError::NotFound`]
    /// if `id` has no record.
    ///
    /// # Panics
    ///
    /// See [`Self::get`].
    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError> {
        self.inner
            .write()
            .expect(
                "RwLock poisoned: a prior holder panicked, which no operation here should ever do",
            )
            .update_age(id, age)?;
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
        let store = GlobalRwLockStore::new(sample(), Vec::new());
        assert_eq!(
            store.get(Uuid::from_u128(1)).unwrap().unwrap().breed,
            "labrador"
        );
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().unwrap().age, 42);

        assert!(matches!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(ConcurrencyError::Store(crate::store::StoreError::NotFound(
                _
            )))
        ));
    }

    #[test]
    fn scan_ages_returns_every_age() {
        let store = GlobalRwLockStore::new(sample(), Vec::new());
        let mut ages = store.scan_ages().unwrap();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3, 5]);
    }

    /// The flagship correctness property for this variant — see
    /// `run_concurrency_stress_test`'s own doc comment.
    #[test]
    fn concurrent_stress_matches_sequential_replay() {
        run_concurrency_stress_test::<GlobalRwLockStore>();
    }
}
