//! Tier 2, variant 8: off-the-shelf embedded engine (`redb`).
//!
//! # Why `redb` over `sled`
//!
//! `sled` has been in a long-standing "still beta" state with a
//! documented history of crash-safety caveats depending on version — a
//! real concern for a durability benchmark whose whole point is
//! measuring genuine crash-recoverability, not just raw speed. `redb` is
//! younger but built around explicit ACID write transactions from the
//! start (its own on-disk B-tree with MVCC), pure Rust with no C
//! dependency, and doesn't carry an equivalent caveat.
//!
//! # Scope-down: thin, not a full port
//!
//! Per the task's own framing ("should be the least code of any variant,
//! since the crate owns durability itself"): `redb` holds **only the
//! mutable `age` field**, keyed by UUID — the one thing about a
//! `DogRecord` that ever changes, mirroring [`super::MmapAgeStore`]
//! (variant 6)'s identical scope-down for the same reason. Records'
//! immutable fields (id, breed) and the derived breed/adjacency indexes
//! are rebuilt in memory from the externally-supplied `records`/`edges`
//! at `open`/`create` time — the same "base dataset supplied externally,
//! rebuild derived structure" convention every other variant in this
//! module follows. This keeps the actual `redb` integration to a handful
//! of transactions (one table, one primitive value type) rather than a
//! from-scratch port of `CanonicalCachedState`'s full shape into `redb`
//! tables.

use super::{DurabilityError, PARALLEL_CONSTRUCTION_THRESHOLD};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use redb::{Database, TableDefinition};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

const AGES_TABLE: TableDefinition<u128, u32> = TableDefinition::new("ages");

fn engine_err(error: impl std::fmt::Display) -> DurabilityError {
    DurabilityError::Engine(error.to_string())
}

/// `redb`-backed durable store (ages only — see module docs).
pub struct RedbStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    db: Database,
}

/// The immutable-after-construction pieces of [`RedbStore`]'s state —
/// factored out purely for readability (mirrors [`super::mmap_store`]'s
/// and [`super::lsm_store`]'s identically-motivated `Indexes`).
struct Indexes {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
}

/// See `CanonicalCachedState::new`'s doc comment (`src/durability/mod.rs`)
/// for why this splits into a spawned thread building the adjacency index
/// (only touches `edges`) running alongside the breed index and records-map
/// construction (only touches `records`) — same disjoint-input, no-merge
/// shape, same `std::thread::scope` fix, duplicated here rather than shared
/// per this crate's existing convention of small explicit duplication
/// across structurally similar backends (see `wal_buffered.rs`'s own module
/// docs). Below `PARALLEL_CONSTRUCTION_THRESHOLD` records, falls back to
/// `build_indexes_sequential` — see that constant's own doc comment
/// (`src/durability/mod.rs`) for why and how it was measured.
fn build_indexes(records: &[DogRecord], edges: Vec<(Uuid, Uuid)>) -> Indexes {
    if records.len() < PARALLEL_CONSTRUCTION_THRESHOLD {
        return build_indexes_sequential(records, edges);
    }

    let (adjacency_index, (breed_index, records_map)) = std::thread::scope(|scope| {
        let adjacency_handle = scope.spawn(move || {
            let mut adjacency_index: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
            for (a, b) in edges {
                adjacency_index.entry(a).or_default().push(b);
                adjacency_index.entry(b).or_default().push(a);
            }
            adjacency_index
        });

        let mut breed_index: HashMap<String, Vec<Uuid>> = HashMap::new();
        for record in records {
            breed_index
                .entry(record.breed.clone())
                .or_default()
                .push(record.id);
        }
        let records_map = records.iter().cloned().map(|r| (r.id, r)).collect();

        let adjacency_index = match adjacency_handle.join() {
            Ok(adjacency_index) => adjacency_index,
            Err(panic_payload) => std::panic::resume_unwind(panic_payload),
        };

        (adjacency_index, (breed_index, records_map))
    });

    Indexes {
        records: records_map,
        breed_index,
        adjacency_index,
    }
}

/// The original, single-threaded construction — same phases as
/// `build_indexes`'s parallel path, just run in sequence on one thread.
/// Used below `PARALLEL_CONSTRUCTION_THRESHOLD`, where spawning a thread
/// would cost more than it saves.
fn build_indexes_sequential(records: &[DogRecord], edges: Vec<(Uuid, Uuid)>) -> Indexes {
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

impl RedbStore {
    fn read_age(&self, id: Uuid) -> Result<Option<u32>, DurabilityError> {
        let read_txn = self.db.begin_read().map_err(engine_err)?;
        let table = read_txn.open_table(AGES_TABLE).map_err(engine_err)?;
        let value = table.get(id.as_u128()).map_err(engine_err)?;
        Ok(value.map(|guard| guard.value()))
    }

    fn write_age(&self, id: Uuid, age: u32) -> Result<(), DurabilityError> {
        let write_txn = self.db.begin_write().map_err(engine_err)?;
        {
            let mut table = write_txn.open_table(AGES_TABLE).map_err(engine_err)?;
            table.insert(id.as_u128(), age).map_err(engine_err)?;
        }
        write_txn.commit().map_err(engine_err)?;
        Ok(())
    }

    /// Build fresh indexes from `records`/`edges`, create a new `redb`
    /// database at `path`, and insert every record's starting age in one
    /// write transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, or [`DurabilityError::Engine`] if `redb` fails to create
    /// the database or commit the initial insert transaction.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let indexes = build_indexes(&records, edges);

        let db = Database::create(path).map_err(engine_err)?;
        let write_txn = db.begin_write().map_err(engine_err)?;
        {
            let mut table = write_txn.open_table(AGES_TABLE).map_err(engine_err)?;
            for record in &records {
                table
                    .insert(record.id.as_u128(), record.age)
                    .map_err(engine_err)?;
            }
        }
        write_txn.commit().map_err(engine_err)?;

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            db,
        })
    }

    /// Rebuild indexes from the externally-supplied `records`/`edges`
    /// (their `age` fields are ignored — the `redb` database is the
    /// source of truth for ages) and open the existing database at
    /// `path`. The "load/replay/startup" path this variant's benchmark
    /// measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Engine`] if `path` doesn't exist or
    /// isn't a valid `redb` database.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let indexes = build_indexes(&records, edges);
        let db = Database::open(path).map_err(engine_err)?;
        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            db,
        })
    }
}

impl DogStore for RedbStore {
    fn get(&self, id: Uuid) -> Option<DogRecord> {
        let record = self.records.get(&id)?;
        let age = self.read_age(id).ok().flatten()?;
        Some(DogRecord::new(record.id, record.breed.clone(), age))
    }

    fn scan_ages(&self) -> Vec<u32> {
        self.records
            .keys()
            .filter_map(|&id| self.read_age(id).ok().flatten())
            .collect()
    }

    /// One `redb` write transaction per call — every write is a real,
    /// individually committed, ACID transaction, not batched. This is
    /// what the "least code" thin-integration choice costs: no attempt
    /// to batch or amortize transaction overhead across calls, unlike a
    /// production `redb` consumer might.
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError> {
        if !self.records.contains_key(&id) {
            return Err(StoreError::NotFound(id));
        }
        self.write_age(id, age)?;
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
        let dir = crate::bench_support::fresh_temp_dir("redb_basic").unwrap();
        let path = dir.join("store.redb");
        let mut store = RedbStore::create(sample_records(), sample_edges(), &path).unwrap();

        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 3);
        store.update_age(Uuid::from_u128(1), 42).unwrap();
        assert_eq!(store.get(Uuid::from_u128(1)).unwrap().age, 42);

        assert_eq!(
            store.update_age(Uuid::from_u128(99), 1),
            Err(StoreError::NotFound(Uuid::from_u128(99)))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip correctness: write ages (each a committed transaction),
    /// reopen (fresh `RedbStore`, same records/edges, same path) — the
    /// reopened store must see every committed write, no separate flush
    /// step needed since `redb` commits are already durable.
    #[test]
    fn reopen_sees_every_committed_write() {
        let dir = crate::bench_support::fresh_temp_dir("redb_roundtrip").unwrap();
        let path = dir.join("store.redb");

        {
            let mut store = RedbStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.update_age(Uuid::from_u128(1), 77).unwrap();
            store.update_age(Uuid::from_u128(3), 12).unwrap();
        }

        let reopened = RedbStore::open(sample_records(), sample_edges(), &path).unwrap();
        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 77);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 5);
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_breed_and_neighbors_work() {
        let dir = crate::bench_support::fresh_temp_dir("redb_indexes").unwrap();
        let path = dir.join("store.redb");
        let store = RedbStore::create(sample_records(), sample_edges(), &path).unwrap();
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
