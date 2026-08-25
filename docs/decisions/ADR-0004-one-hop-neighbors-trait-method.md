# ADR-0004: One `neighbors` trait method (one-hop only); multi-hop traversal built generically outside the storage layer

- Status: Accepted
- Date: 2026-08-25
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0001-three-backend-empirical-comparison.md`, `STORAGE-002`, `STORAGE-006`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

The original hypothesis behind this repo (see the charter and ADR-0001) is
that a UUID-canonical store can serve row, column, *and graph* access as
views over one canonical map, rather than as separate physical copies.
`same_breed` was built as this project's graph-view stand-in, but it's a
grouping over a shared *attribute* (`breed`), not traversal over a real
*edge* — it never required following an actual relationship between two
specific records. The row/column verdict (`CanonicalCachedStore` wins or
ties every workload benchmarked so far) is closed and out of scope for this
ADR; this is about testing the graph leg of the hypothesis for the first
time with a real relationship (`littermate_of`, a synthetic 0-3-edges-per-dog
relationship added to the generator — see `STORAGE-006`).

Once a real edge relationship exists, a question the `same_breed` design
never had to answer comes up immediately: how far does the storage-layer
interface go? A graph relationship naturally invites multi-hop questions
("littermates of littermates", "littermates within 3 hops"), and there are
two structurally different places that logic could live:

1. **In the trait**, e.g. `fn neighbors_n_hops(&self, id: Uuid, hops: usize) -> Vec<Uuid>`,
   implemented separately by each backend.
2. **Outside the trait**, as a generic function that calls a single one-hop
   `neighbors` method repeatedly, implemented once and shared by every
   backend.

## Decision drivers

- **Avoid speculative generality.** The task motivating this ADR explicitly
  scopes the work to "one relationship type and 1-2 hop traversal, not a
  general graph query layer" and explicitly directs "one new trait method,
  not a new trait." A general N-hop graph query interface is exactly the
  kind of forward-looking abstraction this repo's engineering constraints
  (and the charter's non-goals) rule out building without a concrete need.
- **Keep the backend/harness boundary where it already is.** Per the
  architecture doc's existing separation: backends store data and answer
  point queries (`get`, `scan_ages`, `update_age`, `same_breed`); workload
  and traversal *logic* lives in `benches/`. A multi-hop trait method would
  put non-trivial traversal logic (loop-and-dedupe) inside every backend
  implementation, duplicated four times, when it's the exact same logic
  regardless of backend.
- **Backends should only need to answer "who is this connected to."**
  Adding depth as a trait parameter forces every backend to either
  implement its own multi-hop loop (four copies of the same dedup logic) or
  delegate to a shared free function anyway — at which point the trait
  parameter adds nothing but interface surface.
- **This is a benchmark harness, not a production graph database.** The
  question under test is whether a one-hop adjacency-index view over the
  canonical store is competitive with a linear scan, not how to build a
  general traversal engine. If the benchmark results here suggest a real
  graph engine would be worth building, that's a finding to report, not a
  reason to build one now (see `RESULTS.md`'s open questions).

## Considered options

1. **A generic multi-hop trait method**
   (`fn neighbors_n_hops(&self, id: Uuid, hops: usize) -> Vec<Uuid>`).
   Rejected. Every backend would need its own BFS/loop-and-dedupe
   implementation (or all four would delegate to one shared helper anyway,
   making the trait method a redundant wrapper). It also invites exactly
   the kind of general-purpose graph-query surface the task explicitly
   rules out, for a relationship this pass only needs to traverse 1-2 hops
   into.
2. **A separate `GraphStore` trait, alongside `DogStore`.** Rejected —
   the task is explicit that this should be "one new trait method, not a
   new trait." A second trait would also duplicate the "one interface, four
   backends, same generated input" structure ADR-0001 established, for a
   relationship that's still just one more access pattern over the same
   records.
3. **One one-hop `neighbors` method on `DogStore`; 2-hop (and any deeper
   traversal this pass might need) implemented once, generically, outside
   the trait — as repeated calls to `neighbors`.** Chosen.

## Decision

`DogStore` gains exactly one new method:

```rust
fn neighbors(&self, id: Uuid) -> Vec<Uuid>;
```

returning every UUID connected to `id` by a `littermate_of` edge, in either
direction (the relationship is symmetric), or an empty `Vec` if `id` is
unknown or has no edges. Each backend implements this using the same
strategy it already uses for `same_breed`:

- `AosStore`/`SoaStore`: a linear scan of a stored flat edge list
  (`Vec<(Uuid, Uuid)>`) — the naive baseline, same role these backends play
  in every other workload.
- `CanonicalStore`/`CanonicalCachedStore`: a `HashMap<Uuid, Vec<Uuid>>`
  adjacency index built once at construction, inserting each edge in both
  directions — structurally identical to the existing breed index, just
  keyed by UUID instead of breed name.

2-hop traversal is **not** a trait method. It is implemented once, in
`src/bench_support.rs` (shared by both bench targets and available to
tests), as `two_hop_neighbors`:

```rust
pub fn two_hop_neighbors<S: DogStore>(store: &S, id: Uuid) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    for one_hop in store.neighbors(id) {
        for two_hop in store.neighbors(one_hop) {
            seen.insert(two_hop);
        }
    }
    seen.into_iter().collect()
}
```

This function only ever calls the public `neighbors` method — it has no
special knowledge of any backend's internals, and every backend gets 2-hop
traversal "for free" the moment it implements one-hop `neighbors`
correctly. Whether a future pass needs 3+ hops, this same pattern (another
generic loop over `neighbors`, or `two_hop_neighbors` composed with itself)
extends without touching the trait or any backend.

## Consequences

### Positive

- Minimal trait surface: one method, symmetric in cost/behavior with
  `same_breed`, no new fallibility (`neighbors` follows the existing
  "unknown id is an empty result, not an error" convention).
- No backend needs to know or care how many hops a caller might eventually
  want — traversal depth is entirely a benchmark/test-code concern.
- 2-hop correctness is verified once (`two_hop_neighbors`'s own logic) and
  automatically correct for every backend that passes the one-hop
  consistency test — no per-backend 2-hop test duplication needed, though
  a cross-backend 2-hop consistency test still exists as a direct check on
  the shared helper itself.
- Extending to 3+ hops later, if ever motivated, is an addition to
  `bench_support.rs`, not a `DogStore` trait change — no backend
  implementation would need to change.

### Negative / tradeoffs

- This is not a real graph query engine: `two_hop_neighbors` recomputes
  from scratch on every call (no caching of intermediate frontiers across
  calls, no early termination, no support for weighted/typed edges). It
  would not scale gracefully to many-hop traversals over a
  high-fan-out graph — that's a deliberate scope boundary, not an
  oversight, and is called out explicitly in `RESULTS.md`'s open questions
  rather than quietly left unaddressed.
- `neighbors`'s cost model is identical to `same_breed`'s cost model (one
  `HashMap` lookup for canonical/canonical-cached, one linear scan for
  AoS/SoA) — this ADR doesn't investigate whether a different index
  structure (e.g. a compact adjacency list packed by position rather than
  `Vec<Uuid>` per key) would perform meaningfully differently; that's the
  same class of open question ADR-0001 already left for the breed index.

## Validation and revisit triggers

- Validated by: `tests/cross_backend.rs`'s
  `all_backends_agree_on_neighbors_one_hop` (edge-list-vs-adjacency-index
  consistency — the highest-priority test for this feature, equivalent in
  role to `CanonicalCachedStore`'s stale-cache test) and
  `all_backends_agree_on_neighbors_two_hop`, plus the 1-hop/2-hop Criterion
  suites in `benches/workloads.rs`/`benches/cache_events.rs`.
- Revisit if: a future pass needs traversal depth to be run-time
  configurable rather than a fixed 1-2 hops, or needs to traverse a second,
  differently-shaped relationship — at that point a
  `neighbors_n_hops`-style trait method (or a small typed-relationship
  abstraction) might earn its complexity; not before.
- Revisit if: `RESULTS.md`'s graph-traversal numbers suggest the naive
  "recompute from scratch, no caching" 2-hop composition is a real
  bottleneck at scale — flagged as an open question there, not a reason to
  build a caching layer speculatively now.
