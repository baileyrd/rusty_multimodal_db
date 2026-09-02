# ADR-0006: Tier 2 durability — three alternate architectures (mmap, LSM-style, `redb`), each scoped to the mutable `age` field only

- Status: Accepted
- Date: 2026-08-25
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0005-wal-snapshot-hybrid-durability.md`, `STORAGE-009`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

ADR-0005 covers the WAL/snapshot/hybrid family — five variants built and
benchmarked at full rigor. The same durability task also asked for three
structurally different architectures, explicitly at a lighter rigor:
"proof-of-concept... comparable numbers, not production-hardened," with
LSM-tree compaction specifically named as a known rabbit hole to scope
down rather than either silently under-deliver or let balloon into its
own multi-day project. Each of the three (memory-mapped storage,
LSM-tree-style storage, an off-the-shelf embedded engine) represents a
genuinely different point in the durability design space from anything in
ADR-0005 — not a variation on WAL-or-snapshot — and each raised its own
scope question:

- **mmap**: a real mmap-based store would map full records into a
  fixed-layout region, but this crate's `breed: String` is
  variable-length, which would require either a string-heap-with-offsets
  scheme or capping breed length (a record-shape change explicitly
  outside this task's authority).
- **LSM-tree**: a real LSM tree compacts old flushed files; building that
  is a substantial project in its own right, and the task named it
  explicitly as a rabbit hole to avoid falling into.
- **Embedded engine**: `sled` and `redb` are the two realistic pure-Rust
  choices, with different maturity/durability tradeoffs to weigh.

## Decision drivers

- **"Least code, not production code" for this tier.** The task's own
  framing for the embedded-engine variant — "should be the least code of
  any variant, since the crate owns durability itself" — applies in
  spirit to all three: comparable, honest numbers are the bar, not a
  fully general implementation.
- **A scope-down must be stated plainly, not discovered later.** Every
  Tier 2 variant's module doc explains exactly what was cut and why,
  rather than presenting a narrower implementation as if it were the full
  thing.
- **Compaction is out of scope, explicitly, not silently.** Per the
  task's own instruction: if LSM turns into a multi-day project, scope it
  down and say so in Open Questions rather than under- or over-building.
- **Crash-safety maturity matters more here than usual.** This whole
  section exists to measure genuine crash-recoverability, not just raw
  speed — a durability library with known crash-safety caveats would
  undermine the comparison it's being benchmarked as part of.

## Considered options

### mmap scope

1. **Map the full record (id + breed + age) into a fixed-layout region.**
   Rejected — needs either a string-heap-with-offsets file format (a
   real, non-trivial design of its own) or capping `breed`'s length
   (an unapproved record-shape change).
2. **Map only the mutable `age` field**, as a flat `[u32]` array indexed
   by position — exactly `CanonicalCachedStore`'s own `age_cache` design,
   backed by `MmapMut` instead of `Vec<u32>`; records/edges rebuilt in
   memory from externally-supplied input at open time, the same
   "base dataset supplied externally" convention every durability variant
   in this crate follows. Chosen — durably persists exactly the one field
   that changes, honestly scoped as a "minimal POC," not a general
   record store.

### LSM-tree scope

1. **Full LSM tree with compaction/merge.** Rejected outright per the
   task's explicit instruction — this is the named rabbit hole.
2. **Memtable + WAL + periodic flush to immutable sorted files, no
   compaction, flagged as future work.** Chosen. `get` checks the
   memtable, then flushed files newest-to-oldest, then the base dataset —
   the LSM read-path shape without the maintenance machinery that makes a
   production LSM tree actually bounded over a long lifetime. The missing
   piece (old, superseded values never reclaimed; every flush adds one
   more file every future read must check) is named directly in the
   module's own docs and in `RESULTS.md`'s open questions, not discovered
   by a future reader.

### Embedded engine choice

1. **`sled`.** Rejected — long-standing "still beta" status with a
   documented history of crash-safety caveats depending on version, a
   real concern for a benchmark whose whole point is measuring genuine
   crash-recoverability, not just speed.
2. **`redb`.** Chosen — built around explicit ACID write transactions
   from the start (its own on-disk B-tree with MVCC), pure Rust with no C
   dependency to build or vendor, no equivalent crash-safety caveat.

### Embedded engine's own scope-down (once `redb` was chosen)

1. **Port `CanonicalCachedState`'s full shape into `redb` tables**
   (records, breed index, adjacency index, ages, all as tables).
   Rejected — this is exactly the kind of "not production code" gold-plating
   Tier 2 is meant to avoid; it would also duplicate work `redb` doesn't
   need to do, since the derived indexes are cheap to rebuild in memory
   regardless of which engine holds the durable state.
2. **`redb` holds only the mutable `age` field**, keyed by UUID; records'
   immutable fields and derived indexes rebuilt in memory at `open`/
   `create` time, mirroring the mmap variant's identical reasoning.
   Chosen — keeps the actual `redb` integration to one table, one
   primitive value type, a handful of transactions.

## Decision

- `src/durability/mmap_store.rs` (`MmapAgeStore`): `age` only, backed by
  `memmap2::MmapMut` over a flat `[u32]` file; `unsafe { MmapMut::map_mut }`
  with an explicit SAFETY comment justifying the single-process
  exclusive-access assumption; `flush()` (`msync`) is the durability-forcing
  operation, the mmap analogue of every other variant's `checkpoint`.
- `src/durability/lsm_store.rs` (`LsmStore`): `BTreeMap<Uuid, u32>`
  memtable of age overrides + a WAL (reusing ADR-0005's shared
  `WalEntry`/`append_wal_entry`/`read_wal_entries` format) for whatever's
  unflushed; `flush()` serializes the current memtable to a new numbered
  `sst_N.bin` file, clears the memtable, starts a fresh WAL. No
  compaction — flagged in the module's own docs, in `RESULTS.md`'s
  Durability section, and in its open questions, not silently absent.
- `src/durability/embedded_store.rs` (`RedbStore`): one `redb` table
  (`TableDefinition<u128, u32>`, UUID → age), one write transaction per
  `update_age` call (individually committed, not batched — the explicit
  cost of the "least code" choice), records/breed index/adjacency index
  rebuilt in memory at `open`/`create`.
- New dependencies, flagged explicitly, Tier 2 only: `memmap2` (variant
  6), `redb` (variant 8) — both justified inline in `Cargo.toml`, in
  addition to ADR-0005's `serde`/`bincode` (shared by all eight variants).
- All three implement `DogStore`, tested (round-trip-across-reopen
  correctness — the same standard ADR-0005's variants are held to — plus,
  for LSM specifically, the newest-flushed-generation-wins-across-multiple-flushes
  property, its own defining correctness behavior) and benchmarked at all
  three dataset sizes for whichever of per-write/checkpoint/load costs
  actually apply to each (`redb` has no separate checkpoint operation to
  benchmark — every `update_age` is already its own committed
  transaction — so it's compile-time excluded from that one metric via
  `benches/durability.rs`'s local `Checkpointable` trait, per
  `STORAGE-009`).

## Consequences

### Positive

- Each variant's scope-down is a documented, deliberate decision with a
  stated reason, not a discovered gap — a reader of `mmap_store.rs`,
  `lsm_store.rs`, or `embedded_store.rs`'s module docs sees exactly what
  was cut and why before reading a line of implementation.
- Real numbers validate the "least code" framing rather than just
  asserting it: `RESULTS.md`'s Durability section shows `redb` matching
  WAL-fsync's per-write cost and zero-loss-window guarantee with no
  hand-written WAL/checkpoint logic at all.
- mmap's ages-only scope turned out to be the standout result of the
  entire durability pass, not just an acceptable compromise — cheaper per
  write than the non-durable in-memory baseline itself, and the cheapest
  explicit durability-forcing step of any variant by two to three orders
  of magnitude (see `RESULTS.md`). The scope-down that looked like the
  most restrictive cut on paper produced the best numbers.
- LSM's explicit no-compaction flag means its comparatively good numbers
  in this pass are reported with an honest asterisk (see `RESULTS.md`'s
  recommendation and open questions) rather than implicitly endorsed as
  production-ready.

### Negative / tradeoffs

- None of the three Tier 2 variants persist records'/edges' own
  durability — only `age`. This is explicitly not a general-purpose
  durable store; it fits this crate's specific "one field mutates,
  everything else is static" record shape and would need real redesign
  (the same fixed-layout/string-heap problem mmap's scope-down exists to
  avoid) to cover a record shape where more than one field changes.
- LSM's no-compaction gap is real, not just theoretical: every flush adds
  one more file every future `get`/`scan_ages` must check (unbounded read
  amplification over a long-running store's lifetime), and a repeatedly-
  updated key leaves every old value on disk forever. This benchmark's
  fixed-write-volume, single-flush-per-checkpoint pattern doesn't exercise
  that cost — see `RESULTS.md`'s open questions.
- `redb`'s per-write cost (one committed transaction per call, no
  batching) is the honest cost of "least code" — a production consumer
  wanting cheaper writes would need to batch multiple updates into one
  transaction, which this variant deliberately doesn't do.
- Three more independent on-disk formats/dependencies to maintain,
  beyond ADR-0005's five — `RESULTS.md`'s explicit recommendation exists
  so this tier doesn't default to "keep everything forever" either.

## Validation and revisit triggers

- Validated by: `src/durability/{mmap_store,lsm_store,embedded_store}.rs`'s
  own unit tests (flush-then-reopen round-trip for mmap;
  newest-generation-wins-across-multiple-flushes and
  reopen-recovers-flushed-and-unflushed-writes for LSM;
  reopen-sees-every-committed-write for `redb`) and
  `benches/durability.rs`'s per-write/load costs for all three, plus
  checkpoint/flush cost for mmap and LSM (`redb` excluded from that one
  metric by design — see Decision above).
- Revisit if: a future record shape needs more than one mutable field
  persisted — at that point mmap's and `redb`'s ages-only scope-down
  would need real redesign (the string-heap/fixed-layout problem this ADR
  chose not to solve), not just an incremental extension. *Triggered
  by `Order` (`GENERIC-SCHEMA-DESIGN` §4.2) and answered for the generic
  library's mmap store by `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`
  / `ADR-0020` (Accepted): a per-field `MmapScanned` layer over the
  existing slot format, fixed-width fields only; `redb`'s scope-down and
  variable-width fields remain as stated here.*
- Revisit if: LSM compaction becomes a real requirement — e.g. a
  long-running-store benchmark that actually accumulates enough flushed
  generations to show the unbounded-read-amplification cost this pass's
  fixed write volume doesn't surface (see `RESULTS.md`'s open questions).
  At that point compaction is its own scoped follow-up, not a
  retrofit onto this pass's `LsmStore`.
- Revisit if: `redb`'s per-write transaction-per-call cost becomes a
  bottleneck at a write volume this pass didn't test — batching multiple
  `update_age` calls into one `redb` transaction is the natural next step,
  not a redesign.
- **Addendum, from a later pass (`PRODUCTION-DEFAULT`'s diagnosis round)**:
  this ADR's own benchmark (`benches/durability.rs`) only ever measured
  `MmapAgeStore`'s per-write/checkpoint/load costs, never `get`/
  `scan_ages`/`same_breed`/`neighbors`. Once `ProductionStore`
  (`src/production.rs`) benchmarked those for the first time,
  `scan_ages`'s per-position indexed-read loop (`read_age` called once per
  position, four individually-bounds-checked byte reads each) turned out
  to be 25–32× slower than `CanonicalCachedStore`'s packed-`Vec` clone —
  a real, avoidable inefficiency, not an inherent durability cost, fixed
  via a safe `chunks_exact(4)` bulk read (12–17× faster, no `unsafe`, no
  new dependency). This ADR's own `update_age` finding (mmap cheaper than
  the non-durable baseline) is unaffected and reconfirmed directly in that
  same diagnosis. See `RESULTS.md`'s `## Production recommendation`
  section for the full investigation and corrected numbers.
