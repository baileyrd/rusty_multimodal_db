# STORAGE-010 — Concurrent access prototypes for `CanonicalCachedStore`: four reader/writer safety strategies

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-005`
- Supersedes: none

## Purpose and scope

Every workload benchmarked before this spec assumes single-threaded
access to a store. This spec covers four concurrency strategies for
`CanonicalCachedStore` — the only backend that gets a concurrency story,
per the same reasoning `STORAGE-005` established for that backend getting
a durability story: two full-rigor, correctness-stress-tested,
fully-benchmarked variants (global `RwLock`, sharded locking, `dashmap`),
and one lighter proof-of-concept (actor/single-writer-thread). See
ADR-0007 for the design decisions and `RESULTS.md`'s `## Concurrency`
section for the numbers and recommendation.

## Non-goals

- Not paired with any specific `STORAGE-008`/`STORAGE-009` durability
  variant — every variant here is purely in-memory, by explicit task
  scope. Combining a concurrency strategy with a durability design is a
  named future round, not this spec.
- Not a full `DogStore` implementation for the sharded/`dashmap` variants
  — both cover only `get`/`update_age`/`scan_ages` (the new
  `ConcurrentStore` trait's surface), the slice this pass's benchmark
  needs. `same_breed`/`neighbors` are explicitly out of scope for these
  two variants — sharding those cleanly would require sharding the
  breed/adjacency indexes too, a materially bigger design problem (see
  ADR-0007's Considered options and `sharded.rs`'s module docs).
- Not distributed locking and not an async runtime — in-process
  multi-threading only, per the motivating task's explicit constraint.
- Not a formal linearizability checker (e.g. Jepsen/Knossos) — the
  correctness test is an honestly-documented smoke test: real completion
  order recorded and replayed sequentially, compared to the concurrent
  result, plus a secondary torn-write membership check. See ADR-0007 and
  `run_concurrency_stress_test`'s own doc comment.
- Not scaling `ActorStore` beyond one owning thread — its measured
  throughput ceiling is a structural property of the single-writer-thread
  pattern, not addressed by this pass (see `RESULTS.md`'s open
  questions).

## Context and terminology

- **`ConcurrentStore`**: the new trait (`src/concurrency/mod.rs`) every
  variant implements — `&self` on every method, including `update_age`
  (unlike `DogStore::update_age`'s `&mut self`), since these types are
  shared across threads via `Arc` and each variant owns its own internal
  synchronization.
- **Tier 1**: global `RwLock`, sharded locking, `dashmap` — full
  implementation, correctness-stress-tested, full benchmark sweep.
- **Tier 2**: actor/single-writer-thread — lighter proof-of-concept,
  comparable numbers, per the motivating task's explicit two-tier split.
- **Flagship stress test**: `run_concurrency_stress_test`
  (`src/concurrency/mod.rs`, test-only) — 16 threads × 2,000 iterations
  each, randomly interleaved `get`/`update_age` against a 20-id contended
  pool drawn from a 500-record dataset, checked against a sequential
  replay of the real recorded write order.

## Requirements

- `STORAGE-010-FR-001`: **Variant 1 (global `RwLock`)** —
  `GlobalRwLockStore` wraps an unmodified `CanonicalCachedStore` in one
  `std::sync::RwLock`; `get`/`scan_ages` take a read lock, `update_age`
  takes a write lock.
- `STORAGE-010-FR-002`: **Variant 2 (sharded locking)** — `ShardedStore`
  partitions records across `SHARD_COUNT = 64` independently-
  `RwLock`-guarded `HashMap<Uuid, DogRecord>` shards, routed by
  `id.as_u128() % SHARD_COUNT`. `get`/`update_age` lock only the one
  shard `id` routes to; `scan_ages` acquires each shard's read lock in
  turn (not simultaneously) and concatenates ages.
- `STORAGE-010-FR-003`: **Variant 3 (`dashmap`)** — `DashMapStore` swaps
  the canonical map for `dashmap::DashMap<Uuid, DogRecord>`; `get`/
  `update_age`/`scan_ages` implemented directly against `DashMap`'s own
  API with no additional locking.
- `STORAGE-010-FR-004`: **Variant 4 (actor/single-writer-thread)** —
  `ActorStore` spawns one detached worker thread owning a plain,
  unsynchronized `CanonicalCachedStore`; every other thread sends a
  request (`Get`/`ScanAges`/`UpdateAge`, each carrying a one-shot
  `mpsc::Sender` reply channel) over a `Mutex<mpsc::Sender<ActorRequest>>`
  and blocks on the reply.
- `STORAGE-010-FR-005`: All four variants implement `ConcurrentStore`
  (`Send + Sync`, `get`/`scan_ages`/`update_age`, `Result`-returning
  throughout) and pass `run_concurrency_stress_test`: 16 threads × 2,000
  iterations each against a 20-id contended pool, with the concurrent
  store's final state matching a sequential replay of the real recorded
  write order exactly (no lost updates), and every id's final value
  matching either its initial value or a value some thread genuinely
  attempted to write (no torn reads).
- `STORAGE-010-FR-006`: `MixedWorkloadDriver` (`src/bench_support.rs`)
  gains a new, additive `run_one_concurrent` method driving a
  `ConcurrentStore` instead of a `DogStore` — `run_one` and every
  benchmark using it unchanged.
- `STORAGE-010-FR-007`: A new custom (non-Criterion) benchmark harness,
  `benches/concurrency.rs`, measures aggregate operations/second across
  concurrently-running worker threads for all four variants, sweeping
  dataset size (1,000/100,000), write ratio (10%/50%/90%, matching
  `STORAGE-007`'s sweep), and thread count (1/4/8/16). Reports
  `std::thread::available_parallelism()` so thread-count numbers are
  interpretable against the real machine's core count.
- `STORAGE-010-FR-008`: New dependency, flagged explicitly: `dashmap =
  "6"`, for variant 3 only. No other new dependency for this spec.

## Architecture and interfaces

`src/concurrency/mod.rs` — `ConcurrentStore` trait, `ConcurrencyError`
enum, and (test-only) `run_concurrency_stress_test`.
`src/concurrency/global_rwlock.rs` — `GlobalRwLockStore`.
`src/concurrency/sharded.rs` — `ShardedStore`.
`src/concurrency/dashmap_store.rs` — `DashMapStore`.
`src/concurrency/actor.rs` — `ActorStore`. `src/bench_support.rs` —
extends `MixedWorkloadDriver` with `run_one_concurrent` (additive only).
`benches/concurrency.rs` — new bench target. No changes to
`src/store/{aos,soa,canonical,canonical_cached}.rs`, `src/generator.rs`,
or anything under `src/durability/`.

## Data/state and invariants

- Every variant is constructed from the same `(records: Vec<DogRecord>,
  edges: Vec<(Uuid, Uuid)>)` inputs every other backend/variant in this
  crate takes — `edges` is accepted but unused by the sharded/`dashmap`/
  actor variants' own read/write surface, since `neighbors` isn't part of
  `ConcurrentStore` (kept for signature uniformity across all four
  variants and consistency with the rest of this crate's construction
  convention).
- `ShardedStore`'s shard routing (`id.as_u128() % SHARD_COUNT`) is fixed
  at construction and never reshards — consistent with this crate's
  existing invariant that a store's record set doesn't grow/shrink after
  construction.
- `ActorStore`'s worker thread is detached, not joined — it exits
  naturally once every clone of `request_tx` is dropped (the channel
  closes, `recv` returns `Err`, the loop ends), which happens when the
  owning `ActorStore` (and every `Arc` clone of it) is dropped.

## Errors, failure, recovery, and observability

New `ConcurrencyError` enum (`src/concurrency/mod.rs`):
`Store(#[from] StoreError)` for the one failure mode every variant shares
(`update_age` on an unknown UUID); `ActorDisconnected`, specific to
`ActorStore`, for a channel send/recv failing because the worker thread
has already exited (structurally can't happen while the owning
`ActorStore` is alive). Every fallible path returns `Result` and uses
`?`. `.expect()` appears only on lock/mutex acquisition where poisoning
is documented, per callsite, as unable to happen under normal operation
(every operation performed while holding the lock is infallible/
panic-free by construction) — the explicit, task-granted exception to "no
unwrap/expect outside tests," matching the standard this crate's
durability variants already established for the same kind of call.

## Security, privacy, and compatibility

Not applicable — synthetic in-memory data only, same as every other spec
in this tree.

## Acceptance criteria

- `cargo test --all-features` passes, including all four variants'
  `concurrent_stress_matches_sequential_replay` tests and their other
  unit tests (construction/read/write/scan sanity checks per variant,
  plus sharded's own "every shard is reachable" test).
- `cargo bench --bench concurrency` completes the full
  size×ratio×thread-count×variant sweep without panics, reporting
  `std::thread::available_parallelism()` alongside the numbers.
- `RESULTS.md`'s `## Concurrency` section covers all four variants with a
  verdict per (size × write-ratio) configuration, an explicit correctness
  section describing the stress test and its result, and a recommendation
  that doesn't claim one overall winner (matching `## Durability`'s
  established reporting standard).
- No `src/store/{aos,soa,canonical,canonical_cached}.rs`,
  `src/generator.rs`, or `src/durability/**` changes — verified by the
  diff touching only `src/concurrency/**`, `src/bench_support.rs`
  (additive `run_one_concurrent` method), `benches/concurrency.rs`, and
  `Cargo.toml` (the `dashmap` dependency and new `[[bench]]` entry).

## Verification plan

- Unit tests per variant: construction, basic read/write, scan
  correctness, and (sharded only) cross-shard reachability.
- Flagship correctness test per variant: 16 threads × 2,000 iterations
  each against a 20-id contended pool — no lost updates, no torn reads.
- Custom throughput harness (`benches/concurrency.rs`) at 1,000/100,000
  records, 10%/50%/90% write ratios, 1/4/8/16 threads, all four variants.

## Traceability

Implements: the "concurrent access prototypes (reader/writer safety)"
deliverable.
