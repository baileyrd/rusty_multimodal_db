//! Tier 2, variant 6: mmap-backed store.
//!
//! # Scope-down (deliberate, not a shortcut taken quietly)
//!
//! A real mmap-based store would map *everything* — ids, breeds, ages —
//! directly into a fixed-layout on-disk region. That needs a fixed-size
//! row format, and this crate's `breed: String` is variable-length, so
//! mapping full records directly would mean either a secondary
//! string-heap-with-offsets scheme (a real, non-trivial file format to
//! design) or capping breed length (changing the record shape, which
//! the charter treats as a hard-to-reverse, explicitly-approved-only
//! decision). Neither is "minimal, not a real LMDB reimplementation" —
//! the task's own framing for this tier.
//!
//! Instead: **only `age` — the one field that ever mutates — lives in the
//! memory-mapped region**, as a flat, fixed-size `[u32]` array indexed by
//! position (exactly `CanonicalCachedStore`'s existing `age_cache`
//! design, just backed by `MmapMut` instead of `Vec<u32>`). `update_age`
//! writes straight into mapped memory; the OS's own page-cache write-back
//! handles getting it to disk, with [`Self::flush`] available to force
//! that (via `msync`) when a caller wants a durability guarantee before
//! moving on — the direct mmap analogue of every other variant's
//! `checkpoint`. Records/edges (immutable after construction) are
//! supplied externally at `open`/`create` time, same convention as the
//! WAL variants — this variant durably persists exactly the field that
//! can change, and nothing else, which is the most honest "minimal POC"
//! reading of what an mmap-backed store's value proposition actually is
//! for this crate's fixed three-field record.

use super::{DurabilityError, PARALLEL_CONSTRUCTION_THRESHOLD};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use memmap2::MmapMut;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// mmap-backed durable store (ages only — see module docs).
pub struct MmapAgeStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    position_index: HashMap<Uuid, usize>,
    mmap: MmapMut,
    #[allow(dead_code)] // kept for symmetry with the other variants' Self; not read again
    path: PathBuf,
}

/// The immutable-after-construction pieces of [`MmapAgeStore`]'s state —
/// everything except the mapped age region itself. Factored into its own
/// struct (rather than a 4-tuple) purely for readability at the
/// `create`/`open` call sites.
struct Indexes {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    position_index: HashMap<Uuid, usize>,
}

/// See `CanonicalCachedState::new`'s doc comment (`src/durability/mod.rs`)
/// for why this splits into a spawned thread building the adjacency index
/// (only touches `edges`) running alongside the breed/position index and
/// records-map construction (only touches `records`) — same disjoint-input,
/// no-merge shape, same `std::thread::scope` fix, duplicated here rather
/// than shared per this crate's existing convention of small explicit
/// duplication across structurally similar backends (see `wal_buffered.rs`'s
/// own module docs). Below `PARALLEL_CONSTRUCTION_THRESHOLD` records, falls
/// back to `build_indexes_sequential` — see that constant's own doc comment
/// (`src/durability/mod.rs`) for why and how it was measured.
fn build_indexes(records: &[DogRecord], edges: Vec<(Uuid, Uuid)>) -> Indexes {
    if records.len() < PARALLEL_CONSTRUCTION_THRESHOLD {
        return build_indexes_sequential(records, edges);
    }

    let (adjacency_index, (breed_index, position_index, records_map)) =
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
            let mut position_index = HashMap::with_capacity(records.len());
            for (position, record) in records.iter().enumerate() {
                breed_index
                    .entry(record.breed.clone())
                    .or_default()
                    .push(record.id);
                position_index.insert(record.id, position);
            }
            let records_map = records.iter().cloned().map(|r| (r.id, r)).collect();

            let adjacency_index = match adjacency_handle.join() {
                Ok(adjacency_index) => adjacency_index,
                Err(panic_payload) => std::panic::resume_unwind(panic_payload),
            };

            (adjacency_index, (breed_index, position_index, records_map))
        });

    Indexes {
        records: records_map,
        breed_index,
        adjacency_index,
        position_index,
    }
}

/// The original, single-threaded construction — same phases as
/// `build_indexes`'s parallel path, just run in sequence on one thread.
/// Used below `PARALLEL_CONSTRUCTION_THRESHOLD`, where spawning a thread
/// would cost more than it saves.
fn build_indexes_sequential(records: &[DogRecord], edges: Vec<(Uuid, Uuid)>) -> Indexes {
    let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut position_index = HashMap::with_capacity(records.len());
    for (position, record) in records.iter().enumerate() {
        breed_index
            .entry(record.breed.clone())
            .or_default()
            .push(record.id);
        position_index.insert(record.id, position);
    }
    let mut adjacency_index: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (a, b) in edges {
        adjacency_index.entry(a).or_default().push(b);
        adjacency_index.entry(b).or_default().push(a);
    }
    let records_map = records.iter().cloned().map(|r| (r.id, r)).collect();
    Indexes {
        records: records_map,
        breed_index,
        adjacency_index,
        position_index,
    }
}

impl MmapAgeStore {
    /// One bounds check on a 4-byte slice, not four separate bounds checks
    /// on individual byte indices — see `scan_ages`'s own doc comment for
    /// why this matters far more there than it does for this single-position
    /// lookup (used by `get`, one call at a time).
    fn read_age(&self, position: usize) -> u32 {
        let start = position * 4;
        let bytes: [u8; 4] = self.mmap[start..start + 4]
            .try_into()
            .expect("slice taken as start..start + 4 is always exactly 4 bytes");
        u32::from_le_bytes(bytes)
    }

    fn write_age(&mut self, position: usize, age: u32) {
        let start = position * 4;
        self.mmap[start..start + 4].copy_from_slice(&age.to_le_bytes());
    }

    /// Build fresh indexes from `records`/`edges`, create a new
    /// `4 * records.len()`-byte file at `path` initialized with each
    /// record's starting age, and memory-map it.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized, or the mapping fails.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let initial_ages: Vec<u32> = records.iter().map(|r| r.age).collect();
        let indexes = build_indexes(&records, edges);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((initial_ages.len() * 4) as u64)?;

        // SAFETY: this process holds exclusive read/write access to the
        // freshly-created file at `path` for the lifetime of the mapping;
        // nothing else concurrently truncates or writes to it out from
        // under the mapping, which is the actual UB risk `mmap` carries.
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        for (position, age) in initial_ages.iter().enumerate() {
            mmap[position * 4..position * 4 + 4].copy_from_slice(&age.to_le_bytes());
        }
        mmap.flush()?;

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            position_index: indexes.position_index,
            mmap,
            path: path.to_path_buf(),
        })
    }

    /// Rebuild indexes from the externally-supplied `records`/`edges`
    /// (their `age` fields are ignored — the mapped file is the source of
    /// truth for ages), and memory-map the existing file at `path`. The
    /// "load/replay/startup" path this variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist or can't
    /// be mapped.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let indexes = build_indexes(&records, edges);

        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: see `create` — same single-process exclusive-access
        // assumption.
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            position_index: indexes.position_index,
            mmap,
            path: path.to_path_buf(),
        })
    }

    /// Force the mapped ages to physical disk (`msync`) — the mmap
    /// analogue of every other variant's `checkpoint`. `update_age`
    /// writes straight into mapped memory either way; this is what
    /// upgrades "reached the OS page cache" to "guaranteed on disk."
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the flush syscall fails.
    pub fn flush(&self) -> Result<(), DurabilityError> {
        self.mmap.flush()?;
        Ok(())
    }
}

impl DogStore for MmapAgeStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        let record = self.records.get(&id)?;
        let position = *self.position_index.get(&id)?;
        Some(DogRecord::new(
            record.id,
            record.breed.clone(),
            self.read_age(position),
        ))
    }

    /// Reads via `chunks_exact(4)` directly over the mapped region, not via
    /// `read_age` in a `0..len/4` loop. The original per-position loop paid
    /// four individually-bounds-checked byte indices per element (plus a
    /// per-element function-call frame); `chunks_exact` pays one bounds
    /// check per 4-byte chunk and lets the compiler reason about the whole
    /// scan as one pass, not `n` independent lookups. Diagnosed via a
    /// same-machine, same-process isolated benchmark (not assumed): the
    /// original loop was 25-32x slower than a `chunks_exact` read of the
    /// identical bytes, and was the entire measured cause of
    /// `ProductionStore`'s large, thread-count-invariant concurrency
    /// throughput tax at 100K records (see `RESULTS.md`'s `## Durability`
    /// and `## Production recommendation` sections). This change doesn't
    /// touch `update_age`/`flush`/the durability guarantee at all — it's a
    /// read-path-only fix.
    fn scan_ages(&self) -> Vec<u32> {
        self.mmap
            .chunks_exact(4)
            .map(|chunk| {
                u32::from_le_bytes(
                    chunk
                        .try_into()
                        .expect("chunks_exact(4) always yields exactly 4-byte chunks"),
                )
            })
            .collect()
    }

    /// Writes straight into mapped memory — no explicit syscall on this
    /// path at all (unlike every other variant, whose `update_age` does
    /// at least one `write`/`write_all` call). See [`Self::flush`] for
    /// the operation that actually forces it to disk.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        let position = *self
            .position_index
            .get(&id)
            .ok_or(StoreError::NotFound(id))?;
        self.write_age(position, age);
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
        let dir = crate::test_support::fresh_temp_dir("mmap_basic").unwrap();
        let path = dir.join("ages.mmap");
        let mut store = MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 3);
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        assert_eq!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip correctness: write ages, flush, reopen (fresh
    /// `MmapAgeStore`, same records/edges, same path) — the reopened
    /// store must see every flushed write.
    #[test]
    fn flush_then_reopen_sees_the_written_ages() {
        let dir = crate::test_support::fresh_temp_dir("mmap_roundtrip").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut store = MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.update_age(Uuid::from_u128(1), 77).unwrap();
            store.update_age(Uuid::from_u128(3), 12).unwrap();
            store.flush().unwrap();
        }

        let reopened = MmapAgeStore::open(sample_records(), sample_edges(), &path).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 77);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 5);
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_breed_and_neighbors_work() {
        let dir = crate::test_support::fresh_temp_dir("mmap_indexes").unwrap();
        let path = dir.join("ages.mmap");
        let store = MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();
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
