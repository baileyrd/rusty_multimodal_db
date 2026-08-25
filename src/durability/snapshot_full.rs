//! Tier 1, variant 4: snapshot, save-as-is.
//!
//! Durability model: identical write-path cost to variant 3
//! ([`super::SnapshotRebuildStore`]) — `update_age` only mutates
//! in-memory state, zero disk I/O per write. The difference is entirely
//! in what [`Self::checkpoint`] persists: the *whole*
//! [`super::CanonicalCachedState`] — canonical map, breed index, age
//! cache, position index, and adjacency index — serialized directly, with
//! no reconstruction step. [`Self::open`] deserializes it back exactly as
//! it was, with no rebuild.
//!
//! # The tradeoff this variant accepts (and variant 3 doesn't)
//!
//! Persisting the derived indexes as-is means a bug that corrupts one of
//! them in memory (e.g. a future change that updates `age_cache` but
//! forgets to update `position_index` to match) gets faithfully written
//! to disk and faithfully read back — this variant has no way to notice
//! or self-correct that, because it never re-derives anything from the
//! canonical source. Variant 3's rebuild-on-load *would* self-correct
//! such a bug, since every index it uses is freshly rebuilt from
//! `records`/`edges` on every `open`, not carried over from a possibly-
//! stale prior in-memory state. This is a real, structural cost of
//! "save-as-is," not a hypothetical — worth stating plainly rather than
//! only citing this variant's speed advantage. Not covered by a
//! corruption-injection test here (per the task that motivated this
//! module: noting the tradeoff is the deliverable, not building a fault
//! injector for a bug class this crate doesn't currently have).

use super::{CanonicalCachedState, DurabilityError};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Save-as-is-snapshot durable store. See module docs for the durability
/// model and its corruption-propagation tradeoff vs. variant 3.
pub struct SnapshotFullStore {
    state: CanonicalCachedState,
    path: PathBuf,
}

impl SnapshotFullStore {
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
            state: CanonicalCachedState::new(records, edges),
            path: path.to_path_buf(),
        })
    }

    /// Deserialize the full state at `path` directly — no rebuild. The
    /// "load/replay/startup" path this variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if
    /// `path` doesn't exist or can't be deserialized.
    pub fn open(path: &Path) -> Result<Self, DurabilityError> {
        let state = CanonicalCachedState::read_from(path)?;
        Ok(Self {
            state,
            path: path.to_path_buf(),
        })
    }

    /// Serialize the whole current state to `path` directly, replacing
    /// whatever was there.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// state can't be serialized or written.
    pub fn checkpoint(&mut self) -> Result<(), DurabilityError> {
        self.state.write_to(&self.path)
    }
}

impl DogStore for SnapshotFullStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.state.get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.state.scan_ages()
    }

    /// No disk I/O — see module docs. Everything since the last
    /// `checkpoint` is lost if the process dies before the next one runs,
    /// same data-loss window as variant 3.
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
        let dir = crate::bench_support::fresh_temp_dir("snapshot_full_basic").unwrap();
        let path = dir.join("snapshot.bin");
        let mut store = SnapshotFullStore::create(sample_records(), sample_edges(), &path).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip correctness: save then load must produce a store
    /// identical to one freshly constructed from the same data.
    #[test]
    fn checkpoint_then_open_matches_a_fresh_store_built_from_the_same_data() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_full_roundtrip").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotFullStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.update_age(Uuid::from_u128(1), 88).unwrap();
            store.update_age(Uuid::from_u128(3), 15).unwrap();
            store.checkpoint().unwrap();
        }

        let loaded = SnapshotFullStore::open(&path).unwrap();
        assert_eq!(loaded.get(Uuid::from_u128(1)).unwrap().age, 88);
        assert_eq!(loaded.get(Uuid::from_u128(3)).unwrap().age, 15);
        assert_eq!(loaded.get(Uuid::from_u128(2)).unwrap().age, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The indexes themselves round-trip correctly too (this variant's
    /// whole premise: they're persisted as-is, so they'd better survive
    /// the trip unchanged).
    #[test]
    fn indexes_round_trip_correctly() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_full_indexes").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotFullStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.checkpoint().unwrap();
        }

        let loaded = SnapshotFullStore::open(&path).unwrap();
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

    #[test]
    fn writes_after_last_checkpoint_are_not_recovered() {
        let dir = crate::bench_support::fresh_temp_dir("snapshot_full_loss_window").unwrap();
        let path = dir.join("snapshot.bin");

        {
            let mut store =
                SnapshotFullStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.checkpoint().unwrap();
            store.update_age(Uuid::from_u128(1), 99).unwrap();
        }

        let loaded = SnapshotFullStore::open(&path).unwrap();
        assert_eq!(
            loaded.get(Uuid::from_u128(1)).unwrap().age,
            3,
            "age 99 was written after the last checkpoint and should not have survived reopen"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
