# Project Status

- Last verified main commit: (none yet — this is the initial bootstrap PR; `main` has no commits until it merges)
- Verified at: 2026-08-24
- Current milestone: all `BOOTSTRAP`/`GENERATOR`/`BACKENDS`/`BENCH-SUITE`/`CACHE-MISS`/`RESULTS` roadmap units implemented in one PR (see "Sequencing notes" in the roadmap for why this landed as one pass rather than five separate PRs)
- Health: green (no blockers; one deferred item — real cache-miss numbers — tracked as an open question, not a blocker)

## Completed

- `BOOTSTRAP` — charter, architecture, ADR-0001, ADR-0002, spec tree (`STORAGE-001..004`), roadmap, traceability, `AGENTS.md`, `WORKFLOW.md`, Cargo scaffolding, CI.
- `GENERATOR` — `DogRecord` + seeded, configurable dataset generator (`src/record.rs`, `src/generator.rs`); 8 unit tests, all passing.
- `BACKENDS` — `DogStore` trait + `AosStore`/`SoaStore`/`CanonicalStore` (`src/store/**`); 18 backend unit tests + 4 cross-backend equivalence tests, all passing.
- `BENCH-SUITE` — Criterion suite (`benches/workloads.rs`), 4 workloads × 3 sizes × 3 backends = 36 cases, all run successfully.
- `CACHE-MISS` — `perf-events`-gated target (`benches/cache_events.rs`), builds and links on Linux; confirmed (via both `perf stat` and running the built binary) that this session's own environment lacks hardware performance-counter access, so real numbers are deferred to a run on real hardware (see ADR-0002, `RESULTS.md`).
- `RESULTS` — `RESULTS.md` published with real `cargo bench` numbers, a verdict per workload, explicit canonical-store win/loss call-outs, and an open-questions section.

Evidence for all of the above: this PR's diff and `cargo test --all-features` / `cargo bench` output referenced in `RESULTS.md`.

## In progress

- None — all roadmap units for this pass are implemented pending PR review/merge.

## Blocked

- (none)

## Next

1. Merge this bootstrap PR, refresh `main`, and record the real commit SHA here.
2. Real cache-miss numbers from `baileyai` (or equivalent bare-metal Linux): `cargo bench --features perf-events --bench cache_events`, folded into `RESULTS.md`.
3. Decide, based on `RESULTS.md`, whether the hybrid backend (canonical store + materialized column cache) is worth a follow-up roadmap unit — would need its own ADR per `RESULTS.md`'s open questions.

## Validation

- `cargo fmt --all --check`: clean.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo test --all-features`: 32/32 passing (28 unit + 4 integration).
- `cargo bench` (`benches/workloads.rs`, default features): 36/36 cases completed; results in `RESULTS.md`.
- `cargo build --benches --features perf-events` (Linux): succeeds. Runtime execution in this session's own environment fails fast and deterministically with `Could not create counter: Os { code: 2, kind: NotFound, ... }` — expected, see ADR-0002/`RESULTS.md`; not a code defect.

## Risks and decisions needed

- Cache-miss hardware counters are not obtainable from this bootstrap session's own environment (verified two ways — see ADR-0002 and `RESULTS.md`). Real numbers require a follow-up run on `baileyai` or equivalent bare-metal Linux. Known, documented gap, not a blocker for the rest of the roadmap.
- Repo is named `rusty_multimodal_db` on GitHub; the originating task suggested `rusty_multimodel_bench` as a less ambiguous name once the benchmark shape was clear. Recorded in the charter; no action taken since renaming a GitHub repo isn't something this session can do.
- `RESULTS.md` surfaces a candidate fourth backend (canonical store + materialized column cache) as a strong follow-up given `scan_ages`'s clear loss for the pure canonical design — decision on whether to pursue it is the owner's, not made here.
