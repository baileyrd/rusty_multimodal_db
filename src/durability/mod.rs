//! Durability prototypes for `CanonicalCachedStore` — the only backend
//! that gets persistence in this pass. AoS/SoA/`CanonicalStore` remain
//! untouched, purely in-memory baselines; nobody would deploy them, so
//! building persistence for them would be pure scope creep. See
//! `docs/decisions/ADR-0005-wal-snapshot-hybrid-durability.md` (Tier 1:
//! WAL/snapshot/hybrid) and
//! `docs/decisions/ADR-0006-tier-2-durability-architectures.md` (Tier 2:
//! mmap/LSM/embedded-engine) for the design decisions behind each variant,
//! and `docs/specifications/storage/STORAGE-008-durability-tier1.md` /
//! `STORAGE-009-durability-tier2.md` for the requirements each satisfies.
//!
//! # Why a new module, not a change to `src/store/canonical_cached.rs`
//!
//! Every durability variant here implements [`DogStore`] so it plugs into
//! the same benchmark/test patterns the four existing backends use, but
//! none of them are built by modifying `CanonicalCachedStore` itself —
//! that file is closed, already-benchmarked backend code (see prior
//! sessions' row/column/graph/mixed-workload work), and this is new,
//! separate scope layered on top of the same *architecture*
//! (canonical map + breed index + age cache + adjacency index), not a
//! literal reuse of the private struct. [`CanonicalCachedState`] below is
//! that shared architecture, rebuilt here so every variant's read path
//! (`get`/`scan_ages`/`same_breed`/`neighbors` — identical across all of
//! them) is written once instead of duplicated eight times; only
//! construction, `update_age`, and on-disk persistence differ per
//! variant.
//!
//! # Shared conventions across variants
//!
//! - **Base dataset supplied externally at open time.** Nothing in this
//!   crate currently persists the *initial* generated dataset — every
//!   backend everywhere else is built fresh from `Vec<DogRecord>` per
//!   benchmark/test run (see `bench_support::build_dataset`). The
//!   durability variants keep that convention: `open`/`create` take
//!   `records`/`edges` the same way `CanonicalCachedStore::new` does, and
//!   durability covers what happens to that base dataset *after*
//!   construction — specifically, whether `update_age` calls survive a
//!   simulated process restart (dropping the store and reopening from the
//!   same on-disk path). This is a deliberate scope boundary: modeling
//!   "the initial bulk load is itself durable" would need its own
//!   mechanism per variant and isn't what any of these prototypes test.
//! - **One error type**, [`DurabilityError`], shared by every variant
//!   (I/O, (de)serialization, and the existing [`StoreError`] all fold
//!   into it) rather than one bespoke error enum per variant.
//! - **Every fallible path returns `Result` and uses `?`** — no
//!   `unwrap`/`expect` outside `#[cfg(test)]`, same as every other module
//!   in this crate.

use crate::record::DogRecord;
use crate::store::StoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

pub mod embedded_store;
pub mod hybrid;
pub mod lsm_store;
pub mod mmap_store;
pub mod snapshot_full;
pub mod snapshot_rebuild;
pub mod wal_buffered;
pub mod wal_fsync;

pub use embedded_store::RedbStore;
pub use hybrid::HybridStore;
pub use lsm_store::LsmStore;
pub use mmap_store::MmapAgeStore;
pub use snapshot_full::SnapshotFullStore;
pub use snapshot_rebuild::SnapshotRebuildStore;
pub use wal_buffered::WalBufferedStore;
pub use wal_fsync::WalFsyncStore;

/// Every fallible outcome across every durability variant. One type,
/// rather than a bespoke error enum per variant — each variant's failure
/// modes (I/O, serialization, an unknown UUID) are the same kinds of
/// thing, just triggered by different code paths.
#[derive(Debug, Error)]
pub enum DurabilityError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("(de)serialization error: {0}")]
    Serde(#[from] bincode::Error),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    /// Wraps a Tier 2 variant's own engine error (e.g. `redb`) as a
    /// string — those crates' error types don't all implement a common
    /// trait this crate can `#[from]` directly, and introducing a
    /// per-engine error variant for just one Tier 2 backend isn't worth
    /// the added enum surface.
    #[error("embedded engine error: {0}")]
    Engine(String),
}

/// Lets every variant's `DogStore::update_age` impl use `?` directly on a
/// `Result<_, DurabilityError>`-returning call, even though the trait
/// method itself must return `Result<(), StoreError>`. An unknown-UUID
/// failure (the one case `StoreError` already models) round-trips as
/// itself; anything else becomes `StoreError::Durability`.
impl From<DurabilityError> for StoreError {
    fn from(error: DurabilityError) -> Self {
        match error {
            DurabilityError::Store(store_error) => store_error,
            other => StoreError::Durability(other.to_string()),
        }
    }
}

/// Append one entry to `writer`, length-prefixed (a 4-byte little-endian
/// length followed by that many bincode-serialized bytes) so a reader
/// doesn't need to know each entry's size in advance and can detect a
/// torn trailing write (see [`read_wal_entries`]) rather than
/// misinterpreting one. Shared by every WAL-writing variant (1, 2, 5, 7).
pub(crate) fn append_wal_entry(
    writer: &mut impl std::io::Write,
    entry: &WalEntry,
) -> Result<(), DurabilityError> {
    let bytes = bincode::serialize(entry)?;
    let len = bytes.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

/// Read every entry from a length-prefixed WAL file written by
/// [`append_wal_entry`], in the order they were appended. A missing file
/// reads as an empty WAL (no entries yet), not an error — this is the
/// expected state for a variant's very first `open` before any
/// `update_age` call. A torn trailing write (a length prefix claiming more
/// bytes than the file actually has — the file left mid-write when a
/// process died) stops replay at that point rather than erroring: every
/// entry before the tear is still valid and recoverable, which is the
/// whole point of a WAL.
pub(crate) fn read_wal_entries(path: &std::path::Path) -> Result<Vec<WalEntry>, DurabilityError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + len > bytes.len() {
            break;
        }
        let entry: WalEntry = bincode::deserialize(&bytes[offset..offset + len])?;
        entries.push(entry);
        offset += len;
    }
    Ok(entries)
}

/// One logged `update_age` call: `seq` is a strictly monotonically
/// increasing sequence number (assigned by the writer, one per call),
/// used by every WAL-based variant to order replay and — for
/// [`HybridStore`] specifically — to decide which entries are already
/// covered by the latest snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalEntry {
    pub seq: u64,
    pub id: Uuid,
    pub age: u32,
}

/// The shared in-memory architecture every durability variant is built
/// around: identical in shape to `CanonicalCachedStore`'s private fields
/// (canonical map, breed index, age cache, position index, adjacency
/// index), rebuilt here so it can be shared across eight variants instead
/// of duplicated. `Serialize`/`Deserialize` are what make
/// [`SnapshotFullStore`] (variant 4, "save-as-is") possible — it persists
/// this struct directly, no rebuild step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalCachedState {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    age_cache: Vec<u32>,
    position_index: HashMap<Uuid, usize>,
}

impl CanonicalCachedState {
    /// Build from records and littermate edges — identical construction
    /// logic to `CanonicalCachedStore::new`/`CanonicalStore::new`.
    ///
    /// # Reopen-path profiling (STORAGE-008/009 follow-up)
    ///
    /// Real profiling of `SnapshotRebuildStore::open` (the one variant whose
    /// benchmarked `open` calls this constructor — see
    /// `benches/durability.rs`'s `run_load`/`RESULTS.md`'s reopen-cost
    /// section for the full breakdown across all 8 variants) found the work
    /// below splits into two genuinely independent phases at this dataset
    /// shape's dominant cost: everything keyed by *record* (breed index,
    /// age cache, position index, then the canonical records map) only
    /// touches `records`; the adjacency index only touches `edges`.
    /// Disjoint inputs, disjoint outputs, no merge step — `std::thread::scope`
    /// (stdlib, no new dependency) is enough to run them concurrently;
    /// `rayon`'s data-parallel iterators would be overkill for a plain
    /// two-way split with nothing left to subdivide further.
    pub fn new(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>) -> Self {
        let (adjacency_index, (records, breed_index, age_cache, position_index)) =
            std::thread::scope(|scope| {
                let adjacency_handle = scope.spawn(move || {
                    let mut adjacency_index: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
                    for (a, b) in edges {
                        adjacency_index.entry(a).or_default().push(b);
                        adjacency_index.entry(b).or_default().push(a);
                    }
                    adjacency_index
                });

                let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
                let mut age_cache = Vec::with_capacity(records.len());
                let mut position_index = HashMap::with_capacity(records.len());
                for (position, record) in records.iter().enumerate() {
                    breed_index
                        .entry(record.breed.clone())
                        .or_default()
                        .push(record.id);
                    age_cache.push(record.age);
                    position_index.insert(record.id, position);
                }
                let records: HashMap<Uuid, DogRecord> =
                    records.into_iter().map(|r| (r.id, r)).collect();

                // The spawned closure above only ever does infallible,
                // side-effect-free HashMap/Vec insertion — nothing that can
                // fail in normal operation. `resume_unwind` (not
                // unwrap/expect) re-raises the original panic exactly as it
                // occurred if that ever changes, rather than fabricating a
                // new one or silently discarding the failure.
                let adjacency_index = match adjacency_handle.join() {
                    Ok(adjacency_index) => adjacency_index,
                    Err(panic_payload) => std::panic::resume_unwind(panic_payload),
                };

                (
                    adjacency_index,
                    (records, breed_index, age_cache, position_index),
                )
            });

        Self {
            records,
            breed_index,
            adjacency_index,
            age_cache,
            position_index,
        }
    }

    pub fn get(&self, id: Uuid) -> Option<DogRecord> {
        self.records.get(&id).cloned()
    }

    pub fn scan_ages(&self) -> Vec<u32> {
        self.age_cache.clone()
    }

    pub fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let record = self.records.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        record.age = age;

        let position = *self
            .position_index
            .get(&id)
            .ok_or(StoreError::NotFound(id))?;
        self.age_cache[position] = age;

        Ok(())
    }

    pub fn same_breed(&self, id: Uuid) -> Vec<Uuid> {
        let Some(target) = self.records.get(&id) else {
            return Vec::new();
        };
        match self.breed_index.get(&target.breed) {
            Some(ids) => ids.iter().copied().filter(|&other| other != id).collect(),
            None => Vec::new(),
        }
    }

    pub fn neighbors(&self, id: Uuid) -> Vec<Uuid> {
        self.adjacency_index.get(&id).cloned().unwrap_or_default()
    }

    /// Serialize this state to `path` via bincode, replacing whatever was
    /// there. Used by every snapshot-writing variant (3, 4, 5) — variant 3
    /// calls this on a state built from records/edges only (see its own
    /// module docs for why that's still safe), variant 4 on the full
    /// state, variant 5 alongside a recorded cutoff sequence number.
    pub(crate) fn write_to(&self, path: &Path) -> Result<(), DurabilityError> {
        let bytes = bincode::serialize(self)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub(crate) fn read_from(path: &Path) -> Result<Self, DurabilityError> {
        let bytes = std::fs::read(path)?;
        Ok(bincode::deserialize(&bytes)?)
    }

    /// Every current record, as a plain `Vec` — used by variant 3
    /// ([`snapshot_rebuild`]) to reconstruct a `{records, edges}` snapshot
    /// from live state, since that variant's on-disk format doesn't keep
    /// the full `CanonicalCachedState` shape around the way variant 4's
    /// does.
    pub(crate) fn records_snapshot(&self) -> Vec<DogRecord> {
        self.records.values().cloned().collect()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn sample_records() -> Vec<DogRecord> {
        vec![
            DogRecord::new(Uuid::from_u128(1), "labrador", 3),
            DogRecord::new(Uuid::from_u128(2), "labrador", 5),
            DogRecord::new(Uuid::from_u128(3), "poodle", 2),
        ]
    }

    pub(crate) fn sample_edges() -> Vec<(Uuid, Uuid)> {
        vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn state_matches_dogstore_shape() {
        let mut state = CanonicalCachedState::new(sample_records(), sample_edges());
        assert_eq!(state.get(Uuid::from_u128(1)).unwrap().breed, "labrador");
        assert_eq!(state.get(Uuid::from_u128(99)), None);

        state.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(state.get(Uuid::from_u128(1)).unwrap().age, 42);
        assert!(state.scan_ages().contains(&42));

        assert_eq!(
            state.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        assert_eq!(
            state.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            state.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
    }

    #[test]
    fn write_to_then_read_from_round_trips() {
        let dir = crate::bench_support::fresh_temp_dir("canonical_cached_state").unwrap();
        let path = dir.join("state.bin");

        let mut state = CanonicalCachedState::new(sample_records(), sample_edges());
        state.update_age(Uuid::from_u128(2), 77).unwrap();
        state.write_to(&path).unwrap();

        let loaded = CanonicalCachedState::read_from(&path).unwrap();
        assert_eq!(loaded.get(Uuid::from_u128(2)).unwrap().age, 77);
        assert_eq!(
            loaded.same_breed(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );
        assert_eq!(
            loaded.neighbors(Uuid::from_u128(1)),
            vec![Uuid::from_u128(2)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wal_entries_round_trip_in_order() {
        let dir = crate::bench_support::fresh_temp_dir("wal_entries").unwrap();
        let path = dir.join("wal.log");

        let entries = vec![
            WalEntry {
                seq: 0,
                id: Uuid::from_u128(1),
                age: 4,
            },
            WalEntry {
                seq: 1,
                id: Uuid::from_u128(2),
                age: 9,
            },
            WalEntry {
                seq: 2,
                id: Uuid::from_u128(1),
                age: 5,
            },
        ];

        {
            let mut file = std::fs::File::create(&path).unwrap();
            for entry in &entries {
                append_wal_entry(&mut file, entry).unwrap();
            }
        }

        assert_eq!(read_wal_entries(&path).unwrap(), entries);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_wal_file_reads_as_empty() {
        let dir = crate::bench_support::fresh_temp_dir("wal_missing").unwrap();
        let path = dir.join("does_not_exist.log");
        assert!(read_wal_entries(&path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A torn trailing write (process died mid-append) must not corrupt
    /// replay of the entries written *before* the tear — this is the
    /// entire reason a WAL is safe to append to without an explicit
    /// "commit" step.
    #[test]
    fn torn_trailing_write_does_not_lose_earlier_entries() {
        let dir = crate::bench_support::fresh_temp_dir("wal_torn").unwrap();
        let path = dir.join("wal.log");

        let good_entry = WalEntry {
            seq: 0,
            id: Uuid::from_u128(1),
            age: 4,
        };
        {
            let mut file = std::fs::File::create(&path).unwrap();
            append_wal_entry(&mut file, &good_entry).unwrap();
        }
        // Simulate a torn write: a length prefix claiming a 100-byte entry
        // follows, but no entry bytes actually made it to disk.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(&100u32.to_le_bytes()).unwrap();
        }

        assert_eq!(read_wal_entries(&path).unwrap(), vec![good_entry]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
