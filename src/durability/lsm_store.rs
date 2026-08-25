//! Tier 2, variant 7: LSM-tree style — in-memory memtable + WAL, periodic
//! flush of the memtable to an immutable sorted file on disk.
//!
//! # Design
//!
//! - **Memtable**: a `BTreeMap<Uuid, u32>` of age *overrides* on top of
//!   the base dataset (`records`, supplied externally at `open`/`create`,
//!   same convention as every other durability variant) — sorted, per
//!   the task's own framing ("immutable *sorted* file"), and holding only
//!   what's actually changed since the last flush, not a full copy of
//!   every record.
//! - **WAL**: reuses [`super::WalEntry`]/[`super::append_wal_entry`], the
//!   same format every WAL-based Tier 1 variant uses — durability for
//!   whatever's still in the memtable and hasn't been flushed yet.
//! - **Flush** ([`Self::flush`]): serializes the current memtable to a new
//!   numbered, immutable file (`sst_N.bin`), then clears the memtable and
//!   starts a fresh WAL — the flushed generation no longer needs WAL
//!   coverage, since it's now durable in its own file.
//! - **`get`**: checks the memtable first, then flushed files
//!   newest-to-oldest, then the base dataset — exactly the order the task
//!   specifies.
//!
//! # No compaction — flagged as future work, not silently skipped
//!
//! Real LSM trees merge/compact old SST files so reads don't have to
//! check an ever-growing list of them and so superseded keys eventually
//! get reclaimed. This prototype does neither: every flush adds one more
//! file `get`/`scan_ages` must check (read amplification that gets worse,
//! not better, the longer the store runs), and a key overridden many
//! times leaves its old values on disk forever. This is a known,
//! deliberate scope cut for a benchmark-only POC — see `RESULTS.md`'s
//! durability section and this crate's spec tree for where it's called
//! out as future work, not implemented here.

use super::{append_wal_entry, read_wal_entries, DurabilityError, WalEntry};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The immutable-after-construction pieces of [`LsmStore`]'s state —
/// factored out purely for readability at the `build_indexes` call sites
/// (mirrors [`super::mmap_store`]'s identically-motivated `Indexes`).
struct Indexes {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
}

/// LSM-tree-style durable store. See module docs for the design and its
/// explicitly out-of-scope compaction.
pub struct LsmStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    memtable: BTreeMap<Uuid, u32>,
    /// How many `sst_N.bin` files exist, `N` in `0..flushed_generations`.
    flushed_generations: usize,
    dir: PathBuf,
    wal_file: File,
    next_seq: u64,
}

impl LsmStore {
    fn generations_path(dir: &Path) -> PathBuf {
        dir.join("generations.bin")
    }

    fn wal_path(dir: &Path) -> PathBuf {
        dir.join("wal.log")
    }

    fn sst_path(dir: &Path, generation: usize) -> PathBuf {
        dir.join(format!("sst_{generation}.bin"))
    }

    fn build_indexes(records: &[DogRecord], edges: Vec<(Uuid, Uuid)>) -> Indexes {
        let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
        for record in records {
            breed_index
                .entry(record.breed.clone())
                .or_default()
                .push(record.id);
        }
        let mut adjacency_index: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (a, b) in edges {
            adjacency_index.entry(a).or_default().push(b);
            adjacency_index.entry(b).or_default().push(a);
        }
        let records = records.iter().cloned().map(|r| (r.id, r)).collect();
        Indexes {
            records,
            breed_index,
            adjacency_index,
        }
    }

    /// Resolve `id`'s current age: memtable, then flushed generations
    /// newest-to-oldest, then the base dataset. Returns `None` only if
    /// `id` isn't a known record at all.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if a
    /// flushed SST file exists but can't be read/deserialized.
    fn resolve_age(&self, id: Uuid) -> Result<Option<u32>, DurabilityError> {
        if let Some(&age) = self.memtable.get(&id) {
            return Ok(Some(age));
        }
        for generation in (0..self.flushed_generations).rev() {
            let bytes = std::fs::read(Self::sst_path(&self.dir, generation))?;
            let sst: BTreeMap<Uuid, u32> = bincode::deserialize(&bytes)?;
            if let Some(&age) = sst.get(&id) {
                return Ok(Some(age));
            }
        }
        Ok(self.records.get(&id).map(|r| r.age))
    }

    /// Build fresh state from `records`/`edges` and start a new, empty WAL
    /// at `dir` — no flushed generations exist yet.
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
        let indexes = Self::build_indexes(&records, edges);
        let wal_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(Self::wal_path(dir))?;
        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            memtable: BTreeMap::new(),
            flushed_generations: 0,
            dir: dir.to_path_buf(),
            wal_file,
            next_seq: 0,
        })
    }

    /// Rebuild indexes from `records`/`edges`, read how many flushed
    /// generations exist (0 if this is the first-ever open), and replay
    /// the current WAL to reconstruct the memtable as it stood before the
    /// simulated restart. The "load/replay/startup" path this variant's
    /// benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// generation count or WAL exist but can't be read/deserialized.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        dir: &Path,
    ) -> Result<Self, DurabilityError> {
        std::fs::create_dir_all(dir)?;
        let indexes = Self::build_indexes(&records, edges);

        let flushed_generations = match std::fs::read(Self::generations_path(dir)) {
            Ok(bytes) => bincode::deserialize(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };

        let mut memtable = BTreeMap::new();
        let mut next_seq = 0u64;
        for entry in read_wal_entries(&Self::wal_path(dir))? {
            memtable.insert(entry.id, entry.age);
            next_seq = entry.seq + 1;
        }

        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::wal_path(dir))?;

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            memtable,
            flushed_generations,
            dir: dir.to_path_buf(),
            wal_file,
            next_seq,
        })
    }

    /// Serialize the current memtable to a new, immutable, numbered file,
    /// then clear the memtable and start a fresh WAL — the just-flushed
    /// generation no longer needs WAL coverage.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`]/[`DurabilityError::Serde`] if the
    /// SST file, generation count, or fresh WAL can't be written.
    pub fn flush(&mut self) -> Result<(), DurabilityError> {
        let bytes = bincode::serialize(&self.memtable)?;
        std::fs::write(Self::sst_path(&self.dir, self.flushed_generations), bytes)?;
        self.flushed_generations += 1;
        std::fs::write(
            Self::generations_path(&self.dir),
            bincode::serialize(&self.flushed_generations)?,
        )?;

        self.memtable.clear();
        self.wal_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(Self::wal_path(&self.dir))?;

        Ok(())
    }
}

impl DogStore for LsmStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        let record = self.records.get(&id)?;
        let age = self.resolve_age(id).ok().flatten()?;
        Some(DogRecord::new(record.id, record.breed.clone(), age))
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.records
            .keys()
            .filter_map(|&id| self.resolve_age(id).ok().flatten())
            .collect()
    }

    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        if !self.records.contains_key(&id) {
            return Err(StoreError::NotFound(id));
        }
        let entry = WalEntry {
            seq: self.next_seq,
            id,
            age,
        };
        append_wal_entry(&mut self.wal_file, &entry)?;
        self.next_seq += 1;
        self.memtable.insert(id, age);
        Ok(())
    }

    fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        let Some(target) = self.records.get(&id) else {
            return Vec::new();
        };
        match self.breed_index.get(&target.breed) {
            Some(ids) => ids.iter().copied().filter(|&other| other != id).collect(),
            None => Vec::new(),
        }
    }

    fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.adjacency_index.get(&id).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durability::test_support::*;

    #[test]
    fn create_then_read_and_write() {
        let dir = crate::bench_support::fresh_temp_dir("lsm_basic").unwrap();
        let mut store = LsmStore::create(sample_records(), sample_edges(), &dir).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 3);
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        assert_eq!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The defining correctness property for this variant: a key updated
    /// across multiple flush generations must resolve to its *newest*
    /// value — memtable beats the latest flushed file, which beats older
    /// flushed files, which beats the base dataset.
    #[test]
    fn newest_generation_wins_across_multiple_flushes() {
        let dir = crate::bench_support::fresh_temp_dir("lsm_newest_wins").unwrap();
        let mut store = LsmStore::create(sample_records(), sample_edges(), &dir).unwrap();

        store.update_age(Uuid::from_u128(1), 10).unwrap();
        store.flush().unwrap(); // sst_0: {1: 10}

        store.update_age(Uuid::from_u128(1), 20).unwrap();
        store.flush().unwrap(); // sst_1: {1: 20}, supersedes sst_0

        assert_eq!(
            store.get(Uuid::from_u128(1)).unwrap().age,
            20,
            "sst_1 (newer) should win over sst_0 (older)"
        );

        store.update_age(Uuid::from_u128(1), 30).unwrap(); // memtable, not yet flushed
        assert_eq!(
            store.get(Uuid::from_u128(1)).unwrap().age,
            30,
            "memtable should win over every flushed generation"
        );

        // A record never updated at all still resolves to its base age.
        assert_eq!(store.get(Uuid::from_u128(2)).unwrap().age, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip / restart correctness: writes flushed to disk survive a
    /// simulated restart, and so do writes still only in the WAL
    /// (unflushed) at the moment of "crash."
    #[test]
    fn reopen_recovers_flushed_and_unflushed_writes() {
        let dir = crate::bench_support::fresh_temp_dir("lsm_reopen").unwrap();
        {
            let mut store = LsmStore::create(sample_records(), sample_edges(), &dir).unwrap();
            store.update_age(Uuid::from_u128(1), 10).unwrap();
            store.flush().unwrap();
            store.update_age(Uuid::from_u128(2), 20).unwrap(); // unflushed at "crash"
        }

        let reopened = LsmStore::open(sample_records(), sample_edges(), &dir).unwrap();
        assert_eq!(
            reopened.get(Uuid::from_u128(1)).unwrap().age,
            10,
            "flushed write should survive reopen via the SST file"
        );
        assert_eq!(
            reopened.get(Uuid::from_u128(2)).unwrap().age,
            20,
            "unflushed write should survive reopen via WAL replay into the memtable"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_breed_and_neighbors_work() {
        let dir = crate::bench_support::fresh_temp_dir("lsm_indexes").unwrap();
        let store = LsmStore::create(sample_records(), sample_edges(), &dir).unwrap();
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
}
