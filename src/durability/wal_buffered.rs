//! Tier 1, variant 2: write-ahead log, buffered/no-fsync.
//!
//! Durability model: identical structure to [`super::WalFsyncStore`]
//! (variant 1) — same WAL entry format, same append-then-mutate ordering,
//! same checkpoint/truncate semantics — with exactly one difference:
//! `update_age` never calls `File::sync_all`. The `write_all` call still
//! reaches the OS's own page cache immediately (this is a plain `File`,
//! not a userspace-buffered `BufWriter` — "buffered" here means "relying
//! on the OS's write buffering," not adding a second buffering layer on
//! top), so this variant *is* durable against this process crashing: a
//! reopen after a crash (but not a machine-level crash) will see every
//! write that returned `Ok`. What it does **not** protect against is the
//! OS or the machine itself going down before the kernel flushes those
//! dirty pages to physical disk on its own schedule — that's the entire
//! difference from variant 1, and the entire reason this variant is
//! cheaper per write (see `RESULTS.md`'s durability section for the
//! measured gap).
//!
//! Deliberately near-identical code to `wal_fsync.rs` rather than sharing
//! an abstraction over "with or without fsync" — this repo's established
//! style accepts small explicit duplication between structurally similar
//! backends (e.g. `CanonicalStore`/`CanonicalCachedStore` both duplicate
//! index-building logic) over a generic parameter whose only job is to
//! toggle one call.

use super::{append_wal_entry, read_wal_entries, CanonicalCachedState, DurabilityError};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// WAL-buffered (no fsync) durable store. See module docs for the
/// durability model and how it differs from [`super::WalFsyncStore`].
pub struct WalBufferedStore {
    state: CanonicalCachedState,
    base_path: PathBuf,
    wal_path: PathBuf,
    wal_file: File,
    next_seq: u64,
}

impl WalBufferedStore {
    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join("base.bin"), dir.join("wal.log"))
    }

    /// See [`super::WalFsyncStore::create`] — identical semantics.
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

    /// See [`super::WalFsyncStore::open`] — identical semantics.
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

    /// See [`super::WalFsyncStore::checkpoint`] — identical semantics.
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

impl DogStore for WalBufferedStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.state.get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.state.scan_ages()
    }

    /// Write-ahead, **not** fsync'd: the entry reaches the OS's page
    /// cache before this call mutates in-memory state or returns `Ok`,
    /// but isn't forced to physical disk. See module docs for exactly
    /// what that does and doesn't protect against.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let entry = super::WalEntry {
            seq: self.next_seq,
            id,
            age,
        };
        append_wal_entry(&mut self.wal_file, &entry)?;
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
        let dir = crate::bench_support::fresh_temp_dir("wal_buffered_basic").unwrap();
        let mut store = WalBufferedStore::create(sample_records(), sample_edges(), &dir).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        assert_eq!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same highest-priority property as variant 1: write N ops,
    /// reconstruct purely from the log, exact match expected. The absence
    /// of `sync_all` doesn't change *within-process* recovery — the OS
    /// page cache still has everything a same-machine reopen can see;
    /// only a true power-loss scenario (not testable here) would show the
    /// gap.
    #[test]
    fn reconstructing_from_wal_matches_expected_state() {
        let dir = crate::bench_support::fresh_temp_dir("wal_buffered_reconstruct").unwrap();
        {
            let mut store =
                WalBufferedStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 10).unwrap();
            store.update_age(Uuid::from_u128(2), 20).unwrap();
            store.update_age(Uuid::from_u128(1), 11).unwrap();
            store.update_age(Uuid::from_u128(3), 30).unwrap();
        }

        let reopened = WalBufferedStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 11);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 20);
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 30);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_then_reopen_matches_pre_checkpoint_state() {
        let dir = crate::bench_support::fresh_temp_dir("wal_buffered_checkpoint").unwrap();
        {
            let mut store =
                WalBufferedStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 15).unwrap();
            store.checkpoint().unwrap();
            store.update_age(Uuid::from_u128(2), 25).unwrap();
        }

        let reopened = WalBufferedStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 15);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 25);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
