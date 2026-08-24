# Results: AoS vs. SoA vs. UUID-canonical-store

This is the decision document per `docs/specifications/storage/STORAGE-004-results-writeup.md`. It reports real numbers from an actual `cargo bench` run, structured workload × dataset size × backend, with a **verdict per workload** — not one overall winner. See `docs/charter/CHARTER.md` for the hypothesis under test and `docs/decisions/ADR-0001-three-backend-empirical-comparison.md` for why the comparison is structured this way.

## Methodology

- Backends: `AosStore` (`Vec<DogRecord>`), `SoaStore` (parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`), `CanonicalStore` (`HashMap<Uuid, DogRecord>` + a breed→UUIDs index, `scan_ages`/`same_breed` implemented as views over the map, not copies — see ADR-0001).
- Dataset sizes: 1,000 / 100,000 / 1,000,000 records.
- Breed cardinality: fixed at 50 distinct breeds across all sizes (a reuse-heavy case, representative of "~50 real dog breeds shared across however many dogs") — the case where a breed index is expected to help most. A cardinality sweep was not run this pass; see Open Questions.
- Seed: fixed (`20260824`), so this run is reproducible via `cargo bench`.
- Point-workload benchmarks (`get`, `update_age`, `same_breed`) rotate through 200 pre-sampled target UUIDs per dataset rather than hitting one fixed UUID, specifically to avoid keeping a single record's cache line artificially hot across iterations — see `STORAGE-003`'s data/state notes and `benches/workloads.rs`.
- Criterion run with reduced sampling for this first pass (`--warm-up-time 1 --measurement-time 2 --sample-size 20`, vs. Criterion's defaults of 3s/5s/100) to keep total run time reasonable. Confidence intervals were still tight (typically ≤5% spread between low/high estimates; see raw output), so this reduction doesn't call the verdicts below into question, but a final/published number set should use Criterion's defaults or higher. Machine: this session's cloud Linux container — not the owner's Windows dev machine or `baileyai`; see the cache-miss section below for why that matters more for hardware counters than for wall-clock numbers.
- Full raw Criterion output is reproducible via `cargo bench` (`benches/workloads.rs`); this document reports the point estimate (the middle of Criterion's `[low median high]` triple) for each case.

All numbers below are **median wall-clock time per call**, lower is better.

## `get` (full-record read by UUID)

| Size | AoS | SoA | Canonical |
|---|---:|---:|---:|
| 1,000 | 499.8 ns | 491.2 ns | **59.9 ns** |
| 100,000 | 76.26 µs | 47.43 µs | **73.6 ns** |
| 1,000,000 | 2.692 ms | 658.2 µs | **70.1 ns** |

**Verdict: Canonical wins clearly, and the margin grows with scale.** AoS and SoA both pay for a linear scan to find a UUID; Canonical's `HashMap` lookup is effectively O(1) regardless of size. At 1M records, Canonical is ~38,000× faster than AoS and ~9,400× faster than SoA. This is the clearest confirmation of the hypothesis's core claim for point access: a UUID-keyed canonical store is unambiguously the right structure when the access pattern is "find this one record by its identity."

## `scan_ages` (column scan / average-age aggregate)

| Size | AoS | SoA | Canonical |
|---|---:|---:|---:|
| 1,000 | 474.3 ns | **121.5 ns** | 2.383 µs |
| 100,000 | 176.3 µs | **15.80 µs** | 374.6 µs |
| 1,000,000 | 5.182 ms | **299.8 µs** | 14.33 ms |

**Verdict: Canonical loses — to *both* baselines, and the loss grows with scale. This is the hypothesis's clearest failure mode, and it's worth stating plainly: for a column scan, the canonical store isn't just slower than the column-oriented SoA baseline (expected — that's SoA's whole reason to exist), it's slower than the row-oriented AoS baseline too**, by a growing margin (2.0× at 100K, 2.8× at 1M). The reason: `scan_ages`'s "view" over `HashMap<Uuid, DogRecord>` has to walk scattered hash-table buckets and follow a heap pointer to each `DogRecord` (which itself contains a heap-allocated `String`) just to read one `u32` field — worse cache behavior than even AoS's "read the whole contiguous record to get one field," and far worse than SoA's "the `u32`s are already one contiguous array." SoA wins decisively at every size (11–17× faster than AoS, 24–48× faster than Canonical at scale).

## `update_age` (random single-field update)

| Size | AoS | SoA | Canonical |
|---|---:|---:|---:|
| 1,000 | 443.6 ns | 486.7 ns | **36.1 ns** |
| 100,000 | 79.18 µs | 45.27 µs | **44.9 ns** |
| 1,000,000 | 2.600 ms | 795.3 µs | **47.1 ns** |

**Verdict: Canonical wins clearly, and the margin grows with scale** — same shape as `get`, for the same reason: locating the record to mutate is the dominant cost, and only Canonical does that in O(1). At 1M records, Canonical is ~55,000× faster than AoS and ~16,900× faster than SoA.

## `same_breed` (one-hop lookup — graph-view stand-in)

| Size | AoS | SoA | Canonical |
|---|---:|---:|---:|
| 1,000 | 3.740 µs | 3.985 µs | **364.7 ns** |
| 100,000 | 451.2 µs | 430.5 µs | **5.434 µs** |
| 1,000,000 | 13.93 ms | 10.18 ms | **73.86 µs** |

**Verdict: Canonical wins clearly, and the margin grows sharply with scale.** AoS and SoA both scan every record to find breed matches; Canonical's breed→UUIDs index turns this into an O(1) index lookup plus O(k) for the k matches. At 1M records, Canonical is ~189× faster than AoS and ~138× faster than SoA. This is the strongest evidence so far that treating a one-hop access pattern as a *view with its own index* — rather than a full scan over any physical layout — is the right design, which is directly relevant to the eventual graph-access-pattern ambition in the charter.

## Where the hypothesis wins vs. loses — explicit call-out

- **Canonical loses clearly**: `scan_ages` — worse than *both* baselines, not just the specialized one, and the gap widens with `n`. If a real workload is dominated by column scans/aggregates, this design is the wrong choice as built.
- **Canonical wins clearly**: `get`, `update_age`, `same_breed` — all three point/one-hop access patterns, by margins that widen dramatically with `n` (from ~8–14× at 1K records to 4–5 orders of magnitude at 1M for `get`/`update_age`, and ~140–190× for `same_breed`).
- **Net picture**: this is exactly the hybrid outcome the charter flagged as an acceptable, likely finding — not a single winner. A canonical store is a strong choice when the workload is point-lookup- or one-hop-dominated; a materialized column cache (or the SoA layout directly) is the strong choice when column scans dominate. A hybrid backend (canonical store as source of truth, with a lazily/eagerly materialized column cache for `scan_ages`-shaped access) is the natural next design to test — see Open Questions.

## Cache-miss measurement

Per ADR-0002, cache-miss counting was **not obtained from within this bootstrap session** — this environment lacks hardware performance-counter access, confirmed two ways:

1. `perf stat -e cache-misses,cache-references,instructions,cycles -- /bin/true` reports `<not supported>` for every counter (see ADR-0002).
2. Building and running the `perf-events`-gated suite (`cargo bench --features perf-events --bench cache_events`) in this same environment fails immediately and deterministically:
   ```
   thread 'main' panicked at .../criterion-perf-events-0.4.0/src/lib.rs:71:
   Could not create counter: Os { code: 2, kind: NotFound, message: "No such file or directory" }
   ```
   This confirms the gap is real (the kernel here doesn't expose the perf subsystem to this sandboxed container at all) and that the failure is fast and legible, not a silent hang or a misleading number — it fails the same way the moment `perf_event_open` isn't available, on any Linux box without counter access.

The `perf-events` feature and `benches/cache_events.rs` target build and are ready to run as-is on hardware that does expose counters (the owner's `baileyai` Fedora box, or any bare-metal Linux with `perf_event_paranoid` permissive). **The wall-clock numbers above stand on their own for this pass, but the cache-miss numbers this hypothesis most directly concerns — to confirm *why* Canonical loses `scan_ages` and wins the point workloads, rather than just observing that it does — are deferred to a follow-up run on real hardware.** This is the single most important open item below.

## Open questions

- **Cache-miss counts on real hardware** (the item above): would confirm the locality explanation for the `scan_ages` loss and the point-workload wins, rather than inferring it from wall-clock shape alone. Run `cargo bench --features perf-events --bench cache_events` on `baileyai` or equivalent and fold the results in here.
- **A fourth, hybrid backend**: canonical store as source of truth, with a materialized `Vec<u32>` (or similar) column cache for `scan_ages`, invalidated/refreshed on `update_age`. The charter flagged this as a likely finding; these results make the case for actually building and benchmarking it. Not built in this pass — would need its own ADR (this is exactly the kind of consequential design choice that warrants one) since it reintroduces the "second physical copy" ADR-0001 deliberately avoided for the pure canonical-store test.
- **Write-heavy / mixed read-write workloads**: this pass benchmarks each workload in isolation. A realistic workload mixing `get`/`scan_ages`/`update_age`/`same_breed` in some ratio — and its effect on a hybrid backend's cache-invalidation cost — wasn't tested.
- **Memory overhead per backend**: not measured this pass. SoA and Canonical both carry indexing/bookkeeping overhead (parallel-array bookkeeping vs. hash table + breed index) that AoS doesn't; a real design decision needs to weigh this against the time numbers above, especially for a hybrid backend that would carry *both* a canonical map and a materialized cache.
- **Behavior beyond the 1M boundary**: the 1M numbers already show the dominant trends holding (margins widening, not narrowing, with `n`), which suggests they'd hold at 10M+, but that's not verified. Worth a follow-up pass if a design decision hinges on very large `n`, particularly because SoA/Canonical's relative advantage in `scan_ages`/point-lookup respectively could shift if working sets stop fitting in cache tiers differently at 10M+ than at 1M.
- **Breed-cardinality sweep**: this pass fixed cardinality at 50 (a reuse-heavy case). The generator supports sweeping cardinality (`STORAGE-001`); a follow-up could show how much of Canonical's `same_breed` advantage depends on breeds being heavily reused vs. more evenly distributed — relevant since the charter frames cardinality/reuse-ratio as central to whether normalization pays off.
- **`CanonicalStore`'s breed-index design**: ADR-0001 flagged the `HashMap<String, Vec<Uuid>>` breed index itself as a design choice that could be benchmarked against alternatives (e.g. a sorted `Vec` with binary search) — not done this pass.
