//! Tier 1, variant 1: write-ahead log, fsync on every write.
//!
//! Durability model: `update_age` appends a [`WalEntry`] to the WAL file
//! and calls `File::sync_all` (fsync) *before* mutating in-memory state
//! and returning `Ok`. If the process dies at any point after that
//! `sync_all` returns, the entry is guaranteed to be on physical disk —
//! this is the strongest per-write durability guarantee any variant in
//! this crate offers, and (per `RESULTS.md`'s durability section) the
//! most expensive one.
//!
//! `checkpoint` collapses current state into a fresh base snapshot and
//! truncates the WAL — see `src/durability/mod.rs`'s module docs for why
//! that's the right definition of "checkpoint" for a variant with no
//! separate snapshot cadence of its own.

use super::{append_wal_entry, read_wal_entries, CanonicalCachedState, DurabilityError};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// WAL-fsync-backed durable store. See module docs for the durability
/// model.
pub struct WalFsyncStore {
    state: CanonicalCachedState,
    base_path: PathBuf,
    wal_path: PathBuf,
    wal_file: File,
    next_seq: u64,
}

impl WalFsyncStore {
    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join("base.bin"), dir.join("wal.log"))
    }

    /// Build fresh state from `records`/`edges` and start a new, empty WAL
    /// at `dir` (any existing files there are overwritten) — the
    /// "first-ever start" case, before any `update_age` call.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `dir` can't be created or the
    /// WAL file can't be opened for writing.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        dir: &Path,
    ) -> Result<Self, DurabilityError> {
        std::fs::create_dir_all(dir)?;
        let (base_path, wal_path) = Self::paths(dir);
        let wal_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&wal_path)?;
        Ok(Self {
            state: CanonicalCachedState::new(records, edges),
            base_path,
            wal_path,
            wal_file,
            next_seq: 0,
        })
    }

    /// Reconstruct state: start from the base snapshot at `dir` if one
    /// exists (from a prior `checkpoint`), else from `records`/`edges`
    /// fresh (the base dataset, supplied externally — see module docs);
    /// then replay every WAL entry at `dir` in order. This is the
    /// "load/replay/startup" path this variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// base snapshot or WAL file exist but can't be read/deserialized.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        dir: &Path,
    ) -> Result<Self, DurabilityError> {
        std::fs::create_dir_all(dir)?;
        let (base_path, wal_path) = Self::paths(dir);

        let mut state = if base_path.exists() {
            CanonicalCachedState::read_from(&base_path)?
        } else {
            CanonicalCachedState::new(records, edges)
        };

        let mut next_seq = 0u64;
        for entry in read_wal_entries(&wal_path)? {
            state.update_age(entry.id, entry.age)?;
            next_seq = entry.seq + 1;
        }

        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)?;

        Ok(Self {
            state,
            base_path,
            wal_path,
            wal_file,
            next_seq,
        })
    }

    /// Write the current state as a fresh base snapshot, then truncate the
    /// WAL — after this call, replaying an empty WAL against the new base
    /// snapshot reconstructs exactly the current state. Bounds
    /// [`Self::open`]'s replay cost, which would otherwise grow with every
    /// `update_age` ever called.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the snapshot or the truncated
    /// WAL can't be written.
    pub fn checkpoint(&mut self) -> Result<(), DurabilityError> {
        self.state.write_to(&self.base_path)?;
        self.wal_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.wal_path)?;
        Ok(())
    }
}

impl DogStore for WalFsyncStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.state.get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.state.scan_ages()
    }

    /// Write-ahead, fsync'd: the entry is durable on disk *before* this
    /// call mutates in-memory state or returns `Ok`.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let entry = super::WalEntry {
            seq: self.next_seq,
            id,
            age,
        };
        append_wal_entry(&mut self.wal_file, &entry)?;
        self.wal_file.sync_all().map_err(DurabilityError::from)?;
        self.next_seq += 1;
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
        let dir = crate::bench_support::fresh_temp_dir("wal_fsync_basic").unwrap();
        let mut store = WalFsyncStore::create(sample_records(), sample_edges(), &dir).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        assert_eq!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The highest-priority correctness property for every WAL variant:
    /// write N ops, reconstruct purely from the log, and the result must
    /// match a store built directly from the same op sequence — exactly.
    #[test]
    fn reconstructing_from_wal_matches_expected_state() {
        let dir = crate::bench_support::fresh_temp_dir("wal_fsync_reconstruct").unwrap();
        {
            let mut store = WalFsyncStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 10).unwrap();
            store.update_age(Uuid::from_u128(2), 20).unwrap();
            store.update_age(Uuid::from_u128(1), 11).unwrap();
            store.update_age(Uuid::from_u128(3), 30).unwrap();
            // Store dropped here — simulates the process exiting after
            // these four writes, with no explicit checkpoint.
        }

        let reopened = WalFsyncStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 11);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 20);
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 30);

        let mut expected = CanonicalCachedState::new(sample_records(), sample_edges());
        expected.update_age(Uuid::from_u128(1), 10).unwrap();
        expected.update_age(Uuid::from_u128(2), 20).unwrap();
        expected.update_age(Uuid::from_u128(1), 11).unwrap();
        expected.update_age(Uuid::from_u128(3), 30).unwrap();
        let mut expected_ages = expected.scan_ages();
        let mut actual_ages = reopened.scan_ages();
        expected_ages.sort_unstable();
        actual_ages.sort_unstable();
        assert_eq!(actual_ages, expected_ages);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_then_reopen_matches_pre_checkpoint_state() {
        let dir = crate::bench_support::fresh_temp_dir("wal_fsync_checkpoint").unwrap();
        {
            let mut store = WalFsyncStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 15).unwrap();
            store.checkpoint().unwrap();
            store.update_age(Uuid::from_u128(2), 25).unwrap();
        }

        let reopened = WalFsyncStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 15);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 25);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_breed_and_neighbors_survive_reopen() {
        let dir = crate::bench_support::fresh_temp_dir("wal_fsync_indexes").unwrap();
        {
            let _store = WalFsyncStore::create(sample_records(), sample_edges(), &dir).unwrap();
        }
        let reopened = WalFsyncStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(
            reopened.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            reopened.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
