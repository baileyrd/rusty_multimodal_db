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
//! memory-mapped region**. `update_age` writes straight into mapped
//! memory; the OS's own page-cache write-back handles getting it to disk,
//! with [`Self::flush`] available to force that (via `msync`) when a
//! caller wants a durability guarantee before moving on — the direct mmap
//! analogue of every other variant's `checkpoint`. Records/edges
//! (immutable after construction) are supplied externally at `open`/
//! `create` time, same convention as the WAL variants — this variant
//! durably persists exactly the field that can change, and nothing else,
//! which is the most honest "minimal POC" reading of what an mmap-backed
//! store's value proposition actually is for this crate's fixed
//! three-field record.
//!
//! # Persisted values are keyed by record identity, not array position
//!
//! **Motivated by `crate::generic::mmap_store::GenericMmapStore`'s own
//! record-identity-keying + schema-version-header fix** (see that
//! module's own doc comment for the original diagnosis) — this store had
//! the identical bug, just never independently diagnosed: `open`
//! rebuilds `position_index` fresh from whatever order the caller-
//! supplied `records`/`edges` happen to arrive in on that specific call,
//! and (before this fix) trusted that array position N in the file still
//! held position N's age from `create` time. Nothing in the file recorded
//! which record an age belonged to. If a caller ever supplied `records`
//! in a different order between `create` and a later `open`, ages got
//! silently attributed to the wrong dog: no error, no panic, just wrong
//! data under a real id. No caller in this crate's own test/bench suite
//! ever triggered this (every existing call site supplies the same
//! generator output, in the same order, at both `create` and `open`), but
//! nothing about `ProductionStore`'s own public contract ever required
//! that — it's a real, latent correctness gap this closes.
//!
//! # Columnar, not row-oriented — a deliberate departure from `GenericMmapStore`
//!
//! A first pass at this fix ported `GenericMmapStore`'s exact on-disk
//! shape: one interleaved `[id: 16 bytes][age: 4 bytes][COMMITTED marker: 1
//! byte]` slot per record. It worked, but `scan_ages`'s existing
//! `chunks_exact(4)` bulk read (see that method's own doc comment for the
//! original diagnosis — a 25-32× win over a per-position loop) depends on
//! the mapped region being *exactly* a flat, homogeneous `[u32]` array.
//! Interleaved slots break that: a `chunks_exact(21)` read touches 21
//! bytes to extract 4, over 5× the memory traffic — measured at 2.2-6.1×
//! slower across 1K-1M records (see `RESULTS.md`), reversing this
//! crate's own headline external-database finding (`ProductionStore`
//! beating DuckDB on `scan_ages` at 1M, not the other way around). That
//! regression was real and unacceptable for this crate's flagship
//! read-side workload, so this format is columnar instead: three
//! separate, contiguous regions after the header — every id, then every
//! age, then every commit marker, each its own flat array, in the same
//! record order. `scan_ages`'s fast path goes back to a pure
//! `chunks_exact(4)` over *only* the ages region — see that method's own
//! doc comment for the recovered numbers.
//!
//! Layout, in file order: `HEADER_LEN`-byte header (8-byte `MAGIC`,
//! then `SCHEMA_VERSION` as little-endian `u32`), then `N` ids (16
//! bytes each), then `N` ages (4 bytes each, little-endian), then `N`
//! commit markers (1 byte each) — where `N` is derived from the file's
//! total length (`(len - HEADER_LEN) / 21`), not stored separately, since
//! the three regions' combined per-record cost is fixed at 21 bytes
//! regardless of how they're arranged. [`Self::open`] reads every
//! *committed* `(id, age)` pair (marker byte `COMMITTED`, skipping
//! anything else — a slot never reached, or a crash mid-write, both read
//! the same safe way: absent) into an `id -> (position, age)` map, then
//! reconciles the caller-supplied `records` against it **by id** — same
//! reconciliation logic `GenericMmapStore` established, just against a
//! different physical layout.
//!
//! # The real cost of going columnar: appends can't stay cheap and in-place
//!
//! A row-oriented slot can grow by appending one fixed-width chunk to the
//! end of the file (`GenericMmapStore`'s own `O_APPEND` approach). A
//! columnar layout can't: adding one record means inserting bytes into
//! *three* separate, non-adjacent regions, which isn't an append to any
//! of them. Rather than reintroduce row-orientation just for that one
//! case (which would give up the whole point of this redesign), a record
//! with no persisted slot triggers a **full rewrite**: every record in
//! `records` (ages sourced from the persisted map where available, else
//! from the record's own `age` field — exactly `create`'s own seeding
//! logic) is written fresh, columnar, in `records`' own order. Stale
//! persisted ids (in the file but not in `records`) are dropped in the
//! same pass, *whenever a rewrite happens to be triggered anyway* — a
//! deliberate difference from `GenericMmapStore`'s own "stale bytes stay,
//! inert, no compaction" choice, but only realized incidentally: a
//! reopen that only *removes* records, with nothing new to add, never
//! triggers a rewrite at all (there's no missing entry to force one), so
//! stale bytes from a drop-only reopen linger exactly like
//! `GenericMmapStore`'s do, until some later reopen adds a record and
//! rewrites everything at once. This format is not "compact-on-every-
//! stale-drop" — only "compact opportunistically when a rewrite is
//! already happening for another reason."
//!
//! **This path is never exercised by any benchmark or existing call site
//! in this crate** — every one of them supplies the identical record set
//! at `create` and every later `open` (see `bench_support::build_dataset`
//! and every test's own fixed sample data). It exists purely for
//! correctness on a caller-supplied record set that genuinely changed
//! between reopens, and its cost (an O(N) rewrite) was deliberately not
//! optimized — see this module's own tests
//! (`a_new_record_since_the_last_write_is_durable_from_this_reopen_forward`,
//! `both_mismatch_cases_in_one_reopen`) for the correctness coverage, not
//! a benchmark, since none was warranted for a path nothing in this
//! crate's own measured workloads ever takes.
//!
//! **The rewrite itself is crash-safe via write-to-temp-then-rename, not
//! in-place truncation** — truncating `path` directly and rewriting it in
//! place would mean a crash mid-rewrite could leave neither the old nor
//! the new generation intact, a real regression from `GenericMmapStore`'s
//! own append-only approach (which never touches already-durable bytes).
//! Instead: the new generation is written in full to a sibling temp path,
//! `fsync`'d, then moved into place via [`std::fs::rename`] — atomic on
//! the same filesystem on every platform this crate targets. A crash
//! before the rename leaves the original file at `path` completely
//! untouched; a crash after leaves the new, complete file in place. There
//! is never a window where `path` holds a partial generation, which is
//! strictly stronger than the row-oriented append design's own guarantee
//! (safe, but by a different mechanism — an interrupted append leaves one
//! trailing slot correctly read back as absent, not the whole file
//! generation-consistent by construction).
//!
//! **Included here, not attempted as new diagnosis work**: the
//! `COMMITTED`-marker torn-write safety for the in-place `update_age`
//! path is the same mechanism, same reasoning, `GenericMmapStore`'s own
//! crash-safety round already validated (see that module's own doc
//! comment) — this port reuses the argument by structural analogy (same
//! marker byte, same single-instruction-store reasoning for a fixed-width
//! in-place value overwrite), not a fresh empirical fault-injection trial
//! against this specific store. A `MmapAgeStore`-specific harness
//! (mirroring `src/bin/crash_safety_harness.rs`/`multiprocess_harness.rs`,
//! both `Order`/`Customer`-specific today) remains a real, separate,
//! unscoped follow-up if that transfer-by-analogy is ever judged
//! insufficient.

use super::{DurabilityError, PARALLEL_CONSTRUCTION_THRESHOLD};
use crate::record::DogRecord;
use crate::store::{DogStore, StoreError};
use memmap2::MmapMut;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Identifies a file as one [`MmapAgeStore`] wrote — distinct bytes from
/// `GenericMmapStore`'s own `MAGIC`, so pointing either store's `open` at
/// the other's file fails cleanly (`DurabilityError::InvalidMagic`)
/// rather than misreading it. Arbitrary but fixed, ASCII-readable purely
/// for `hexdump`/`xxd` convenience.
const MAGIC: [u8; 8] = *b"DOGMMAP\0";

/// This store's on-disk layout, versioned from its first release.
const SCHEMA_VERSION: u32 = 1;

/// [`MAGIC`] followed by [`SCHEMA_VERSION`] as a little-endian `u32`.
const HEADER_LEN: usize = MAGIC.len() + 4;

/// The value [`MmapAgeStore::is_committed`] looks for in a record's
/// marker byte. Any other byte — including `0`, what a freshly-extended
/// file's zero-filled bytes already are — means "not committed": a
/// marker never reached and one whose write was interrupted by a crash
/// both read the same safe way, absent, not corrupted.
const COMMITTED: u8 = 1;

/// `Uuid`'s raw byte width — this store's id type is fixed (unlike
/// `GenericMmapStore`, generic over `R::Id`), so this is a plain
/// constant, not a trait-derived one.
const ID_WIDTH: usize = 16;

/// `u32`'s raw byte width — this store's persisted value type (`age`) is
/// likewise fixed.
const AGE_WIDTH: usize = 4;

/// Bytes per record across all three columnar regions combined: id, age,
/// and the commit marker. Used to derive a mapped file's record count
/// from its total length — `(len - HEADER_LEN) / RECORD_STRIDE` — since
/// the three regions aren't adjacent per-record the way a single slot
/// width would imply.
const RECORD_STRIDE: usize = ID_WIDTH + AGE_WIDTH + 1;

/// mmap-backed durable store (ages only — see module docs).
pub struct MmapAgeStore {
    records: HashMap<Uuid, DogRecord>,
    breed_index: HashMap<String, Vec<Uuid>>,
    adjacency_index: HashMap<Uuid, Vec<Uuid>>,
    /// `id` -> that id's *current* position in the columnar regions —
    /// built by matching persisted ids against `records`, not by array
    /// index. See module docs for the two mismatch cases this
    /// reconciliation has to decide between.
    position_index: HashMap<Uuid, usize>,
    mmap: MmapMut,
    #[allow(dead_code)] // kept for symmetry with GenericMmapStore's Self; not read again
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
}

/// See `CanonicalCachedState::new`'s doc comment (`src/durability/mod.rs`)
/// for why this splits into a spawned thread building the adjacency index
/// (only touches `edges`) running alongside the breed-index and
/// records-map construction (only touches `records`) — same disjoint-input,
/// no-merge shape, same `std::thread::scope` fix, duplicated here rather
/// than shared per this crate's existing convention of small explicit
/// duplication across structurally similar backends (see `wal_buffered.rs`'s
/// own module docs). Below `PARALLEL_CONSTRUCTION_THRESHOLD` records, falls
/// back to `build_indexes_sequential` — see that constant's own doc comment
/// (`src/durability/mod.rs`) for why and how it was measured. No longer
/// builds `position_index` (identity-keyed reconciliation against the file
/// replaces that — see module docs), only the two indexes derived purely
/// from record/edge content.
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
        for record in records.iter() {
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
    for record in records.iter() {
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
    let records_map = records.iter().cloned().map(|r| (r.id, r)).collect();
    Indexes {
        records: records_map,
        breed_index,
        adjacency_index,
    }
}

impl MmapAgeStore {
    /// Number of records a mapped file of `mmap_len` bytes currently
    /// holds, derived from its total length — see module docs for why no
    /// count is stored separately.
    fn record_count_for(mmap_len: usize) -> usize {
        (mmap_len - HEADER_LEN) / RECORD_STRIDE
    }

    /// Byte offset the ids region starts at — always right after the
    /// header, first of the three columnar regions.
    fn ids_start() -> usize {
        HEADER_LEN
    }

    /// Byte offset the ages region starts at — right after all `record_count`
    /// ids.
    fn ages_start(record_count: usize) -> usize {
        Self::ids_start() + record_count * ID_WIDTH
    }

    /// Byte offset the commit-marker region starts at — right after all
    /// `record_count` ages.
    fn markers_start(record_count: usize) -> usize {
        Self::ages_start(record_count) + record_count * AGE_WIDTH
    }

    fn record_count(&self) -> usize {
        Self::record_count_for(self.mmap.len())
    }

    /// One bounds check on a 4-byte slice, not four separate bounds checks
    /// on individual byte indices — see `scan_ages`'s own doc comment for
    /// why this matters far more there than it does for this single-position
    /// lookup (used by `get`, one call at a time).
    fn read_age(&self, position: usize) -> u32 {
        let start = Self::ages_start(self.record_count()) + position * AGE_WIDTH;
        let bytes: [u8; AGE_WIDTH] = self.mmap[start..start + AGE_WIDTH]
            .try_into()
            .expect("slice taken as start..start + AGE_WIDTH is always exactly AGE_WIDTH bytes");
        u32::from_le_bytes(bytes)
    }

    fn write_age(&mut self, position: usize, age: u32) {
        let record_count = self.record_count();
        let start = Self::ages_start(record_count) + position * AGE_WIDTH;
        self.mmap[start..start + AGE_WIDTH].copy_from_slice(&age.to_le_bytes());
    }

    /// Whether the record at `position` is committed — the one thing
    /// [`Self::open`]'s reconciliation pass trusts before treating a
    /// record's id/age bytes as real data at all.
    fn is_committed(mmap: &MmapMut, record_count: usize, position: usize) -> bool {
        mmap[Self::markers_start(record_count) + position] == COMMITTED
    }

    /// Write the fixed header ([`MAGIC`] + [`SCHEMA_VERSION`]) at the very
    /// start of `buf`.
    fn write_header(buf: &mut [u8]) {
        buf[0..MAGIC.len()].copy_from_slice(&MAGIC);
        buf[MAGIC.len()..HEADER_LEN].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    }

    /// Read and validate the header at the start of `mmap`. Called by
    /// [`Self::open`] before any record data is read; a file that fails
    /// this check has none of its data touched at all.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::InvalidMagic`] if `mmap` is shorter than
    /// [`HEADER_LEN`] or its first [`MAGIC`]`.len()` bytes don't match, or
    /// [`DurabilityError::SchemaVersionMismatch`] if the magic matches but
    /// the recorded version doesn't.
    fn read_header(mmap: &MmapMut) -> Result<(), DurabilityError> {
        if mmap.len() < HEADER_LEN || mmap[0..MAGIC.len()] != MAGIC {
            return Err(DurabilityError::InvalidMagic);
        }
        let found = u32::from_le_bytes(
            mmap[MAGIC.len()..HEADER_LEN]
                .try_into()
                .expect("MAGIC.len()..HEADER_LEN is always exactly 4 bytes"),
        );
        if found != SCHEMA_VERSION {
            return Err(DurabilityError::SchemaVersionMismatch {
                found,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    /// True once every record currently in the file maps to a live entry
    /// in `position_index` — i.e. no persisted record has ever been
    /// dropped between an `open` and the `records` this store was
    /// actually built from. `scan_ages`'s fast bulk-read path is only
    /// valid when this holds — see that method's own doc comment.
    fn is_gapless(&self) -> bool {
        self.position_index.len() == self.record_count()
    }

    /// Build the complete on-disk byte image for `entries` (id, age)
    /// pairs, in that order — header, then the ids region, then the ages
    /// region, then the markers region, every marker [`COMMITTED`] (this
    /// is only ever used to write a fully-formed, immediately-valid
    /// generation, never a partial one). Shared by [`Self::create`] and
    /// [`Self::open`]'s rewrite path so both produce byte-for-byte the
    /// same layout for the same input.
    fn build_image(entries: &[(Uuid, u32)]) -> Vec<u8> {
        let record_count = entries.len();
        let mut buf = vec![0u8; HEADER_LEN + record_count * RECORD_STRIDE];
        Self::write_header(&mut buf);
        let ids_start = Self::ids_start();
        let ages_start = Self::ages_start(record_count);
        let markers_start = Self::markers_start(record_count);
        for (position, (id, age)) in entries.iter().enumerate() {
            let id_at = ids_start + position * ID_WIDTH;
            buf[id_at..id_at + ID_WIDTH].copy_from_slice(id.as_bytes());
            let age_at = ages_start + position * AGE_WIDTH;
            buf[age_at..age_at + AGE_WIDTH].copy_from_slice(&age.to_le_bytes());
            buf[markers_start + position] = COMMITTED;
        }
        buf
    }

    /// Write `entries`' image directly to a freshly created/truncated
    /// `path` and map it — used by [`Self::create`], where there is no
    /// prior generation to protect (the file doesn't exist, or the caller
    /// is deliberately starting over).
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized/written, or the mapping
    /// fails.
    fn write_fresh(entries: &[(Uuid, u32)], path: &Path) -> Result<MmapMut, DurabilityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let image = Self::build_image(entries);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(&image)?;
        file.sync_all()?;
        // SAFETY: this process holds exclusive read/write access to the
        // file at `path` for the lifetime of the mapping; nothing else
        // concurrently truncates or writes to it out from under the
        // mapping, which is the actual UB risk `mmap` carries.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(mmap)
    }

    /// Write `entries`' image to a sibling temp path, `fsync` it, then
    /// atomically rename it over `path` — used by [`Self::open`]'s
    /// rewrite branch, where `path` may already hold a real, durable
    /// prior generation that must never be left half-replaced. See module
    /// docs for why this, not in-place truncation.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if the temp file can't be created,
    /// written, or renamed into place, or the final mapping fails.
    fn write_via_rename(entries: &[(Uuid, u32)], path: &Path) -> Result<MmapMut, DurabilityError> {
        let mut temp_path = path.as_os_str().to_owned();
        temp_path.push(".rewrite-tmp");
        let temp_path = PathBuf::from(temp_path);

        let image = Self::build_image(entries);
        {
            let mut temp_file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)?;
            temp_file.write_all(&image)?;
            temp_file.sync_all()?;
        }
        std::fs::rename(&temp_path, path)?;

        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // SAFETY: see `write_fresh` — same single-process exclusive-access
        // assumption, now against the file the rename just put at `path`.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(mmap)
    }

    /// Build fresh indexes from `records`/`edges`, write a new columnar
    /// file at `path` seeded with each record's starting age, and
    /// memory-map it.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path`'s parent can't be
    /// created, the file can't be created/sized/written, or the mapping
    /// fails.
    pub fn create(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let indexes = build_indexes(&records, edges);
        let entries: Vec<(Uuid, u32)> = records.iter().map(|r| (r.id, r.age)).collect();
        let position_index: HashMap<Uuid, usize> = entries
            .iter()
            .enumerate()
            .map(|(position, (id, _))| (*id, position))
            .collect();
        let mmap = Self::write_fresh(&entries, path)?;

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            position_index,
            mmap,
            path: path.to_path_buf(),
        })
    }

    /// Reopen `path`, reconciling its persisted `(id, age)` pairs against
    /// the externally-supplied `records`/`edges` **by id**, not by array
    /// position — see module docs for the two mismatch cases this
    /// reconciliation decides between. If every record in `records`
    /// already has a persisted entry, the existing file is mapped
    /// directly (the common, benchmarked case — no rewrite). If any
    /// record is new since the last write, the whole file is rewritten
    /// (crash-safely, via `Self::write_via_rename`) — see module docs
    /// for why a columnar layout can't append cheaply the way a
    /// row-oriented one can. The "load/replay/startup" path this
    /// variant's benchmark measures.
    ///
    /// # Errors
    ///
    /// Returns [`DurabilityError::Io`] if `path` doesn't exist, can't be
    /// mapped, or a rewrite can't be completed; [`DurabilityError::InvalidMagic`]
    /// or [`DurabilityError::SchemaVersionMismatch`] if the file's header
    /// doesn't check out. Either header failure returns before any record
    /// data is read.
    pub fn open(
        records: Vec<DogRecord>,
        edges: Vec<(Uuid, Uuid)>,
        path: &Path,
    ) -> Result<Self, DurabilityError> {
        let indexes = build_indexes(&records, edges);

        let file = OpenOptions::new().read(true).write(true).open(path)?;

        // First pass: check the header, then (only once it checks out)
        // read every *committed* (id, age) pair, keyed by id. A record
        // whose marker byte isn't `COMMITTED` is skipped entirely — it
        // falls through reconciliation below exactly as if it had never
        // been persisted, rather than handing a torn id/age pair to a
        // caller.
        let persisted: HashMap<Uuid, (usize, u32)> = {
            // SAFETY: see `write_fresh` — same single-process
            // exclusive-access assumption for the *mapping* itself. This
            // mapping is read, then dropped before any rewrite below.
            let mmap = unsafe { MmapMut::map_mut(&file)? };
            Self::read_header(&mmap)?;
            let record_count = Self::record_count_for(mmap.len());
            let ids_start = Self::ids_start();
            let ages_start = Self::ages_start(record_count);
            let mut persisted = HashMap::with_capacity(record_count);
            for position in 0..record_count {
                if !Self::is_committed(&mmap, record_count, position) {
                    continue;
                }
                let id_at = ids_start + position * ID_WIDTH;
                let id = Uuid::from_bytes(
                    mmap[id_at..id_at + ID_WIDTH]
                        .try_into()
                        .expect("id field is always exactly ID_WIDTH bytes"),
                );
                let age_at = ages_start + position * AGE_WIDTH;
                let age = u32::from_le_bytes(
                    mmap[age_at..age_at + AGE_WIDTH]
                        .try_into()
                        .expect("age field is always exactly AGE_WIDTH bytes"),
                );
                persisted.insert(id, (position, age));
            }
            persisted
        };

        // Reconcile: every record in `records` either already has a
        // persisted entry (its age is whatever the file holds — the
        // caller-supplied `records`' own `age` field is ignored for those,
        // same convention `create` never needed to establish since it has
        // no prior generation) or doesn't (seeded from its own `age`
        // field, exactly `create`'s behavior for a brand new record). A
        // persisted id with no matching record in `records` is simply
        // dropped — see module docs' "stale" case and why this format
        // compacts them out rather than leaving them inert.
        let missing = records.iter().any(|r| !persisted.contains_key(&r.id));

        // `position_index` must reflect where each id's age *actually
        // lives in the mapped file* — that's `records`' own order only
        // when a rewrite just wrote the file in exactly that order; when
        // no rewrite happens, the file's positions are whatever the
        // *original* generation assigned (captured in `persisted`), which
        // has no relationship to the order `records` happens to arrive in
        // on this call. Building `position_index` from `records`'
        // enumeration order unconditionally would silently reintroduce
        // this exact module's own bug on every no-rewrite reopen.
        let (mmap, position_index) = if missing {
            let entries: Vec<(Uuid, u32)> = records
                .iter()
                .map(|r| {
                    let age = persisted.get(&r.id).map_or(r.age, |&(_, age)| age);
                    (r.id, age)
                })
                .collect();
            // Drop the read-only mapping above before rewriting the file
            // it was backed by.
            drop(file);
            let mmap = Self::write_via_rename(&entries, path)?;
            let position_index = entries
                .iter()
                .enumerate()
                .map(|(position, (id, _))| (*id, position))
                .collect();
            (mmap, position_index)
        } else {
            // SAFETY: see `write_fresh`.
            let mmap = unsafe { MmapMut::map_mut(&file)? };
            let position_index = records.iter().map(|r| (r.id, persisted[&r.id].0)).collect();
            (mmap, position_index)
        };

        Ok(Self {
            records: indexes.records,
            breed_index: indexes.breed_index,
            adjacency_index: indexes.adjacency_index,
            position_index,
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

    /// Fast path (every record in the file live — `Self::is_gapless`):
    /// reads via `chunks_exact(4)` directly over just the ages region —
    /// back to the original flat bulk read this method's very first
    /// version established (25-32× faster than a naive per-position
    /// loop), now that ages live in their own contiguous region again
    /// rather than interleaved with id/marker bytes per record — see
    /// module docs for why an interleaved, row-oriented identity-keyed
    /// format cost 2.2-6.1× here and why this columnar format doesn't.
    /// Falls back to a sorted `position_index` walk (mirrors
    /// `GenericMmapStore::scan`'s identical fallback) whenever the file
    /// holds any record no longer in `records` — a real, reachable case,
    /// not just defense-in-depth: stale ids are only compacted out on an
    /// `open` that *also* has a genuinely new record to add (see module
    /// docs — a rewrite is what removes them, and nothing else does). A
    /// reopen that only *drops* records (no new ones) never triggers a
    /// rewrite at all, so the file — and `is_gapless`'s own `record_count`
    /// — keeps the dropped ids' bytes around, uncompacted, until some
    /// later reopen adds a new record and rewrites everything at once.
    fn scan_ages(&self) -> Vec<u32> {
        let record_count = self.record_count();
        if self.is_gapless() {
            let ages_start = Self::ages_start(record_count);
            let ages_end = ages_start + record_count * AGE_WIDTH;
            return self.mmap[ages_start..ages_end]
                .chunks_exact(AGE_WIDTH)
                .map(|chunk| {
                    u32::from_le_bytes(
                        chunk.try_into().expect(
                            "chunks_exact(AGE_WIDTH) always yields exactly AGE_WIDTH bytes",
                        ),
                    )
                })
                .collect();
        }
        let mut positions: Vec<usize> = self.position_index.values().copied().collect();
        positions.sort_unstable();
        positions
            .into_iter()
            .map(|position| self.read_age(position))
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

    /// The exact bug this closes: `records` supplied to `open` in a
    /// **different order** than at `create` time. Before the fix, ages
    /// would have been silently misattributed by position; after it,
    /// reconciliation is by id, so reordering has no effect at all.
    #[test]
    fn reopening_with_reordered_records_reads_the_correct_age_per_id() {
        let dir = crate::test_support::fresh_temp_dir("mmap_reorder").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut store = MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.update_age(Uuid::from_u128(1), 100).unwrap();
            store.update_age(Uuid::from_u128(2), 200).unwrap();
            store.update_age(Uuid::from_u128(3), 300).unwrap();
            store.flush().unwrap();
        }

        // `sample_records()` returns ids 1, 2, 3 in that order — reverse
        // it, so id 1 (originally position 0) now sits at position 2, id 3
        // (originally position 2) now sits at position 0.
        let mut reordered = sample_records();
        reordered.reverse();
        let reopened = MmapAgeStore::open(reordered, sample_edges(), &path).unwrap();

        assert_eq!(reopened.get(Uuid::from_u128(1)).unwrap().age, 100);
        assert_eq!(reopened.get(Uuid::from_u128(2)).unwrap().age, 200);
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 300);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A record present at `create` time but omitted from `open`'s
    /// `records` (the "stale id" case): must be completely invisible —
    /// not reachable via `get`, and not present in `scan_ages`'s output.
    /// This format compacts stale entries out during the reconciliation
    /// rewrite that dropping a record always triggers here (see module
    /// docs), rather than leaving inert bytes behind.
    #[test]
    fn a_record_missing_from_reopen_is_invisible_not_erroring() {
        let dir = crate::test_support::fresh_temp_dir("mmap_stale").unwrap();
        let path = dir.join("ages.mmap");

        {
            let store = MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();
            store.flush().unwrap();
        }

        let mut without_id_2 = sample_records();
        without_id_2.retain(|r| r.id != Uuid::from_u128(2));
        let reopened = MmapAgeStore::open(without_id_2, Vec::new(), &path).unwrap();

        assert_eq!(reopened.get(Uuid::from_u128(2)), None);
        let mut ages = reopened.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 3]); // ids 3 and 1's ages from sample_records()
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A record present in `open`'s `records` but never persisted before
    /// (the "new record" case): it must be durable from this reopen
    /// forward — visible immediately, and still visible after a second
    /// reopen with no further changes. Exercises the rewrite-via-rename
    /// path (see module docs).
    #[test]
    fn a_new_record_since_the_last_write_is_durable_from_this_reopen_forward() {
        let dir = crate::test_support::fresh_temp_dir("mmap_new_record").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut initial = sample_records();
            initial.retain(|r| r.id != Uuid::from_u128(3));
            let store = MmapAgeStore::create(initial, Vec::new(), &path).unwrap();
            store.flush().unwrap();
        }

        {
            let reopened = MmapAgeStore::open(sample_records(), sample_edges(), &path).unwrap();
            assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 2);
        }

        // A second reopen, with no changes since, must still see it —
        // and must not need another rewrite (every record already has a
        // persisted entry at this point).
        let reopened_again = MmapAgeStore::open(sample_records(), sample_edges(), &path).unwrap();
        assert_eq!(reopened_again.get(Uuid::from_u128(3)).unwrap().age, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Both mismatch cases in the same reopen: one stale id dropped, one
    /// new id added, at once — a single rewrite has to handle both
    /// simultaneously.
    #[test]
    fn both_mismatch_cases_in_one_reopen() {
        let dir = crate::test_support::fresh_temp_dir("mmap_both_mismatch").unwrap();
        let path = dir.join("ages.mmap");

        {
            let mut initial = sample_records();
            initial.retain(|r| r.id != Uuid::from_u128(3));
            let store = MmapAgeStore::create(initial, Vec::new(), &path).unwrap();
            store.flush().unwrap();
        }

        let mut new_records = sample_records();
        new_records.retain(|r| r.id != Uuid::from_u128(1));
        let reopened = MmapAgeStore::open(new_records, Vec::new(), &path).unwrap();

        assert_eq!(reopened.get(Uuid::from_u128(1)), None); // dropped
        assert_eq!(reopened.get(Uuid::from_u128(3)).unwrap().age, 2); // newly appended
        let mut ages = reopened.scan_ages();
        ages.sort_unstable();
        assert_eq!(ages, vec![2, 5]); // ids 3 and 2 only
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Baseline: a file written by this build opens cleanly — the header
    /// check itself must not break the ordinary case.
    #[test]
    fn opening_a_file_at_the_current_schema_version_succeeds() {
        let dir = crate::test_support::fresh_temp_dir("mmap_header_ok").unwrap();
        let path = dir.join("ages.mmap");
        MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();
        assert!(
            MmapAgeStore::open(sample_records(), sample_edges(), &path).is_ok(),
            "a file written at the current schema version must open cleanly"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file whose magic matches but whose recorded version doesn't —
    /// the shape a future, incompatible layout would produce — must fail
    /// as `SchemaVersionMismatch`, not silently misread or panic.
    #[test]
    fn opening_a_file_with_a_mismatched_schema_version_fails_distinctly() {
        let dir = crate::test_support::fresh_temp_dir("mmap_header_version").unwrap();
        let path = dir.join("ages.mmap");
        MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();

        let bogus_version: u32 = SCHEMA_VERSION.wrapping_add(1);
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            mmap[MAGIC.len()..HEADER_LEN].copy_from_slice(&bogus_version.to_le_bytes());
            mmap.flush().unwrap();
        }

        let result = MmapAgeStore::open(sample_records(), sample_edges(), &path);
        match result.err() {
            Some(DurabilityError::SchemaVersionMismatch { found, expected }) => {
                assert_eq!(found, bogus_version);
                assert_eq!(expected, SCHEMA_VERSION);
            }
            other => panic!("expected SchemaVersionMismatch, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Corrupting only the magic bytes (not the version) must fail as
    /// `InvalidMagic`, distinctly from a version mismatch.
    #[test]
    fn opening_a_file_with_the_wrong_magic_number_fails_distinctly_from_a_version_mismatch() {
        let dir = crate::test_support::fresh_temp_dir("mmap_header_magic").unwrap();
        let path = dir.join("ages.mmap");
        MmapAgeStore::create(sample_records(), sample_edges(), &path).unwrap();

        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut mmap = unsafe { MmapMut::map_mut(&file).unwrap() };
            mmap[0..MAGIC.len()].copy_from_slice(&[0xFFu8; MAGIC.len()]);
            mmap.flush().unwrap();
        }

        let result = MmapAgeStore::open(sample_records(), sample_edges(), &path);
        assert!(
            matches!(result, Err(DurabilityError::InvalidMagic)),
            "expected InvalidMagic, got {:?}",
            result.err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file shorter than the header itself — must fail cleanly as
    /// `InvalidMagic`, not panic on an out-of-bounds slice.
    #[test]
    fn a_file_shorter_than_the_header_fails_as_invalid_magic_not_a_panic() {
        let dir = crate::test_support::fresh_temp_dir("mmap_header_short").unwrap();
        let path = dir.join("ages.mmap");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, [0u8; 4]).unwrap(); // shorter than HEADER_LEN

        let result = MmapAgeStore::open(sample_records(), sample_edges(), &path);
        assert!(
            matches!(result, Err(DurabilityError::InvalidMagic)),
            "expected InvalidMagic, got {:?}",
            result.err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
