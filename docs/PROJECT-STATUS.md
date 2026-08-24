# Project Status

- Last verified main commit: `5bfc8c5` (bootstrap: charter/ADRs/spec tree/3-backend suite/`RESULTS.md`, first pass) — `main` and the feature branch were identical at this commit (GitHub auto-set `main` to the first push into an otherwise-empty repo), so the owner elected to leave `main` as-is rather than force-reset it for a formal PR; see this file's git history / session record for that call.
- Verified at: 2026-08-24
- Current milestone: `HYBRID-BACKEND` (4th backend, `CanonicalCachedStore`) implemented on `claude/storage-hypothesis-benchmark-c26t7h`, ahead of `main` this time (real diff exists, so this one *can* go through an actual PR) — see "Next" below.
- Health: green (no blockers; two deferred items — real cache-miss numbers, and the memory/write-heavy-workload open questions — tracked as open questions, not blockers)

## Completed

- `BOOTSTRAP` — charter, architecture, ADR-0001, ADR-0002, spec tree (`STORAGE-001..004`), roadmap, traceability, `AGENTS.md`, `WORKFLOW.md`, Cargo scaffolding, CI. On `main` at `5bfc8c5`.
- `GENERATOR` — `DogRecord` + seeded, configurable dataset generator (`src/record.rs`, `src/generator.rs`); 8 unit tests, all passing. On `main`.
- `BACKENDS` — `DogStore` trait + `AosStore`/`SoaStore`/`CanonicalStore` (`src/store/**`); 18 backend unit tests + 4 cross-backend equivalence tests, all passing. On `main`.
- `BENCH-SUITE` — Criterion suite (`benches/workloads.rs`), 4 workloads × 3 sizes × 3 backends = 36 cases, all run successfully. On `main`.
- `CACHE-MISS` — `perf-events`-gated target (`benches/cache_events.rs`), builds and links on Linux; confirmed (via both `perf stat` and running the built binary) that this session's own environment lacks hardware performance-counter access, so real numbers are deferred to a run on real hardware (see ADR-0002, `RESULTS.md`). On `main`.
- `RESULTS` — `RESULTS.md` published with real `cargo bench` numbers, a verdict per workload, explicit canonical-store win/loss call-outs, and an open-questions section (first pass, 3 backends). On `main`.
- `HYBRID-BACKEND` — `CanonicalCachedStore` (`src/store/canonical_cached.rs`): `CanonicalStore`'s map + breed index, plus a packed `Vec<u32>` age cache kept in sync by eager write-through (ADR-0003). 8 unit tests (staleness test highest-priority) + 5 cross-backend tests, all passing. Wired into both `benches/workloads.rs` and `benches/cache_events.rs`. `RESULTS.md` revised to a 4-way comparison: `scan_ages` gap closed (from losing to both baselines to beating AoS by ~17.7× and landing within ~14% of SoA); `update_age` write-through costs ~1.5× at every size (well under the ~10× check-in threshold, so this proceeded without pausing) while remaining 4–5 orders of magnitude faster than AoS/SoA. On `claude/storage-hypothesis-benchmark-c26t7h`, not yet merged.

Evidence: `cargo test --all-features` / `cargo bench` output referenced in `RESULTS.md`; this session's diff.

## In progress

- None — `HYBRID-BACKEND` is implemented pending PR review/merge.

## Blocked

- (none)

## Next

1. Open a PR for `HYBRID-BACKEND` (branch now genuinely diverges from `main` — unlike the bootstrap PR attempt, this one is mergeable normally), get it reviewed, and merge.
2. After merge: refresh `main`, record the real merge commit SHA here.
3. Real cache-miss numbers from `baileyai` (or equivalent bare-metal Linux): `cargo bench --features perf-events --bench cache_events`, now covering all four backends, folded into `RESULTS.md`.
4. Decide whether a write-heavy mixed-workload benchmark or a lazy-invalidation fifth backend/mode is worth pursuing, per `RESULTS.md`'s and ADR-0003's open questions — owner's call, not made here.

## Validation

- `cargo fmt --all --check`: clean.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo test --all-features`: 41/41 passing (36 unit + 5 integration).
- `cargo bench` (`benches/workloads.rs`, default features): 48/48 cases completed (4 workloads × 3 sizes × 4 backends); results in `RESULTS.md`.
- `cargo build --benches --features perf-events` (Linux): succeeds, now including `CanonicalCachedStore`. Runtime execution in this session's own environment still fails fast and deterministically with `Could not create counter: Os { code: 2, kind: NotFound, ... }` — expected, see ADR-0002/`RESULTS.md`; not a code defect.

## Risks and decisions needed

- Cache-miss hardware counters are still not obtainable from this session's own environment (verified two ways — see ADR-0002 and `RESULTS.md`). Real numbers require a follow-up run on `baileyai` or equivalent bare-metal Linux. Known, documented gap, not a blocker.
- Repo is named `rusty_multimodal_db` on GitHub; the originating task suggested `rusty_multimodel_bench` as a less ambiguous name once the benchmark shape was clear. Recorded in the charter; no action taken since renaming a GitHub repo isn't something this session can do.
- `RESULTS.md`'s open questions now center on memory overhead (`CanonicalCachedStore` carries the most bookkeeping of any backend) and write-heavy workload behavior (where eager write-through's cost profile could look different from the isolated-workload numbers here) — owner's call whether either is worth a follow-up pass.
