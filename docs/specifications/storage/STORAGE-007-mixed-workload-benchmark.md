# STORAGE-007 — Mixed read/write workload benchmark

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-003`, `STORAGE-005`
- Supersedes: none

## Purpose and scope

Every workload benchmarked so far in this crate (`get`, `scan_ages`,
`update_age`, `same_breed`, `neighbors_one_hop`, `neighbors_two_hop`)
isolates one call type per benchmark. Real usage blends reads and writes,
and `CanonicalCachedStore` carries one known cost from that blend — the
eager write-through tax on `update_age` (`STORAGE-005`/ADR-0003, ~1.5×
over plain `CanonicalStore` in isolation) — that could in principle
compound differently once writes are actually interleaved with the reads
they exist to keep correct for. This spec defines a benchmark driver that
issues a blended sequence of `get`/`update_age`/`scan_ages` calls against
a store, swept across write ratios, to test that directly.

## Non-goals

- Not a new backend or trait method — this is benchmark-harness-only work.
  No backend implementation changes; `DogStore` is unchanged.
- Not a general workload-scripting DSL — one configurable dimension
  (write ratio), with the read remainder always split evenly between
  `get` and `scan_ages`, not an arbitrary weighted mix of arbitrary
  operations.
- Not modeling bursty/correlated access patterns (e.g. "100 writes in a
  row, then 100 reads") — each call's op kind is drawn independently at
  random per the configured ratio. Flagged as an open question in
  `RESULTS.md`, not built here.
- Not sweeping write ratios beyond the three specified (10%/50%/90%), nor
  combining this sweep with a breed-cardinality or littermate-degree
  sweep — those remain separate, unswept dimensions per prior specs' open
  questions.

## Context and terminology

- **Write ratio**: the probability that a given call in the blended
  sequence is `update_age`. The remaining probability mass is split
  evenly between `get` and `scan_ages` (e.g. write ratio 0.10 → 10%
  `update_age`, 45% `get`, 45% `scan_ages`).
- **Blended sequence**: the ordered stream of calls a
  `MixedWorkloadDriver` produces against one store, one call per
  `run_one` invocation, each call type drawn independently per the
  configured write ratio.

## Requirements

- `STORAGE-007-FR-001`: A `MixedWorkloadConfig` type validates
  `write_ratio` to the inclusive range `[0.0, 1.0]` at construction,
  returning `Result<Self, MixedWorkloadConfigError>` — no panic on an
  invalid ratio.
- `STORAGE-007-FR-002`: A `MixedWorkloadDriver` issues one operation per
  call to `run_one`, chosen per `MixedWorkloadConfig`'s write ratio:
  `update_age` with probability `write_ratio`, else `get` or `scan_ages`
  with equal probability. Selection is driven by a seeded RNG, so a given
  `(config, seed)` pair always produces the same sequence.
- `STORAGE-007-FR-003`: `MixedWorkloadDriver` reuses the existing
  `RoundRobin` sample-ID rotation (the same infrastructure `get`,
  `update_age`, `same_breed`, and `neighbors` benchmarks already use) for
  every op kind that needs a target UUID — no new ID-sampling logic.
- `STORAGE-007-FR-004`: `run_one` returns `Result<(), StoreError>`,
  propagating `update_age`'s fallible path with `?` rather than
  `unwrap`/`expect`. In practice this can't fail when `sample_ids` comes
  from the same dataset the store was built from (verified by
  `mixed_workload_driver_never_errors_against_its_own_dataset`).
- `STORAGE-007-FR-005`: Two new Criterion wall-clock benchmark groups per
  write ratio (`mixed_workload_write10`, `mixed_workload_write50`,
  `mixed_workload_write90`) in `benches/workloads.rs`, each covering all
  four backends at all three existing dataset sizes (1K/100K/1M) — 3
  ratios × 3 sizes × 4 backends = 36 cases, matching the existing suite's
  backend/size structure. Matching groups added to the Linux-only
  `benches/cache_events.rs`.
- `STORAGE-007-FR-006`: The reported benchmark number per configuration is
  the median wall-clock time per operation in the blended sequence — not
  broken out per call type. This falls directly out of Criterion's own
  per-iteration median when each `b.iter` closure performs exactly one
  blended-sequence operation, which is how `run_mixed_workload` is
  structured — no separate aggregation step is needed.

## Architecture and interfaces

`src/bench_support.rs` — `MixedOp`, `MixedWorkloadConfigError`,
`MixedWorkloadConfig`, `MixedWorkloadDriver`, `MIXED_WRITE_RATIOS`
constant. `benches/workloads.rs`, `benches/cache_events.rs` —
`bench_mixed_workload`/`run_mixed_workload` wiring. No changes to
`src/store/**` or `src/generator.rs`.

## Data/state and invariants

- `MixedWorkloadDriver` owns its own `RoundRobin` cursor and `next_age`
  counter, constructed once per (backend, size, write-ratio) combination
  and reused across that benchmark's iterations — mirrors
  `run_update_age`'s existing "build once, rotate targets" pattern rather
  than rebuilding per iteration.
- `get`/`scan_ages` results are `black_box`ed (via `std::hint::black_box`,
  not a new dependency) before being discarded inside `run_one`, since
  this crate's `[profile.bench]` (`opt-level = 3, lto = true`) could
  otherwise prove an unused pure read has no observable effect and elide
  the call.

## Errors, failure, recovery, and observability

`MixedWorkloadConfig::new` is the only fallible constructor
(`MixedWorkloadConfigError::InvalidWriteRatio`). `MixedWorkloadDriver::run_one`
propagates `StoreError::NotFound` via `?` but this path is unreachable
given how this crate always constructs the driver (see FR-004).

## Security, privacy, and compatibility

Not applicable — synthetic in-memory data only, same as every other spec
in this tree.

## Acceptance criteria

- `cargo bench` (default features) includes `mixed_workload_write10`,
  `mixed_workload_write50`, and `mixed_workload_write90` groups, 36 total
  cases, completing without panics.
- `RESULTS.md` has a `## Mixed read/write workload` section, structured
  like the rest of the file (one table + verdict per configuration), that
  explicitly answers whether `CanonicalCachedStore` loses to
  `CanonicalStore` or `SoaStore` at any tested write ratio.
- No `src/store/**` or `src/generator.rs` changes — this spec is
  benchmark-harness-only, verified by the diff touching only
  `src/bench_support.rs` and `benches/*.rs`.

## Verification plan

- Unit tests in `src/bench_support.rs`: `MixedWorkloadConfig` bounds
  validation (including `NaN`), determinism given a fixed seed, different
  seeds producing different sequences, the configured ratio's long-run
  distribution (statistical check, large N, generous tolerance — see the
  test's own doc comment for why it's non-flaky), the driver never
  erroring against its own generated dataset (highest-priority
  correctness property — an id/store mismatch here would silently corrupt
  the reported numbers, similar in spirit to `STORAGE-006`'s
  edge-list-vs-adjacency-index consistency check), and an end-to-end check
  that a write-only driver actually mutates ages, not just returns `Ok`.
- 4-way, 3-ratio Criterion suite (`benches/workloads.rs`), and the
  Linux-only `benches/cache_events.rs` build (real counter numbers
  deferred to `baileyai` per ADR-0002's established pattern; confirmed
  this session's environment reproduces the same `NotFound` failure as
  every other workload, not silently assumed).

## Traceability

Implements: the "mixed read/write workload benchmark" deliverable.
Depends on: `STORAGE-001`/`STORAGE-002` (generator/trait this benchmarks
against), `STORAGE-003` (benchmark suite structure this extends),
`STORAGE-005` (the write-through tax this workload specifically tests
under blended load). Feeds: `RESULTS.md`'s `## Mixed read/write workload`
section.

## Open questions

- Write ratios beyond 90% weren't swept — the closest margin in this pass
  was at 90% writes, so pushing further in that direction (95%, 99%,
  100%) is the more informative next step if this is revisited.
- Bursty/correlated access patterns (writes and reads clustered rather
  than independently drawn per call) are unmeasured — see `RESULTS.md`'s
  open questions.
- This sweep didn't combine with a breed-cardinality or littermate-degree
  sweep — write ratio was the only swept dimension, cardinality/degree
  held at their existing fixed benchmark-suite values.

## Change history

- 0.1.0 (2026-08-25): Initial accepted draft.
