# STORAGE-002 — `DogStore` trait and three backend implementations

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`
- Supersedes: none

## Purpose and scope

Define the single trait all three storage backends implement, and the
three implementations under comparison: AoS, SoA, and UUID-canonical
store with derived views. See ADR-0001 for the rationale behind testing
these empirically behind one interface.

## Non-goals

- No persistence, concurrency, or transactions.
- No fourth backend (e.g. materialized-column-cache hybrid) in this pass
  — noted as a likely future finding, not built now.
- No generic multi-hop graph traversal — `same_breed` is a deliberately
  narrow one-hop stand-in.

## Context and terminology

See ADR-0001 for the AoS/SoA/canonical-store definitions and the "view,
not copy" boundary for the canonical store's derived access methods.

## Requirements

- `STORAGE-002-FR-001`: One trait:
  ```rust
  trait DogStore {
      fn get(&self, id: Uuid) -> Option<DogRecord>;
      fn scan_ages(&self) -> Vec<u32>;
      fn update_age(&mut self, id: Uuid, age: u32) -> Result<(), StoreError>;
      fn same_breed(&self, id: Uuid) -> Vec<Uuid>;
  }
  ```
- `STORAGE-002-FR-002`: `AosStore` wraps `Vec<DogRecord>`. `get` is a
  linear scan by `id` (this is the honest cost of AoS for point lookup —
  no secret index added to make it look better than it is).
- `STORAGE-002-FR-003`: `SoaStore` wraps parallel `Vec<Uuid>`,
  `Vec<String>`, `Vec<u32>`, tied together by shared index position.
  `get` and `update_age` locate the position by scanning the `Vec<Uuid>`.
- `STORAGE-002-FR-004`: `CanonicalStore` wraps `HashMap<Uuid, DogRecord>`
  as the only physical copy of record data, plus a `HashMap<String,
  Vec<Uuid>>` breed index built at construction to serve `same_breed`.
  `scan_ages` iterates `HashMap` values directly — no cached `Vec<u32>`.
  `get` and `update_age` are `HashMap` lookups.
- `STORAGE-002-FR-005`: `same_breed(id)` returns all UUIDs (excluding or
  including `id` itself — pick one and document it consistently across
  all three backends) sharing that record's breed. If `id` is unknown,
  return an empty `Vec`, not an error.
- `STORAGE-002-FR-006`: `update_age` returns
  `Err(StoreError::NotFound(id))` when `id` doesn't exist in the store,
  for all three backends identically.
- `STORAGE-002-FR-007`: All three backends are constructible from a
  `Vec<DogRecord>` (e.g. via `From<Vec<DogRecord>>` or a `new` /
  `from_records` constructor), so the same generator output feeds all
  three identically.

## Architecture and interfaces

`src/store/mod.rs` — `DogStore` trait, `StoreError` enum
(`thiserror`-derived, single `NotFound(Uuid)` variant for now).
`src/store/aos.rs`, `src/store/soa.rs`, `src/store/canonical.rs` — one
implementation each.

## Data/state and invariants

- `SoaStore`'s three parallel vecs are always the same length; that's an
  internal invariant enforced by construction and by `update_age` never
  changing length (age updates only ever mutate an existing slot).
- `CanonicalStore`'s breed index is rebuilt in full whenever breed
  membership could change. `update_age` never changes breed, so the index
  does not need to be touched by `update_age` in this pass — noted
  explicitly since it's the kind of invariant that's easy to silently
  break if a future change adds a breed-mutating operation.

## Errors, failure, recovery, and observability

Only `update_age` is fallible (`StoreError::NotFound`). `get` and
`same_breed` use `Option`/empty-`Vec` for "not found" — normal outcomes,
not errors, per the architecture doc's failure model.

## Security, privacy, and compatibility

Not applicable.

## Acceptance criteria

- Given the same `Vec<DogRecord>`, all three backends return identical
  `get`, `scan_ages` (as a multiset/sorted comparison — order need not be
  identical across backends), and `same_breed` results for the same
  inputs.
- `update_age` on an unknown UUID returns `Err` from all three backends;
  on a known UUID, updates `get`'s subsequent result and does not disturb
  any other record.
- `same_breed` on a record whose breed is unique returns an empty (or
  self-only, per the documented convention) result.

## Verification plan

Unit tests per backend module covering: `get` hit/miss, `scan_ages`
correctness against a hand-built small dataset, `update_age` success and
`NotFound` failure, `same_breed` for a shared breed and a unique breed. A
shared cross-backend test (same input, same expected output from all
three) is preferred over triplicated backend-specific tests where
practical, to keep the comparison itself under test.

## Traceability

Implements: "Three backend implementations behind one trait" deliverable.
Depends on: `STORAGE-001` for `DogRecord` and generator output used in
tests. Feeds: `STORAGE-003` (benchmark suite exercises these
implementations).

## Open questions

- Whether `same_breed` should include or exclude the queried `id` itself
  in its result — resolved during implementation and documented in the
  function's docstring; flagged here so the convention is traceable to a
  deliberate choice, not an accident.

## Change history

- 0.1.0 (2026-08-24): Initial accepted draft.
