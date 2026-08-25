# System Architecture

## Start here: `ProductionStore`

Six rounds of benchmarking after this document's original context below
was written (mixed-workload, durability, and three concurrency-throughput
passes) converged on one recommended combination:
`CanonicalCachedStore`'s architecture, made durable via mmap, made safe
for concurrent reader/writer access via one global `RwLock`. That
combination is `src/production.rs`'s `ProductionStore` —
`RwLock<MmapAgeStore>`, implementing both `DogStore` (drop-in for
single-owner code) and `ConcurrentStore` (for genuine multi-threaded
sharing behind an `Arc`). See `docs/decisions/ADR-0008-production-default.md`
for which round justified each layer and `RESULTS.md`'s `## Production
recommendation` section for the numbers.

Everything else described below — the three-backend comparison this
document was originally written around, plus the durability
(`src/durability/`) and concurrency (`src/concurrency/`) modules added in
later rounds — remains in the tree as the empirical evidence
`ProductionStore` is built on, not the recommended path. Reach for one of
them directly only if your workload's shape is genuinely one of the narrow
corners named in ADR-0008/ADR-0007/ADR-0006 (e.g. sharded locking for a
small, write-heavy, high-thread-count deployment); otherwise, use
`ProductionStore`.

## Context

This crate has one runtime shape: a benchmark process (`cargo bench`) that,
per iteration, builds a dataset once, constructs each of the three backend
implementations from it, and drives the same workload functions against
each through a single trait object boundary. There is no server, no
client, no network, no persistence. The "system" is the process's own
memory during a `cargo bench` run, plus (optionally) `perf stat` wrapping
that same process on Linux.

```
                    ┌─────────────────────────┐
                    │   generator (seeded)     │
                    │  Vec<DogRecord>           │
                    └────────────┬─────────────┘
                                 │ same dataset feeds all three
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
      ┌───────────────┐  ┌───────────────┐  ┌────────────────────┐
      │ AosStore       │  │ SoaStore      │  │ CanonicalStore      │
      │ Vec<DogRecord> │  │ parallel Vecs │  │ HashMap<Uuid,       │
      │                │  │               │  │   DogRecord> + idx  │
      └───────┬────────┘  └───────┬───────┘  └──────────┬──────────┘
              │                   │                     │
              └───────────────────┴─────────────────────┘
                                 │  impl DogStore
                                 ▼
                    ┌─────────────────────────┐
                    │  Criterion benchmark      │
                    │  suite (4 workloads ×      │
                    │  3 sizes × 3 backends)     │
                    └────────────┬─────────────┘
                                 │
                                 ▼
                    ┌─────────────────────────┐
                    │  RESULTS.md              │
                    └─────────────────────────┘
```

## Architectural principles

1. **One trait, three implementations, zero shared cheating.** The
   benchmark harness only ever holds a `&dyn DogStore` / generic `S:
   DogStore`. It must not special-case any backend. This is what makes the
   comparison fair — if the harness code path differs per backend, the
   benchmark stops measuring the backend and starts measuring the harness.
2. **Views, not copies, for the canonical store.** The entire point under
   test is whether `scan_ages` and `same_breed` can be served as
   *derived* access over `HashMap<Uuid, DogRecord>` — via an index
   structure that maps back into the canonical store — rather than by
   maintaining a second physical copy of ages or breeds. If the canonical
   backend's `same_breed` secretly kept a `Vec<DogRecord>` on the side,
   the experiment would be testing nothing. See ADR-0001.
3. **Same generated input, always.** The generator is deterministic given
   `(n, cardinality, seed)`. Every backend for a given benchmark
   parameterization is built from the exact same `Vec<DogRecord>`, so any
   measured difference is attributable to the backend's memory layout and
   access pattern, not to differences in the data.
4. **No speculative generality.** `DogRecord` is fixed at three fields.
   There is no generic column/schema abstraction. If a future pass needs
   more fields or a different record shape, that is a deliberate,
   user-approved schema change (see the charter's engineering
   constraints), not something to pre-build support for now.
5. **Fallible operations return `Result`.** The only operation that can
   fail at the trait level is `update_age` (unknown UUID). `get` and
   `same_breed` return empty/`None` for an unknown UUID rather than an
   error — "not found" is a normal outcome for a lookup, not a failure
   condition.
6. **Cross-platform correctness, Linux-only cache-miss measurement.** The
   crate and its Criterion wall-clock benchmarks build and run on any
   platform Rust supports, including native Windows. Cache-miss
   instrumentation via `perf_event_open` is Linux-only by construction (it
   is a Linux kernel syscall); see ADR-0002 for how that's isolated so it
   degrades to "skipped, documented why" rather than a build failure
   elsewhere.

## Module boundaries

| Module | Responsibility | Depends on |
|---|---|---|
| `record` | `DogRecord` definition | — |
| `generator` | Seeded synthetic dataset generation | `record` |
| `store` | `DogStore` trait, `StoreError` | `record` |
| `store::aos` | Row-oriented backend | `store`, `record` |
| `store::soa` | Column-oriented backend | `store`, `record` |
| `store::canonical` | UUID-canonical backend with derived views | `store`, `record` |
| `store::canonical_cached` | Canonical store + materialized age cache (the architecture `production` builds on) | `store`, `record` |
| `durability` | Eight durability prototypes for `CanonicalCachedStore`'s architecture (WAL/snapshot/hybrid/mmap/LSM/`redb`) | `store`, `record` |
| `concurrency` | Four concurrent-access strategies for `CanonicalCachedStore` (global `RwLock`/sharded/`dashmap`/actor) | `store`, `record` |
| `production` | **Start here** — `ProductionStore`, wiring `durability::MmapAgeStore` and the `concurrency::global_rwlock` pattern together | `durability`, `concurrency`, `store`, `record`, `bench_support` |
| `benches/workloads` | Criterion harness, backend-agnostic | `generator`, `store`, all backends including `production` |

Dependency direction is one-way: backends depend on `store` and `record`,
never the reverse; `production` depends on one durability variant and one
concurrency variant (both otherwise-independent siblings of every other
variant in their own modules); the bench harness depends on everything,
nothing depends on the bench harness.

## Data model

```rust
struct DogRecord {
    id: Uuid,
    breed: String,
    age: u32,
}
```

No relationships, no nested structures, no optional fields. This is
intentional per the charter's non-goals — the hypothesis is about storage
*layout*, not schema expressiveness.

## Failure model

- `StoreError::NotFound(Uuid)` — the only error variant, returned by
  `update_age` when the UUID doesn't exist in the store. Every other
  trait method treats "not found" as a normal empty result, not an error.
- No panics in library code outside of documented invariant violations
  (there are none expected in this crate's scope). `unwrap()`/`expect()`
  are confined to test code.

## Operational concerns

There are none in the production-service sense (no deployment, no
observability stack, no SLOs) — this crate's only "operation" is a
benchmark run. What replaces ops concerns here:

- **Reproducibility**: the generator is seeded; a given `(n, cardinality,
  seed)` tuple always produces the same dataset.
- **Comparability**: benchmark parameterization (dataset sizes, workload
  definitions) lives in one place (`benches/workloads.rs`) so all three
  backends see identical treatment.
- **Cache-miss measurement platform gap**: recorded and worked around per
  ADR-0002, not silently dropped.

## Where this can go next (out of scope for this pass)

Recorded here so a future reader knows these were considered and
deliberately deferred, not missed:

- A real graph-view backend supporting multi-hop traversal.
- A materialized-column-cache hybrid backend (canonical store as source of
  truth, with a column cache rebuilt lazily/eagerly) — flagged as a likely
  finding in the charter, not yet implemented.
- Mixed read/write workload benchmarks (this pass benchmarks each workload
  in isolation).
- Memory-overhead-per-backend measurement (`RESULTS.md` open questions).
- Dataset sizes beyond 1M records.
