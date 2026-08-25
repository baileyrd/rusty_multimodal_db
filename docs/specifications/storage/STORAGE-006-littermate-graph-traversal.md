# STORAGE-006 — `littermate_of` graph relationship: edge generation, `neighbors` trait method, and graph-traversal benchmarks

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-003`
- Supersedes: none

## Purpose and scope

Test the third, previously-untested leg of the original row/column/graph
hypothesis (see the charter and ADR-0001): whether a UUID-canonical store
can serve real one-hop graph traversal — not a shared-attribute grouping
like `same_breed` — as a view over the canonical map, on the same terms as
row/column access. Adds a synthetic `littermate_of` relationship to the
generator, a `neighbors` method to `DogStore`, implementations for all four
existing backends, and 1-hop/2-hop Criterion benchmarks matching the
existing suite's structure exactly.

## Non-goals

- Not a general graph query layer — this is one relationship type and
  1-2 hop traversal, not N-hop, not weighted/typed edges, not a query
  language. See ADR-0004 for why multi-hop is deliberately kept out of the
  trait.
- Not revisiting the row/column verdict — `CanonicalCachedStore`
  winning/tying every workload benchmarked so far (see `RESULTS.md`) is
  closed and unchanged by this spec.
- Not changing `same_breed` — it remains the shared-attribute stand-in it
  always was; `neighbors` is a separate, additional access pattern over a
  real edge relationship.
- Not persisting the edge list, and not supporting edge mutation (no
  `add_edge`/`remove_edge` operation exists or is needed — edges are fixed
  at construction, same lifecycle as records' `breed`).

## Context and terminology

- **`littermate_of`**: a synthetic, symmetric relationship between two
  dogs (if A is a littermate of B, B is a littermate of A). Modeled as an
  edge list, `Vec<(Uuid, Uuid)>`, generated alongside the existing
  `Vec<DogRecord>` dataset.
- **Out-degree**: the number of edges a given dog originates as the first
  element of the pair during generation (always 0-3, see FR-002 below).
  Because edges are symmetric, a dog's *total* neighbor count (via
  `neighbors`) can exceed its own out-degree if other dogs also generated
  edges pointing at it — that's an expected property of a randomly
  generated symmetric graph, not a bug.
- **`littermate_avg_degree`**: the configurable expected out-degree per
  dog, mirroring `breed_cardinality`'s role as the generator's other
  configurable distribution parameter.

## Requirements

- `STORAGE-006-FR-001`: `GeneratorConfig` gains a `littermate_avg_degree:
  f64` field, validated to the inclusive range `[0.0, 3.0]` at
  construction (`GeneratorConfig::new` returns
  `Err(GeneratorConfigError::InvalidLittermateAvgDegree)` outside that
  range).
- `STORAGE-006-FR-002`: `generate_littermates(config, records) ->
  Vec<(Uuid, Uuid)>` is a pure function of `(config, records)`: the same
  inputs always produce the same edge list. It draws from a `StdRng`
  stream independent of `generate`'s own (seeded from `config.seed()`
  XORed with a fixed constant), so calling both against the same config is
  reproducible and one call's draws don't perturb the other's.
- `STORAGE-006-FR-003`: Each dog's out-degree is drawn as 3 independent
  Bernoulli trials with success probability
  `littermate_avg_degree / 3.0`, so out-degree is always 0, 1, 2, or 3, and
  the expected out-degree across the dataset equals
  `littermate_avg_degree`.
- `STORAGE-006-FR-004`: No self-loops — a dog is never generated as its own
  littermate.
- `STORAGE-006-FR-005`: `DogStore` gains exactly one new method:
  ```rust
  fn neighbors(&self, id: Uuid) -> Vec<Uuid>;
  ```
  returning all UUIDs connected to `id` by a `littermate_of` edge in either
  direction, excluding `id` itself; an empty `Vec` if `id` is unknown or
  has no edges. See ADR-0004 for why this is the only new trait method
  (no multi-hop trait method).
- `STORAGE-006-FR-006`: `AosStore`/`SoaStore` implement `neighbors` via a
  linear scan of a stored flat edge list — the naive baseline, matching
  their role in every other workload.
- `STORAGE-006-FR-007`: `CanonicalStore`/`CanonicalCachedStore` implement
  `neighbors` via a `HashMap<Uuid, Vec<Uuid>>` adjacency index built once
  at construction, inserting each edge in both directions (structurally
  identical to the existing breed index, keyed by UUID instead of breed
  name).
- `STORAGE-006-FR-008`: All four backends are constructible from
  `(Vec<DogRecord>, Vec<(Uuid, Uuid)>)` (via `From`), in addition to their
  existing `From<Vec<DogRecord>>` (which now builds with an empty edge
  list, for workloads that don't exercise `neighbors`).
- `STORAGE-006-FR-009`: 2-hop traversal is implemented once, generically,
  as `bench_support::two_hop_neighbors<S: DogStore>(store: &S, id: Uuid)
  -> Vec<Uuid>` — the deduplicated union of `neighbors(n)` for every `n` in
  `neighbors(id)`. No backend implements or is aware of multi-hop
  traversal.
- `STORAGE-006-FR-010`: Two new Criterion wall-clock benchmark groups
  (`neighbors_one_hop`, `neighbors_two_hop`) in `benches/workloads.rs`, and
  matching groups in the Linux-only `benches/cache_events.rs`, each
  covering all four backends at all three existing dataset sizes (1K /
  100K / 1M), built from the same generated dataset (including edges) as
  every other workload in that size's group.

## Architecture and interfaces

- `src/generator.rs` — `GeneratorConfig::littermate_avg_degree`,
  `generate_littermates`.
- `src/store/mod.rs` — `DogStore::neighbors`.
- `src/store/{aos,soa,canonical,canonical_cached}.rs` — `neighbors`
  implementations; each backend's constructor and `From<Vec<DogRecord>>`
  impl updated to also accept/store an edge list.
- `src/bench_support.rs` — `Dataset::edges`, `LITTERMATE_AVG_DEGREE`
  constant, `two_hop_neighbors`.
- `benches/workloads.rs`, `benches/cache_events.rs` — `neighbors_one_hop`
  and `neighbors_two_hop` benchmark groups.

## Data/state and invariants

- The edge list is fixed at construction for every backend; no operation
  in this crate mutates it after that point (mirrors `CanonicalStore`'s
  existing breed-index invariant: nothing currently changes `breed`
  either).
- `littermate_of` is symmetric by construction: `CanonicalStore`'s and
  `CanonicalCachedStore`'s adjacency indexes insert each `(a, b)` edge in
  both directions even though the edge list itself lists the pair once;
  `AosStore`/`SoaStore`'s linear scan checks both positions per edge to get
  the same symmetric result without a doubled list.
- `neighbors`'s result for a given id may contain duplicates if the
  generated edge list contains duplicate/parallel edges for that id — this
  is intentional: it's the honest reflection of what's actually in the
  edge list, consistent between the naive linear scan and the adjacency
  index (verified as a set, not a strict-order/no-duplicates list, by the
  cross-backend consistency test).

## Errors, failure, recovery, and observability

`neighbors` follows the same convention as `same_breed`: an unknown `id`
or a record with no edges is a normal empty `Vec`, not an error.
`GeneratorConfig::new` returns `Err` for an out-of-range
`littermate_avg_degree`, matching the existing `ZeroCardinalityWithRecords`
pattern (validated at construction, no panic).

## Security, privacy, and compatibility

Not applicable — synthetic in-memory data only, same as every other spec
in this tree.

## Acceptance criteria

- For every record's id in a generated dataset, `AosStore`'s
  linear-scan `neighbors` (ground truth over the raw edge list) and every
  other backend's adjacency-index-based `neighbors` return the exact same
  set of UUIDs. This is the highest-priority acceptance criterion for this
  spec — the direct equivalent of `STORAGE-005`'s stale-cache check.
- The same agreement holds for `two_hop_neighbors` across all four
  backends.
- `cargo bench` (default features) includes `neighbors_one_hop` and
  `neighbors_two_hop` groups at all three sizes across all four backends,
  completing without panics.
- `RESULTS.md` has a `## Graph traversal` section, structured like the
  rest of the file (workload × size × backend table, verdict per
  workload), clearly separated from the existing row/column section
  without altering that section's already-closed verdict.

## Verification plan

- Generator unit tests (`src/generator.rs`): determinism, degree-bounds
  validation, out-degree-exactly-0-to-3, no self-loops, `n < 2` edge case.
- Per-backend unit tests (hit/miss/no-edges), mirroring `same_breed`'s
  existing test shape.
- Cross-backend tests (`tests/cross_backend.rs`):
  `all_backends_agree_on_neighbors_one_hop` (highest priority) and
  `all_backends_agree_on_neighbors_two_hop`.
- `bench_support::two_hop_neighbors`'s own unit tests, verifying the
  dedup/union logic directly against a small hand-built graph.
- 4-way Criterion suite (`benches/workloads.rs`), and the Linux-only
  `benches/cache_events.rs` build (real counter numbers deferred per
  ADR-0002's established pattern if this session's environment lacks PMU
  access).

## Traceability

Implements: the "graph traversal benchmark" deliverable — the third leg of
the original row/column/graph hypothesis. Depends on: `STORAGE-001`
(generator/record shape this extends), `STORAGE-002` (the `DogStore` trait
this adds one method to), `STORAGE-003` (the benchmark-suite structure this
extends). Feeds: `RESULTS.md`'s new `## Graph traversal` section.

## Open questions

- Whether the numbers in `RESULTS.md`'s graph-traversal section suggest a
  real graph engine (persistent adjacency structures, cached multi-hop
  frontiers, weighted/typed edges) is worth building — a finding to
  surface there if so, not something this spec builds.
- Whether `littermate_avg_degree` should scale with dataset size rather
  than stay fixed (mirrors `BREED_CARDINALITY`'s same open question from
  `STORAGE-001`/`RESULTS.md` — not tested this pass).
- Whether a directed-only (non-symmetric) edge semantics would better
  model a future, different relationship type — `littermate_of` is
  naturally symmetric, so this wasn't a live question for this pass, but a
  future asymmetric relationship (e.g. `parent_of`) would need to revisit
  the "insert both directions" adjacency-index convention this spec
  established.
- Memory overhead of the adjacency index on top of the breed index and
  (for `CanonicalCachedStore`) the age cache — not measured this pass, same
  category as `STORAGE-005`'s existing open memory-overhead question.

## Change history

- 0.1.0 (2026-08-25): Initial accepted draft.
