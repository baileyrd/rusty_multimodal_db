# STORAGE-005 — `CanonicalCachedStore`: canonical store with a materialized age cache

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`
- Supersedes: none

## Purpose and scope

Define the fourth `DogStore` backend: `CanonicalStore`'s `HashMap<Uuid,
DogRecord>` and breed index as source of truth, plus a materialized,
packed `Vec<u32>` age cache to fix `scan_ages`'s loss to both baselines
(see `RESULTS.md`'s first pass). See
`docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md` for
the eager-vs-lazy invalidation decision this backend is built around.

## Non-goals

- Not a general materialized-view system — this caches exactly one field
  (`age`), for exactly one workload (`scan_ages`), because that's the one
  workload the first three backends left genuinely unresolved. No other
  field gets a cache in this pass.
- Not implementing lazy/dirty-flag invalidation — ADR-0003 chose eager
  write-through for this pass; lazy remains a documented option for a
  future backend/mode if a write-heavy workload benchmark motivates it.
- Not changing `CanonicalStore` itself — it stays exactly as ADR-0001
  specified (one physical copy, views only). `CanonicalCachedStore` is a
  separate, additional implementation.

## Context and terminology

"Write-through" here means: `update_age` writes the canonical record and
the cache slot in the same call, synchronously, before returning `Ok(())`
— there is no deferred or background sync step.

## Requirements

- `STORAGE-005-FR-001`: `CanonicalCachedStore` implements `DogStore`
  identically in behavior to `CanonicalStore` for `get` and `same_breed`
  (same `HashMap`/breed-index lookups; the age cache is not involved in
  either).
- `STORAGE-005-FR-002`: `scan_ages` reads a packed `Vec<u32>` age cache
  directly — no `HashMap` traversal, no per-record heap dereference.
- `STORAGE-005-FR-003`: `update_age` writes through to both the canonical
  `HashMap<Uuid, DogRecord>` record and the corresponding `age_cache`
  slot in the same call. There is no code path where `update_age` returns
  `Ok(())` without both being updated, and no separate "flush"/"sync"
  method exists or is needed.
- `STORAGE-005-FR-004`: The cache slot for a given UUID is located via a
  `HashMap<Uuid, usize>` position index built once at construction — O(1)
  lookup, not a linear scan of the cache.
- `STORAGE-005-FR-005`: `update_age` on an unknown UUID returns
  `Err(StoreError::NotFound)` and does not modify the age cache (matching
  the other three backends' "not found" behavior, and specifically
  verified since a partial write on a `NotFound` error would itself be a
  staleness bug).
- `STORAGE-005-FR-006`: Constructible from a `Vec<DogRecord>` via `From`,
  matching the other three backends, so it plugs into the same benchmark
  and test harness without special-casing.

## Architecture and interfaces

`src/store/canonical_cached.rs` — `CanonicalCachedStore` struct
(`records: HashMap<Uuid, DogRecord>`, `breed_index: HashMap<String,
Vec<Uuid>>`, `age_cache: Vec<u32>`, `position_index: HashMap<Uuid,
usize>`), implementing `DogStore`.

## Data/state and invariants

- `age_cache.len() == records.len() == position_index.len()`, always.
- For every `(id, position)` in `position_index`, `age_cache[position]`
  equals `records[&id].age` — the core invariant this backend exists to
  guarantee is never violated, checked directly by
  `scan_ages_reflects_update_age_immediately`.
- `update_age` never changes `breed`, so — like `CanonicalStore` —
  `breed_index` doesn't need updating by any operation in this crate; if
  a future breed-mutating operation is added, both the breed index *and*
  the age-cache invariant above need auditing.

## Errors, failure, recovery, and observability

Same as `CanonicalStore`: `update_age` is the only fallible operation,
returning `StoreError::NotFound` for an unknown UUID, with no partial
mutation of either the canonical record or the cache on that path.

## Security, privacy, and compatibility

Not applicable.

## Acceptance criteria

- `get`/`same_breed` results are identical to `CanonicalStore`'s for the
  same input (verified by the cross-backend equivalence tests).
- `scan_ages` immediately reflects the most recent `update_age` for every
  record, with no staleness window.
- `update_age` on an unknown UUID leaves `scan_ages`'s output unchanged.
- Benchmarked cost of `update_age`'s write-through vs. `CanonicalStore`'s
  single write, and of `scan_ages`'s cached read vs. `CanonicalStore`'s
  view — both reported in `RESULTS.md`, not asserted without evidence.

## Verification plan

Unit tests in `src/store/canonical_cached.rs` (staleness test
highest-priority, then the same hit/miss/not-found/same-breed shape as
`CanonicalStore`'s), plus the shared cross-backend equivalence tests in
`tests/cross_backend.rs` extended to include this backend, plus the
4-way Criterion suite in `benches/workloads.rs`.

## Traceability

Implements: the "4th backend" deliverable that closes `scan_ages`'s gap
identified in `RESULTS.md`'s first pass. Depends on: `STORAGE-001`
(dataset/records), `STORAGE-002` (trait and the `CanonicalStore` design
this extends). Feeds: `RESULTS.md`'s revised 4-way comparison.

## Open questions

- Whether a fifth backend/mode implementing lazy invalidation is worth
  building, given the measured eager write-through cost — see
  `RESULTS.md`'s open questions and ADR-0003's revisit triggers.
- Memory overhead of carrying both a breed index and a position index
  alongside the canonical map and the age cache — not measured this pass.

## Change history

- 0.1.0 (2026-08-24): Initial accepted draft, following the first
  `RESULTS.md` pass's `scan_ages` finding.
