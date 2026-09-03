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
//! named temp-file backing (via [`crate::test_support::fresh_temp_dir`],
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
//!
//! # File portability: two files, one path (STORAGE-014)
//!
//! `MmapAgeStore`'s file persists only `age` — the one field anything in
//! this crate ever mutates — so on its own it can't reconstruct a store:
//! [`ProductionStore::open`] needs the caller to supply the full record
//! set again. Since `MMAP-AGE-STORE-IDENTITY-FIX` that has been this
//! project's own standing "not portable the way SQLite's/DuckDB's file
//! is" gap. `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md`
//! (ADR-0016, Accepted) closes it *additively*: alongside the unchanged
//! ages file at `path`, `create` also writes a **companion record blob**
//! at `<path>.records` (`crate::durability::record_blob`) holding the
//! immutable half of the state — every `id`/`breed` and every edge —
//! and a new constructor, [`ProductionStore::open_portable`], reopens
//! from `path` alone by reading that blob first. Copy the two files
//! together and the store travels. Nothing about `MmapAgeStore`'s
//! format, or about `create`/`open`'s signatures, changed.
//!
//! The blob is write-once at `create`; the only other time it's written
//! is when [`ProductionStore::open`] is handed a record set whose
//! immutable content (`id`s, `breed`s, edges) differs from what the blob
//! holds — the same "the caller's dataset is the truth" reconciliation
//! `MmapAgeStore::open` already performs for the ages file. That check
//! is a content fingerprint in the blob's header against one computed
//! from the caller's records: no serialization and a 20-byte read on
//! every `open` (blob version 2; version 1 compared full serialized
//! images, measured at +27% on `open` at 1M records). A missing,
//! unreadable, or version-1 blob simply counts as stale and is
//! (re)written, which also upgrades a pre-`STORAGE-014` directory
//! holding only an ages file on its first `open`. Only `open_portable`
//! ever *requires* the blob — and a missing or corrupt one there is
//! [`DurabilityError::RecordBlobUnreadable`], a variant distinct from
//! the ages file's own `InvalidMagic`/`SchemaVersionMismatch`, never a
//! panic.

use crate::concurrency::{ConcurrencyError, ConcurrentStore};
use crate::durability::record_blob::{companion_path, RecordBlob};
use crate::durability::{DurabilityError, MmapAgeStore};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use crate::test_support::fresh_temp_dir;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use uuid::Uuid;

/// This crate's recommended production type: `CanonicalCachedStore`'s
/// architecture, durable via mmap, safe for concurrent access via one
/// global `RwLock`. See module docs for the composition and why it isn't
/// three literally nested types.
///
/// # Examples
///
/// ```
/// use rusty_multimodal_db::{DogRecord, DogStore, ProductionStore};
/// use uuid::Uuid;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = std::env::temp_dir().join(format!("production_store_doctest_{}", std::process::id()));
/// std::fs::create_dir_all(&dir)?;
/// let path = dir.join("dogs.mmap");
///
/// let rex = Uuid::from_u128(1);
/// let records = vec![DogRecord::new(rex, "Corgi", 3)];
///
/// // No littermate edges needed for this example — see `ProductionStore::create`
/// // for what the second argument is for.
/// let mut store = ProductionStore::create(records, Vec::new(), &path)?;
/// assert_eq!(store.get(rex).unwrap().age, 3);
///
/// store.update_age(rex, 4)?;
/// assert_eq!(store.get(rex).unwrap().age, 4);
///
/// # std::fs::remove_dir_all(&dir).ok();
/// # Ok(())
/// # }
/// ```
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
    /// `records`' starting ages, wrapped in a `RwLock` — and, new as of
    /// `STORAGE-014`, the companion record blob at `<path>.records` that
    /// lets [`Self::open_portable`] reopen this store from `path` alone.
    /// See module docs ("File portability").
    ///
    /// The blob is encoded before `records`/`edges` move into
    /// `MmapAgeStore` (no clone of the record set) and written after the
    /// ages file exists, so a failure partway leaves at most an ages file
    /// without its companion — exactly the state a later [`Self::open`]
    /// already heals.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] under the same conditions
    /// [`MmapAgeStore::create`] does, or if the companion blob can't be
    /// written; [`DurabilityError::Serde`] if the record set can't be
    /// serialized (not reachable for any `DogRecord` this crate builds).
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let blob = RecordBlob { records, edges };
        let encoded = blob.encode()?;
        let inner = MmapAgeStore::create(blob.records, blob.edges, path)?;
        encoded.write(&companion_path(path))?;
        Ok(Self {
            inner: RwLock::new(inner),
        })
    }

    /// Reopen an existing mmap-backed file at `path` (matching
    /// [`MmapAgeStore::open`]'s semantics exactly): `records`/`edges`
    /// rebuild the in-memory indexes, the file on disk remains the source
    /// of truth for every age.
    ///
    /// As of `STORAGE-014` this also keeps the companion record blob at
    /// `<path>.records` in step with the caller's record set: the blob's
    /// header fingerprint is compared with the fingerprint of `records`/
    /// `edges` (their `id`s, `breed`s, and edges — not their ages, which
    /// the ages file owns); if the blob is missing, unreadable, from an
    /// older blob version, or fingerprints differently, it's (re)written
    /// after the ages file opens. A directory holding only an ages file
    /// written before this companion existed is therefore upgraded on its
    /// first `open` — no migration step — and so is a version-1 blob
    /// written before the header carried a fingerprint. The steady-state
    /// check (same dataset every `open`) is one hash pass over the
    /// in-memory records plus a 20-byte read; the record set is only
    /// serialized when a rewrite is actually needed. The rewrite runs
    /// *after* `MmapAgeStore::open` succeeds, so an ages-file error never
    /// clobbers a valid blob belonging to a different dataset.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] under the same conditions
    /// [`MmapAgeStore::open`] does, or if a stale/missing companion blob
    /// can't be rewritten; [`DurabilityError::Serde`] if a stale blob's
    /// replacement can't be serialized (not reachable for any `DogRecord`
    /// this crate builds).
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let blob = RecordBlob { records, edges };
        let companion = companion_path(path);
        let replacement = if blob.is_current_at(&companion) {
            None
        } else {
            Some(blob.encode()?)
        };
        let inner = MmapAgeStore::open(blob.records, blob.edges, path)?;
        if let Some(encoded) = replacement {
            encoded.write(&companion)?;
        }
        Ok(Self {
            inner: RwLock::new(inner),
        })
    }

    /// Reopen from `path` alone — no `records`/`edges` needed
    /// (`STORAGE-014-FR-002`): reads the companion record blob at
    /// `<path>.records` for the immutable half of the state (`id`/`breed`/
    /// edges), then opens the ages file at `path` exactly as
    /// [`Self::open`] would with that record set. Copy both files to a
    /// fresh location and this reopens them there; the result is
    /// indistinguishable from the store `create` produced, including every
    /// `breed`, every `same_breed`/`neighbors` answer, and every age the
    /// ages file holds.
    ///
    /// The blob is known current here (it *is* the record set), so no
    /// comparison or rewrite happens — this is the one constructor that
    /// never writes the companion.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::RecordBlobUnreadable`] if the companion
    /// blob is missing (e.g. only the `.mmap` file was copied), isn't a
    /// record blob, was written by an incompatible build, or doesn't
    /// decode — never a panic, and never the ages file's own
    /// `InvalidMagic`/`SchemaVersionMismatch` (`STORAGE-014-FR-005`).
    /// Otherwise, the same errors [`MmapAgeStore::open`] returns.
    pub fn open_portable(path: &Path) -> Result<Self, DurabilityError> {
        let blob = RecordBlob::read(&companion_path(path))?;
        Ok(Self {
            inner: RwLock::new(MmapAgeStore::open(blob.records, blob.edges, path)?),
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

/// A store that can hold one exclusive-access critical section spanning
/// multiple logical operations — the real mechanism behind the server
/// layer's `Request::Transaction` atomicity guarantee
/// (`docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013,
/// `TXN-FR-002`/`TXN-FR-006`). Implemented by [`ProductionStore`] only —
/// not every [`ConcurrentStore`] implementor: the other
/// `src/concurrency/**` variants are benchmarked historical alternatives,
/// never wrapped by `server::dog::DogConnectionStore` in practice (every
/// real call site uses `ProductionStore`), so extending this capability
/// to them would be speculative generality with no real caller.
pub trait TransactionalStore {
    /// The type [`TransactionalStore::with_exclusive`]'s closure gets
    /// exclusive access to.
    type Exclusive: DogStore;

    /// Runs `f` with exclusive write access held for `f`'s entire
    /// duration — the same internal lock every other `&self`/
    /// [`ConcurrentStore`] method on this store already acquires and
    /// releases per call, exposed here as one continuous critical section
    /// instead of many short ones.
    fn with_exclusive<R>(&self, f: impl FnOnce(&mut Self::Exclusive) -> R) -> R;
}

impl TransactionalStore for ProductionStore {
    type Exclusive = MmapAgeStore;

    /// # Panics
    ///
    /// Panics if the lock is poisoned — see `LOCK_POISONED`.
    fn with_exclusive<R>(&self, f: impl FnOnce(&mut MmapAgeStore) -> R) -> R {
        let mut guard = self.inner.write().expect(LOCK_POISONED);
        f(&mut guard)
    }
}

/// `server::dog::DogConnectionStore::scan_all`'s own primitive
/// (`SQL-FR-005`, ADR-0034,
/// `docs/design/SERVER-SQL-SELECT-DESIGN.md`) — a small, separate trait
/// in this same shape as [`TransactionalStore`] above, for the identical
/// reason: a server-facing-only capability, not part of the `DogStore`
/// trait the four `research`-gated backends also implement, and (like
/// `TransactionalStore`) never needed by them since none is ever wrapped
/// by a `ConnectionStore` adapter.
pub trait AllIds {
    /// Every id this store currently holds, unspecified order.
    fn all_ids(&self) -> Vec<Uuid>;
}

impl AllIds for ProductionStore {
    fn all_ids(&self) -> Vec<Uuid> {
        self.inner.read().expect(LOCK_POISONED).ids()
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

    /// STORAGE-014 acceptance: `create` → `open_portable` round trip with
    /// no records/edges supplied, identical on every `DogStore` method
    /// including `breed` (the field the ages file never held).
    #[test]
    fn open_portable_reconstructs_the_full_store_from_the_path_alone() {
        let dir = fresh_temp_dir("production_portable").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut store = ProductionStore::create(sample(), sample_edges(), &path).unwrap();
            DogStore::update_age(&mut store, Uuid::from_u128(1), 77).unwrap();
            store.flush().unwrap();
        }
        assert!(companion_path(&path).exists(), "create must write the blob");

        let reopened = ProductionStore::open_portable(&path).unwrap();
        let rex = DogStore::get(&reopened, Uuid::from_u128(1)).unwrap();
        assert_eq!(rex.breed, "labrador");
        assert_eq!(rex.age, 77, "the ages file, not the blob's seed, wins");
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(3)).unwrap().breed,
            "poodle"
        );
        assert_eq!(
            reopened.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            reopened.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(DogStore::scan_ages(&reopened), vec![77, 5, 2]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// STORAGE-014 acceptance: the two files are portable as a unit —
    /// copy both to a fresh directory, `open_portable` there.
    #[test]
    fn open_portable_works_on_both_files_copied_to_a_fresh_directory() {
        let source = fresh_temp_dir("production_portable_src").unwrap();
        let path = source.join("ages.mmap");
        {
            let mut store = ProductionStore::create(sample(), sample_edges(), &path).unwrap();
            DogStore::update_age(&mut store, Uuid::from_u128(2), 9).unwrap();
            store.flush().unwrap();
        }

        let destination = fresh_temp_dir("production_portable_dst").unwrap();
        // A different file name at the destination: the companion is
        // derived from whatever path the caller opens, not remembered.
        let moved = destination.join("dogs.db");
        std::fs::copy(&path, &moved).unwrap();
        std::fs::copy(companion_path(&path), companion_path(&moved)).unwrap();

        let reopened = ProductionStore::open_portable(&moved).unwrap();
        assert_eq!(DogStore::get(&reopened, Uuid::from_u128(2)).unwrap().age, 9);
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(2)).unwrap().breed,
            "labrador"
        );
        assert_eq!(
            reopened.neighbors(Uuid::from_u128(2)),
            vec![Uuid::from_u128(1)]
        );

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&destination);
    }

    /// STORAGE-014 acceptance: only the `.mmap` file copied (a
    /// pre-portability backup) → the distinct typed error, not a panic
    /// and not the ages file's own `InvalidMagic`.
    #[test]
    fn open_portable_without_the_blob_is_a_typed_error_not_a_panic() {
        let dir = fresh_temp_dir("production_portable_missing").unwrap();
        let path = dir.join("ages.mmap");
        ProductionStore::create(sample(), sample_edges(), &path).unwrap();
        std::fs::remove_file(companion_path(&path)).unwrap();

        let result = ProductionStore::open_portable(&path);
        match result {
            Err(DurabilityError::RecordBlobUnreadable { path: reported, .. }) => {
                assert_eq!(reported, companion_path(&path));
            }
            other => panic!("expected RecordBlobUnreadable, got {:?}", other.err()),
        }
        // The ages file itself is untouched and still fine.
        assert!(ProductionStore::open(sample(), sample_edges(), &path).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An ages file from before STORAGE-014 (no companion at all) is
    /// upgraded by the first `open` — after which `open_portable` works.
    #[test]
    fn open_heals_a_legacy_directory_holding_only_the_ages_file() {
        let dir = fresh_temp_dir("production_portable_legacy").unwrap();
        let path = dir.join("ages.mmap");
        {
            // Bypass `ProductionStore::create` — build the legacy layout
            // exactly as the pre-STORAGE-014 code did.
            let mut legacy = MmapAgeStore::create(sample(), sample_edges(), &path).unwrap();
            legacy.update_age(Uuid::from_u128(3), 11).unwrap();
            legacy.flush().unwrap();
        }
        assert!(!companion_path(&path).exists());
        assert!(matches!(
            ProductionStore::open_portable(&path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));

        drop(ProductionStore::open(sample(), sample_edges(), &path).unwrap());
        assert!(companion_path(&path).exists(), "open must write the blob");

        let reopened = ProductionStore::open_portable(&path).unwrap();
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(3)).unwrap().age,
            11
        );
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(3)).unwrap().breed,
            "poodle"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A blob written before its header carried a fingerprint (blob
    /// version 1) is upgraded in place by the first `open` — after which
    /// `open_portable` works. Same shape as the ages-only legacy case.
    #[test]
    fn open_upgrades_a_version_1_blob_in_place() {
        let dir = fresh_temp_dir("production_portable_v1").unwrap();
        let path = dir.join("ages.mmap");
        ProductionStore::create(sample(), sample_edges(), &path).unwrap();
        let companion = companion_path(&path);
        let blob = RecordBlob {
            records: sample(),
            edges: sample_edges(),
        };
        blob.write_legacy_v1(&companion).unwrap();
        assert!(matches!(
            ProductionStore::open_portable(&path),
            Err(DurabilityError::RecordBlobUnreadable { .. })
        ));

        drop(ProductionStore::open(sample(), sample_edges(), &path).unwrap());
        assert!(
            blob.is_current_at(&companion),
            "open must rewrite a v1 blob"
        );
        let reopened = ProductionStore::open_portable(&path).unwrap();
        assert_eq!(
            DogStore::get(&reopened, Uuid::from_u128(3)).unwrap().breed,
            "poodle"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `open` with a *changed* record set (the identity-keyed
    /// reconciliation `MMAP-AGE-STORE-IDENTITY-FIX` provides) refreshes the
    /// blob, so a later `open_portable` sees the new set — and an `open`
    /// with the *same* set (even with different seed ages, which the ages
    /// file owns) leaves the blob's bytes alone.
    #[test]
    fn open_refreshes_the_blob_only_when_the_record_set_changed() {
        let dir = fresh_temp_dir("production_portable_refresh").unwrap();
        let path = dir.join("ages.mmap");
        ProductionStore::create(sample(), sample_edges(), &path).unwrap();
        let companion = companion_path(&path);
        let original = std::fs::read(&companion).unwrap();
        let original_modified = std::fs::metadata(&companion).unwrap().modified().unwrap();

        drop(ProductionStore::open(sample(), sample_edges(), &path).unwrap());
        let mut reaged = sample();
        reaged[0].age = 99;
        drop(ProductionStore::open(reaged, sample_edges(), &path).unwrap());
        assert_eq!(std::fs::read(&companion).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&companion).unwrap().modified().unwrap(),
            original_modified,
            "an unchanged record set must not rewrite the blob"
        );

        let mut grown = sample();
        grown.push(DogRecord::new(Uuid::from_u128(4), "beagle", 1));
        let mut grown_edges = sample_edges();
        grown_edges.push((Uuid::from_u128(3), Uuid::from_u128(4)));
        {
            let mut store = ProductionStore::open(grown.clone(), grown_edges, &path).unwrap();
            DogStore::update_age(&mut store, Uuid::from_u128(4), 8).unwrap();
            store.flush().unwrap();
        }
        assert_ne!(std::fs::read(&companion).unwrap(), original);

        let reopened = ProductionStore::open_portable(&path).unwrap();
        let newcomer = DogStore::get(&reopened, Uuid::from_u128(4)).unwrap();
        assert_eq!(newcomer.breed, "beagle");
        assert_eq!(newcomer.age, 8);
        assert_eq!(
            reopened.neighbors(Uuid::from_u128(3)),
            vec![Uuid::from_u128(4)]
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
