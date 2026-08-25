# ADR-0008: Consolidate six rounds of empirical work into one recommended production type

- Status: Accepted
- Date: 2026-08-25
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0001-three-backend-empirical-comparison.md`, `ADR-0003-eager-write-through-cache-invalidation.md`, `ADR-0005-wal-snapshot-hybrid-durability.md`, `ADR-0006-tier-2-durability-architectures.md`, `ADR-0007-concurrency-strategies.md`, `STORAGE-011`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

Six rounds of empirical work — the original row/column/graph comparison,
the mixed read/write workload sweep, the eight-variant durability round,
and three concurrency-throughput passes (a 4-core container, the owner's
24-core Windows machine, and `baileyai`'s 32 cores) — have all pointed at
one combination: `CanonicalCachedStore`'s storage architecture, made
durable via mmap, made safe for concurrent access via one global
`RwLock`. Until this round, that combination existed only as three
separate, individually-benchmarked pieces spread across `src/store/`,
`src/durability/`, and `src/concurrency/`, each documented as one option
among several with no overall winner declared — appropriate while the
comparison was still open, but no longer reflecting where the evidence
actually points now that all three axes have been measured.

This round's job is narrow: **wire the three existing, closed pieces
together into one type someone would actually deploy, verify the
combination as a whole (not just its parts in isolation), and update the
crate's documentation to lead with it** — not re-derive, re-benchmark, or
change any of the three picks themselves. Each pick's justification lives
in its own round; this ADR's job is to record *that* they were combined
and *why each one*, not to re-argue any of them.

## Decision drivers

- **The evidence already exists — this round should not re-litigate it.**
  Every prior ADR (0001/0003/0005/0006/0007) and `RESULTS.md` section
  already made its case; this decision cites them rather than repeating
  their reasoning.
- **Don't touch closed code.** `CanonicalCachedStore`
  (`src/store/canonical_cached.rs`), the mmap durability variant
  (`src/durability/mmap_store.rs`), and the global-`RwLock` concurrency
  variant (`src/concurrency/global_rwlock.rs`) are each independently
  tested and benchmarked; this round wires them together, it doesn't
  modify their internals.
- **A composed type needs its own correctness proof, not just its parts'.**
  Mmap durability and `RwLock` concurrency have only ever been verified
  separately — each against a bare, non-durable/non-concurrent baseline.
  Nothing has tested them running together on top of the shared
  architecture until this round.
- **Keep the alternatives, don't delete them.** The other three storage
  backends, seven other durability variants, and three other concurrency
  strategies are the evidence this recommendation is built on. Removing
  them would make the recommendation unverifiable by anyone reading the
  repo later.
- **Minimal new dependencies — none should be needed.** This round
  composes existing pieces; no new crate is justified by "wire three
  things together."

## Considered options

### Composition shape: three nested structs, or reuse the existing mmap variant directly

1. **Three literally nested types**: `RwLock<MmapWrapper<CanonicalCachedStore>>`,
   where a new `MmapWrapper` adds mmap persistence around an otherwise
   untouched `CanonicalCachedStore`. Rejected — this doesn't match how the
   durability round actually built mmap support.
   `src/durability/mmap_store.rs`'s `MmapAgeStore` doesn't wrap
   `CanonicalCachedStore`; it rebuilds the same canonical-map/breed-index/
   adjacency-index/position-index architecture directly (with `age` backed
   by `MmapMut` instead of `Vec<u32>`), because `CanonicalCachedStore`'s
   fields are private and reusable only by duplicating the architecture,
   not by wrapping the closed type. Building a second, competing
   mmap-wrapper here would mean **two different mmap-backed
   implementations of the same idea** in the tree — a real maintenance and
   correctness-parity risk (which one is "the" mmap variant?) for zero
   benefit, and would touch `src/durability/` in spirit even if not in
   name.
2. **`RwLock<MmapAgeStore>`**, reusing the existing, closed mmap variant
   directly. Chosen — `MmapAgeStore` already *is* "`CanonicalCachedStore`'s
   architecture, made durable"; there is no separate, non-durable
   `CanonicalCachedStore` instance left to nest inside it. This is also
   the literal, one-line composition [`crate::concurrency::global_rwlock`]
   already established as its own pattern: wrap the existing storage type
   in one `RwLock`, change nothing inside it. `ProductionStore`
   (`src/production.rs`) is exactly that: `RwLock<MmapAgeStore>`, no new
   struct duplicating either piece's internals.

### Trait surface: `DogStore` only, `ConcurrentStore` only, or both

1. **`DogStore` only.** Rejected — would make `ProductionStore` a drop-in
   for existing single-threaded code, but its whole reason for existing
   (safe concurrent sharing) would be unreachable through the trait it
   implements; `DogStore::update_age`'s `&mut self` can't be called
   through a shared `Arc`.
2. **`ConcurrentStore` only** (matching `ShardedStore`/`DashMapStore`'s
   own scope-down to `get`/`update_age`/`scan_ages`). Rejected —
   `MmapAgeStore` already implements the *full* `DogStore` surface,
   including `same_breed`/`neighbors`; artificially narrowing
   `ProductionStore` to `ConcurrentStore`'s smaller surface would throw
   away real, already-available capability for no reason, and would break
   drop-in compatibility with every existing `S: DogStore`-generic
   benchmark/test helper.
3. **Both traits.** Chosen — `DogStore` for drop-in compatibility with
   every existing single-owner benchmark/test helper (reusing
   `benches/workloads.rs`'s generic runners, `bench_support::two_hop_neighbors`,
   `MixedWorkloadDriver::run_one`, with zero changes on their end);
   `ConcurrentStore` for the genuine multi-threaded sharing story that's
   the actual point of the `RwLock` layer, reusing the existing flagship
   stress test (`run_concurrency_stress_test`) and throughput harness
   (`benches/concurrency.rs`) with zero new test/bench infrastructure.
   The one cost: call sites where both traits are simultaneously in scope
   for a concrete `ProductionStore` value must disambiguate `get`/
   `scan_ages`/`update_age` with a qualified path (`DogStore::get(&store,
   id)`) — a real but narrow ergonomic tax, confined to test code in this
   pass (generic code bound by only one trait never sees the ambiguity).

### `ConcurrentStore::new`'s fixed, path-less, infallible signature vs. mmap's real path/fallibility requirement

1. **Change `ConcurrentStore::new`'s signature** to take a path and/or
   return `Result`. Rejected — this is a shared trait
   (`src/concurrency/mod.rs`) three other, closed variants already
   implement with a genuinely infallible, path-less constructor; changing
   its signature to accommodate one new implementor would touch
   `ShardedStore`/`DashMapStore`/`ActorStore` for a capability only
   `ProductionStore` needs, in tension with this round's "don't touch
   closed code" driver.
2. **Allocate a fresh, uniquely-named temp-file backing inside `new`,
   `.expect()` on failure.** Chosen — mirrors the "explicit, documented
   exception to no unwrap/expect outside tests" `GlobalRwLockStore`
   already established for `RwLock` poisoning: a temp-directory creation
   failure here means the OS temp dir is unusable, a genuinely exceptional
   environment problem no caller of an infallible constructor could
   sensibly recover from. Callers who need a real, caller-supplied,
   persistent path and genuine fallibility use
   `ProductionStore::create`/`open` directly, which return `Result`
   throughout — `ConcurrentStore::new` is specifically for the
   "share across threads, don't care where the backing file lives" case
   `benches/concurrency.rs`'s existing sweep already assumes for every
   other variant.

### Reorganization: physically move the alternatives under `src/comparisons/`, or a docs-only lead

1. **Physically move** `src/store/`, `src/durability/`, `src/concurrency/`
   under a new `src/comparisons/` tree. Considered, and explicitly checked
   in on before proceeding (per this round's own working-style
   instruction). Rejected for this pass: it would touch import paths
   across every bench, test, example, and internal cross-reference file in
   the crate (`benches/workloads.rs`, `benches/durability.rs`,
   `benches/concurrency.rs`, `benches/cache_events.rs`,
   `examples/memory_footprint.rs`, `tests/cross_backend.rs`,
   `src/bench_support.rs`, plus every module's own internal `use`
   statements) — a large, purely mechanical diff for a documentation-level
   goal ("read as secondary"), decided against in favor of the lighter
   option below when offered the choice directly.
2. **Docs-only lead**: no file moves; `src/lib.rs`'s crate-level doc
   comment, `README.md`, and `docs/architecture/SYSTEM-ARCHITECTURE.md`
   rewritten to put `ProductionStore` first and frame `store`/
   `durability`/`concurrency` explicitly as "benchmarked alternatives, not
   the recommended path." Chosen — achieves the actual goal (a reader
   encountering this crate sees the recommendation first) without the
   mechanical-move risk, and without touching any of the three closed
   modules' own internals or paths.

## Decision

- `src/production.rs` (new): `ProductionStore`, wrapping
  `RwLock<MmapAgeStore>` — `MmapAgeStore` (`src/durability/mmap_store.rs`,
  unmodified) already *is* "`CanonicalCachedStore`'s architecture, made
  durable via mmap"; this round adds nothing but the `RwLock` layer and
  the wiring to expose it through both `DogStore` and `ConcurrentStore`.
  `create`/`open`/`flush` delegate directly to `MmapAgeStore`'s own
  methods of the same name. No changes to `src/durability/mmap_store.rs`
  or `src/concurrency/global_rwlock.rs`.
- `ProductionStore` implements both `DogStore` (full surface, including
  `same_breed`/`neighbors`, for drop-in use with every existing
  single-threaded benchmark/test helper) and `ConcurrentStore` (`get`/
  `scan_ages`/`update_age`, for genuine multi-threaded sharing behind an
  `Arc`, reusing the existing flagship stress test and throughput
  harness).
- `ConcurrentStore::new` and the two `From` impls allocate a fresh temp-
  file backing via `bench_support::fresh_temp_dir` (already used by every
  durability variant's own tests) and `.expect()` on failure — the same
  documented-panic-exception pattern `GlobalRwLockStore` already
  established for `RwLock` poisoning.
- `src/lib.rs` gains `pub mod production;` and `pub use
  production::ProductionStore;`, and its crate-level doc comment is
  rewritten to lead with `ProductionStore` and frame `store`/
  `durability`/`concurrency` as benchmarked alternatives — no file moves
  (see Considered options above).
- New flagship integration test, `tests/production_integration.rs`: two
  phases of 16-thread × 2,000-iteration concurrent contention (matching
  `run_concurrency_stress_test`'s own rigor), separated by a genuine
  `drop` + reopen from disk, verified two ways — linearizability (replay
  the full two-phase write log sequentially against a fresh reference
  store) and persistence (a *third*, fresh open, after the second store
  handle is fully dropped, must see every write from both phases). This is
  the first test exercising mmap durability and `RwLock` concurrency
  together; each was previously verified only in isolation.
- `ProductionStore` is wired into `benches/workloads.rs` (listed first in
  every one of the seven existing workload groups — `get`/`scan_ages`/
  `update_age`/`same_breed`/`neighbors_one_hop`/`neighbors_two_hop`/mixed-
  workload) and into `benches/concurrency.rs`'s existing size × write-
  ratio × thread-count sweep, rather than a new, separate bench target —
  this reuses every existing generic runner unchanged and produces numbers
  directly comparable, in the same tables, to the four backends and four
  concurrency variants already benchmarked there.
- No new dependency.

## Consequences

### Positive

- The crate now has one clear answer to "what should I actually use,"
  stated first in `src/lib.rs`, `README.md`, and `RESULTS.md`, rather than
  requiring a reader to synthesize six sections' worth of per-axis
  recommendations themselves.
- Reusing `MmapAgeStore` directly (rather than building a second,
  competing mmap wrapper) means there is exactly one mmap-backed
  implementation in the tree — no risk of the two drifting out of sync or
  a reader being unsure which one is authoritative.
- The flagship integration test closes a real, previously-unverified gap:
  nothing had tested mmap durability and `RwLock` concurrency running
  together before this round, only each in isolation against a bare
  baseline.
- `ProductionStore` getting the *full* `DogStore` surface (not
  `ConcurrentStore`'s narrower scope-down) for free, because `MmapAgeStore`
  already implements it, means this composition didn't need to repeat
  `ShardedStore`/`DashMapStore`'s `same_breed`/`neighbors` scope
  limitation — a real advantage of picking the global-`RwLock` variant
  specifically, worth naming since it wasn't the deciding factor in
  ADR-0007's own concurrency recommendation.

### Negative / tradeoffs

- Implementing both `DogStore` and `ConcurrentStore` on one type means
  `get`/`scan_ages`/`update_age` are ambiguous at any call site where both
  traits are simultaneously in scope for a concrete `ProductionStore`
  value — callers must qualify (`DogStore::get(&store, id)`). This only
  bit test code in this pass (generic benchmark/test helpers bound by only
  one trait never see the ambiguity), but it's a real, permanent property
  of this type, not a one-time migration cost.
- `ConcurrentStore::new`'s temp-file-and-`.expect()` construction path
  means `ProductionStore::new` can panic under a genuinely exceptional
  environment failure (no space/permission on the OS temp dir) where every
  other `ConcurrentStore` implementor's `new` cannot fail at all — a real,
  if narrow, asymmetry `ConcurrentStore`'s own infallible signature can't
  express honestly for a durable variant. `ProductionStore::create`/`open`
  are the fallible escape hatch for callers who need one.
- The docs-only reorganization (chosen over physically moving files) means
  `src/store/`, `src/durability/`, `src/concurrency/` still sit at the top
  level of `src/`, alongside `src/production.rs`, rather than visually
  separated into a `comparisons/` tree — a reader browsing the file tree
  directly (rather than reading `src/lib.rs`'s doc comment first) doesn't
  get the same "this is secondary" signal a physical move would give.
- This round doesn't re-benchmark or re-verify any of the three picks
  themselves — it inherits whatever open questions each round's own
  `RESULTS.md` section and ADR already flagged (e.g. mmap's ages-only
  durability scope, `RwLock`'s throughput degradation under sustained
  high-thread-count write contention on a small dataset). None of those
  are resolved or worsened by this round; they're simply now inherited by
  `ProductionStore` too.

## Validation and revisit triggers

- Validated by: `src/production.rs`'s own unit tests (create/open/flush
  round-trip, `DogStore` drop-in behavior, `ConcurrentStore` behavior, the
  `From` impls, and `run_concurrency_stress_test::<ProductionStore>()`);
  the new flagship integration test,
  `tests/production_integration.rs::concurrent_writers_survive_a_drop_and_reopen_with_no_lost_updates`;
  `ProductionStore`'s numbers in `benches/workloads.rs`'s existing suite
  and `benches/concurrency.rs`'s existing sweep, reported in `RESULTS.md`'s
  `## Production recommendation` section.
- Revisit if: a future round pairs a *different* concurrency strategy
  (e.g. sharded locking) with mmap durability specifically for the small,
  write-heavy, high-thread-count shape ADR-0007 already identified as
  sharded's one real advantage — `ProductionStore` would then need a
  sibling type, not a change to this one, since the global-`RwLock` pick
  is deliberately the *default*, not the only combination this crate ever
  recommends for every shape.
- Revisit if: `RwLock`'s known throughput-degradation weakness (write-heavy
  contention on a small dataset — see ADR-0007) or mmap's known ages-only
  durability scope (see ADR-0006) become real constraints for a concrete
  deployment shape — the fix in either case is a new, sibling composed
  type (documented the same way this one is), not a change to
  `ProductionStore` itself, which should keep meaning "the default
  recommendation" rather than growing configuration knobs.
- Revisit if: the docs-only reorganization proves insufficient in
  practice (e.g. repeated confusion about which module to use) — the
  physical move to `src/comparisons/` remains available as a follow-up,
  scoped and check-in-gated the same way this round scoped it.
