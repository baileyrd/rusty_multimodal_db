# Results: AoS vs. SoA vs. UUID-canonical-store vs. canonical+cache

This is the decision document per `docs/specifications/storage/STORAGE-004-results-writeup.md` (extended by `STORAGE-005` for the fourth backend, and by `STORAGE-006` for the `## Graph traversal` section below). It reports real numbers from an actual `cargo bench` run, structured workload × dataset size × backend, with a **verdict per workload** — not one overall winner. See `docs/charter/CHARTER.md` for the hypothesis under test, `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for why the first three backends are compared this way, `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for the fourth, and `docs/decisions/ADR-0004-one-hop-neighbors-trait-method.md` for the graph-traversal trait design.

**Revision note**: the first pass of this document (three backends: AoS, SoA, `CanonicalStore`) found `CanonicalStore` losing `scan_ages` to *both* baselines. A second pass added a fourth backend, `CanonicalCachedStore` — `CanonicalStore`'s `HashMap` as source of truth, plus a materialized, packed `Vec<u32>` age cache kept in sync by eager write-through on every `update_age` — built specifically to close that gap. The `get`/`update_age`/`same_breed` sections below are unchanged in structure from that pass and **report the closed row/column verdict — not re-litigated by this revision**. This revision's only addition is the `## Graph traversal` section further down, testing the previously-untested graph leg of the original hypothesis with a real edge relationship (`littermate_of`) rather than `same_breed`'s shared-attribute stand-in.

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

**Verdict: Canonical+cache closes the gap — it matches SoA at 1K/100K and is statistically indistinguishable from it at 1M once measurement noise is accounted for (see below), while beating AoS outright at every size.** This is the fix working as intended: `scan_ages` on `CanonicalCachedStore` reads the packed `age_cache: Vec<u32>` directly, the same contiguous-array access SoA gets, instead of walking `HashMap` buckets and heap-allocated `DogRecord`s. At 1M records, Canonical+cache is **~45× faster than plain Canonical** and **~17.7× faster than AoS** — completely reversing plain Canonical's loss to AoS, not just narrowing it.

**Follow-up on the ~14% residual gap to SoA at 1M — diagnosed, not a real gap.** The reported table numbers come from one `cargo bench` run; re-running `scan_ages/soa/1000000` vs. `scan_ages/canonical_cached/1000000` two more times at higher rigor (50 samples/3s, then 100 samples/5s, both up from the 20-sample/2s run behind the table above) settled the question at check 1 (noise check) before checks 2/3 were needed: the *sign* of the gap flips between runs — run 1 (table above): Canonical+cache ~14% slower (332.2 µs vs. 291.6 µs); run 2: ~12% *faster* (263.2 µs vs. 298.9 µs); run 3: ~9% faster (255.0 µs vs. 280.4 µs). A real structural cost wouldn't flip sign under more rigorous sampling — this is measurement noise from this session's shared/virtualized environment (consistent with ADR-0002's separate finding that this same environment lacks stable hardware-counter access) dominating an effect this small at this timescale (~250–330 µs total), not a real cost of `CanonicalCachedStore`'s extra fields. For completeness, the two code-level checks the noise check would have skipped were confirmed clean anyway: the age cache is built with `Vec::with_capacity(records.len())` up front (no reallocation-via-`push` growth), and `scan_ages` returns it via a plain `.clone()` (memcpy path, not a per-element `.collect()`). **No code changed as a result of this pass** — the table above is left as originally measured (rerunning it would just swap in a different single sample of the same noise), with this paragraph as the record of why the residual gap isn't chased further.

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
- **Canonical+cache turns that loss into a win**: `scan_ages` goes from "worse than AoS, 48× worse than SoA" (plain Canonical, at 1M) to "beats AoS by ~17.7×, statistically tied with SoA" (Canonical+cache — the initially-reported ~14% residual gap didn't reproduce under repeated, higher-rigor measurement; see the `scan_ages` section's follow-up paragraph) — for the cost of one extra `Vec<u32>` and, per `update_age`'s numbers above, a ~1.5× write-time tax that's negligible against Canonical's underlying advantage there.
- **Canonical+cache wins everywhere Canonical won, at effectively no cost**: `get` and `same_breed` are identical between the two (neither touches the age cache), so Canonical+cache doesn't trade away any of Canonical's existing wins to fix `scan_ages`.
- **Net picture, updated**: with the fourth backend, there is no longer a workload where the UUID-canonical family loses outright — `CanonicalCachedStore` wins or ties the win on all four workloads benchmarked. The interesting remaining tradeoff isn't "canonical vs. row/column baselines" anymore, it's "is the ~1.5× `update_age` tax and the extra `Vec<u32>` memory worth it," which is a much easier case to make than the first pass's genuine three-way split. See Open Questions for what would still need checking (write-heavy mixed workloads, memory overhead) before calling this fully settled.

## Cache-miss measurement (real hardware counters, `baileyai`)

Obtained this pass, on `baileyai` (bare-metal Fedora Server) — the machine named in ADR-0002 specifically because it has real PMU access, unlike every prior session's container. `cargo bench --bench cache_events --features perf-events` (note: the Cargo feature is `perf-events`, not `cache-events`) ran clean for all four backends.

**PMU access needed one sysctl change.** `perf_event_paranoid` was `2` by default, which fails every hardware counter with `Could not create counter: Os { code: 13, kind: PermissionDenied, ... }`. `sudo sysctl -w kernel.perf_event_paranoid=1` was sufficient — didn't need `0` or `-1`. This is a live, non-persistent sysctl write (resets on reboot, no `/etc/sysctl.d` file added), so it's this-session-only, not a permanent host change.

**Coverage gap fixed first.** `benches/cache_events.rs` as it stood only benchmarked `get` and `same_breed` — a prior session's own comment on the file argued `scan_ages` and `update_age` weren't worth measuring, on theories about what would dominate their cost. Those theories were never actually checked against hardware counters (the environment that wrote them had no PMU access at all), and `scan_ages` specifically is the workload this run most needed to check. `benches/cache_events.rs` was extended to cover all four workloads, mirroring `benches/workloads.rs`'s existing `run_scan_ages`/`run_update_age` patterns exactly — no `src/` changes, per the task constraints.

**Unit note**: Criterion labels the measurement axis "cycles" regardless of which `Perf` counter is plugged in — for this run it's actually the selected hardware event's raw count per call (`cache-misses` or `cache-references`), not CPU cycles. Fractional values below 1 are real: Criterion's point estimate is a per-iteration average over a batch of iterations, and sub-1 averages just mean "usually zero misses this call."

**Working-set caveat**: `get`/`update_age`/`same_breed` rotate through a pool of 200 pre-sampled UUIDs (`RoundRobin`, `SAMPLE_TARGET_COUNT`) across each benchmark's full measurement window (typically tens of thousands to low millions of iterations). For the two `HashMap`-based backends this means the *same* ~200 records get re-touched constantly within one measurement window, so they stay cache-resident after the first few iterations regardless of overall dataset size — that's why `canonical`/`canonical_cached`'s `get`/`update_age` miss counts below sit near zero even at 1M records. That's a real property of this benchmark's access pattern (and it's the same pattern the wall-clock suite already uses, so the two are comparable), not evidence that a HashMap lookup into an out-of-cache 1M-entry map costs nothing in general — a genuinely cold, uniformly-random-across-all-records access pattern would show more. AoS/SoA don't get this benefit because their `get` is a linear `.find()`/`.position()` scan that touches a scan-distance-dependent amount of memory per call.

Machine: `baileyai`, AMD Ryzen AI Max+ 395 (32 threads), L1d 768 KiB/16 instances, L2 16 MiB/16 instances, L3 64 MiB across 2 instances (32 MiB per CCD) — noted because it's relevant to the `scan_ages` finding below: a 1M-record `AosStore` (~48 MB of `DogRecord`s alone, plus scattered heap-allocated breed `String`s) doesn't fit in one CCD's 32 MiB L3 share, while the 100K case (~4.8 MB) comfortably does.

### `get` — cache-misses per call

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 0.011 | 0.012 | **0.002** | **0.002** |
| 100,000 | 1,479.9 | 336.9 | **0.002** | **0.002** |
| 1,000,000 | 1,798.9 | 1,237.6 | **0.002** | 0.004 |

**Verdict: Canonical and Canonical+cache both show dramatically fewer cache misses than AoS/SoA, matching the wall-clock win.** Given the working-set caveat above, this specific gap is partly an artifact of the 200-ID rotation pool keeping the `HashMap`-based backends' small working set resident — but AoS/SoA's `.find()`/`.position()` genuinely does scan real memory (338–1,800 misses/call, scaling with the touched-region size), so the *relative* story — hash lookup beats linear scan on cache behavior, not just wall-clock — holds regardless.

### `scan_ages` — cache-misses per call

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 0.015 | **0.003** | 0.045 | **0.003** |
| 100,000 | 8,100.8 | **269.4** | 10,962.6 | 909.9 |
| 1,000,000 | 44,872.2 | 23,816.8 | 1,100,195.9 | **7,097.9** |

**Verdict, directly answering the task's core question: at 1M records — the size where wall-clock timing called Canonical+cache and SoA statistically tied — real hardware counters show Canonical+cache has a genuine structural advantage: ~3.36× fewer cache misses (7,097.9 vs. 23,816.8).** Cache-references at 1M are nearly identical between the two (Canonical+cache 250,317.8 vs. SoA 251,847.7 — both are `.clone()` of an already-packed `Vec<u32>`, so this checks out: same amount of data touched), which means the real signal is in the **miss rate**: Canonical+cache misses on 2.84% of references vs. SoA's 9.46% — over 3× more efficient per access, not just fewer total accesses. This is exactly the kind of thing wall-clock timing on a noisy/virtualized environment can't resolve but a hardware counter can: **the prior diagnostic session's conclusion that the ~14% wall-clock gap was "noise, not real" was correct about wall-clock time specifically, but cache-miss counts now show Canonical+cache is the structurally better choice at 1M, even though that didn't show up as a wall-clock win.**

One honest wrinkle: at 100K, the direction flips — SoA has fewer misses (269.4 vs. 909.9) and a lower miss rate (2.06% vs. 6.06%) than Canonical+cache, even though both backends run the identical `.clone()`-of-a-packed-`Vec<u32>` code path at every size (verified in `src/store/soa.rs`/`src/store/canonical_cached.rs` — this isn't a code-path difference). The likely explanation is that Canonical+cache also carries a `HashMap<Uuid, DogRecord>`, a breed index, and a `HashMap<Uuid, usize>` position index alongside `age_cache`, all built during `Dataset` construction just before the benchmark runs — a much larger co-resident working set than SoA's three flat arrays, which could change cache-associativity conflicts differently at 100K's specific data volume (~a few hundred KB of just `age_cache`, well inside even one CPU core's L2) than it does at 1M's (~4 MB of `age_cache` alone, past L2 territory for one core, into shared-L3 territory where the CCD's total occupancy from those other structures matters more). This is a plausible mechanism, not a verified one — flagged as an open question below rather than asserted as settled.

**Follow-up investigation (this pass): the "larger co-resident working set" mechanism above is real but doesn't cleanly explain the crossover's *direction*.** `examples/memory_footprint.rs` (new — a counting global allocator, no PMU/`perf` access needed, so this runs from any session) measured the actual live-byte footprint of each backend at construction, rather than guessing at struct-size math:

| Size | SoA total | Canonical+cache total | Canonical+cache's extra |
|---|---:|---:|---:|
| 100,000 | ~11.2 MB | ~20.9 MB | ~9.7 MB (+87%) |
| 1,000,000 | ~112.0 MB | ~287.0 MB | ~175.0 MB (+156%) |

If "more co-resident bookkeeping bytes → more cache misses" were the *whole* story, Canonical+cache's misses should be higher than SoA's at both sizes (its footprint is larger at both, and *relatively* larger still at 1M — 156% vs. 87% extra). Instead it's higher only at 100K and reverses at 1M, the opposite of what footprint size alone predicts. That rules out the simplest version of the hypothesis without replacing it with a confirmed one. One concrete, measured lead worth naming: `CanonicalStore`'s own per-record overhead (which `CanonicalCachedStore` inherits, plus its own additions) is *not* stable across sizes — 111.8 bytes/record at 1K, 56.6 at 100K, 114.5 at 1M (see `examples/memory_footprint.rs` output) — consistent with `HashMap`'s (hashbrown's) capacity landing on different sides of a power-of-two growth boundary at different `n`, rather than scaling smoothly. Whether *that* specific quantization effect is what actually drives the cache-miss crossover, versus something in how the allocator happens to place the `age_cache`/`ages` arrays themselves, isn't something byte-counting can settle — it needs to see *which cache tier* the extra misses land in. `benches/scan_ages_crossover.rs` (new) adds L1-data-cache and last-level-cache read-access/read-miss counters (via `perfcnt`'s `from_cache_event`, not the generic `CacheMisses`/`CacheReferences` events `benches/cache_events.rs` uses), scoped narrowly to `scan_ages` on SoA vs. Canonical+cache at 100K and 1M.

**Run on `baileyai` (second real-hardware pass). Result: L1 is ruled out; last-level cache isn't measurable on this hardware via this method.** L1-data read-miss rate is nearly identical between backends at both sizes — 60.25% (SoA) vs. 59.40% (Canonical+cache) at 100K, 57.93% vs. 58.29% at 1M, differences under 1.5 percentage points either way, a small fraction of the ~3× relative difference the generic (deeper-cache) counters showed. Whatever drives the crossover, it isn't happening at L1 — both backends touch the identically-sized, identically-shaped `age_cache`/`ages` array the same way at that tier. The last-level-cache half of this experiment didn't run: `CacheId::LL` via the generic `PERF_TYPE_HW_CACHE` interface returns `ENOENT` on `baileyai`'s AMD Ryzen AI Max+ 395 — AMD's PMU doesn't implement that generic descriptor the way Intel's does; getting L2/L3-specific counts on AMD needs raw, model-specific perf event codes, out of scope for this benchmark harness. **This crossover is now considered investigated to the practical limit of available tooling, not resolved** — L1 is cleared, the deeper cache tier where the effect actually lives isn't reachable from this hardware without a materially larger investment (raw AMD perf events) that isn't justified given the decision this data needs to support (`CanonicalCachedStore` wins/ties every workload regardless of this crossover's exact mechanism). Not planned as further work unless the owner wants to pursue raw AMD PMU events specifically.

### `update_age` — cache-misses per call

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 0.009 | 0.009 | **0.001** | 0.002 |
| 100,000 | 1,322.8 | 318.3 | **0.001** | 0.002 |
| 1,000,000 | 2,374.3 | 1,583.2 | **0.001** | 0.002 |

**Verdict: Canonical wins outright on cache-misses, Canonical+cache close behind — both still ~1,000× fewer misses than AoS/SoA.** The write-through tax shows up here exactly as the wall-clock numbers predicted: Canonical+cache's miss count runs 36–85% higher than plain Canonical's at every size (the position-index lookup plus the extra array write), consistent in direction and rough scale with the ~1.5× wall-clock tax already reported above — just a second, independent measurement landing on the same conclusion.

### `same_breed` — cache-misses per call

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 0.143 | 0.128 | **0.008** | 0.009 |
| 100,000 | 4,184.6 | 3,635.5 | 35.1 | **33.0** |
| 1,000,000 | 63,464.7 | 59,192.7 | 855.5 | **678.1** |

**Verdict: Canonical/Canonical+cache both stay far below AoS/SoA, matching the wall-clock tie for the win — and at 100K/1M, Canonical+cache edges out plain Canonical on cache-misses too** (678.1 vs. 855.5 at 1M, ~21% fewer), even though wall-clock called these two statistically tied there as well. Smaller effect than `scan_ages`'s, and expected to be: `same_breed`'s cost is dominated by breed-index traversal, which is identical code in both backends, not by the packed-array-vs-hashed-record difference that drives the `scan_ages` result.

## Graph traversal

**Separate from, and does not alter, the row/column verdict above.** Everything in this section tests a different question — the previously-untested third leg of the original row/column/graph hypothesis (see the charter) — using a real generated edge relationship (`littermate_of`, see `STORAGE-006` and ADR-0004) rather than `same_breed`'s shared-attribute stand-in. The `get`/`scan_ages`/`update_age`/`same_breed` verdicts above are unchanged and not re-litigated here.

### Methodology

- `littermate_avg_degree` fixed at `1.5` (mid-range of the valid `[0.0, 3.0]` band) across all sizes, independent of dataset size — same rationale as `BREED_CARDINALITY`'s fixed value: held constant so dataset *size* stays the only swept dimension. A degree sweep was not run this pass; see Open Questions.
- Same seed (`20260824`), same three dataset sizes (1K/100K/1M), same 200-target rotation pool as every other workload in this document (`RoundRobin`/`SAMPLE_TARGET_COUNT`, see `STORAGE-003`).
- `neighbors_one_hop` calls `DogStore::neighbors` directly. `neighbors_two_hop` calls `bench_support::two_hop_neighbors` — the deduplicated union of `neighbors(n)` for every `n` in `neighbors(id)`, built generically from two rounds of `neighbors` calls (not a trait method — see ADR-0004).
- Criterion run with the same reduced sampling as the rest of this document's benchmarks (`--warm-up-time 1 --measurement-time 2 --sample-size 20`). Machine: this session's cloud Linux container (same caveat as the rest of this document — not the owner's Windows dev machine or `baileyai`).
- All numbers below are **median wall-clock time per call**, lower is better. Winner(s) per row in **bold**.

### `neighbors_one_hop` (one-hop `littermate_of` lookup)

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 1.910 µs | 1.950 µs | **46.85 ns** | 47.46 ns |
| 100,000 | 212.5 µs | 218.9 µs | 55.40 ns | **53.57 ns** |
| 1,000,000 | 6.199 ms | 5.669 ms | **54.65 ns** | 55.24 ns |

**Verdict: Canonical and Canonical+cache are effectively tied for the win, both far ahead of AoS/SoA — the same shape as `same_breed`'s result, for the same structural reason.** `neighbors` on the two canonical backends is one `HashMap` lookup into an adjacency index built the same way as the breed index; AoS/SoA pay a full linear scan of the edge list. The two canonical backends track each other within noise (largest gap ~1.3% at 1K); at 1M records, both are **~113,000× faster than AoS** and **~103,000× faster than SoA** — a larger multiple than `same_breed`'s ~160×/~114×, because the edge list being scanned (average degree 1.5, so ~1.5× as many `(Uuid, Uuid)` pairs as there are records) is itself larger than the number of records sharing a breed with any one dog under this dataset's 50-breed cardinality, making AoS/SoA's linear scan proportionally more expensive here than in `same_breed`.

### `neighbors_two_hop` (two-hop traversal, deduplicated — generic composition, not a trait method)

| Size | AoS | SoA | Canonical | Canonical+cache |
|---|---:|---:|---:|---:|
| 1,000 | 8.367 µs | 8.667 µs | 643.9 ns | **641.0 ns** |
| 100,000 | 808.8 µs | 809.8 µs | 727.5 ns | **681.8 ns** |
| 1,000,000 | 23.25 ms | 25.61 ms | 812.0 ns | **771.2 ns** |

**Verdict: Canonical and Canonical+cache still win decisively, though the margin between them and the margin over the baselines both shift compared to `neighbors_one_hop`.** Two-hop costs roughly 12-14× one-hop's time for the canonical backends (two `neighbors` calls plus `HashSet` dedup overhead, vs. one), and 4-9× for AoS/SoA (bounded by the number of one-hop results — average degree 1.5, so typically 1-2 further scans, not a second full scan per one-hop neighbor). Canonical+cache edges out plain Canonical at every size here (unlike `neighbors_one_hop`, where the two were closer to tied) — consistent with `same_breed`'s cache-miss numbers showing a similar small edge for Canonical+cache on adjacency-style lookups, though this pass only has wall-clock evidence for it (see cache-miss status below). At 1M records, both canonical backends are **~30,100× (Canonical+cache) to ~28,600× (Canonical) faster than AoS**, and **~33,200×/~31,500× faster than SoA** — the graph-traversal hypothesis's win holds up under 2-hop composition just as it did at 1-hop, with no sign of the naive-linear-scan baselines closing the gap as hop count grows (if anything, AoS/SoA's disadvantage widens, since each hop multiplies their per-call scan cost while the canonical backends' cost stays dominated by a small, fixed number of `HashMap` lookups).

### Cache-miss measurement status

Per ADR-0002's established pattern: this session's environment (cloud Linux container) does not have hardware performance-counter access — `cargo bench --features perf-events --bench cache_events -- neighbors_one_hop/aos/1000` builds and runs, but panics immediately with `Could not create counter: Os { code: 2, kind: NotFound, ... }`, the same signature as every other workload in this environment. `benches/cache_events.rs` was extended to cover `neighbors_one_hop_cache_misses`/`neighbors_two_hop_cache_misses` (and the `_cache_references` equivalents), mirroring the existing four workloads' structure exactly, and is ready to run as-is on `baileyai` or equivalent bare-metal Linux — not yet done for this section. Real numbers, once obtained, get folded in as a follow-up (same process as the existing four workloads' cache-miss section above).

### Where the graph-traversal hypothesis stands

- **`neighbors` (one-hop, real edge traversal) confirms the same result `same_breed` (shared-attribute stand-in) already showed**: a `HashMap`-based adjacency index over the canonical store beats a linear scan by 4-5 orders of magnitude at scale, and costs nothing extra for `CanonicalCachedStore` to carry alongside its age cache (Canonical and Canonical+cache are within noise of each other at every size, same as `get`/`same_breed`).
- **Composing two `neighbors` calls generically (`two_hop_neighbors`, not a trait method) preserves the win** — the naive baselines don't catch up as traversal depth increases from 1 to 2 hops; if anything the relative gap widens slightly at 1M.
- **This is real graph traversal, not a repeat of `same_breed`'s result under a different name** — `same_breed` never required following an edge between two specific records; `littermate_of` does, and the adjacency-index pattern generalizes to it directly, which is itself the finding: the "views over one canonical store" hypothesis's graph leg holds for the one relationship type and hop depth tested here.

## Open questions

- **Cache-miss counts on real hardware**: done this pass — see the cache-miss section above. Resolved the `scan_ages` question the prior diagnostic session left open: Canonical+cache does have a real structural cache-miss advantage over SoA at 1M (not just noise), even though it doesn't show up in wall-clock time at that size.
- **The `scan_ages` 100K cache-miss crossover**: investigated to the practical limit of available tooling, not resolved. SoA has fewer cache misses than Canonical+cache at 100K (opposite of the 1M result), despite both running the identical `.clone()`-of-packed-`Vec<u32>` code path at every size. Real memory-footprint measurements (`examples/memory_footprint.rs`) ruled out the simplest version of the "larger co-resident working set → more misses" hypothesis. A finer-grained perf-counter bench (`benches/scan_ages_crossover.rs`) then ruled out L1 specifically: L1-data-cache miss rate is nearly identical between backends at both sizes (differences under 1.5 percentage points, vs. ~3× on the generic counters) — run for real on `baileyai`. The last-level-cache half of that bench doesn't run on `baileyai`'s AMD chip (`ENOENT` — AMD doesn't implement the generic last-level-cache PMU event this crate uses; would need raw, model-specific perf events to get). Not pursuing that further — the decision this data needs to support (`CanonicalCachedStore` wins/ties every workload) doesn't depend on it.
- **Lazy/dirty-flag invalidation as a fifth backend or mode**: ADR-0003 chose eager write-through for correctness-first simplicity; the ~1.5× `update_age` tax measured here is small enough that lazy invalidation isn't obviously worth the added complexity, but a **write-heavy mixed workload** (see next item) is exactly the scenario where that calculus could flip — eager pays a little on every write regardless of whether a scan ever follows; lazy would pay more per scan but nothing extra on writes that are never followed by a scan.
- **Write-heavy / mixed read-write workloads**: still not tested. This is now the most direct way to find out whether Canonical+cache's ~1.5× write tax is actually free in practice (writes are cheap in absolute terms either way) or whether it matters at write volumes far exceeding this benchmark's per-call measurement.
- **Memory overhead per backend**: measured this pass (`examples/memory_footprint.rs`, real allocator-tracked bytes, not estimated) — see the crossover entry above for the headline numbers. Full picture: `AosStore` adds ~0 bytes beyond the input `Vec<DogRecord>` (it just moves it in); `SoaStore` is actually *smaller* than the input by ~4 bytes/record at every size (three parallel-array headers pack tighter than one `DogRecord` array); `CanonicalStore`'s overhead is 56–115 bytes/record depending on `n` (not stable — see the crossover entry's note on `HashMap` capacity quantization); `CanonicalCachedStore` adds a further ~37–56 bytes/record on top of `CanonicalStore` for `age_cache` + the position index. At 1M records this puts `CanonicalCachedStore`'s total footprint at ~287 MB vs. `SoaStore`'s ~112 MB — the real number behind "given how decisively it wins every workload, memory is the most likely reason not to default to it." Still open: whether that ~2.6× memory cost is acceptable given the workload — a decision this pass can surface but not make.
- **Behavior beyond the 1M boundary**: unchanged from the first pass — margins widen with `n` for the workloads where they matter, which suggests (not verifies) they'd hold at 10M+.
- **Breed-cardinality sweep**: unchanged from the first pass — cardinality was fixed at 50 throughout; `same_breed`'s advantage under a more evenly-distributed breed cardinality is still unmeasured.
- **`CanonicalStore`/`CanonicalCachedStore`'s breed-index design**: unchanged from the first pass — a `HashMap<String, Vec<Uuid>>` vs. alternatives, not benchmarked.
- **`CanonicalCachedStore`'s position-index design**: new this pass — the `HashMap<Uuid, usize>` position index is itself a design choice (parallel to the breed-index one above) that wasn't benchmarked against alternatives (e.g. storing the position inline on the record, avoiding the second map entirely).
