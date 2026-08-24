# ADR-0001: Test the UUID-canonical-store hypothesis empirically against AoS and SoA baselines, behind one trait

- Status: Accepted
- Date: 2026-08-24
- Deciders: baileyrd
- Related: `docs/charter/CHARTER.md`, `docs/architecture/SYSTEM-ARCHITECTURE.md`, `STORAGE-001`
- Supersedes/Superseded by: none

## Context

There's a storage-design hypothesis on the table: a canonical store keyed
by UUID (`HashMap<Uuid, DogRecord>`) can serve as the single source of
truth for row, column, and (eventually) graph access, with those access
patterns implemented as *views* over the canonical store rather than as
separate physical copies. This is attractive on paper — one source of
truth, no sync problem between representations — but the actual claim
being made is about **memory locality and access-pattern performance**,
which is exactly the kind of claim that intuition gets wrong. A
`HashMap<Uuid, DogRecord>` scatters records across the heap; a column
scan over it has to chase a pointer per record, which is precisely the
cache-unfriendly access pattern that SoA layouts exist to avoid.

Two conventional alternatives exist and are well understood:

- **AoS** (`Vec<DogRecord>`): contiguous records, good for full-record
  reads, bad for single-column scans (touches every field of every record
  even when only one field is needed).
- **SoA** (parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`): contiguous
  per-field arrays, good for column scans and aggregates, bad for
  full-record reconstruction (has to touch three separate arrays at the
  same index) and index-position bookkeeping.

The question this ADR answers is not "which one is best" — it's "how do
we structure the comparison so the answer is trustworthy."

## Decision drivers

- The hypothesis must be falsifiable and the harness must not be built to
  favor it. A benchmark suite written by someone who wants the canonical
  store to win is not a benchmark suite, it's a demo.
- The three backends must be genuinely comparable: same trait, same
  generated input, same workload code, no backend-specific fast paths in
  the harness.
- The canonical store's `scan_ages` and `same_breed` must be real
  *derived* views over `HashMap<Uuid, DogRecord>` — not a disguised second
  copy of the data (e.g. secretly also holding a `Vec<u32>` of ages). If
  it held that, it would just be a fourth backend (canonical store +
  materialized column cache) wearing the canonical store's name, and the
  comparison would be measuring the wrong thing.
- Results should support a real, possibly non-uniform decision — a
  workload-by-workload verdict, not a forced single winner.

## Considered options

1. **Implement the canonical store only and reason about AoS/SoA
   theoretically.** Rejected — this is exactly "picking one on theory,"
   which the task explicitly rules out. Cache behavior on modern hardware
   is not reliably predictable from first principles at these data sizes;
   it has to be measured.
2. **Implement all three behind one trait, with a shared Criterion
   benchmark suite parameterized by workload and dataset size, run against
   an identically-generated dataset.** Chosen. This is the only structure
   where a result can be attributed to the backend rather than to
   differences in test setup.
3. **Implement all three as independent ad hoc benchmarks (no shared
   trait).** Rejected — without a shared trait, it's too easy for subtle
   differences in how each backend is exercised (e.g. one backend getting
   a warmed cache the others don't) to contaminate the comparison, and the
   benchmark harness code triples for no benefit.

## Decision

Implement a single `DogStore` trait:

```rust
trait DogStore {
    fn get(&self, id: Uuid) -> Option<DogRecord>;
    fn scan_ages(&self) -> Vec<u32>;
    fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError>;
    fn same_breed(&self, id: Uuid) -> Vec<Uuid>;
}
```

and three implementations — `AosStore` (`Vec<DogRecord>`), `SoaStore`
(parallel `Vec<Uuid>`/`Vec<String>`/`Vec<u32>`), and `CanonicalStore`
(`HashMap<Uuid, DogRecord>` as the only physical copy, with `scan_ages`
and `same_breed` implemented as views/derived indexes over it — not as a
second copy).

For `CanonicalStore`, "view" means: `scan_ages` iterates the `HashMap`'s
values and reads `.age` per record (no cached `Vec<u32>`); `same_breed`
is backed by a `HashMap<String, Vec<Uuid>>` breed → UUID index built once
at construction (an index of *keys*, not a duplicate of the breed
strings' role as data — it exists to make one-hop lookup possible at all,
the same way a real graph-view would need *some* index structure, not to
cache a column). This is a deliberate boundary: an index that only stores
UUIDs to make a lookup fast is fair; a materialized `Vec<u32>` of ages
that duplicates `scan_ages`'s answer is not, because it would silently
turn `CanonicalStore` into the fourth hybrid design without benchmarking
it honestly as one.

A single Criterion benchmark suite (`benches/workloads.rs`) runs the same
four workloads (`get`, `scan_ages`, `update_age`, `same_breed`) against
all three backends at three dataset sizes (1K / 100K / 1M), fed by the
same seeded generator output per size. `RESULTS.md` reports a per-workload
verdict, explicitly calling out any workload where `CanonicalStore` loses
to a baseline.

## Consequences

### Positive

- Results are directly attributable to backend design, not test-harness
  artifacts.
- The hypothesis can genuinely lose on a workload without that being
  treated as a bug in the harness — a loss is a valid, useful outcome.
- A hybrid finding (canonical store wins some workloads, a baseline wins
  others, or a future materialized-column-cache hybrid wins yet others)
  is a first-class possible outcome, not something the structure forces
  away.
- The trait boundary keeps the door open for a fourth backend later
  (e.g. canonical store + materialized column cache) without touching the
  benchmark harness.

### Negative / tradeoffs

- `CanonicalStore`'s `same_breed` index is itself a design choice (a
  `HashMap<String, Vec<Uuid>>`) that could itself be benchmarked against
  alternatives (e.g. a sorted `Vec` with binary search) — out of scope for
  this pass; flagged as an open question in `RESULTS.md` if it turns out
  to matter.
- Restricting `DogRecord` to three fields (per the charter's
  no-speculative-generality constraint) means these results don't
  directly generalize to wider records; that's a known limitation, not an
  oversight.
- `get` returning an owned `DogRecord` (clone) rather than a reference
  keeps the trait object-safe and identical in cost shape across all three
  backends, but means `get`'s benchmark numbers include a clone cost that
  a reference-returning API wouldn't pay. This is consistent across all
  three backends, so the comparison stays fair, but it's worth noting for
  anyone comparing these absolute numbers to a different API design later.

## Validation and revisit triggers

- Validated by: the benchmark suite actually running against all three
  backends at all three sizes and producing `RESULTS.md`.
- Revisit if: a fourth backend (e.g. materialized column cache hybrid) is
  added — this ADR's "same trait, same generated input" structure should
  extend to it directly, but note the extension here or in a new ADR.
  **Done**: `CanonicalCachedStore` was added per
  `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md`,
  reusing this ADR's trait/dataset structure unchanged — see `RESULTS.md`'s
  revised 4-way comparison.
- Revisit if: `DogRecord`'s shape changes — per the charter, that's a
  hard-to-reverse decision requiring explicit sign-off, and would warrant
  re-running this comparison rather than assuming these results still
  hold.
