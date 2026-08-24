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

## Out of scope for this roadmap (see architecture doc "where this can go
next")

- A fifth backend/mode implementing lazy (dirty-flag) cache invalidation,
  as an alternative to `HYBRID-BACKEND`'s eager write-through — see
  ADR-0003's revisit triggers.
- Real multi-hop graph traversal.
- Mixed read/write workload benchmarking.
- Memory-overhead-per-backend measurement.
- Dataset sizes beyond 1M.

These may become future roadmap units if `RESULTS.md`'s open questions
motivate them, but are not committed to now.
