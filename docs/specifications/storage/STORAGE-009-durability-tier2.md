# STORAGE-009 — Durability, Tier 2: alternate architectures (mmap, LSM-style, `redb`) for `CanonicalCachedStore`

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-005`, `STORAGE-008`
- Supersedes: none

## Purpose and scope

Alongside `STORAGE-008`'s WAL/snapshot/hybrid family, this spec covers
three structurally different durability architectures for
`CanonicalCachedStore`, deliberately at a lighter rigor than `STORAGE-008`:
proof-of-concept implementations producing comparable benchmark numbers,
explicitly not production-hardened. See ADR-0006 for the design decisions
(including each variant's specific scope-down and why) and `RESULTS.md`'s
`## Durability` section for the numbers and recommendation.

## Non-goals

- Not production-ready implementations — "comparable numbers, not
  production code" is the explicit bar for this tier, per the task that
  motivated it.
- Not a general-purpose durable record store — all three variants persist
  only the mutable `age` field; records'/edges' own durability is out of
  scope for all three (see ADR-0006's Context on why: a real full-record
  mmap format needs a string-heap/fixed-layout design this task doesn't
  have authority to build, given `breed: String`'s variable length).
- Not LSM-tree compaction — explicitly named by the task as a known
  rabbit hole. `LsmStore` has no merge/compaction step; every flush adds
  one more file every future read checks, and superseded values are never
  reclaimed. Flagged in `lsm_store.rs`'s own module docs and in
  `RESULTS.md`'s open questions, not silently absent.
- Not batching multiple writes into one `redb` transaction — `RedbStore`
  commits one transaction per `update_age` call, the direct cost of the
  "least code" choice ADR-0006 made.
- Not benchmarking `RedbStore`'s checkpoint/flush cost — it has no
  separate checkpoint operation (every write is already an independently
  committed transaction), so it's excluded from that one metric at
  compile time via `benches/durability.rs`'s local `Checkpointable`
  trait, per the task's "if a metric doesn't map, don't force it"
  instruction.

## Context and terminology

- **Ages-only scope-down**: the shared design choice across all three
  Tier 2 variants — only `age` (the one field that ever mutates) is
  durably persisted by the variant's own mechanism; `id`/`breed` and the
  derived breed/adjacency indexes are rebuilt in memory from
  externally-supplied `records`/`edges` at `open`/`create` time, the same
  "base dataset supplied externally" convention `STORAGE-008`'s variants
  follow.
- **Memtable/SST** (LSM-specific): `LsmStore`'s in-memory
  `BTreeMap<Uuid, u32>` of unflushed age overrides, and the immutable,
  numbered files (`sst_N.bin`) each `flush()` call produces.
- **Generation** (LSM-specific): the index `N` of a flushed `sst_N.bin`
  file; `LsmStore::open` reads how many exist via a small `generations.bin`
  metadata file.

## Requirements

- `STORAGE-009-FR-001`: **Variant 6 (mmap)** — `MmapAgeStore` maps only
  the `age` field as a flat `[u32]` array (indexed by record position)
  into a `memmap2::MmapMut`-backed file. `update_age` writes directly into
  mapped memory (no syscall on that path). `flush()` calls `msync` (via
  `MmapMut::flush`) — the mmap analogue of every other variant's
  `checkpoint`, and the operation that upgrades "reached the OS page
  cache" to "guaranteed on disk." `create`/`open` use `unsafe {
  MmapMut::map_mut }`, each with an explicit SAFETY comment justifying
  the single-process exclusive-access assumption.
- `STORAGE-009-FR-002`: **Variant 7 (LSM-tree style)** — `LsmStore` holds
  a `BTreeMap<Uuid, u32>` memtable of age overrides plus a WAL (reusing
  `STORAGE-008`'s shared `WalEntry`/`append_wal_entry`/`read_wal_entries`
  format). `get`/`scan_ages` resolve an id's age by checking the
  memtable, then flushed generations newest-to-oldest, then the base
  dataset. `flush()` serializes the current memtable to a new numbered
  `sst_N.bin` file, increments the generation count, clears the memtable,
  and starts a fresh WAL. No compaction (see Non-goals).
- `STORAGE-009-FR-003`: **Variant 8 (embedded engine)** — `RedbStore`
  uses one `redb` table (`TableDefinition<u128, u32>`, UUID → age).
  `update_age` performs one `redb` write transaction (open table, insert,
  commit) per call — each individually committed, not batched.
  `create`/`open` rebuild records/breed index/adjacency index in memory
  from externally-supplied `records`/`edges`; `redb` itself holds only
  ages. `redb` is chosen over `sled` specifically for its ACID-transactions-
  from-the-start design and lack of `sled`'s documented crash-safety
  caveats (see ADR-0006).
- `STORAGE-009-FR-004`: All three variants implement `DogStore`
  (`get`/`scan_ages`/`update_age`/`same_breed`/`neighbors`), each with a
  correctness test appropriate to its own architecture: round-trip-across-
  reopen for mmap and `redb` (mirroring `STORAGE-008`'s snapshot variants'
  standard), plus, for LSM specifically, a
  newest-flushed-generation-wins-across-multiple-flushes test — its own
  defining correctness property, since it's the one variant of the three
  with more than one place an age value could live at once.
- `STORAGE-009-FR-005`: `benches/durability.rs` benchmarks all three
  Tier 2 variants at 1K/100K/1M records: per-write and load cost for all
  three; checkpoint/flush cost for mmap and LSM only (`RedbStore`
  excluded per Non-goals).
- `STORAGE-009-FR-006`: New dependencies for this tier, flagged
  explicitly in `Cargo.toml`: `memmap2` (variant 6 only), `redb`
  (variant 8 only).

## Architecture and interfaces

`src/durability/mmap_store.rs` — `MmapAgeStore`.
`src/durability/lsm_store.rs` — `LsmStore`.
`src/durability/embedded_store.rs` — `RedbStore`. All three depend on
`src/durability/mod.rs`'s shared `DurabilityError` (and, for LSM, the
shared WAL helpers) but not on `CanonicalCachedState` — each has its own
minimal state shape, since none of them need the full canonical-store
architecture to persist a single `u32`-per-record field.
`benches/durability.rs` — extends the same three-metric suite
`STORAGE-008` defines. No changes to `src/store/{aos,soa,canonical,
canonical_cached}.rs` or `src/generator.rs`.

## Data/state and invariants

- All three variants build their breed/adjacency indexes (and, for mmap,
  a position index) from externally-supplied `records`/`edges` at
  `open`/`create` time — identical convention to `STORAGE-008`'s variants
  and to every other backend in this crate.
- `LsmStore::flushed_generations` is the sole source of truth for how many
  `sst_N.bin` files exist; persisted via a small `generations.bin`
  metadata file so `open` knows how many to check without listing the
  directory.
- `MmapAgeStore`'s mapped file size is fixed at `4 * records.len()` bytes,
  set at `create` time and never resized — consistent with this crate's
  invariant that a store's record set doesn't grow/shrink after
  construction (only `age` values change).

## Errors, failure, recovery, and observability

Shares `STORAGE-008`'s `DurabilityError` type. `DurabilityError::Engine(String)`
(unused by Tier 1) wraps `redb`'s own error types, which don't implement a
common trait this crate could `#[from]` directly — introducing a
per-engine error variant for one Tier 2 backend wasn't judged worth the
added enum surface. Every fallible path returns `Result` and uses `?`, no
`unwrap`/`expect` outside `#[cfg(test)]`.

## Security, privacy, and compatibility

Not applicable — synthetic in-memory/on-disk data only, same as
`STORAGE-008` and every other spec in this tree.

## Acceptance criteria

- `cargo test --all-features` passes, including all three Tier 2
  variants' unit tests, notably `LsmStore`'s
  `newest_generation_wins_across_multiple_flushes` and
  `reopen_recovers_flushed_and_unflushed_writes`.
- `cargo bench --bench durability` completes all Tier 2 cases (per-write
  and load for all three variants; checkpoint/flush for mmap and LSM
  only) without panics.
- `RESULTS.md`'s `## Durability` section covers all three Tier 2
  variants with the same per-configuration reporting standard as Tier 1,
  explicitly noting `RedbStore`'s absence from the checkpoint table and
  why.
- No `src/store/{aos,soa,canonical,canonical_cached}.rs` or
  `src/generator.rs` changes — verified by the diff touching only
  `src/durability/{mmap_store,lsm_store,embedded_store}.rs`,
  `benches/durability.rs`, and `Cargo.toml` (dependency additions) beyond
  what `STORAGE-008` already touches.

## Verification plan

- Unit tests per variant: construction, basic read/write, the
  variant-specific highest-priority correctness property (see FR-004),
  and index survival across a simulated restart.
- 3-variant Criterion suite (`benches/durability.rs`) at 1K/100K/1M,
  per-write and load cost for all three, checkpoint/flush cost for mmap
  and LSM.

## Traceability

Implements: the "durability, Tier 2 (mmap/LSM-style/embedded-engine
alternate architectures)" deliverable.
