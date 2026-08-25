# STORAGE-011 — Production default: consolidate storage, durability, and concurrency picks into one recommended type

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-005`, `STORAGE-007`, `STORAGE-008`, `STORAGE-009`, `STORAGE-010`
- Supersedes: none

## Purpose and scope

Six rounds of empirical work have all converged on one combination:
`CanonicalCachedStore`'s storage architecture, made durable via mmap
(`STORAGE-009`'s `MmapAgeStore`), made safe for concurrent access via one
global `RwLock` (`STORAGE-010`'s `GlobalRwLockStore` pattern). This spec
covers wiring that combination into one new type,
[`ProductionStore`](../../../src/production.rs), verifying it as a
composed stack (not just its already-verified individual pieces), and
updating the crate's documentation to lead with it. See ADR-0008 for which
round justified each layer and `RESULTS.md`'s `## Production
recommendation` section for the numbers.

## Non-goals

- Not a change to `CanonicalCachedStore`, `MmapAgeStore`, or
  `GlobalRwLockStore`'s internals — this spec wires three existing, closed
  types together; it doesn't modify any of them (see ADR-0008's Decision
  drivers).
- Not a physical file reorganization (`src/store/`, `src/durability/`,
  `src/concurrency/` moved under a `src/comparisons/` tree) — considered,
  checked in on explicitly, and rejected for this pass in favor of a
  docs-only lead (`src/lib.rs`, `README.md`,
  `docs/architecture/SYSTEM-ARCHITECTURE.md`); see ADR-0008's Considered
  options.
- Not deleting any of the other three storage backends, seven other
  durability variants, or three other concurrency strategies — they
  remain in the tree as the evidence this recommendation is built on.
- Not a new concurrency-strategy-plus-durability-variant combination
  beyond this one specific pick (`RwLock` + mmap) — `ShardedStore`+mmap or
  any other pairing is out of scope, per ADR-0007's own Context, which
  named "concurrency + a specific durability variant, combined" as a
  distinct future round this spec picks exactly one instance of, not the
  general design space.
- Not new benchmark infrastructure — `ProductionStore` is wired into the
  existing `benches/workloads.rs` and `benches/concurrency.rs` targets as
  an additional variant, reusing every existing generic runner.

## Context and terminology

- **`ProductionStore`**: the new type (`src/production.rs`),
  `RwLock<MmapAgeStore>`. Implements both `DogStore` (drop-in for
  single-owner code) and `ConcurrentStore` (for genuine multi-threaded
  sharing).
- **The three picks**: storage — `CanonicalCachedStore`'s architecture
  (`STORAGE-005`, ADR-0003, reaffirmed by `STORAGE-006`/`STORAGE-007`);
  durability — mmap (`STORAGE-009`, ADR-0006); concurrency — global
  `RwLock` (`STORAGE-010`, ADR-0007, reaffirmed across the container/
  Windows-machine/`baileyai` throughput passes).
- **Flagship integration test**: a new test,
  `tests/production_integration.rs`, the highest-priority deliverable in
  this spec — the first test exercising mmap durability and `RwLock`
  concurrency running together, on top of the shared architecture, rather
  than each in isolation.

## Requirements

- `STORAGE-011-FR-001`: **`ProductionStore` composition** —
  `src/production.rs` defines `ProductionStore` as `RwLock<MmapAgeStore>`.
  No new struct duplicates `MmapAgeStore`'s or `CanonicalCachedStore`'s
  internal fields; `create`/`open`/`flush` delegate directly to
  `MmapAgeStore`'s own methods of the same name.
- `STORAGE-011-FR-002`: **`DogStore` implementation** — `ProductionStore`
  implements the full `DogStore` trait (`get`/`scan_ages`/`update_age`/
  `same_breed`/`neighbors`), each delegating through the `RwLock` to
  `MmapAgeStore`'s own `DogStore` impl.
- `STORAGE-011-FR-003`: **`ConcurrentStore` implementation** —
  `ProductionStore` implements `ConcurrentStore` (`get`/`scan_ages`/
  `update_age`, `&self` throughout). `ConcurrentStore::new` and the two
  `From` impls (`From<Vec<DogRecord>>`, `From<(Vec<DogRecord>,
  Vec<(Uuid, Uuid)>)>`) allocate a fresh, uniquely-named temp-file backing
  via `bench_support::fresh_temp_dir` and `.expect()` on failure — a
  documented exception to "no unwrap/expect outside tests," on the same
  footing `GlobalRwLockStore`'s `RwLock`-poisoning exception already
  established. `ProductionStore::create`/`open` remain the fully-fallible
  path for callers with a real, caller-supplied path.
- `STORAGE-011-FR-004`: **Flagship integration test** —
  `tests/production_integration.rs` runs two phases of 16 threads × 2,000
  iterations each of interleaved `get`/`update_age` calls (matching
  `run_concurrency_stress_test`'s own rigor) against a shared
  `ProductionStore`, separated by a genuine `drop` + `ProductionStore::open`
  reopen from disk. Verified two ways: (1) linearizability — the full,
  two-phase recorded write order replayed sequentially against a fresh
  reference `CanonicalCachedStore` must match the final concurrent result
  exactly (no lost updates), and every contended id's final value must be
  either its initial value or a value some thread genuinely attempted to
  write (no torn reads); (2) persistence — a *third*, fresh
  `ProductionStore::open` call, made only after the second store handle is
  fully dropped, must see every write from both phases.
- `STORAGE-011-FR-005`: **Standard benchmark suite** — `ProductionStore`
  is added as the first-listed variant in every one of
  `benches/workloads.rs`'s seven existing groups (`get`/`scan_ages`/
  `update_age`/`same_breed`/`neighbors_one_hop`/`neighbors_two_hop`/mixed-
  workload at all three write ratios) and in `benches/concurrency.rs`'s
  existing size × write-ratio × thread-count sweep — no new bench target,
  no changes to either file's existing per-variant coverage.
- `STORAGE-011-FR-006`: **Documentation lead** — `src/lib.rs`'s
  crate-level doc comment, `README.md`, and
  `docs/architecture/SYSTEM-ARCHITECTURE.md` are updated to introduce
  `ProductionStore` first and frame `store`/`durability`/`concurrency`
  explicitly as benchmarked alternatives, not the recommended path. New
  ADR-0008 records the consolidation decision. `RESULTS.md` gains a new
  `## Production recommendation` section tying the six prior rounds
  together with pointers back to each detailed section, rather than
  repeating their numbers.
- `STORAGE-011-FR-007`: **No new dependency** — this spec composes
  existing, already-dependency-justified pieces.

## Architecture and interfaces

`src/production.rs` — `ProductionStore`, its `DogStore`/`ConcurrentStore`
impls, `create`/`open`/`flush`, and the two `From` impls.
`tests/production_integration.rs` — the flagship integration test.
`benches/workloads.rs`/`benches/concurrency.rs` — extended with
`ProductionStore` as an additional variant (additive changes only). No
changes to `src/store/canonical_cached.rs`, `src/durability/mmap_store.rs`,
`src/durability/mod.rs`, `src/concurrency/global_rwlock.rs`, or
`src/concurrency/mod.rs`.

## Data/state and invariants

- `ProductionStore` is constructed from the same `(records: Vec<DogRecord>,
  edges: Vec<(Uuid, Uuid)>)` inputs every other backend/variant in this
  crate takes, plus a filesystem `path` for `create`/`open` (mirroring
  `MmapAgeStore`'s own signature exactly) or an internally-allocated fresh
  temp path for `ConcurrentStore::new`/the `From` impls.
- Ages are the only durable, mutable field — inherited directly from
  `MmapAgeStore`'s own scope-down (records/edges are immutable after
  construction and supplied externally at `create`/`open` time, same
  convention as every durability variant).
- `get`/`scan_ages`/`same_breed`/`neighbors` take a read lock;
  `update_age` (through either trait) takes a write lock; `flush` takes a
  write lock (a checkpoint wants a quiescent snapshot, not a value racing
  an in-flight write).

## Errors, failure, recovery, and observability

`ProductionStore::create`/`open`/`flush` return `Result<_,
DurabilityError>`, reusing the existing shared error type — no new error
enum. `DogStore::update_age`/`ConcurrentStore::update_age` return
`StoreError`/`ConcurrencyError` respectively, matching every other
backend/variant. `.expect()` appears only where documented per this crate's
existing convention: `RwLock` poisoning (matching `GlobalRwLockStore`) and
`ConcurrentStore::new`/the `From` impls' fresh-temp-file allocation
(matching this spec's own FR-003, a new but analogous documented
exception).

## Security, privacy, and compatibility

Not applicable — synthetic in-memory/on-disk data only, same as every
other spec in this tree.

## Acceptance criteria

- `cargo test --all-features` passes, including
  `production::tests::*` (create/open/flush round-trip, `DogStore` and
  `ConcurrentStore` behavior, the `From` impls, and
  `run_concurrency_stress_test::<ProductionStore>()`) and the new flagship
  integration test.
- `cargo bench --bench workloads -- production` and
  `cargo bench --bench concurrency` complete without panics, with
  `ProductionStore` numbers reported alongside every existing variant in
  `RESULTS.md`.
- No `src/store/canonical_cached.rs`, `src/durability/mmap_store.rs`,
  `src/durability/mod.rs`, `src/concurrency/global_rwlock.rs`, or
  `src/concurrency/mod.rs` changes — verified by the diff touching only
  `src/production.rs` (new), `src/lib.rs`, `benches/workloads.rs`,
  `benches/concurrency.rs`, `tests/production_integration.rs` (new), and
  documentation.
- `RESULTS.md`'s `## Production recommendation` section exists, ties the
  six prior rounds together, and points back to their detailed sections
  rather than repeating their numbers.

## Verification plan

- Unit tests: construction (`create`/`open`), flush-then-reopen round
  trip, `DogStore` drop-in behavior (including `same_breed`/`neighbors`,
  not just `get`/`update_age`/`scan_ages`), `ConcurrentStore` behavior, the
  two `From` impls, and the reused `run_concurrency_stress_test` flagship
  correctness check.
- Flagship integration test: two 16-thread × 2,000-iteration concurrent
  contention phases separated by a genuine drop + reopen, verified via
  sequential-replay linearizability and a third, post-drop reopen for
  persistence.
- Benchmark run: `ProductionStore` at every (workload × size) cell in
  `benches/workloads.rs`'s existing suite and every (size × write-ratio ×
  thread-count) cell in `benches/concurrency.rs`'s existing sweep.

## Traceability

Implements: the "consolidate the production default" deliverable.
