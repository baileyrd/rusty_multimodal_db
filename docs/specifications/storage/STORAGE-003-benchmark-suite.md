# STORAGE-003 — Criterion benchmark suite and cache-miss instrumentation

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`
- Supersedes: none

## Purpose and scope

Define the benchmark harness structure: one Criterion group per workload,
each running all three backends at each dataset size, plus a
feature-gated, Linux-only cache-miss measurement path. See ADR-0002 for
the platform decision behind the latter.

## Non-goals

- No mixed read/write workload benchmarking in this pass (see
  `RESULTS.md` open questions).
- No memory-overhead measurement in this pass.
- No dataset sizes beyond 1M records in this pass.

## Context and terminology

Dataset sizes: 1,000 / 100,000 / 1,000,000 records, chosen per the task's
"1M rather than 10M as the upper bound for a first pass, to keep
iteration time reasonable" guidance.

## Requirements

- `STORAGE-003-FR-001`: Four Criterion benchmark groups, one per
  workload: `get` (full-record read by UUID), `scan_ages` (column
  scan/aggregate — average age across all records), `update_age` (random
  single-field update), `same_breed` (one-hop lookup).
- `STORAGE-003-FR-002`: Each group is parameterized over dataset size
  (1K/100K/1M) and backend (AoS/SoA/Canonical) via Criterion's
  `BenchmarkId`, so results render as a directly comparable table/chart.
- `STORAGE-003-FR-003`: All three backends in a given group are built
  from the identical generator output for that dataset size (same seed,
  same cardinality) — the dataset is generated once per size and shared
  across the three backend constructions within that group's setup, not
  regenerated per backend.
- `STORAGE-003-FR-004`: Benchmarked operations that need a target UUID
  (`get`, `update_age`, `same_breed`) select it via the shared seeded RNG
  so the choice is reproducible and identical across backends within a
  comparison.
- `STORAGE-003-FR-005`: A `perf-events` Cargo feature, gated to
  `cfg(target_os = "linux")`, adds a cache-miss-counting benchmark target
  using `criterion-perf-events`, measuring `cache-misses` and
  `cache-references` for at least the `scan_ages` and `get` workloads
  (the two most locality-sensitive). Not part of the default `cargo
  bench` run.
- `STORAGE-003-NFR-001`: The default (non-`perf-events`) benchmark suite
  builds and runs on Windows, Linux, and macOS without modification.

## Architecture and interfaces

`benches/workloads.rs` — default wall-clock suite, `criterion_group!` /
`criterion_main!`. `benches/cache_events.rs` — `perf-events`-gated
suite, only compiled when the feature is enabled (guarded in
`Cargo.toml`'s `[[bench]]` table via `required-features`).

## Data/state and invariants

- `get`/`update_age`/`same_breed` rotate through a fixed pool of
  pre-selected target UUIDs (sampled once per dataset size, not
  regenerated per iteration) rather than repeatedly hitting a single
  UUID. A single fixed target would keep that one record's cache line
  artificially hot across iterations and understate real point-lookup
  cost at scale — rotation is what makes the measurement representative.
- `update_age` overwrites a record's `age` in place; it does not resize
  any backing `Vec`/`HashMap` or touch the breed index (age is not part
  of any backend's breed index). Because each iteration's cost doesn't
  depend on the specific age value written by a prior iteration, the
  store is built once per dataset size and reused across iterations
  (with rotating target IDs, per above) rather than rebuilt from scratch
  every iteration — rebuilding a 1M-record store per Criterion iteration
  would make the suite impractically slow for no measurement benefit,
  since nothing about `update_age`'s cost is order- or
  history-dependent for this record shape.

## Errors, failure, recovery, and observability

Not applicable in the panic sense; benchmark setup uses the same
`Result`-returning constructors as the library and propagates failures
via `?` in setup closures where Criterion's API allows, or documents why
not where it doesn't.

## Security, privacy, and compatibility

Not applicable.

## Acceptance criteria

- `cargo bench` (default features) completes and produces Criterion
  output for all 4 workloads × 3 sizes × 3 backends = 36 benchmark
  cases.
- `cargo bench --features perf-events --bench cache_events` builds on
  Linux; documented as needing to run on real hardware (not this
  session's virtualized environment) to produce non-`<not supported>`
  counter values.
- `cargo build`/`cargo check` (no bench) succeeds on the default feature
  set without pulling in Linux-only dependencies on other platforms.

## Verification plan

Run `cargo bench` locally (or on `baileyai`) and inspect Criterion's
output/HTML report for all 36 cases completing without panics or
`<not supported>`-equivalent failures in the default suite.

## Traceability

Implements: "Criterion benchmark suite" and "cache-miss instrumentation"
deliverables. Depends on: `STORAGE-001`, `STORAGE-002`. Feeds:
`STORAGE-004` (results writeup consumes this suite's output).

## Open questions

- Whether 1M is worth pushing to 10M+ in a follow-up pass — deferred to
  `RESULTS.md`'s open questions per the task's own framing.

## Change history

- 0.1.0 (2026-08-24): Initial accepted draft.
