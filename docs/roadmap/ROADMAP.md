# Roadmap

Status vocabulary: `Proposed`, `Draft`, `Accepted`, `In Progress`,
`Implemented`, `Verified`, `Blocked`, `Deferred`, `Deprecated`,
`Superseded`.

| Unit | Outcome | Depends on | Specs | Exit gate | Status | Evidence |
|---|---|---|---|---|---|---|
| `BOOTSTRAP` | Charter, architecture, ADR-0001, ADR-0002, spec tree, roadmap, AGENTS.md/WORKFLOW.md/PROJECT-STATUS.md, Cargo scaffolding, CI | — | — | Docs merged, `cargo check` passes on empty scaffold | Implemented | this PR |
| `GENERATOR` | `DogRecord` + seeded, configurable dataset generator with unit tests | `BOOTSTRAP` | `STORAGE-001` | `cargo test` green for `src/generator.rs` | Implemented | this PR |
| `BACKENDS` | `DogStore` trait + `AosStore`/`SoaStore`/`CanonicalStore` with unit tests | `GENERATOR` | `STORAGE-002` | `cargo test` green for `src/store/**`, cross-backend equivalence tests pass | Implemented | this PR |
| `BENCH-SUITE` | Criterion suite: 4 workloads × 3 sizes × 3 backends, default features | `BACKENDS` | `STORAGE-003` | `cargo bench` completes all 36 cases | Implemented | this PR |
| `CACHE-MISS` | `perf-events`-gated cache-miss benchmark target, Linux-only | `BENCH-SUITE` | `STORAGE-003` | `cargo build --features perf-events` succeeds on Linux; ADR-0002 path documented | Implemented (build/feature only — real counter numbers deferred to a `baileyai` run, see `RESULTS.md`) | this PR |
| `RESULTS` | `RESULTS.md` with per-workload verdicts, explicit win/loss call-outs, open questions | `BENCH-SUITE`, `CACHE-MISS` | `STORAGE-004` | `RESULTS.md` merged, meets `STORAGE-004` acceptance criteria | Implemented | this PR |
| `HYBRID-BACKEND` | `CanonicalCachedStore` (canonical store + eager write-through age cache) closing `scan_ages`'s gap; ADR-0003; `RESULTS.md` revised to 4-way comparison | `RESULTS` | `STORAGE-005` | `cargo test`/`cargo bench` green for the 4th backend; `RESULTS.md` reports `scan_ages`/`update_age` specifically | Implemented | follow-on PR |
| `GRAPH-TRAVERSAL` | `littermate_of` edge generation; `DogStore::neighbors` (one-hop); ADR-0004; generic `two_hop_neighbors`; `neighbors_one_hop`/`neighbors_two_hop` benchmarks; `RESULTS.md`'s `## Graph traversal` section | `HYBRID-BACKEND` | `STORAGE-006` | `cargo test`/`cargo bench` green for `neighbors`/`two_hop_neighbors` across all 4 backends; `RESULTS.md` reports both workloads | Implemented | follow-on PR |
| `MIXED-WORKLOAD` | `MixedWorkloadDriver` (blended `get`/`update_age`/`scan_ages`, reusing `RoundRobin`); `mixed_workload_write{10,50,90}` benchmarks; `RESULTS.md`'s `## Mixed read/write workload` section, directly answering whether `CanonicalCachedStore` ever loses to `CanonicalStore`/`SoaStore` at any tested write ratio | `GRAPH-TRAVERSAL` | `STORAGE-007` | `cargo test`/`cargo bench` green for all 36 mixed-workload cases; `RESULTS.md` reports a verdict per (write ratio × size) | Implemented | follow-on PR |
| `DURABILITY-TIER1` | `CanonicalCachedStore`-only durability, five variants (WAL fsync, WAL buffered, snapshot rebuild, snapshot save-as-is, hybrid snapshot+WAL), built around a new shared `CanonicalCachedState` core; ADR-0005; `benches/durability.rs`'s per-write/checkpoint/load suite; `RESULTS.md`'s `## Durability` section | `MIXED-WORKLOAD` | `STORAGE-008` | `cargo test`/`cargo bench --bench durability` green for all 5 Tier 1 variants; `RESULTS.md` reports per-configuration numbers, the 1,000-write recoverability comparison, and an explicit recommendation | Implemented | follow-on PR |
| `DURABILITY-TIER2` | Three alternate durability architectures for `CanonicalCachedStore` (mmap-backed ages, LSM-tree-style with no compaction, `redb`-backed embedded engine), each scoped to the mutable `age` field only; ADR-0006; `RESULTS.md`'s `## Durability` section extended to cover all eight variants | `DURABILITY-TIER1` | `STORAGE-009` | `cargo test`/`cargo bench --bench durability` green for all 3 Tier 2 variants; `RESULTS.md` reports per-configuration numbers with the same rigor as Tier 1 | Implemented | follow-on PR |
| `CONCURRENCY-PROTOTYPES` | Four concurrent access strategies for `CanonicalCachedStore` (global `RwLock`, sharded locking, `dashmap`, actor/single-writer-thread), a new `ConcurrentStore` trait, a linearizability-style stress test, and a custom multi-threaded throughput harness; ADR-0007; `RESULTS.md`'s `## Concurrency` section | `DURABILITY-TIER2` | `STORAGE-010` | `cargo test` green for all four variants' flagship stress tests (16 threads × 2,000 iterations each, no lost updates/torn reads); `cargo bench --bench concurrency` completes the full size×ratio×thread-count×variant sweep; `RESULTS.md` reports a verdict per (size × write-ratio) configuration and an explicit recommendation | Implemented | follow-on PR |

## Sequencing notes

This roadmap is intentionally linear (each unit depends on the previous)
because the task specifies this exact working order: "bootstrap → dataset
generator → trait + three backends (with unit tests) → benchmark suite →
run it → write up results," with an explicit check-in point before the
benchmark run if the record shape, workload list, or dataset sizes need to
change. `CACHE-MISS` runs alongside `BENCH-SUITE` rather than strictly
after it in practice (they land in the same PR), but is listed separately
because it has its own exit gate (ADR-0002 compliance) distinct from the
default suite's.

`HYBRID-BACKEND` was itself an out-of-scope item from the original
roadmap, promoted to an actual unit once `RESULTS.md`'s first pass made
the case for it concrete (`scan_ages` losing to both baselines). That's
the expected lifecycle for this section: an item here becomes a real unit
when a finding motivates it, not on a fixed schedule.

`GRAPH-TRAVERSAL` follows the same lifecycle: it was the untested third
leg of the original row/column/graph hypothesis, promoted once a real
edge relationship (`littermate_of`) and 1-2 hop traversal became the
concrete, scoped next step per the task that motivated it — not a general
graph-query-layer unit (see ADR-0004 and `STORAGE-006`'s non-goals).

`MIXED-WORKLOAD` follows the same lifecycle again: "mixed read/write
workload benchmarking" was an explicit out-of-scope item on this roadmap
until it became the concrete next step for testing whether
`HYBRID-BACKEND`'s eager write-through tax (ADR-0003) changes the
recommendation once reads and writes are actually blended — it doesn't
(see `RESULTS.md`), so no ADR revision followed, per `STORAGE-007`'s own
scoping.

`DURABILITY-TIER1`/`DURABILITY-TIER2` are the first units in this
roadmap not motivated by a prior finding — every workload above them is
purely in-memory, and durability was always the next open question once
`CanonicalCachedStore` was settled as the recommendation. Split into two
units (rather than one) to match the two rigor levels the motivating task
specified: `DURABILITY-TIER1` covers five fully-tested, fully-benchmarked
WAL/snapshot/hybrid variants (ADR-0005); `DURABILITY-TIER2` covers three
lighter proof-of-concept alternate architectures (ADR-0006), explicitly
not held to the same production-hardening bar. Both apply to
`CanonicalCachedStore` only, per the same "don't build for backends
nobody would deploy" reasoning `HYBRID-BACKEND` established for choosing
which backend gets new capability.

`CONCURRENCY-PROTOTYPES` follows `DURABILITY-TIER1`/`DURABILITY-TIER2` as
the next previously-out-of-scope axis: every workload up to and including
durability assumes single-threaded access, and reader/writer safety was
the natural next open question once `CanonicalCachedStore` had both a
recommendation and a durability story. Deliberately kept as one unit,
unlike the durability split, because the task's own two-tier rigor split
(three full-rigor Tier 1 variants, one lighter Tier 2 variant) fit inside
a single roadmap entry without needing a second one — and deliberately
*not* combined with `DURABILITY-TIER1`/`DURABILITY-TIER2` into a
"concurrent durable store" unit, since the motivating task named that
combination as a distinct, later follow-up round rather than part of this
one (see ADR-0007's Context).

## Out of scope for this roadmap (see architecture doc "where this can go
next")

- A fifth backend/mode implementing lazy (dirty-flag) cache invalidation,
  as an alternative to `HYBRID-BACKEND`'s eager write-through — see
  ADR-0003's revisit triggers. `MIXED-WORKLOAD` tested the scenario most
  likely to motivate this (a write-heavy blended workload) and found no
  crossover, so this remains out of scope, not promoted.
- General N-hop (3+) or typed/weighted-edge graph traversal — `GRAPH-TRAVERSAL`
  scoped this to one relationship type (`littermate_of`) and 1-2 hops only;
  see ADR-0004 and `STORAGE-006`'s open questions.
- Memory-overhead-per-backend measurement.
- Dataset sizes beyond 1M.
- Write ratios beyond 90%, and bursty/correlated (as opposed to
  independently-drawn) mixed-workload access patterns — see
  `STORAGE-007`'s open questions.
- LSM-tree compaction — `DURABILITY-TIER2`'s `LsmStore` explicitly ships
  without it, per the motivating task's own instruction to scope this
  down rather than let it become its own multi-day project; see
  ADR-0006 and `RESULTS.md`'s durability open questions.
- Durability for `AosStore`/`SoaStore`/`CanonicalStore` — both
  `DURABILITY-TIER1` and `DURABILITY-TIER2` apply to `CanonicalCachedStore`
  only, per ADR-0005's Context.
- Extending any durability variant to persist more than `age` (i.e. full
  record durability for the Tier 2 mmap/`redb` variants, which currently
  persist only the mutable field) — see ADR-0006's revisit triggers.
- An explicit physical-disk (`fsync`) guarantee on `checkpoint()` for the
  Tier 1 variants whose `checkpoint` doesn't currently call `sync_all` —
  see ADR-0005's Consequences and revisit triggers.
- Batching multiple writes into one `redb` transaction — `RedbStore`
  commits one transaction per `update_age` call; see ADR-0006.
- A concurrency strategy paired with a specific durability variant (e.g.
  a sharded store where each shard also owns its own WAL) — named
  explicitly by the motivating task as a distinct future round, not part
  of `CONCURRENCY-PROTOTYPES`; see ADR-0007's Context.
- `ShardedStore`'s shard count swept against alternatives to its fixed
  `SHARD_COUNT = 64` — see `RESULTS.md`'s concurrency open questions.
- Scaling `ActorStore` beyond one owning thread (e.g. sharding the actors
  themselves) — its measured throughput ceiling is a structural property
  of the single-writer-thread pattern this pass didn't attempt to lift;
  see ADR-0007's revisit triggers.
- `same_breed`/`neighbors` support for the sharded-locking and `dashmap`
  concurrency variants — both cover only `get`/`update_age`/`scan_ages`,
  the `ConcurrentStore` trait's scope; see ADR-0007's Considered options
  and `STORAGE-010`'s Non-goals.

These may become future roadmap units if `RESULTS.md`'s open questions
motivate them, but are not committed to now.
