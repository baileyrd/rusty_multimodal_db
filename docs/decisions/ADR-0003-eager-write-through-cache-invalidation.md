# ADR-0003: Eager write-through cache invalidation for `CanonicalCachedStore`

- Status: Accepted
- Date: 2026-08-24
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0001-three-backend-empirical-comparison.md`, `STORAGE-005`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

`RESULTS.md` (first pass) showed `CanonicalStore` losing `scan_ages` to
*both* baselines — not just SoA, but even AoS — because reading `age` out
of `HashMap<Uuid, DogRecord>` means chasing a hash-table bucket and then a
heap-allocated `DogRecord` (with its own heap-allocated `String`) per
value, for a `u32` that a packed array would give up for free. The
natural fix, flagged as a follow-up in that document, is a fourth
backend: keep the canonical `HashMap` as source of truth for `get` and
`same_breed`, but additionally hold a packed `Vec<u32>` age cache for
`scan_ages` — `CanonicalCachedStore`.

Adding a second physical copy of `age` reopens a question ADR-0001
deliberately closed for `CanonicalStore` itself: how does the cache stay
correct when `update_age` writes a new value? Two standard strategies
exist:

- **Eager (write-through)**: every `update_age` call writes both the
  canonical record and the cache slot, in the same call. The cache is
  never observably stale; the cost is paid on every write.
- **Lazy (dirty-flag / rebuild-on-read)**: `update_age` only writes the
  canonical record and marks the cache dirty; `scan_ages` checks the flag
  and rebuilds the whole cache from the canonical map if dirty, then
  serves from it. Writes stay cheap; the first read after a write (or a
  burst of writes) pays a full rebuild instead of one slot.

## Decision drivers

- **Correctness is not negotiable.** A cache that can return a stale
  `age` is worse than no cache — it would silently corrupt exactly the
  aggregate (`scan_ages`'s average-age use case) this backend exists to
  make fast. Whichever strategy is chosen must make staleness
  unreachable by construction and by test, not just unlikely.
- **Simplicity first, backed by a direct test.** The task explicitly
  asked for eager first, as the simplest correct option, and named the
  exact test that would catch the one bug this backend can introduce
  that the other three can't: call `update_age`, then immediately call
  `scan_ages`, assert the new value is present. Lazy invalidation is
  harder to get right (the dirty flag itself becomes a piece of state
  that can be forgotten in a code path, e.g. if a future method mutates
  breed and someone forgets it doesn't touch the age cache but does
  touch the breed index) and harder to verify with as direct a test.
- **Report the real cost, don't hide it.** If eager write-through blunts
  `update_age`'s current several-orders-of-magnitude win over AoS/SoA by
  an amount that matters, that has to be stated plainly in `RESULTS.md`,
  not quietly traded away for a better number by switching strategies
  mid-analysis.

## Considered options

1. **Lazy / dirty-flag invalidation.** Not chosen for this pass. Keeps
   `update_age` as cheap as `CanonicalStore`'s (a single `HashMap`
   write), at the cost of a rebuild on the next `scan_ages` after any
   write — which, under a workload that interleaves writes and scans
   (exactly the "write-heavy mixed workload" `RESULTS.md`'s open
   questions already flagged as untested), could be *worse* than eager
   in the aggregate, not better, since a full rebuild costs more than
   incrementally maintaining N single-slot writes. It also introduces a
   second piece of state (the dirty flag) that has to be threaded
   through every mutating path correctly forever, which is a larger
   surface for the exact staleness bug this backend most needs to avoid.
2. **Eager write-through.** Chosen. `update_age` writes the canonical
   record and the cache slot in the same call, using a `HashMap<Uuid,
   usize>` position index (built once at construction, alongside the
   packed `Vec<u32>`) to find the slot in O(1) rather than scanning the
   cache. Simplest strategy to reason about and to test — the staleness
   test above is a direct, permanent regression guard.

## Decision

`CanonicalCachedStore::update_age` writes through to both the canonical
`HashMap<Uuid, DogRecord>` and the packed `age_cache: Vec<u32>` in the
same call, locating the cache slot via a `position_index: HashMap<Uuid,
usize>` built at construction. `scan_ages` reads `age_cache` directly —
no rebuild step, no dirty flag, no possibility of serving a value the
last `update_age` call didn't already write.

The correctness test in `src/store/canonical_cached.rs`
(`scan_ages_reflects_update_age_immediately`) is the highest-priority
test for this backend, ahead of the routine hit/miss/not-found tests
that mirror `CanonicalStore`'s — it is the one failure mode specific to
this backend that the other three structurally cannot have.

**This choice is provisional on the measured cost.** Per the task that
motivated this ADR: if benchmarking shows eager write-through regresses
`update_age` by roughly an order of magnitude or more relative to
`CanonicalStore`'s existing win, that is reported plainly in
`RESULTS.md` rather than silently causing a switch to lazy invalidation
after the fact. See `RESULTS.md`'s `update_age` section for the actual
number from this pass and whether it crossed that threshold.

**Measured result** (see `RESULTS.md` for the full table): the regression
is ~1.5× at every dataset size (1K/100K/1M), from the second `HashMap`
lookup (the position index) plus one array write — nowhere near the
order-of-magnitude threshold above, and negligible next to
`CanonicalCachedStore`'s ~9,600–41,400× advantage over SoA/AoS on this
same workload at 1M records. This choice stands as made; lazy
invalidation remains a documented option for a write-heavy workload this
pass didn't test (see `RESULTS.md`'s open questions), not something this
result calls for switching to now.

## Consequences

### Positive

- Cache correctness is structural, not maintained by convention — there
  is no code path where `scan_ages` can observe an age `update_age`
  already reported success for but hasn't applied.
- Directly testable with one focused test, rather than needing to reason
  about dirty-flag lifecycle across every mutating method.
- Straightforward to extend if a future breed-mutating operation is
  added (unlike `CanonicalStore`, this backend's invalidation strategy
  doesn't rely on "no operation currently changes breed" the way
  `CanonicalStore`'s breed index comment does — though this backend
  doesn't cache breed at all, so that specific risk doesn't apply to it
  either).

### Negative / tradeoffs

- `update_age` on this backend pays two `HashMap` lookups (`records`,
  then `position_index`) plus an array write, versus `CanonicalStore`'s
  one `HashMap` lookup — a real, measured cost reported in `RESULTS.md`.
- Under a write-heavy workload not yet benchmarked (see `RESULTS.md`'s
  open questions), eager's per-write cost is paid on every single write
  regardless of whether `scan_ages` is ever called again before the next
  write — a lazy strategy would amortize better in a write-only
  workload, at the cost of the correctness/complexity tradeoffs above.
  Not measured this pass.
- Two physical copies of `age` now exist for this backend specifically
  (unlike `CanonicalStore`, which ADR-0001 kept to one copy on
  principle) — a deliberate, scoped exception for this one field, not a
  reversal of ADR-0001's boundary for `CanonicalStore` itself, which is
  unchanged.

## Validation and revisit triggers

- Validated by: `scan_ages_reflects_update_age_immediately` passing, and
  the 4-way benchmark suite (`benches/workloads.rs`) actually running
  `CanonicalCachedStore` alongside the other three.
- Revisit if: `RESULTS.md` shows the `update_age` regression is large
  enough to matter for a realistic workload — implement and benchmark
  lazy invalidation as a fifth backend (or a mode of this one) rather
  than replacing this implementation, so both strategies stay comparable
  behind the same trait.
- Revisit if: a write-heavy mixed-workload benchmark (flagged as an open
  question since the first `RESULTS.md`) is eventually built — that's
  the workload shape most likely to change this decision's calculus.
