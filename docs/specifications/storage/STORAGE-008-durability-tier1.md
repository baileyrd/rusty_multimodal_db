# STORAGE-008 — Durability, Tier 1: WAL/snapshot/hybrid family for `CanonicalCachedStore`

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-005`
- Supersedes: none

## Purpose and scope

Every backend and benchmark in this crate up to this point is purely
in-memory. This spec defines the first five durability variants for
`CanonicalCachedStore` — the one backend every prior `RESULTS.md` section
recommends — covering the practical WAL/snapshot/hybrid design space at
full implementation rigor: correctness-tested, benchmarked at all three
existing dataset sizes across per-write, checkpoint, and load costs. See
ADR-0005 for the design decisions and `RESULTS.md`'s `## Durability`
section for the numbers and explicit recommendation.

## Non-goals

- Not for `AosStore`/`SoaStore`/`CanonicalStore` — durability applies to
  `CanonicalCachedStore` only; the other three backends are unchanged
  in-memory baselines. See ADR-0005's Context for why.
- Not a modification to `src/store/canonical_cached.rs` — that file is
  closed, already-benchmarked code. Every variant here is built around a
  new, separately-tested `CanonicalCachedState` core in
  `src/durability/mod.rs` that shares the same architecture, not the
  literal private struct.
- Not persisting the *initial* generated dataset — every variant's
  `open`/`create` takes `records`/`edges` externally, the same way
  `CanonicalCachedStore::new` does. Durability covers what happens to
  that base dataset *after* construction (whether `update_age` calls
  survive a simulated restart), not whether the initial bulk load itself
  is durable.
- Not an explicit physical-disk (`fsync`) guarantee on checkpoint —
  `checkpoint()`'s underlying write call does not call `sync_all` for any
  of the five variants (only each WAL variant's *per-write* path does, and
  only for variant 1). See ADR-0005's Consequences for this being a real,
  documented gap, not an oversight, and its revisit trigger.
- Not covering Tier 2 (mmap, LSM-style, embedded-engine variants) — see
  `STORAGE-009`.

## Context and terminology

- **Checkpoint**: an explicit call that persists current state to disk in
  a way that bounds future replay/read cost. For the WAL variants (1, 2),
  this is "write a fresh full snapshot, then truncate the WAL." For the
  snapshot variants (3, 4), it's the only way anything reaches disk at
  all. For the hybrid variant (5), it's "write a fresh full snapshot
  tagged with a cutoff sequence number" — critically, **without**
  truncating the WAL (see below).
- **Sequence number**: a strictly monotonically increasing `u64` assigned
  to each WAL entry by its writer, used to order replay and (for hybrid
  specifically) to determine which entries a snapshot's cutoff already
  covers.
- **Cutoff**: the sequence number a hybrid snapshot records at the moment
  it was taken. On `open`, only WAL entries with `seq > cutoff` are
  replayed on top of the restored snapshot.

## Requirements

- `STORAGE-008-FR-001`: A shared `CanonicalCachedState` type
  (`src/durability/mod.rs`) implements `get`/`scan_ages`/`update_age`/
  `same_breed`/`neighbors` with the same semantics as
  `CanonicalCachedStore`, plus `write_to`/`read_from` (bincode
  (de)serialization of the whole struct) and `records_snapshot`
  (canonical-only extraction). Every Tier 1 variant wraps or embeds this
  type rather than reimplementing the read path.
- `STORAGE-008-FR-002`: A shared `WalEntry { seq, id, age }` type and
  `append_wal_entry`/`read_wal_entries` functions implement one
  length-prefixed binary WAL format used identically by every WAL-writing
  variant (1, 2, 5). `read_wal_entries` treats a missing file as an empty
  WAL (not an error) and stops replay cleanly at a torn trailing write
  (a length prefix claiming more bytes than the file actually has)
  without losing or erroring on entries written before the tear.
- `STORAGE-008-FR-003`: **Variant 1 (WAL fsync-per-write)** —
  `update_age` appends a `WalEntry`, calls `File::sync_all`, then mutates
  in-memory state, in that order, before returning `Ok`. `checkpoint`
  writes a fresh base snapshot and truncates the WAL.
- `STORAGE-008-FR-004`: **Variant 2 (WAL buffered)** — identical
  structure to variant 1, with no `sync_all` call. Durable against a
  same-process restart; not forced to physical disk on any bounded
  schedule this crate controls.
- `STORAGE-008-FR-005`: **Variant 3 (snapshot, canonical-only,
  rebuild-on-load)** — `update_age` performs no disk I/O. `checkpoint`
  persists only `records`/`edges` (via `records_snapshot` plus the
  variant's own retained edge list). `open` rebuilds every derived index
  from that canonical-only snapshot via `CanonicalCachedState::new`, the
  same construction path `create` uses.
- `STORAGE-008-FR-006`: **Variant 4 (snapshot, save-as-is)** — same
  zero-per-write-I/O shape as variant 3; `checkpoint` persists the
  *whole* `CanonicalCachedState` (via `write_to`) with no rebuild step on
  `open` (via `read_from`). Its own module docs state, as a comment (not
  a corruption-injection test — the task's own explicit instruction), the
  tradeoff this accepts and variant 3 doesn't: an in-memory
  index-corruption bug would be faithfully persisted and reloaded, where
  variant 3's rebuild-on-load would self-correct it.
- `STORAGE-008-FR-007`: **Variant 5 (hybrid)** — `update_age` appends a
  buffered (not fsync'd) `WalEntry`, then mutates state, same per-write
  shape as variant 2. `checkpoint` writes a full-state snapshot
  (`HybridSnapshot { seq_at_snapshot, state }`) tagged with the sequence
  number of the last entry it covers, and **does not truncate the WAL**.
  `open` restores the latest snapshot (or builds fresh from
  `records`/`edges` if none exists yet) and replays only WAL entries
  whose `seq` is strictly greater than the snapshot's recorded cutoff.
- `STORAGE-008-FR-008`: `StoreError` gains one new variant,
  `Durability(String)`, and `DurabilityError` (the shared error type for
  every I/O/(de)serialization/engine failure across all eight variants,
  Tier 1 and Tier 2) converts into it via `impl From<DurabilityError> for
  StoreError`, so every variant's `update_age` can still satisfy
  `DogStore`'s `Result<(), StoreError>` signature while using `?` freely
  against `DurabilityError`-returning internal calls.
- `STORAGE-008-FR-009`: `benches/durability.rs` benchmarks all five
  variants at 1K/100K/1M records across three Criterion groups
  (`durability_per_write`, `durability_checkpoint`, `durability_load`),
  via two file-local traits (`DurableVariant`/`Checkpointable`, not part
  of this crate's public API) that normalize the variants' differing
  `create`/`open`/`checkpoint` signatures for iteration.
- `STORAGE-008-FR-010`: A non-Criterion worked comparison — how long
  until a batch of ~1,000 writes is guaranteed recoverable under each
  variant — is reported in `RESULTS.md`'s `## Durability` section,
  distinguishing continuous small per-write cost (WAL-based variants)
  from burst checkpoint cost with a data-loss window until the next
  checkpoint (snapshot-alone variants).

## Architecture and interfaces

`src/durability/mod.rs` — `DurabilityError`, `WalEntry`,
`append_wal_entry`/`read_wal_entries`, `CanonicalCachedState`,
`test_support` (shared `sample_records`/`sample_edges` fixtures for every
variant's unit tests). `src/durability/{wal_fsync,wal_buffered,
snapshot_rebuild,snapshot_full,hybrid}.rs` — one store type per variant,
each implementing `DogStore`. `src/store/mod.rs` — `StoreError::Durability`
variant. `src/lib.rs` — `pub mod durability;`. `benches/durability.rs` —
the three-metric benchmark suite. No changes to
`src/store/{aos,soa,canonical,canonical_cached}.rs` or `src/generator.rs`.

## Data/state and invariants

- Every variant's `open`/`create` takes `records: Vec<DogRecord>`,
  `edges: Vec<(Uuid, Uuid)>` (except variants 3/4's `open`, which take
  only a path — the persisted file itself is the canonical source of
  truth for those two, not an externally-supplied base dataset; see their
  own module docs). This mirrors how every other backend in this crate is
  constructed fresh from generated data per benchmark/test run.
  `fresh_temp_dir` (`src/bench_support.rs`) provides a uniquely-named,
  concurrency-safe temp directory shared by every variant's unit tests
  and by `benches/durability.rs`.
- WAL sequence numbers are per-store-instance counters (`next_seq: u64`),
  reset to the count of replayed entries on `open`, never reused within a
  process lifetime — required for hybrid's cutoff comparison to be
  meaningful across restarts.

## Errors, failure, recovery, and observability

`DurabilityError` (`Io`, `Serde`, `Store`, `Engine` — the last unused by
Tier 1, see `STORAGE-009`) is the single error type for every fallible
path across all eight durability variants. Every fallible function
returns `Result` and uses `?` — no `unwrap`/`expect` outside `#[cfg(test)]`,
matching this crate's existing discipline. A torn trailing WAL write
(process died mid-append) is treated as a recovery boundary, not a hard
error: `read_wal_entries` returns every entry written before the tear.

## Security, privacy, and compatibility

Not applicable — synthetic in-memory/on-disk data only, written to a
process-local temp directory, same as every other spec in this tree.

## Acceptance criteria

- `cargo test --all-features` passes, including every Tier 1 variant's
  unit tests: WAL reconstruction-from-log-matches-expected-state
  (highest priority, variants 1/2), checkpoint-then-open matches a fresh
  store built from the same data (variants 3/4), and the snapshot-plus-
  partial-replay-across-the-cutoff test (highest priority, variant 5).
- `cargo bench --bench durability` completes all Tier 1 cases across
  `durability_per_write`/`durability_checkpoint`/`durability_load`
  (5 variants × 3 sizes × up to 3 metrics) without panics.
- `RESULTS.md` has a `## Durability` section covering every Tier 1
  variant with per-configuration numbers, the 1,000-write recoverability
  worked comparison, and an explicit recommendation.
- No `src/store/{aos,soa,canonical,canonical_cached}.rs` or
  `src/generator.rs` changes — verified by the diff touching only
  `src/durability/**`, `src/store/mod.rs` (the new `StoreError` variant
  only), `src/lib.rs`, `src/record.rs` (`Serialize`/`Deserialize`
  derives), `src/bench_support.rs` (`fresh_temp_dir`), `benches/durability.rs`,
  and `Cargo.toml`.

## Verification plan

- Unit tests per variant (`src/durability/*.rs`): construction, basic
  read/write, the variant-specific highest-priority correctness property
  named in FR-003 through FR-007 above, and (for the WAL/hybrid variants)
  index survival across a simulated restart.
- Shared-core tests (`src/durability/mod.rs`): `CanonicalCachedState`
  shape/behavior, `write_to`/`read_from` round-trip, WAL entry round-trip
  in order, missing-WAL-file-reads-as-empty, and the torn-trailing-write
  recovery test.
- 5-variant, 3-size Criterion suite (`benches/durability.rs`) across all
  three metrics, plus the non-Criterion recoverability comparison
  (`RESULTS.md`).

## Traceability

Implements: the "durability, Tier 1 (WAL/snapshot/hybrid family)"
deliverable.
