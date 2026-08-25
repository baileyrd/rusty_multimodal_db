# Project Status

- Last verified main commit: `ec67ba3` (merge of PR #3, real hardware cache-miss counts from `baileyai` folded into `RESULTS.md`). Prior checkpoints: `d1d9169` (PR #1, `HYBRID-BACKEND`); `ae9a0d0` (PR #2, `scan_ages` wall-clock noise diagnosis); `5bfc8c5` (bootstrap, first pass) — `main` and the feature branch were identical at that commit (GitHub auto-set `main` to the first push into an otherwise-empty repo), so the owner elected to leave `main` as-is rather than force-reset it for a formal PR; PR #1 is the first real PR/merge in this repo's history.
- Verified at: 2026-08-25
- Current milestone: none active — the cache-miss follow-up (PR #3) is the newest completed unit; see "Next" for the open questions it left.
- Health: green (no blockers; open questions — the `scan_ages` 100K cache-miss crossover, memory overhead, and write-heavy-workload behavior — tracked as open questions, not blockers)

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
- `SCAN-AGES-CROSSOVER-INVESTIGATION` — narrowed (didn't resolve) the `scan_ages` 100K cache-miss crossover. Re-verified code parity between `SoaStore`/`CanonicalCachedStore`'s `scan_ages` (already clean, no change needed). Added `examples/memory_footprint.rs` — a counting-global-allocator tool needing no PMU access, so it runs from any session — and measured real per-backend footprint for the first time (also closes most of the separate "memory overhead" open question): `CanonicalCachedStore` is ~87% larger than `SoaStore` at 100K but ~156% larger at 1M, which predicts *more* relative disadvantage at 1M if footprint size alone drove the cache-miss crossover — the opposite of what was observed (Canonical+cache had *more* misses at 100K but *fewer* at 1M). Rules out the simplest version of the "bigger co-resident working set → more misses" hypothesis without replacing it with a confirmed one. Added `benches/scan_ages_crossover.rs` (finer-grained L1-data/last-level cache tier counters via `perfcnt::from_cache_event`, narrowly scoped to `scan_ages`/SoA/Canonical+cache at 100K and 1M) — compiles, not run from this session (same PMU gap), ready for `baileyai`. No `src/` changes. Not yet in a PR.

Evidence: `cargo test --all-features` / `cargo bench` output referenced in `RESULTS.md`; PR #1/#2/#3 diffs and CI runs; `examples/memory_footprint.rs` output for `SCAN-AGES-CROSSOVER-INVESTIGATION`.

## In progress

- `SCAN-AGES-CROSSOVER-INVESTIGATION` — implemented and validated locally (fmt/clippy/test/doc all clean); not yet committed/pushed/PR'd.

## Blocked

- (none)

## Next

1. Commit/push/PR `SCAN-AGES-CROSSOVER-INVESTIGATION`, then run `cargo bench --features perf-events --bench scan_ages_crossover` on `baileyai` to actually settle the `scan_ages` 100K crossover (`RESULTS.md`'s open questions) — the memory-footprint data narrowed the hypothesis space but didn't confirm a mechanism.
2. Decide whether a write-heavy mixed-workload benchmark or a lazy-invalidation fifth backend/mode is worth pursuing, per `RESULTS.md`'s and ADR-0003's open questions — owner's call, not made here.
3. Memory overhead is now measured (see above) but the *decision* it feeds — is `CanonicalCachedStore`'s ~2.6× footprint over `SoaStore` at 1M acceptable for the workload — is still the owner's call, not made here.

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
- `RESULTS.md`'s open questions now center on: the still-open `scan_ages` 100K cache-miss crossover (narrowed, not resolved — needs a `baileyai` run of `benches/scan_ages_crossover.rs`), and write-heavy workload behavior (where eager write-through's cost profile could look different from the isolated-workload numbers here) — owner's call whether either is worth a follow-up pass. Memory overhead is now measured, not just flagged, but the tradeoff decision it feeds is still open.
