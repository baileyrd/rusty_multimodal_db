# ADR-0007: Concurrent access prototypes for `CanonicalCachedStore` — four strategies, benchmarked, no single winner

- Status: Accepted
- Date: 2026-08-25
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0005-wal-snapshot-hybrid-durability.md`, `docs/decisions/ADR-0006-tier-2-durability-architectures.md`, `STORAGE-010`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

Every workload benchmarked in this crate so far — including the durability
prototypes (ADR-0005/ADR-0006) — assumes exactly one thread touches a store
at a time. The motivating task asked for the next, previously out-of-scope
axis: what happens once real reader/writer threads share one
`CanonicalCachedStore` instance, and which of several reasonable
concurrency strategies is worth keeping, decided empirically the same way
every other backend/architecture choice in this crate has been. Per the
same "don't build for backends nobody would deploy" reasoning ADR-0003
established for `CanonicalCachedStore` getting a durability story,
concurrency here is scoped to `CanonicalCachedStore` only —
`AosStore`/`SoaStore`/`CanonicalStore` remain untouched, purely
single-threaded baselines.

The task was explicit that this round is deliberately **not** paired with
a specific durability variant: "concurrency + a specific durability
choice combined is a natural follow-up round, not this one." Combining
the two axes (four concurrency strategies × eight durability variants)
multiplies into a design space this pass isn't scoped to cover — this ADR
establishes the concurrency numbers on their own footing, over the plain,
non-durable `CanonicalCachedStore` shape, first.

`DogStore::update_age` takes `&mut self` — correct for every existing
backend, but incompatible with sharing one store instance across threads,
which is exactly what this round needs. This raised a design question
before any variant could be built: extend `DogStore` itself, or introduce
a new trait.

## Decision drivers

- **Correctness is the highest-priority deliverable, explicitly weighted
  the same as `HYBRID-BACKEND`'s original stale-cache test.** A benchmark
  number for a concurrency strategy that loses updates or exposes torn
  reads under real contention is worse than useless — it would look like
  a working, fast option.
- **Let the numbers decide, same empirical philosophy as every prior
  round.** No variant is assumed to win going in; Tier 1 gets full rigor
  (implementation, stress test, full benchmark sweep) so an honest
  comparison is possible, Tier 2 gets a lighter proof-of-concept per the
  task's own two-tier split.
- **Minimal new dependencies, each justified.** `dashmap` is the one
  addition the task explicitly anticipated needing; anything else must
  earn its place the same way `serde`/`bincode`/`memmap2`/`redb` did for
  durability.
- **Avoid speculative generality.** No distributed locking, no async
  runtime, in-process multi-threading only — the task's own explicit
  constraint.
- **Report the real machine's core count, since thread-count numbers are
  meaningless without it** — the same "measure, don't assume" discipline
  the durability reopen-parallelization work already established when it
  measured the sequential/parallel crossover instead of picking a round
  threshold.

## Considered options

### Trait design: extend `DogStore`, or a new trait

1. **Extend `DogStore` with a concurrent-capable `update_age`.** Rejected
   — `DogStore::update_age(&mut self, ...)` is load-bearing for every
   existing single-threaded backend; changing its receiver type would
   ripple through `AosStore`/`SoaStore`/`CanonicalStore`/
   `CanonicalCachedStore` and every durability variant for a capability
   only the new concurrency variants need.
2. **A new trait, `ConcurrentStore`, with `&self` throughout (including
   `update_age`), returning `Result` uniformly across all four variants.**
   Chosen — each variant owns its own internal synchronization, so `&self`
   is enough; a uniform `Result` signature (mirroring how
   `DogStore::update_age` is already `Result` for every backend even
   though only an unknown UUID ever errors for three of the four) is what
   lets one generic stress test and one generic benchmark loop drive all
   four variants identically, the same trait-uniformity trick `DogStore`
   itself already relies on.

### Tier 1 variant 1: global lock implementation

1. **`parking_lot::RwLock`.** Considered — generally lower per-acquisition
   overhead than the standard library's lock. Rejected for this variant
   specifically: it's meant to be the simplest possible baseline, and a
   new dependency for that isn't worth it up front — flagged as a
   plausible follow-up if the numbers show std's overhead matters (see
   Revisit triggers).
2. **`std::sync::RwLock`.** Chosen — no new dependency, and simple: the
   entire `CanonicalCachedStore` behind one lock, multiple concurrent
   readers or one exclusive writer.

### Tier 1 variant 2: sharded locking, and its scope

1. **Shard the full `DogStore` surface**, including `same_breed`/
   `neighbors`, with correspondingly sharded breed/adjacency indexes.
   Rejected — a real sharded design for those two methods needs each
   shard boundary chosen so a query doesn't sometimes have to lock every
   shard, a genuinely bigger design problem than one pass should take on;
   this is the variant the task itself flagged as the most likely
   candidate to need a check-in if it proved too large.
2. **Shard only `get`/`update_age`/`scan_ages`** — exactly
   `ConcurrentStore`'s surface, which is also all the concurrency
   benchmark (reusing `MixedWorkloadDriver`) ever exercises. Chosen —
   sidesteps the hard cross-shard-index problem entirely without losing
   anything this pass's benchmark needs, avoiding the anticipated
   check-in. `age` lives inline on each shard's own `DogRecord` rather
   than in a separate packed cache, since `CanonicalCachedStore`'s packed
   `Vec<u32>` + position-index trick is a single-threaded, contiguous-array
   optimization that doesn't shard cleanly (positions aren't globally
   contiguous once records are partitioned).

### Tier 1 variant 3: lock-free-ish map

1. **Hand-roll a lock-free hash map.** Rejected — reinventing a
   correctness-critical concurrent data structure from scratch is exactly
   the kind of speculative-generality risk this pass's constraints warn
   against, for a comparison point the ecosystem already has a
   well-tested answer to.
2. **`dashmap::DashMap`.** Chosen — the de facto standard sharded
   concurrent map in the Rust ecosystem; swapping the canonical map for
   it directly tests how far an off-the-shelf structure gets versus the
   hand-rolled alternatives, which is the actual question this variant
   exists to answer.

### Tier 2 variant 4: actor channel type

1. **`crossbeam-channel`.** Considered — faster in general, and offers
   `select!` over multiple channels. Rejected — every call here is a
   single request to one fixed destination (the actor thread) followed by
   blocking on one fixed reply channel, the simplest possible
   request/response shape, with nothing to select over; a new dependency
   for capability this variant doesn't use isn't justified.
2. **`std::sync::mpsc`.** Chosen — already in the standard library,
   covers this exact shape. `request_tx` is wrapped in a `Mutex` (not left
   bare) so `ActorStore`'s `Sync`ness doesn't depend on `mpsc::Sender<T>`'s
   own uncertain-across-versions `Sync` status — `Mutex<T>` is
   unconditionally `Sync` when `T: Send`, regardless of `T`'s own `Sync`
   status.

### Correctness methodology: formal linearizability checker, or a targeted stress test

1. **Build or integrate a formal linearizability checker** (e.g. a
   Jepsen/Knossos-style history checker). Rejected — a genuinely general
   linearizability checker is a substantial project of its own, well
   beyond what a benchmark harness's correctness gate needs, and this
   crate's own constraints call for avoiding speculative generality.
2. **A targeted "record the real completion order, replay it
   sequentially, compare final state" smoke test**, supplemented by a
   secondary "no torn reads" membership check against every value any
   thread ever attempted to write. Chosen — catches the two specific
   failure modes the task named (lost updates, torn reads) under real,
   high-thread-count contention, honestly documented as a smoke test, not
   a formal proof (see `run_concurrency_stress_test`'s own doc comment for
   the one named residual imprecision).

## Decision

- `src/concurrency/mod.rs`: a new `ConcurrentStore` trait (`&self`
  throughout, `Result<_, ConcurrencyError>` uniformly) and a new
  `ConcurrencyError` enum (`Store(#[from] StoreError)` for the one shared
  failure mode, `ActorDisconnected` for the actor variant's own channel
  failure mode). Also hosts the shared flagship correctness test,
  `run_concurrency_stress_test` (test-only).
- `src/concurrency/global_rwlock.rs` (`GlobalRwLockStore`): the whole
  `CanonicalCachedStore`, unmodified, wrapped in one `std::sync::RwLock`.
- `src/concurrency/sharded.rs` (`ShardedStore`): 64 independently-
  `RwLock`-guarded `HashMap<Uuid, DogRecord>` shards, routed by
  `id.as_u128() % 64`; `get`/`update_age` touch only the one shard `id`
  routes to, `scan_ages` acquires each shard's read lock in turn (not
  simultaneously).
- `src/concurrency/dashmap_store.rs` (`DashMapStore`): the canonical map
  swapped for `dashmap::DashMap<Uuid, DogRecord>`, otherwise minimal.
- `src/concurrency/actor.rs` (`ActorStore`): one detached worker thread
  owns a plain `CanonicalCachedStore`; `get`/`scan_ages`/`update_age` each
  send an `ActorRequest` with a one-shot reply channel over a
  `Mutex<mpsc::Sender<ActorRequest>>` and block on the reply.
- New dependency, flagged explicitly: `dashmap = "6"`, for variant 3 only.
  No other new dependency.
- All four variants are correctness-stress-tested (16 threads × 2,000
  iterations each against a 20-id contended pool drawn from a 500-record
  dataset — comfortably above the task's "16+ threads" floor) before any
  throughput number is reported, and benchmarked via a new, additive
  `MixedWorkloadDriver::run_one_concurrent` method (`src/bench_support.rs`
  — `run_one` and every benchmark using it unchanged) driven from a custom
  (non-Criterion) harness, `benches/concurrency.rs`, sweeping dataset size
  (1,000/100,000), write ratio (10%/50%/90%, matching `## Mixed read/write
  workload`'s sweep), and thread count (1/4/8/16, used exactly as
  specified regardless of this environment's measured 4 real cores).

## Consequences

### Positive

- Every variant's scope boundary (the `ConcurrentStore` surface excluding
  `same_breed`/`neighbors`; sharded locking's decision not to shard those
  two methods) is a documented, deliberate decision made before
  implementation, not a discovered gap — avoiding the check-in the task
  anticipated might be needed for sharded locking specifically.
- The flagship stress test found zero correctness issues across all four
  variants at 16 threads × 2,000 iterations each (32,000 operations per
  variant) — a real, if not formally proven, confidence signal that each
  variant's synchronization design (a lock, a shard array, a single
  serializing owner thread) is doing its job.
- The throughput numbers overturned a naive intuition rather than just
  confirming one: "sharding/lock-free should always beat a global lock
  under concurrency" does not hold once `scan_ages`'s O(n) cost dominates
  at 100,000 records — a single lock over `CanonicalCachedStore`'s
  already-efficient packed data structures wins there by a wide margin,
  and sharding only pays off in the one tested corner (small dataset,
  write-heavy, real multi-thread contention) where its actual thesis
  applies. See `RESULTS.md`'s `## Concurrency` section for the numbers.
- Reusing `MixedWorkloadDriver` (rather than building a second workload
  generator) kept this round's new surface area to one additive method,
  consistent with this crate's established pattern of extending rather
  than duplicating (`HybridSnapshotRef` alongside `HybridSnapshot`,
  `*_sequential` fallbacks alongside parallel constructors).

### Negative / tradeoffs

- `dashmap`, the one off-the-shelf, least-effort-to-integrate variant,
  never wins a single benchmarked configuration in this pass — a real
  cost of "least code" here, not just durability's `redb` story repeating
  with a different sign.
- Sharded locking's scope-down (excluding `same_breed`/`neighbors`) means
  `ShardedStore` and `DashMapStore` don't implement the full `DogStore`
  surface — they're concurrency prototypes for the `get`/`update_age`/
  `scan_ages` slice this pass's benchmark needs, not drop-in replacements
  for `CanonicalCachedStore` in every context that trait is used.
- `ActorStore`'s throughput ceiling (roughly 65,000–105,000 ops/sec,
  measured) is a hard structural limit of routing every operation through
  one serial owner thread — it cannot be raised by adding more client
  threads, only by redesigning the actor itself (see Revisit triggers).
- None of the four variants are paired with a durability design — by
  deliberate scope choice, but it means this ADR's recommendation
  currently answers "which concurrency strategy" and "which durability
  strategy" as two separate questions, not the combined one a real
  deployment would eventually need to answer.

## Validation and revisit triggers

- Validated by: `src/concurrency/{global_rwlock,sharded,dashmap_store,
  actor}.rs`'s own unit tests (construction/read/write/scan sanity checks
  per variant, plus sharded's own "every shard is reachable" test) and
  each variant's `concurrent_stress_matches_sequential_replay` test
  (`run_concurrency_stress_test`, `src/concurrency/mod.rs`); throughput
  numbers from `benches/concurrency.rs`'s real run, reported in
  `RESULTS.md`'s `## Concurrency` section.
- Revisit if: a future round pairs a concurrency strategy with a specific
  durability variant — the natural next round this ADR's Context
  explicitly defers, not a retrofit onto these four prototypes.
- Revisit if: `GlobalRwLockStore`'s own thread-count degradation (its one
  measured weakness — aggregate throughput drops, not just plateaus, as
  thread count rises under write-heavy load) becomes a real bottleneck at
  a scale or contention level this pass didn't test — swapping in
  `parking_lot::RwLock` is the natural first thing to try, not a redesign.
- Revisit if: `ActorStore`'s single-owner throughput ceiling becomes a
  real constraint — sharding the actor itself (N owning threads, each
  responsible for a disjoint id range) is the natural next step, and
  would essentially graft the sharded-locking design's partitioning idea
  onto the actor pattern's structural simplicity.
- Revisit if: `ShardedStore`'s fixed `SHARD_COUNT = 64` is shown to matter
  — it was chosen to sit safely above the largest swept thread count, not
  measured against alternatives (see `RESULTS.md`'s open questions).
