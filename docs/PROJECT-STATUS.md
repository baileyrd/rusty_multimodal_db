# Project Status

- Last verified main commit: `fb17f09` (merge of PR #4, `scan_ages` 100K crossover investigation — memory-footprint data, now further updated in this pass with real L1D counters from `baileyai`, not yet committed). Prior checkpoints: `ec67ba3` (PR #3, cache-miss counts from `baileyai`); `d1d9169` (PR #1, `HYBRID-BACKEND`); `ae9a0d0` (PR #2, `scan_ages` wall-clock noise diagnosis); `5bfc8c5` (bootstrap, first pass) — `main` and the feature branch were identical at that commit (GitHub auto-set `main` to the first push into an otherwise-empty repo), so the owner elected to leave `main` as-is rather than force-reset it for a formal PR; PR #1 is the first real PR/merge in this repo's history.
- Verified at: 2026-08-25
- Current milestone: none active — see "Next" for open questions.
- Health: green (no blockers; open questions — the `scan_ages` 100K cache-miss crossover (investigated to the practical limit of available tooling), and write-heavy-workload behavior — tracked as open questions, not blockers)

## Completed

- `BOOTSTRAP` — charter, architecture, ADR-0001, ADR-0002, spec tree (`STORAGE-001..004`), roadmap, traceability, `AGENTS.md`, `WORKFLOW.md`, Cargo scaffolding, CI. On `main` at `5bfc8c5`.
- `GENERATOR` — `DogRecord` + seeded, configurable dataset generator (`src/record.rs`, `src/generator.rs`); 8 unit tests, all passing. On `main`.
- `BACKENDS` — `DogStore` trait + `AosStore`/`SoaStore`/`CanonicalStore` (`src/store/**`); 18 backend unit tests + 4 cross-backend equivalence tests, all passing. On `main`.
- `BENCH-SUITE` — Criterion suite (`benches/workloads.rs`), 4 workloads × 3 sizes × 3 backends = 36 cases, all run successfully. On `main`.
- `CACHE-MISS` — `perf-events`-gated target (`benches/cache_events.rs`), builds and links on Linux; the bootstrap session's own environment lacked hardware performance-counter access (confirmed via both `perf stat` and running the built binary), so real numbers were deferred at the time (see ADR-0002) — resolved by `CACHE-MISS-BAILEYAI` below. On `main`.
- `RESULTS` — `RESULTS.md` published with real `cargo bench` numbers, a verdict per workload, explicit canonical-store win/loss call-outs, and an open-questions section (first pass, 3 backends). On `main`.
- `HYBRID-BACKEND` — `CanonicalCachedStore` (`src/store/canonical_cached.rs`): `CanonicalStore`'s map + breed index, plus a packed `Vec<u32>` age cache kept in sync by eager write-through (ADR-0003). 8 unit tests (staleness test highest-priority) + 5 cross-backend tests, all passing. Wired into both `benches/workloads.rs` and `benches/cache_events.rs`. `RESULTS.md` revised to a 4-way comparison: `scan_ages` gap closed (from losing to both baselines to beating AoS by ~17.7× and landing within ~14% of SoA); `update_age` write-through costs ~1.5× at every size (well under the ~10× check-in threshold, so this proceeded without pausing) while remaining 4–5 orders of magnitude faster than AoS/SoA. Merged via PR #1 (`fa80a74` → merge commit `d1d9169`), CI green (`fmt, clippy, test`) before merge.
- `SCAN-AGES-NOISE-CHECK` — diagnosed `scan_ages`'s reported ~14% wall-clock gap to SoA at 1M: re-running at higher rigor (50 then 100 samples, up from 20) flipped the gap's *sign* twice (14% slower → 12% faster → 9% faster), which settled it as measurement noise from the shared/virtualized session environment, not a real cost. No code changed — `Vec::with_capacity`/`.clone()`-memcpy were already confirmed clean. `RESULTS.md` updated with the diagnostic paragraph. Merged via PR #2 (`3ab260d` → merge commit `ae9a0d0`).
- `CACHE-MISS-BAILEYAI` — real hardware cache-miss/cache-reference counts obtained on `baileyai` (bare-metal, real PMU access — the first run this repo has had with actual counter access). Required dropping `perf_event_paranoid` from 2 to 1 (session-only sysctl, not persisted). `benches/cache_events.rs` extended to cover all four workloads (previously only `get`/`same_breed`, on untested theories about what would dominate `scan_ages`/`update_age`'s cost). **Headline finding, and a correction to `SCAN-AGES-NOISE-CHECK`'s scope**: at 1M records, `scan_ages` on Canonical+cache has ~3.36× fewer cache misses than SoA (7,098 vs. 23,817) and a much lower miss rate (2.84% vs. 9.46%) despite near-identical reference counts — a real structural advantage that wall-clock timing (correctly found to be noisy/tied) couldn't see. New open question: an unexplained crossover at 100K where SoA has fewer misses instead, despite identical code paths at every size. `get`/`update_age`/`same_breed` cache-miss numbers all corroborate the existing wall-clock verdicts. Merged via PR #3 (`fe59233` → merge commit `ec67ba3`) from a separate session working directly on `baileyai`.
- `SCAN-AGES-CROSSOVER-INVESTIGATION` (PR #4) — narrowed the `scan_ages` 100K cache-miss crossover using real memory-footprint data (`examples/memory_footprint.rs`, no PMU needed): `CanonicalCachedStore` is ~87% larger than `SoaStore` at 100K but ~156% larger at 1M, ruling out the simplest "bigger co-resident working set → more misses" hypothesis without confirming a replacement. Also closed most of the separate "memory overhead" open question. Merged.
- `SCAN-AGES-CROSSOVER-L1D` — finished the investigation `benches/scan_ages_crossover.rs` started. First real run on `baileyai` found a bug in the bench itself (all four measurement groups shared one Criterion benchmark-group name, so the `change: ...%` lines compared different counter types against each other — fixed by giving each measurement type its own group name). The real, corrected numbers: **L1-data-cache miss rate is nearly identical between backends at both sizes** (60.25% vs. 59.40% at 100K, 57.93% vs. 58.29% at 1M — under 1.5 percentage points either way, vs. ~3× on the generic deeper-cache counters), ruling out L1 as the crossover's location. The last-level-cache half of the bench doesn't run on `baileyai`'s AMD chip (`ENOENT` — AMD's PMU doesn't implement the generic last-level-cache descriptor this crate's API uses; would need raw, model-specific perf events). **Closing this out as "investigated to the practical limit of available tooling," not pursuing raw AMD PMU events** — the decision this data needs to support doesn't depend on it. `RESULTS.md` updated. Not yet committed.

Evidence: `cargo test --all-features` / `cargo bench` output referenced in `RESULTS.md`; PR #1/#2/#3/#4 diffs and CI runs; `examples/memory_footprint.rs` output; real `baileyai` terminal output for `SCAN-AGES-CROSSOVER-L1D`.

## In progress

- `SCAN-AGES-CROSSOVER-L1D` — bench-file bug fixed, validated locally (fmt/clippy/build all clean), `RESULTS.md`/`PROJECT-STATUS.md` updated with the real `baileyai` numbers; not yet committed/pushed.

## Blocked

- (none)

## Next

1. Decide whether a write-heavy mixed-workload benchmark or a lazy-invalidation fifth backend/mode is worth pursuing, per `RESULTS.md`'s and ADR-0003's open questions — owner's call, not made here.
2. Memory overhead is measured (see above) but the *decision* it feeds — is `CanonicalCachedStore`'s ~2.6× footprint over `SoaStore` at 1M acceptable for the workload — is still the owner's call, not made here.
3. The `scan_ages` 100K crossover's exact last-level-cache mechanism remains unconfirmed and is not planned as further work (see `SCAN-AGES-CROSSOVER-L1D` above) unless the owner explicitly wants to pursue raw AMD PMU events.

## Validation

- `cargo fmt --all --check`: clean.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo test --all-features`: 41/41 passing (36 unit + 5 integration).
- `cargo bench` (`benches/workloads.rs`, default features): 48/48 cases completed (4 workloads × 3 sizes × 4 backends); results in `RESULTS.md`.
- `cargo bench --features perf-events --bench cache_events` on `baileyai`: full run completed with real (non-`<not supported>`) counter values, all four backends, all four workloads (PR #3). In this session's own (non-`baileyai`) environment, the same command still fails fast and deterministically with `Could not create counter: Os { code: 2, kind: NotFound, ... }` — expected, that environment still lacks PMU access; not a code defect, and no longer the blocker it once was now that `baileyai` numbers exist.
- `cargo check --benches --examples --features perf-events`: clean, including the two new targets (`scan_ages_crossover` bench, `memory_footprint` example).
- `cargo run --release --example memory_footprint`: runs cross-platform (no PMU needed); real numbers folded into `RESULTS.md`.

## Risks and decisions needed

- Repo is named `rusty_multimodal_db` on GitHub; the originating task suggested `rusty_multimodel_bench` as a less ambiguous name once the benchmark shape was clear. Recorded in the charter; no action taken since renaming a GitHub repo isn't something this session can do.
- `RESULTS.md`'s open questions now center on: write-heavy workload behavior (where eager write-through's cost profile could look different from the isolated-workload numbers here), and the memory-vs-performance tradeoff decision (`CanonicalCachedStore`'s ~2.6× footprint over `SoaStore` at 1M — measured, not a decision) — owner's call on both. The `scan_ages` 100K crossover is investigated to the practical limit of available tooling (L1 ruled out; last-level cache unreachable on `baileyai`'s AMD hardware via this method) and isn't planned as further work.
