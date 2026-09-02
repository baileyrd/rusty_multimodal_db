//! Tier 1, variant 3: snapshot, canonical-only, rebuild all indexes on
//! load.
//!
//! Durability model: `update_age` only ever mutates in-memory state — no
//! disk I/O per write at all, so its cost should be indistinguishable
//! from the non-durable `CanonicalCachedStore` baseline (see
//! `RESULTS.md`'s durability section for the measured number). Durability
//! comes entirely from an explicit [`Self::checkpoint`] call, which
//! persists **only the canonical source data** — `records` and `edges`,
//! the same two inputs `CanonicalCachedState::new` is built from — not
//! the derived breed index, age cache, position index, or adjacency
//! index. [`Self::open`] rebuilds all four of those on load, via the
//! exact same construction path `create` uses. This mirrors
//! `CanonicalStore`/`CanonicalCachedStore`'s own "views, not copies"
//! philosophy (ADR-0001) at the persistence layer: the *persisted*
//! artifact is canonical-only too.
//!
//! Unlike the WAL variants, this format is self-sufficient: [`Self::open`]
//! needs only a path, not an externally-supplied base dataset, since
//! `records`/`edges` — the actual source of truth — are exactly what's on
//! disk.

use super::{CanonicalCachedState, DurabilityError};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The entire on-disk format for this variant: the canonical source data,
/// nothing derived.
#[derive(Debug, Serialize, Deserialize)]
struct CanonicalOnlySnapshot {
    records: Vec<DogRecord>,
    edges: Vec<(Uuid, Uuid)>,
}

/// Canonical-only-snapshot durable store. See module docs for the
/// durability model.
pub struct SnapshotRebuildStore {
    state: CanonicalCachedState,
    /// Kept alongside `state` (which only retains the *derived* adjacency
    /// index, not the original edge list) so `checkpoint` can re-persist
    /// the same canonical-only shape it was built from.
    edges: Vec<(Uuid, Uuid)>,
    path: PathBuf,
}

impl SnapshotRebuildStore {
    /// Build fresh state from `records`/`edges` — nothing is written to
    /// `path` until [`Self::checkpoint`] is called.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent directory can't
    /// be created.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            state: CanonicalCachedState::new(records, edges.clone()),
            edges,
            path: path.to_path_buf(),
        })
    }

    /// Read the canonical-only snapshot at `path` and rebuild every
    /// derived index from it — the "load/replay/startup" path this
    /// variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if
    /// `path` doesn't exist or can't be deserialized.
    pub fn open(path: &Path) -> Result<Self, DurabilityError> {
        let bytes = std::fs::read(path)?;
        let snapshot: CanonicalOnlySnapshot = crate::codec::decode(&bytes)?;
        let edges = snapshot.edges.clone();
        let state = CanonicalCachedState::new(snapshot.records, snapshot.edges);
        Ok(Self {
            state,
            edges,
            path: path.to_path_buf(),
        })
    }

    /// Persist current `records`/`edges` (not the derived indexes) to
    /// `path`, replacing whatever was there.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// snapshot can't be serialized or written.
    pub fn checkpoint(&mut self) -> Result<(), DurabilityError> {
        let snapshot = CanonicalOnlySnapshot {
            records: self.state.records_snapshot(),
            edges: self.edges.clone(),
        };
        let bytes = crate::codec::encode(&snapshot)?;
        std::fs::write(&self.path, bytes)?;
        Ok(())
    }
}

impl DogStore for SnapshotRebuildStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.state.get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.state.scan_ages()
    }

    /// No disk I/O — see module docs. Everything since the last
    /// `checkpoint` (or since `create`, if `checkpoint` was never called)
    /// is lost if the process dies before the next checkpoint runs.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        self.state.update_age(id, age)
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        self.state.same_breed(id)
    }

    fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.state.neighbors(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durability::test_support::*;

    #[test]
    fn create_then_read_and_write() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_rebuild_basic").unwrap();
        let path = dir.join("snapshot.bin");
        let mut store =
            SnapshotRebuildStore::create(sample_records(), sample_edges(), &path).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip correctness: save then load must produce a store
    /// identical to one freshly constructed from the same data — the
    /// same standard `STORAGE-008`/`STORAGE-009` hold every snapshot
    /// variant to.
    #[test]
    fn checkpoint_then_open_matches_a_fresh_store_built_from_the_same_data() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_rebuild_roundtrip").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotRebuildStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.update_age(Uuid::from_u128(1), 88).unwrap();
            store.update_age(Uuid::from_u128(3), 15).unwrap();
            store.checkpoint().unwrap();
        }

        let loaded = SnapshotRebuildStore::open(&path).unwrap();

        let mut expected = CanonicalCachedState::new(sample_records(), sample_edges());
        expected.update_age(Uuid::from_u128(1), 88).unwrap();
        expected.update_age(Uuid::from_u128(3), 15).unwrap();

        assert_eq!(loaded.get(Uuid::from_u128(1)).unwrap().age, 88);
        assert_eq!(loaded.get(Uuid::from_u128(3)).unwrap().age, 15);
        assert_eq!(loaded.get(Uuid::from_u128(2)).unwrap().age, 5);

        let mut loaded_ages = loaded.scan_ages();
        let mut expected_ages = expected.scan_ages();
        loaded_ages.sort_unstable();
        expected_ages.sort_unstable();
        assert_eq!(loaded_ages, expected_ages);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The defining feature of this variant: derived indexes must come
    /// back correctly even though only `records`/`edges` were persisted.
    #[test]
    fn rebuilt_indexes_are_correct_after_open() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_rebuild_indexes").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotRebuildStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.checkpoint().unwrap();
        }

        let loaded = SnapshotRebuildStore::open(&path).unwrap();
        assert_eq!(
            loaded.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert!(loaded.same_breed(Uuid::from_u128(3)).is_empty());
        assert_eq!(
            loaded.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            loaded.neighbors(Uuid::from_u128(2)),
            vec![Uuid::from_u128(1)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes after the last checkpoint are lost on "restart" (reopening
    /// from the persisted file) — this is the whole tradeoff this variant
    /// exists to demonstrate, not a bug to hide.
    #[test]
    fn writes_after_last_checkpoint_are_not_recovered() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_rebuild_loss_window").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotRebuildStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.checkpoint().unwrap();
            store.update_age(Uuid::from_u128(1), 99).unwrap();
            // No second checkpoint before "crash".
        }

        let loaded = SnapshotRebuildStore::open(&path).unwrap();
        assert_eq!(
            loaded.get(Uuid::from_u128(1)).unwrap().age,
            3,
            "age 99 was written after the last checkpoint and should not have survived reopen"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
