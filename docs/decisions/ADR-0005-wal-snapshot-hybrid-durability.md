# ADR-0005: `CanonicalCachedStore`-only durability, via a shared rebuilt core, with five Tier 1 designs (WAL fsync, WAL buffered, two snapshot shapes, hybrid)

- Status: Accepted
- Date: 2026-08-25
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0001-three-backend-empirical-comparison.md`, `docs/decisions/ADR-0003-eager-write-through-cache-invalidation.md`, `STORAGE-005`, `STORAGE-008`, `docs/decisions/ADR-0006-tier-2-durability-architectures.md`, `RESULTS.md`
- Supersedes/Superseded by: none

## Context

Every backend and every benchmark in this crate up to this point is purely
in-memory: nothing survives a process restart. The task motivating this
ADR asks a different question for the first time — given
`CanonicalCachedStore` as the closed, recommended backend (per every
verdict in `RESULTS.md` above the `## Durability` section), what does it
cost to make its writes actually durable, and which of several reasonable
designs is worth keeping, decided by real benchmark numbers rather than by
argument alone. Three design questions had to be settled before any
variant could be built:

1. **Which backend(s) get persistence?** This crate has four backends;
   only one is a real recommendation.
2. **How do eight structurally different variants share code**, given
   `CanonicalCachedStore`'s own implementation (`src/store/canonical_cached.rs`)
   is closed, already-benchmarked backend code from prior sessions, not
   something this task should be modifying?
3. **Which specific durability designs are worth building at full rigor
   (Tier 1) versus as a lighter, explicitly-flagged proof-of-concept
   (Tier 2)?** This ADR covers Tier 1 — the WAL/snapshot/hybrid family
   (variants 1-5); ADR-0006 covers Tier 2's alternate architectures
   (variants 6-8).

## Decision drivers

- **Avoid scope creep onto backends nobody would deploy.** `AosStore`/
  `SoaStore`/`CanonicalStore` exist in this crate purely as baselines that
  every prior benchmark has shown `CanonicalCachedStore` beats or ties —
  building persistence for them would be work with no plausible payoff.
- **Don't touch closed, already-benchmarked code.** `canonical_cached.rs`'s
  `get`/`scan_ages`/`update_age`/`same_breed`/`neighbors` implementations
  and their reported numbers are settled findings from prior sessions.
  Reopening that file to add persistence would put this task's changes on
  the critical path for re-validating every prior verdict, for no reason —
  the durability work only needs the same *architecture*
  (canonical map + breed index + age cache + position index + adjacency
  index), not literal reuse of the private struct.
- **Eight variants sharing one read path, not eight copies of it.** Every
  variant's `get`/`scan_ages`/`same_breed`/`neighbors` behavior is
  identical — only construction, `update_age`, and on-disk persistence
  differ. Writing that logic eight times would multiply the surface area
  for the exact kind of divergence bug (e.g. one copy forgetting to update
  the position index) this crate's cross-backend consistency tests exist
  to catch elsewhere.
- **`Result`/`?` throughout, no `unwrap`/`expect` outside tests** — this
  crate's existing discipline, extended to every new fallible I/O and
  (de)serialization path.
- **Correctness tests carry the same weight `CanonicalCachedStore`'s own
  stale-cache test did** when it was introduced (ADR-0003/`STORAGE-005`):
  the highest-priority deliverable per variant, not an afterthought after
  the numbers are in.

## Considered options

### Scope: which backend(s) get durability

1. **All four backends.** Rejected — `AosStore`/`SoaStore` are pure
   in-memory baselines by design (see `ADR-0001`); persisting them would
   be built to be thrown away, since no verdict in this document
   recommends deploying either.
2. **`CanonicalCachedStore` only.** Chosen — the one backend every prior
   `RESULTS.md` section actually recommends.

### Code sharing: how eight variants avoid duplicating the read path

1. **Modify `CanonicalCachedStore` directly**, adding persistence fields
   and methods to the existing struct. Rejected — that file is closed,
   already-benchmarked code; opening it for unrelated new scope risks
   destabilizing settled numbers and mixes two kinds of change (new
   feature, review of old code) that don't need to travel together.
2. **Duplicate the shared architecture eight times**, once per variant.
   Rejected — `get`/`scan_ages`/`same_breed`/`neighbors` are identical
   code in every variant; eight copies is eight places a future bug fix
   or index-consistency issue has to be applied identically, with no
   test forcing that consistency the way `tests/cross_backend.rs` does
   for the four production backends.
3. **A new, shared `CanonicalCachedState` type** (`src/durability/mod.rs`),
   rebuilt from scratch to the same shape as `CanonicalCachedStore`
   (canonical map, breed index, age cache, position index, adjacency
   index) with matching inherent methods, that every variant wraps or
   embeds. Chosen — the read path is written once; each variant differs
   only in construction, `update_age`, and how (or whether) it persists
   that state to disk.

### Tier 1 variant selection (this ADR's actual subject)

The task specified these five explicitly, on the reasoning that they cover
the practical design space for a WAL/snapshot-based durable store without
duplicating each other's design point:

1. **WAL, fsync per write.** The strongest possible guarantee this crate
   builds: every write physically on disk before `update_age` returns.
   Included as the ceiling case other variants are measured against.
2. **WAL, buffered (no fsync).** Same format as (1), minus the explicit
   `sync_all` call — tests how much of (1)'s cost is the fsync itself
   versus the WAL-append structure.
3. **Snapshot, canonical-only, rebuild-on-load.** Zero per-write I/O;
   durability comes entirely from an explicit checkpoint that persists
   only `records`/`edges` (the same two inputs `CanonicalCachedState::new`
   takes) and rebuilds every derived index on load — the persistence-layer
   analogue of this crate's "views over one canonical source, not
   physical copies" philosophy (ADR-0001).
4. **Snapshot, save-as-is.** Same zero-per-write-I/O shape as (3), but
   persists the *whole* `CanonicalCachedState` (including derived
   indexes) directly, with no rebuild step on load — a real, cheaper (no
   rebuild) but structurally different tradeoff from (3), not a
   redundant variant.
5. **Hybrid: periodic snapshot + WAL of writes since that snapshot.**
   Bounds WAL replay cost (unlike (1)/(2), whose replay cost grows with
   every write since the last checkpoint) via a snapshot that records its
   own cutoff sequence number, with `open` replaying only WAL entries
   after that cutoff. The one variant among the five that never truncates
   its WAL (see `hybrid.rs`'s own module docs for why that's the actual
   point, not an oversight).

No alternative Tier 1 designs were seriously considered beyond these
five — the task specified them explicitly as covering the practical
design space (per-write-durable vs. batched, rebuild vs. save-as-is,
WAL-only vs. snapshot-only vs. both), and this ADR's job is to record
*why* each of the five earns its place, not to search a wider design
space the task didn't ask for.

## Decision

- Durability applies to `CanonicalCachedStore` only. `AosStore`/
  `SoaStore`/`CanonicalStore` are unchanged.
- A new `src/durability/` module hosts everything: `CanonicalCachedState`
  (the shared, rebuilt-from-scratch core every variant wraps),
  `DurabilityError` (one error type folding I/O, (de)serialization, and
  the existing `StoreError` together, converted back to `StoreError` via
  `impl From<DurabilityError> for StoreError` so every variant's
  `update_age` can still return the trait's required
  `Result<(), StoreError>`), and shared WAL helpers
  (`append_wal_entry`/`read_wal_entries`, a length-prefixed binary format
  tolerant of a torn trailing write).
- Five Tier 1 variants (`wal_fsync.rs`, `wal_buffered.rs`,
  `snapshot_rebuild.rs`, `snapshot_full.rs`, `hybrid.rs`), each
  implementing `DogStore` by delegating `get`/`scan_ages`/`same_breed`/
  `neighbors` straight to their embedded `CanonicalCachedState`, and each
  fully tested (round-trip/reconstruction correctness as the
  highest-priority test per variant, matching `STORAGE-005`'s stale-cache
  test's role) and benchmarked at all three dataset sizes (1K/100K/1M)
  across per-write, checkpoint, and load costs (`STORAGE-008`,
  `benches/durability.rs`).
- New dependencies, flagged explicitly (the first added since this
  crate's inception): `serde` (`derive` feature) and `bincode` for
  snapshot/WAL (de)serialization — `bincode` chosen over `serde_json`
  since this is a benchmark harness, not a tool whose on-disk files need
  to stay human-readable, and one binary format is one fewer thing to
  keep consistent across all eight variants (Tier 1 and Tier 2 alike).

## Consequences

### Positive

- The shared `CanonicalCachedState` core means every variant's read path
  is written and tested once, not five (or eight, counting Tier 2) times
  — new variants inherit correct `get`/`scan_ages`/`same_breed`/
  `neighbors` behavior automatically, the same "shared, tested-once core"
  benefit ADR-0004's `two_hop_neighbors` design got from being built once
  outside the trait.
- `canonical_cached.rs` itself is untouched — every prior `RESULTS.md`
  verdict for that backend remains exactly as measured, with zero risk of
  this task's changes destabilizing them.
- Real numbers, not argument, decide which of the five variants is worth
  keeping (see `RESULTS.md`'s `## Durability` section and its explicit
  recommendation): WAL-fsync is the correctness ceiling at a real,
  measured cost (~190-215 µs/write); the buffered/hybrid/rebuild/full
  variants each demonstrate a genuinely different point on the
  cost-vs-loss-window tradeoff curve, not four variations on one theme.
- The hybrid variant's checkpoint cost being the one clear outlier in the
  whole Tier 1 sweep (traced to an avoidable extra `state.clone()` before
  serializing — see `RESULTS.md`) is exactly the kind of concrete,
  actionable finding this ADR's "measure, don't argue" approach exists to
  surface — a design that looked sound on paper turned out to have a
  real, fixable implementation cost once actually benchmarked.

### Negative / tradeoffs

- `CanonicalCachedState` duplicates `CanonicalCachedStore`'s field shape
  and read-path logic rather than sharing it directly — a deliberate
  tradeoff (see Considered options above) to avoid touching closed code,
  but it does mean the two implementations could drift if one changes
  without the other being updated; nothing currently enforces that they
  stay in sync beyond both being covered by their own test suites.
- None of the five variants' `checkpoint()` calls (nor `write_to`'s
  underlying `std::fs::write`) explicitly call `sync_all`/`fsync` —
  durability at checkpoint time rests on the OS's own page-cache
  write-back for every variant except WAL-fsync's per-write path. This
  is consistent with how the buffered-WAL variants (2, 5) already work,
  but is a real, now-documented gap between "checkpoint completed" and
  "guaranteed on physical disk" that a future pass could close with an
  explicit `sync_all` if a stronger checkpoint guarantee is ever needed.
- Five variants (plus Tier 2's three) is real, hand-maintained surface
  area — eight independent `create`/`open`/persistence implementations,
  even with the shared read-path core. `RESULTS.md`'s explicit
  recommendation exists specifically so this doesn't become "keep all
  eight forever" by default.

## Validation and revisit triggers

- Validated by: `src/durability/{wal_fsync,wal_buffered,snapshot_rebuild,snapshot_full,hybrid}.rs`'s
  own unit tests (reconstruction-from-WAL-matches-expected-state as the
  highest-priority test for the WAL variants; checkpoint-then-open-matches-
  a-fresh-store for the snapshot variants; the cutoff-boundary
  snapshot-plus-partial-replay test for hybrid — the highest-priority test
  for that variant specifically), `src/durability/mod.rs`'s shared-core
  and WAL-format tests (including the torn-trailing-write recovery test),
  and `benches/durability.rs`'s three-metric, three-size Criterion suite
  (`STORAGE-008`).
- Revisit if: a future workload needs checkpoint calls to carry an
  explicit physical-disk guarantee (see the `fsync` gap above) — that's a
  small, scoped addition to each variant's `checkpoint`, not a redesign.
- Revisit if: `RESULTS.md`'s recommendation to prefer WAL-buffered over
  Hybrid for this crate's specific write volume/dataset shape stops
  holding at a workload this pass didn't test (e.g. far larger batches
  between checkpoints, where Hybrid's bounded-replay design might finally
  earn its complexity) — see `RESULTS.md`'s open questions.
- Revisit if: Hybrid's extra `state.clone()` in `checkpoint()` is fixed —
  at that point its checkpoint cost should be re-benchmarked and this
  ADR's/`RESULTS.md`'s recommendation re-checked, since it was the single
  largest measured cost in this entire durability pass.
