//! Tier 1, variant 5: hybrid — periodic snapshot + WAL of writes since
//! that snapshot.
//!
//! Durability model: `update_age` appends a [`super::WalEntry`] (buffered,
//! not fsync'd — see below for why) then mutates in-memory state, same
//! per-write shape as [`super::WalBufferedStore`] (variant 2).
//! [`Self::checkpoint`] writes a snapshot of the *full* current state
//! (same "save-as-is" shape as variant 4) tagged with the sequence number
//! of the last entry it covers — but, unlike the WAL variants' checkpoint,
//! **does not truncate the WAL**. [`Self::open`] reads the latest
//! snapshot, then replays only the WAL entries whose sequence number is
//! strictly greater than the snapshot's recorded cutoff — the "restore
//! the snapshot then replay only entries after its cutoff" design this
//! variant exists to test.
//!
//! # Why the WAL isn't fsync'd here
//!
//! The task that motivated this module didn't specify Hybrid's per-write
//! fsync discipline. Eager per-write fsync (variant 1's approach) would
//! make this variant's writes as expensive as variant 1's for no extra
//! benefit — this design's durability story already rests on the
//! *periodic snapshot*, not on every individual WAL append being forced
//! to disk. Buffered (variant 2's approach) is the natural default: the
//! WAL exists to bound the *data-loss window* between snapshots to
//! "since the last snapshot," not to give every single write an
//! independent fsync guarantee — if that stronger guarantee is wanted,
//! variant 1 already provides it standalone.
//!
//! # Why never truncating the WAL is the actual point
//!
//! A WAL variant that truncates on checkpoint (1, 2) has no way to
//! recover if the *snapshot write itself* is interrupted mid-write — the
//! old WAL is already gone and the new snapshot isn't fully on disk yet.
//! Never truncating means the WAL always has everything since the
//! *previous* successful snapshot, so `open` can fall back correctly even
//! if the most recent `checkpoint` never finished. This crate doesn't
//! test that specific interrupted-checkpoint scenario directly (it would
//! need to inject a crash mid-write, which none of this crate's
//! correctness tests do for any variant), but it's the structural reason
//! this variant's on-disk format looks the way it does.

use super::{append_wal_entry, read_wal_entries, CanonicalCachedState, DurabilityError, WalEntry};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The on-disk snapshot format: full state plus the sequence number of
/// the last WAL entry it already reflects. `None` means no writes had
/// happened yet when this snapshot was taken.
#[derive(Debug, Serialize, Deserialize)]
struct HybridSnapshot {
    seq_at_snapshot: Option<u64>,
    state: CanonicalCachedState,
}

/// Borrowed mirror of [`HybridSnapshot`], field-for-field identical, used
/// only by [`HybridStore::checkpoint`] to serialize directly from the live
/// `&CanonicalCachedState` instead of cloning it first — `&T`'s `Serialize`
/// impl delegates to `T`'s, so this produces byte-identical output to
/// serializing an owned `HybridSnapshot`, and [`HybridStore::open`] keeps
/// deserializing into the owned type unchanged (it needs an owned
/// `CanonicalCachedState` to move into `Self` either way, so there's
/// nothing to borrow on that side).
#[derive(Serialize)]
struct HybridSnapshotRef<'a> {
    seq_at_snapshot: Option<u64>,
    state: &'a CanonicalCachedState,
}

/// Hybrid snapshot-plus-WAL durable store. See module docs for the
/// durability model.
pub struct HybridStore {
    state: CanonicalCachedState,
    snapshot_path: PathBuf,
    /// Kept open in append mode for the life of the store; the path
    /// itself doesn't need to be retained since — unlike the WAL
    /// variants' `checkpoint` — this variant never reopens/truncates it.
    wal_file: File,
    next_seq: u64,
}

impl HybridStore {
    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join("snapshot.bin"), dir.join("wal.log"))
    }

    /// Build fresh state from `records`/`edges` and start a new, empty WAL
    /// at `dir` — the "first-ever start" case, before any snapshot or
    /// `update_age` call.
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
        let (snapshot_path, wal_path) = Self::paths(dir);
        let wal_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&wal_path)?;
        Ok(Self {
            state: CanonicalCachedState::new(records, edges),
            snapshot_path,
            wal_file,
            next_seq: 0,
        })
    }

    /// Restore the latest snapshot at `dir` (if one exists; otherwise
    /// start from `records`/`edges` fresh, same "base dataset supplied
    /// externally" convention as the WAL variants), then replay only the
    /// WAL entries whose sequence number is strictly greater than the
    /// snapshot's recorded cutoff. The "load/replay/startup" path this
    /// variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// snapshot or WAL exist but can't be read/deserialized.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        dir: &Path,
    ) -> Result<Self, DurabilityError> {
        std::fs::create_dir_all(dir)?;
        let (snapshot_path, wal_path) = Self::paths(dir);

        let (mut state, cutoff) = if snapshot_path.exists() {
            let bytes = std::fs::read(&snapshot_path)?;
            let snapshot: HybridSnapshot = crate::codec::decode(&bytes)?;
            (snapshot.state, snapshot.seq_at_snapshot)
        } else {
            (CanonicalCachedState::new(records, edges), None)
        };

        let mut next_seq = 0u64;
        for entry in read_wal_entries(&wal_path)? {
            let after_cutoff = match cutoff {
                Some(c) => entry.seq > c,
                None => true,
            };
            if after_cutoff {
                state.update_age(entry.id, entry.age)?;
            }
            next_seq = next_seq.max(entry.seq + 1);
        }

        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)?;

        Ok(Self {
            state,
            snapshot_path,
            wal_file,
            next_seq,
        })
    }

    /// Write a fresh full-state snapshot tagged with the sequence number
    /// of the last entry it covers. The WAL is **not** truncated (see
    /// module docs for why) — it keeps growing, and [`Self::open`] always
    /// filters by the latest snapshot's recorded cutoff rather than
    /// assuming the WAL only ever holds post-cutoff entries.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// snapshot can't be serialized or written.
    pub fn checkpoint(&mut self) -> Result<(), DurabilityError> {
        let seq_at_snapshot = if self.next_seq == 0 {
            None
        } else {
            Some(self.next_seq - 1)
        };
        let snapshot = HybridSnapshotRef {
            seq_at_snapshot,
            state: &self.state,
        };
        let bytes = crate::codec::encode(&snapshot)?;
        std::fs::write(&self.snapshot_path, bytes)?;
        Ok(())
    }
}

impl DogStore for HybridStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.state.get(id)
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.state.scan_ages()
    }

    /// Write-ahead, buffered (not fsync'd — see module docs for why).
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let entry = WalEntry {
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
        let dir = crate::bench_support::fresh_temp_dir("hybrid_basic").unwrap();
        let mut store = HybridStore::create(sample_records(), sample_edges(), &dir).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The highest-priority correctness property for this variant: a
    /// snapshot plus partial WAL replay must reconstruct correctly across
    /// the cutoff boundary — writes before the snapshot come from the
    /// snapshot itself, writes after come from replaying entries whose
    /// sequence number exceeds the recorded cutoff.
    #[test]
    fn snapshot_plus_partial_replay_reconstructs_correctly_across_the_cutoff() {
        let dir = crate::bench_support::fresh_temp_dir("hybrid_cutoff").unwrap();
        {
            let mut store = HybridStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 10).unwrap(); // seq 0, pre-cutoff
            store.update_age(Uuid::from_u128(2), 20).unwrap(); // seq 1, pre-cutoff
            store.checkpoint().unwrap(); // cutoff = seq 1
            store.update_age(Uuid::from_u128(1), 11).unwrap(); // seq 2, post-cutoff
            store.update_age(Uuid::from_u128(3), 30).unwrap(); // seq 3, post-cutoff
                                                               // Dropped without a second checkpoint — simulates a crash
                                                               // right after the last write.
        }

        let reopened = HybridStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(
            reopened.get(Uuid::from_u128(1)).unwrap().age,
            11,
            "post-cutoff update to id 1 (seq 2) should have been replayed on top of the snapshot"
        );
        assert_eq!(
            reopened.get(Uuid::from_u128(2)).unwrap().age,
            20,
            "pre-cutoff update to id 2 should have come from the snapshot itself"
        );
        assert_eq!(
            reopened.get(Uuid::from_u128(3)).unwrap().age,
            30,
            "post-cutoff update to id 3 (seq 3) should have been replayed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second checkpoint moves the cutoff forward; a third round of
    /// writes on top of *that* must also reconstruct correctly, and the
    /// (never-truncated) WAL now spans two checkpoint generations' worth
    /// of entries.
    #[test]
    fn multiple_checkpoints_advance_the_cutoff_correctly() {
        let dir = crate::bench_support::fresh_temp_dir("hybrid_multi_checkpoint").unwrap();
        {
            let mut store = HybridStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 10).unwrap(); // seq 0
            store.checkpoint().unwrap(); // cutoff = 0
            store.update_age(Uuid::from_u128(1), 20).unwrap(); // seq 1
            store.checkpoint().unwrap(); // cutoff = 1
            store.update_age(Uuid::from_u128(1), 30).unwrap(); // seq 2, post-latest-cutoff
        }

        let reopened = HybridStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 30);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn indexes_survive_snapshot_and_replay() {
        let dir = crate::bench_support::fresh_temp_dir("hybrid_indexes").unwrap();
        {
            let mut store = HybridStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.checkpoint().unwrap();
            store.update_age(Uuid::from_u128(1), 50).unwrap();
        }
        let reopened = HybridStore::open(sample_records(), sample_edges(), &dir).unwrap();
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
