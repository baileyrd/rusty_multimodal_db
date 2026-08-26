//! The crate's recommended production entry point: `CanonicalCachedStore`'s
//! architecture, made durable via mmap, made safe for concurrent
//! reader/writer access via one global `RwLock` — the three picks six
//! rounds of empirical work (row/column/graph, mixed-workload, durability,
//! and three concurrency throughput passes across a container, a Windows
//! desktop, and `baileyai`) converged on. See
//! `docs/decisions/ADR-0008-production-default.md` for which round
//! justified each layer and
//! `docs/specifications/storage/STORAGE-011-production-default.md` for the
//! requirements this module satisfies.
//!
//! # Why `RwLock<MmapAgeStore>`, not three literally nested types
//!
//! The obvious reading of "`CanonicalCachedStore` wrapped in mmap
//! durability wrapped in a global `RwLock`" is three nested structs. That's
//! not how the durability round actually built the mmap variant, though:
//! [`crate::durability::mmap_store::MmapAgeStore`] doesn't wrap
//! `CanonicalCachedStore` — it rebuilds the same canonical-map/breed-index/
//! adjacency-index/position-index architecture directly, with the mutable
//! `age` field backed by `MmapMut` instead of a plain `Vec<u32>` (see that
//! module's own docs for why: `CanonicalCachedStore`'s private fields
//! aren't reusable across eight durability variants without either
//! duplicating them eight times or breaking encapsulation). `MmapAgeStore`
//! *is* "`CanonicalCachedStore`'s architecture, made durable" — there is no
//! separate, undurable `CanonicalCachedStore` instance to nest inside it.
//! So the literal, correct composition of this round's three picks is
//! **`RwLock<MmapAgeStore>`** — storage architecture and durability already
//! combined by the prior round, concurrency layered on top by this one,
//! exactly mirroring how [`crate::concurrency::global_rwlock`] itself
//! chose to wrap `CanonicalCachedStore` directly rather than reinventing
//! its internals. This module changes nothing inside
//! `src/durability/mmap_store.rs` or `src/concurrency/global_rwlock.rs` —
//! it wires the existing, closed types together.
//!
//! # Two trait implementations, not one
//!
//! [`ProductionStore`] implements both [`DogStore`] (this crate's original,
//! single-owner interface — `&mut self` on `update_age`) and
//! [`ConcurrentStore`] (`&self` throughout, for genuine multi-threaded
//! sharing behind an `Arc`). Both are real, not one added just for
//! signature compatibility:
//!
//! - **`DogStore`** makes `ProductionStore` a drop-in for anything already
//!   written against that interface — every existing single-threaded
//!   benchmark/test helper generic over `S: DogStore` (`bench_support`'s
//!   `two_hop_neighbors`, `MixedWorkloadDriver::run_one`,
//!   `benches/workloads.rs`'s per-workload generic runners) accepts
//!   `ProductionStore` with no changes on their end. Unlike
//!   [`crate::concurrency::sharded::ShardedStore`]/
//!   [`crate::concurrency::dashmap_store::DashMapStore`] (which only cover
//!   `ConcurrentStore`'s narrower `get`/`update_age`/`scan_ages` surface),
//!   `ProductionStore` gets the *full* `DogStore` surface including
//!   `same_breed`/`neighbors` for free, because `MmapAgeStore` already
//!   implements all five methods — no new scope-down was needed here.
//! - **`ConcurrentStore`** is the interface that actually matters for this
//!   type's reason for existing: sharing one store across real reader/
//!   writer threads. Reuses this crate's existing flagship correctness
//!   test (`run_concurrency_stress_test`) and throughput harness
//!   (`benches/concurrency.rs`) with zero new test/bench infrastructure.
//!
//! # `ConcurrentStore::new`'s fixed signature vs. mmap's path requirement
//!
//! `ConcurrentStore::new(records, edges) -> Self` is infallible and takes
//! no path — every other implementor's constructor genuinely can't fail
//! (`CanonicalCachedStore::new` is plain in-memory construction), but
//! `MmapAgeStore::create` needs a filesystem path and can fail (I/O,
//! `mmap` syscall). `Self::new`/[`From`] here allocate a fresh, uniquely-
//! named temp-file backing (via [`crate::bench_support::fresh_temp_dir`],
//! the same helper every durability variant's own tests already use) and
//! `.expect()` on failure — an explicit, documented exception to "no
//! unwrap/expect outside tests," on the same footing
//! [`crate::concurrency::global_rwlock::GlobalRwLockStore`] already
//! established for `RwLock` poisoning: a failure here (no space or no
//! permission on the OS temp dir) is a genuinely exceptional environment
//! problem, not a normal operational outcome any caller could sensibly
//! recover from by inspecting a `Result`. Callers who need real fallibility
//! (a caller-supplied, persistent path) should use [`Self::create`]/
//! [`Self::open`] directly, which return `Result` throughout.

use crate::bench_support::fresh_temp_dir;
use crate::concurrency::{ConcurrencyError, ConcurrentStore};
use crate::durability::{DurabilityError, MmapAgeStore};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use uuid::Uuid;

/// This crate's recommended production type: `CanonicalCachedStore`'s
/// architecture, durable via mmap, safe for concurrent access via one
/// global `RwLock`. See module docs for the composition and why it isn't
/// three literally nested types.
pub struct ProductionStore {
    inner: RwLock<MmapAgeStore>,
}

/// Message shared by every `.expect()` in this module — one place to keep
/// the wording consistent, matching `GlobalRwLockStore`'s own per-callsite
/// `# Panics` convention for its one documented poisoning exception.
const LOCK_POISONED: &str =
    "RwLock poisoned: a prior holder panicked, which no operation here should ever do";

impl ProductionStore {
    /// Build fresh (matching [`MmapAgeStore::create`]'s semantics exactly):
    /// creates a new mmap-backed file at `path`, sized and initialized from
    /// `records`' starting ages, wrapped in a `RwLock`.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] under the same conditions
    /// [`MmapAgeStore::create`] does.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Ok(Self {
            inner: RwLock::new(MmapAgeStore::create(records, edges, path)?),
        })
    }

    /// Reopen an existing mmap-backed file at `path` (matching
    /// [`MmapAgeStore::open`]'s semantics exactly): `records`/`edges`
    /// rebuild the in-memory indexes, the file on disk remains the source
    /// of truth for every age.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] under the same conditions
    /// [`MmapAgeStore::open`] does.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        Ok(Self {
            inner: RwLock::new(MmapAgeStore::open(records, edges, path)?),
        })
    }

    /// Force every mapped age to physical disk (`msync`) — the durability
    /// guarantee a caller reaches for before a real or simulated process
    /// restart. Takes the write lock (not just a read lock): a checkpoint
    /// conceptually wants a quiescent snapshot, so this blocks concurrent
    /// `update_age` calls for its duration rather than racing an in-flight
    /// write against the flush.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the flush syscall fails.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — every operation performed while
    /// holding it is infallible and never panics under normal operation,
    /// the same reasoning `GlobalRwLockStore`'s own poisoning exception
    /// documents.
    pub fn flush(&self) -> Result<(), DurabilityError> {
        self.inner.write().expect(LOCK_POISONED).flush()
    }

    /// A fresh, uniquely-named temp file path for the constructors that
    /// don't take one ([`ConcurrentStore::new`], the two `From` impls) —
    /// see module docs for why those can't propagate a `Result`.
    fn fresh_backing_path(label: &str) -> PathBuf {
        let dir = fresh_temp_dir(label).expect(
            "creating a fresh temp directory for ProductionStore's mmap-backed file failed \
             (no space or no permission on the OS temp dir) — a genuinely exceptional \
             environment failure, not a normal outcome any caller of an infallible \
             constructor could sensibly recover from; use ProductionStore::create with an \
             explicit path if real fallibility is needed",
        );
        dir.join("ages.mmap")
    }
}

impl DogStore for ProductionStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.inner.read().expect(LOCK_POISONED).get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.inner.read().expect(LOCK_POISONED).scan_ages()
    }

    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        self.inner
            .get_mut()
            .expect(LOCK_POISONED)
            .update_age(id, age)
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        self.inner.read().expect(LOCK_POISONED).same_breed(id)
    }

    fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.inner.read().expect(LOCK_POISONED).neighbors(id)
    }
}

impl ConcurrentStore for ProductionStore {
    /// See module docs: allocates a fresh temp-file backing since this
    /// signature has no room for a caller-supplied path. Use
    /// [`ProductionStore::create`] directly for a real, persistent path.
    fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        let path = Self::fresh_backing_path("production_concurrent_store");
        Self::create(records, edges, &path).expect(
            "fresh temp-file mmap creation failed immediately after fresh_backing_path \
             succeeded in creating its parent directory — the same exceptional-environment \
             case fresh_backing_path's own panic message already documents",
        )
    }

    fn get(&self, id: Uuid) -> Result<Option<DogRecord>, ConcurrencyError> {
        Ok(self.inner.read().expect(LOCK_POISONED).get(id))
    }

    fn scan_ages(&self) -> Result<Vec<u32>, ConcurrencyError> {
        Ok(self.inner.read().expect(LOCK_POISONED).scan_ages())
    }

    fn update_age(&self, id: Uuid, age: u32) -> Result<(), ConcurrencyError> {
        self.inner
            .write()
            .expect(LOCK_POISONED)
            .update_age(id, age)?;
        Ok(())
    }
}

impl From<Vec<DogRecord>> for ProductionStore {
    /// Convenience for workloads that don't exercise `neighbors` — builds
    /// with no littermate edges, same convention every other backend's
    /// `From<Vec<DogRecord>>` impl follows. Allocates a fresh temp-file
    /// backing — see module docs.
    fn from(records: Vec<DogRecord>) -> Self {
        let path = Self::fresh_backing_path("production_from_records");
        Self::create(records, Vec::new(), &path).expect(
            "fresh temp-file mmap creation failed immediately after fresh_backing_path \
             succeeded in creating its parent directory — the same exceptional-environment \
             case fresh_backing_path's own panic message already documents",
        )
    }
}

impl From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)> for ProductionStore {
    fn from((records, edges): (Vec<DogRecord>, Vec<(Uuid, Uuid)>)) -> Self {
        let path = Self::fresh_backing_path("production_from_records_edges");
        Self::create(records, edges, &path).expect(
            "fresh temp-file mmap creation failed immediately after fresh_backing_path \
             succeeded in creating its parent directory — the same exceptional-environment \
             case fresh_backing_path's own panic message already documents",
        )
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

    fn sample_edges() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }

    #[test]
    fn create_then_read_and_write_as_dogstore() {
        let dir = fresh_temp_dir("production_basic").unwrap();
        let path = dir.join("ages.mmap");
        let mut store = ProductionStore::create(sample(), sample_edges(), &path).unwrap();

        assert_eq!(DogStore::get(&store, Uuid::from_u128(1)).unwrap().age, 3);
        DogStore::update_age(&mut store, Uuid::from_u128(1), 42).unwrap();
        assert_eq!(DogStore::get(&store, Uuid::from_u128(1)).unwrap().age, 42);
        assert!(DogStore::scan_ages(&store).contains(&42));

        assert_eq!(
            DogStore::update_age(&mut store, Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );
        assert_eq!(
            store.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            store.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_then_reopen_sees_the_written_ages() {
        let dir = fresh_temp_dir("production_roundtrip").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut store = ProductionStore::create(sample(), sample_edges(), &path).unwrap();
            DogStore::update_age(&mut store, Uuid::from_u128(1), 77).unwrap();
            store.flush().unwrap();
        }

        let reopened = ProductionStore::open(sample(), sample_edges(), &path).unwrap();
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(1)).unwrap().age,
            77
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_then_read_and_write_as_concurrentstore() {
        let store = ProductionStore::new(sample(), sample_edges());
        assert_eq!(
            ConcurrentStore::get(&store, Uuid::from_u128(1))
                .unwrap()
                .unwrap()
                .breed,
            "labrador"
        );
        ConcurrentStore::update_age(&store, Uuid::from_u128(1), 42).unwrap();
        assert_eq!(
            ConcurrentStore::get(&store, Uuid::from_u128(1))
                .unwrap()
                .unwrap()
                .age,
            42
        );
        assert!(matches!(
            ConcurrentStore::update_age(&store, Uuid::from_u128(99), 1),
            Err(ConcurrencyError::Store(StoreError::NotFound(_)))
        ));
    }

    /// The flagship correctness property for this type's `ConcurrentStore`
    /// side — same bar every Tier 1/Tier 2 concurrency variant is held to.
    #[test]
    fn concurrent_stress_matches_sequential_replay() {
        run_concurrency_stress_test::<ProductionStore>();
    }

    #[test]
    fn from_vec_dog_record_builds_a_usable_store() {
        let store = ProductionStore::from(sample());
        assert_eq!(DogStore::get(&store, Uuid::from_u128(1)).unwrap().age, 3);
        assert!(store.neighbors(Uuid::from_u128(1)).is_empty());
    }

    #[test]
    fn from_records_and_edges_builds_a_usable_store() {
        let store = ProductionStore::from((sample(), sample_edges()));
        assert_eq!(
            store.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
    }
}
