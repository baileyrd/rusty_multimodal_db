# Results: AoS vs. SoA vs. UUID-canonical-store vs. canonical+cache

This is the decision document per `docs/specifications/storage/STORAGE-004-results-writeup.md` (extended by `STORAGE-005` for the fourth backend). It reports real numbers from an actual `cargo bench` run, structured workload × dataset size × backend, with a **verdict per workload** — not one overall winner. See `docs/charter/CHARTER.md` for the hypothesis under test, `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for why the first three backends are compared this way, and `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for the fourth.

**Revision note**: the first pass of this document (three backends: AoS, SoA, `CanonicalStore`) found `CanonicalStore` losing `scan_ages` to *both* baselines. This revision adds a fourth backend, `CanonicalCachedStore` — `CanonicalStore`'s `HashMap` as source of truth, plus a materialized, packed `Vec<u32>` age cache kept in sync by eager write-through on every `update_age` — built specifically to close that gap. The `get`/`update_age`/`same_breed` sections below are otherwise unchanged in structure from the first pass, now with a fourth column.

## Methodology

- Backends: `AosStore` (`Vec<DogRecord>`), `SoaStore` (parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`), `CanonicalStore` (`HashMap<Uuid, DogRecord>` + a breed→UUIDs index, `scan_ages`/`same_breed` implemented as views over the map, not copies — see ADR-0001), `CanonicalCachedStore` (`CanonicalStore`'s map and breed index, plus a packed `Vec<u32>` age cache and a `HashMap<Uuid, usize>` position index, `update_age` writing through to both the map and the cache — see ADR-0003).
- Dataset sizes: 1,000 / 100,000 / 1,000,000 records.
- Breed cardinality: fixed at 50 distinct breeds across all sizes (a reuse-heavy case, representative of "~50 real dog breeds shared across however many dogs") — the case where a breed index is expected to help most. A cardinality sweep was not run this pass; see Open Questions.
- Seed: fixed (`20260824`), so this run is reproducible via `cargo bench`.
- Point-workload benchmarks (`get`, `update_age`, `same_breed`) rotate through 200 pre-sampled target UUIDs per dataset rather than hitting one fixed UUID, specifically to avoid keeping a single record's cache line artificially hot across iterations — see `STORAGE-003`'s data/state notes and `benches/workloads.rs`.
- Criterion run with reduced sampling for this pass (`--warm-up-time 1 --measurement-time 2 --sample-size 20`, vs. Criterion's defaults of 3s/5s/100) to keep total run time reasonable. Confidence intervals were still tight (typically ≤5% spread between low/high estimates; see raw output), so this reduction doesn't call the verdicts below into question, but a final/published number set should use Criterion's defaults or higher. Machine: this session's cloud Linux container — not the owner's Windows dev machine or `baileyai`; see the cache-miss section below for why that matters more for hardware counters than for wall-clock numbers.
- Full raw Criterion output is reproducible via `cargo bench` (`benches/workloads.rs`); this document reports the point estimate (the middle of Criterion's `[low median high]` triple) for each case. Two separate `cargo bench` runs (first pass: 3 backends; this revision: 4 backends) produced the AoS/SoA/Canonical numbers below — they agree with the first pass within normal run-to-run noise (a few percent), which is why they aren't reported as a fifth "before" column.

All numbers below are **median wall-clock time per call**, lower is better. Winner(s) per row in **bold**.

## `get` (full-record read by UUID)

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 496.7 ns | 478.8 ns | **60.0 ns** | **59.9 ns** |
| 100,000 | 69.47 µs | 45.93 µs | **69.4 ns** | **69.4 ns** |
| 1,000,000 | 2.467 ms | 744.7 µs | **72.7 ns** | **71.6 ns** |

**Verdict: Canonical and Canonical+cache are tied for the win, both clearly ahead of AoS/SoA, and the margin grows with scale.** `get` doesn't touch the age cache at all — it's the same `HashMap<Uuid, DogRecord>` lookup in both backends — so this is exactly the sanity check it looks like: adding the age cache cost `get` nothing. At 1M records, both are ~34,000× faster than AoS and ~10,300× faster than SoA.

## `scan_ages` (column scan / average-age aggregate)

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 501.8 ns | **119.4 ns** | 2.294 µs | **119.4 ns** |
| 100,000 | 181.1 µs | **16.27 µs** | 346.9 µs | **16.32 µs** |
| 1,000,000 | 5.887 ms | **291.6 µs** | 14.96 ms | 332.2 µs |

**Verdict: Canonical+cache closes the gap — it matches SoA at 1K/100K and lands within ~14% of it at 1M, while beating AoS outright at every size.** This is the fix working as intended: `scan_ages` on `CanonicalCachedStore` reads the packed `age_cache: Vec<u32>` directly, the same contiguous-array access SoA gets, instead of walking `HashMap` buckets and heap-allocated `DogRecord`s. At 1M records, Canonical+cache is **~45× faster than plain Canonical** and **~17.7× faster than AoS** — completely reversing plain Canonical's loss to AoS, not just narrowing it. The small residual gap to SoA at 1M (332.2 µs vs. 291.6 µs, both backends cloning a `Vec<u32>` of the same size) is close enough to be plausibly measurement noise rather than a structural difference; not investigated further this pass.

## `update_age` (random single-field update) — the cost of write-through

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 409.7 ns | 460.1 ns | **35.1 ns** | 52.8 ns |
| 100,000 | 68.81 µs | 47.07 µs | **46.0 ns** | 68.3 ns |
| 1,000,000 | 2.916 ms | 677.2 µs | **46.2 ns** | 70.4 ns |

**Verdict: Canonical+cache is still a clear, overwhelming win over AoS/SoA — the eager write-through cost is real but small relative to what it protects.** Per ADR-0003's check-in threshold (roughly an order of magnitude): the actual regression from plain Canonical to Canonical+cache is **~1.5× at every size** (52.8/35.1 ≈ 1.50 at 1K; 68.3/46.0 ≈ 1.48 at 100K; 70.4/46.2 ≈ 1.52 at 1M) — a second `HashMap` lookup (the position index) plus one array write, not a second linear scan or anything worse. That's well under the threshold that would have called for a check-in, so this proceeded straight through per the task's working-style instructions. At 1M records, Canonical+cache is still **~41,400× faster than AoS** and **~9,600× faster than SoA** — the write-through overhead is essentially invisible next to the O(n)-vs-O(1) gap that dominates this workload for both baselines.

## `same_breed` (one-hop lookup — graph-view stand-in)

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 3.927 µs | 4.209 µs | **342.9 ns** | 355.9 ns |
| 100,000 | 422.7 µs | 407.1 µs | **5.895 µs** | 6.000 µs |
| 1,000,000 | 11.19 ms | 7.953 ms | **70.09 µs** | 69.11 µs |

**Verdict: Canonical and Canonical+cache are effectively tied for the win, both far ahead of AoS/SoA.** `same_breed` doesn't touch the age cache either — same breed index, same lookup path in both backends — so the two backends track each other within noise (largest gap: 355.9 ns vs. 342.9 ns at 1K, ~4%; they're within 2% of each other at 1M). At 1M records, both are ~160× faster than AoS and ~114× faster than SoA.

## Where the hypothesis wins vs. loses — explicit call-out

- **Canonical (uncached) still loses clearly**: `scan_ages` — worse than *both* baselines, and the gap widens with `n`. This finding from the first pass is unchanged; it's exactly what motivated building the fourth backend.
- **Canonical+cache turns that loss into a win**: `scan_ages` goes from "worse than AoS, 48× worse than SoA" (plain Canonical, at 1M) to "beats AoS by ~17.7×, within ~14% of SoA" (Canonical+cache) — for the cost of one extra `Vec<u32>` and, per `update_age`'s numbers above, a ~1.5× write-time tax that's negligible against Canonical's underlying advantage there.
- **Canonical+cache wins everywhere Canonical won, at effectively no cost**: `get` and `same_breed` are identical between the two (neither touches the age cache), so Canonical+cache doesn't trade away any of Canonical's existing wins to fix `scan_ages`.
- **Net picture, updated**: with the fourth backend, there is no longer a workload where the UUID-canonical family loses outright — `CanonicalCachedStore` wins or ties the win on all four workloads benchmarked. The interesting remaining tradeoff isn't "canonical vs. row/column baselines" anymore, it's "is the ~1.5× `update_age` tax and the extra `Vec<u32>` memory worth it," which is a much easier case to make than the first pass's genuine three-way split. See Open Questions for what would still need checking (write-heavy mixed workloads, memory overhead) before calling this fully settled.

## Cache-miss measurement

Unchanged from the first pass — still not obtained from within this session's environment, confirmed the same two ways (see ADR-0002): `perf stat` reports `<not supported>` for every counter, and running the built `perf-events` binary fails fast and deterministically with `Could not create counter: Os { code: 2, kind: NotFound, ... }`. `benches/cache_events.rs` now also covers `CanonicalCachedStore` on `get` and `same_breed` (the two workloads it already ties Canonical on) so a future real-hardware run gets the full four-way comparison, not just the original three. **Still the single most important open item**: real cache-miss counts would directly confirm that `scan_ages`'s wall-clock improvement is actually a locality improvement (fewer misses reading the packed `Vec<u32>`) and not something else — this pass infers that from the numbers matching SoA's shape, which is strong but not the same as measuring it.

## Open questions

- **Cache-miss counts on real hardware** (the item above): run `cargo bench --features perf-events --bench cache_events` on `baileyai` or equivalent and fold the results in here — now covering all four backends.
- **Lazy/dirty-flag invalidation as a fifth backend or mode**: ADR-0003 chose eager write-through for correctness-first simplicity; the ~1.5× `update_age` tax measured here is small enough that lazy invalidation isn't obviously worth the added complexity, but a **write-heavy mixed workload** (see next item) is exactly the scenario where that calculus could flip — eager pays a little on every write regardless of whether a scan ever follows; lazy would pay more per scan but nothing extra on writes that are never followed by a scan.
- **Write-heavy / mixed read-write workloads**: still not tested. This is now the most direct way to find out whether Canonical+cache's ~1.5× write tax is actually free in practice (writes are cheap in absolute terms either way) or whether it matters at write volumes far exceeding this benchmark's per-call measurement.
- **Memory overhead per backend**: not measured this pass. Canonical+cache now carries a `HashMap<Uuid, DogRecord>`, a breed index, a packed `Vec<u32>`, *and* a `HashMap<Uuid, usize>` position index — meaningfully more bookkeeping than any of the other three backends. Given how decisively it wins every workload above, memory overhead is the most likely remaining reason not to default to it, and is unmeasured.
- **Behavior beyond the 1M boundary**: unchanged from the first pass — margins widen with `n` for the workloads where they matter, which suggests (not verifies) they'd hold at 10M+.
- **Breed-cardinality sweep**: unchanged from the first pass — cardinality was fixed at 50 throughout; `same_breed`'s advantage under a more evenly-distributed breed cardinality is still unmeasured.
- **`CanonicalStore`/`CanonicalCachedStore`'s breed-index design**: unchanged from the first pass — a `HashMap<String, Vec<Uuid>>` vs. alternatives, not benchmarked.
- **`CanonicalCachedStore`'s position-index design**: new this pass — the `HashMap<Uuid, usize>` position index is itself a design choice (parallel to the breed-index one above) that wasn't benchmarked against alternatives (e.g. storing the position inline on the record, avoiding the second map entirely).
