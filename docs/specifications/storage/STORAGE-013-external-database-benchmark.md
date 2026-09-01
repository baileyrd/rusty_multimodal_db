# STORAGE-013 — External database benchmark comparison (SQLite, Postgres, DuckDB)

- Version: 0.4.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-003`, `STORAGE-006`, ADR-0015
- Supersedes: none

## Purpose and scope

Every benchmark in this crate so far compares `ProductionStore` (and the
backends it grew from) against other designs *this crate itself
implements*. This spec adds an external comparison point: `get`
(full-record read by UUID), `scan_ages` (whole-table average-age
aggregate), and `littermate_of` graph traversal at one, two (since
v0.2.0), three (since v0.3.0), and four (since v0.4.0) hops deep — run
against three real, general-purpose databases: SQLite, Postgres, and
DuckDB. `get`/`scan_ages`/`neighbors_one_hop`/`neighbors_two_hop` all
mirror an in-repo benchmark shape exactly; `neighbors_three_hop`/
`neighbors_four_hop` do not — this crate's own `bench_support` composes
at most two hops (see ADR-0004), so those two shapes are measured *only*
here, not in-repo. See ADR-0015 for why these three engines, why these
query shapes, and the full accounting of what this comparison does and
does not claim.

**v0.2.0 note**: v0.1.0 deliberately depth-bounded every graph query to
one hop (see the Non-goals this version updates below). `neighbors_two_hop`
is a later addition, completing this spec's already-accepted benchmark
scope rather than opening a new decision — the same "one round adds the
shape, a later round adds the depth" precedent `benches/workloads.rs`'s
own `neighbors_two_hop` group set for `STORAGE-006`. No new ADR: the
query idiom (a depth-bounded `WITH RECURSIVE`), the engines, and the
dataset/methodology are all unchanged from ADR-0015's original decision;
this just extends the depth bound one hop further, symmetrically across
all three engines.

**v0.3.0 note**: a further later addition, same reasoning as v0.2.0 —
the recursive CTE extends one more hop deeper, still no new ADR. The one
real difference from v0.2.0: `neighbors_three_hop` has no
`benches/workloads.rs`/`bench_support` counterpart to mirror, since
`two_hop_neighbors` is this crate's own composition ceiling (ADR-0004).
`ProductionStore`'s three-hop comparison point is instead
`three_hop_neighbors`, a small, private, bench-target-local function
this spec's own implementation adds directly to `benches/external_db.rs`
— not promoted into `bench_support` alongside `two_hop_neighbors`, since
this benchmark is (so far) its only call site and this project's
established convention is not to generalize before a second real one
exists.

**v0.4.0 note**: a fourth later addition, same "no new ADR" reasoning as
v0.2.0/v0.3.0 — `four_hop_neighbors` composes `three_hop_neighbors` one
more round, same private, bench-target-local placement. Unlike the prior
two additions, this one surfaced a genuinely new *result*, not just a
bigger number on an existing trend: at 1,000,000 records, Postgres
overtakes SQLite as the closest external engine on graph traversal — the
first time any engine other than SQLite has been nearest, at any size or
depth, since v0.1.0. See `RESULTS.md`'s `### Graph traversal, four-hop`
subsection for the full account, including why the "margin narrows every
hop" pattern from hops 1-3 broke down rather than continuing cleanly
into hop 4.

## Non-goals

- Not full SQL parity. No query parser, planner, optimizer, or arbitrary
  join support is built or implied — `docs/FUTURE-GROWTH.md`'s "Path to
  SQLite/DuckDB parity" already scoped that out as a separate, multi-year
  effort. This spec is three fixed, hand-written queries against three
  real engines, nothing more.
- Not a durability or crash-safety comparison across the four systems —
  each external engine runs under its own default durability behavior,
  untuned; see ADR-0015's Consequences.
- Not five-or-more-hop graph traversal against the external engines — the
  recursive CTEs here are depth-bounded to four hops as of v0.4.0
  (`neighbors_one_hop`, `neighbors_two_hop`, `neighbors_three_hop`,
  `neighbors_four_hop`; v0.1.0 covered one hop only, each later version
  added one more).
- Not a promotion of `three_hop_neighbors`/`four_hop_neighbors` into
  `bench_support` — both stay private to `benches/external_db.rs`, this
  benchmark's own file, matching this project's "no abstraction before
  two real call sites" convention (see the v0.3.0/v0.4.0 notes above).
  Composing a third/fourth hop generically inside `bench_support` itself,
  alongside `two_hop_neighbors`, remains a separate, unmade decision.
- Not a new `DogStore` backend and not a change to any existing one — no
  `src/store/**`, `src/production.rs`, or `src/generator.rs` changes.
  Benchmark-harness-only work, same convention `STORAGE-007` (mixed
  workload) and `STORAGE-010` (concurrency prototypes) already followed
  for their own harness-only additions.
- Not exercising `GenericProductionStore` or any non-`Dog` domain — this
  stays inside the `Dog` domain `RESULTS.md`'s existing tables already use,
  for direct side-by-side comparability.

## Context and terminology

- **Embedded engine**: SQLite and DuckDB — run in-process, against a real
  on-disk file (not `:memory:`), via `rusqlite`/`duckdb`'s `bundled`
  build.
- **Client/server engine**: Postgres — a real client (the `postgres`
  crate) talking to a separately-run local server process this benchmark
  does not start, stop, or otherwise manage.
- **Adjacency table**: `littermates(dog_id, littermate_id)`, loaded with
  both directions of every generated edge (see ADR-0015's Consequences for
  why), indexed on `dog_id`, queried via a depth-bounded `WITH RECURSIVE`
  traversal.

## Requirements

- `STORAGE-013-FR-001`: A new Criterion bench target,
  `benches/external_db.rs`, gated behind a new `external-db-bench` Cargo
  feature (additionally requiring `research` for `bench_support`), so it
  never compiles into a default `cargo build`/`cargo test` run.
- `STORAGE-013-FR-002`: Each dataset is built once per size via the
  existing `bench_support::build_dataset` — same generator, same `SEED`,
  same `SIZES`, same 200-UUID `RoundRobin` sample pool as every other
  benchmark in this crate — so results are directly comparable to
  `RESULTS.md`'s existing tables for that size, not built from a
  separately-seeded dataset.
- `STORAGE-013-FR-003`: Schema creation and bulk data load happen once per
  (engine, size) *outside* the timed `b.iter()` closure for every
  workload — matching how every in-repo backend's own construction cost
  is excluded from its own benchmark numbers.
- `STORAGE-013-FR-004`: Every query is prepared once per (engine, size)
  and reused across iterations — no per-call SQL parse/plan cost, the same
  standard a plain Rust method call already gets for free in every
  in-repo backend.
- `STORAGE-013-FR-005`: `get` is `SELECT breed, age FROM dogs WHERE id = ?`
  against an indexed primary key (`dogs.id`); `scan_ages` is
  `SELECT AVG(age) FROM dogs`; the one-hop graph shape is a
  `WITH RECURSIVE` traversal over `littermates`, depth-bounded to 1 hop,
  identical in structure across all three engines modulo each engine's own
  parameter placeholder syntax (`?1` for SQLite/DuckDB, `$1` for
  Postgres).
- `STORAGE-013-FR-008` (v0.2.0): the two-hop graph shape is the same
  `WITH RECURSIVE` traversal, extended one hop deeper (`WHERE hop.depth <
  2` in the recursive term, `WHERE depth = 2` in the final `SELECT`),
  matching `bench_support::two_hop_neighbors`'s exact semantics — the
  deduplicated union of `neighbors(n)` for every `n` in `neighbors(id)` —
  identical in structure across all three engines, same parameter
  placeholder convention as FR-005.
- `STORAGE-013-FR-009` (v0.3.0): the three-hop graph shape is the same
  `WITH RECURSIVE` traversal, extended one hop deeper again (`WHERE
  hop.depth < 3` in the recursive term, `WHERE depth = 3` in the final
  `SELECT`), matching `benches/external_db.rs`'s own private
  `three_hop_neighbors` — the deduplicated union of `neighbors(n)` for
  every `n` in `two_hop_neighbors(id)` — identical in structure across
  all three engines, same parameter placeholder convention as FR-005/
  FR-008.
- `STORAGE-013-FR-010` (v0.4.0): the four-hop graph shape is the same
  `WITH RECURSIVE` traversal, extended one hop deeper again (`WHERE
  hop.depth < 4` in the recursive term, `WHERE depth = 4` in the final
  `SELECT`), matching `benches/external_db.rs`'s own private
  `four_hop_neighbors` — the deduplicated union of `neighbors(n)` for
  every `n` in `three_hop_neighbors(id)` — identical in structure across
  all three engines, same parameter placeholder convention as FR-005/
  FR-008/FR-009.
- `STORAGE-013-FR-006`: `ProductionStore` is re-run fresh in the same
  benchmark process as the three external engines (via its existing
  `From<(Vec<DogRecord>, Vec<(Uuid, Uuid)>)>` impl), not read back from an
  older `RESULTS.md` run — so the comparison is same-day, same-machine,
  same-process for every entry in a given table.
- `STORAGE-013-FR-007`: Postgres connects via `RUSTY_BENCH_POSTGRES_URL`
  if set, else a documented local default
  (`host=127.0.0.1 port=5432 user=postgres dbname=rusty_bench`) — no
  hardcoded, unoverridable connection string.

## Architecture and interfaces

`benches/external_db.rs` (new) — the whole of this spec's implementation.
Reuses `rusty_multimodal_db::bench_support::{build_dataset, Dataset,
RoundRobin, SIZES}` and `rusty_multimodal_db::{DogStore, ProductionStore}`
unchanged. Three new dependencies used only by this bench target
(`rusqlite`, `duckdb`, `postgres`), `optional = true` under
`[dependencies]` (not `[dev-dependencies]` — Cargo has no way to make a
dev-dependency optional, confirmed the hard way, see ADR-0015's
Consequences), `dep:`-gated behind `external-db-bench`, plumbed through
`Cargo.toml`'s `[[bench]]`/`[features]`. No `src/` changes.

## Data/state and invariants

- Each engine's `dogs`/`littermates` schema is created fresh per (engine,
  size) run — SQLite/DuckDB via a fresh scratch file under
  `std::env::temp_dir()`, Postgres via a fresh, size-named schema
  (`bench_{n}`) dropped and recreated with `DROP SCHEMA IF EXISTS ...
  CASCADE`, so repeated runs never accumulate stale data or collide across
  sizes run in the same process.
- Edge symmetry: `DogStore::neighbors`'s in-memory adjacency index treats
  `littermate_of` as symmetric (see `src/store/canonical.rs`'s
  `neighbors_finds_edge_in_either_direction` test) — every external
  engine's `littermates` table is loaded with both `(a, b)` and `(b, a)`
  for each generated edge so a single-column indexed `dog_id = ?` lookup
  reproduces the same symmetric result set, rather than needing an `OR`
  predicate that would defeat a single-column index.

## Errors, failure, recovery, and observability

Every fallible setup/query call `.expect()`s with a message naming what
failed and, for the one call requiring external state this benchmark
doesn't provide (the Postgres connection), how to fix it — matching this
crate's own "no `unwrap`/`expect` outside tests" rule's bench-target
exception (Criterion bench closures aren't `#[cfg(test)]`, but panicking
on setup failure here is the same "genuinely exceptional environment
failure, not a normal outcome" reasoning `ProductionStore::fresh_backing_path`
already documents for its own panics).

## Security, privacy, and compatibility

Not applicable — synthetic in-memory-generated data only, same as every
other spec in this tree. The local Postgres instance this benchmark
expects is not a network-exposed service (default `127.0.0.1`-only setup
in ADR-0015's "Setup" section) and carries no relation to the `server`
feature's `AuthConfig`/`TlsConfig` — this is a benchmark harness dependency,
not the crate's own network surface.

## Acceptance criteria

- `cargo check --bench external_db --features research,external-db-bench`
  compiles clean.
- `cargo bench --features research,external-db-bench --bench external_db`
  runs all six workload groups (`get`, `scan_ages`, `neighbors_one_hop`,
  `neighbors_two_hop`, `neighbors_three_hop`, `neighbors_four_hop`)
  against all four systems (`production`, `sqlite`, `postgres`,
  `duckdb`) at all three sizes, completing without panics, given a
  locally reachable Postgres server per ADR-0015's "Setup" section.
- `RESULTS.md` has a new `## External database comparison` section,
  structured like the rest of the file (one table + verdict per
  workload), that reports real numbers from an actual run and states
  per-workload verdicts honestly — including any workload where
  `ProductionStore` loses to an external engine.
- No `src/` changes — verified by diff, same bar `STORAGE-007`/`STORAGE-010`
  held themselves to.

## Verification plan

- `cargo clippy --all-targets --all-features -- -D warnings` includes
  `benches/external_db.rs` (via `--all-targets`) and stays clean.
- A real `cargo bench --features research,external-db-bench --bench
  external_db` run, this session, against a local Postgres instance
  started per ADR-0015's documented setup — see `RESULTS.md` for the
  actual output this pass produced and the sampling discipline used
  (matching `RESULTS.md`'s own established "reduced sampling for a
  container run" precedent, named explicitly rather than silently
  assumed equivalent to Criterion's defaults).

## Traceability

Implements: the "external database benchmark comparison" deliverable.
Depends on: `STORAGE-001`/`STORAGE-002` (the dataset generator and
`DogStore` trait this benchmarks against), `STORAGE-003` (the benchmark
suite structure and `bench_support` infrastructure this reuses),
`STORAGE-006` (the `littermate_of` edge relationship and one-hop
`neighbors` shape this compares graph traversal against), ADR-0015 (this
spec's own decision record). Feeds: `RESULTS.md`'s `## External database
comparison` section.

## Open questions

- Five-or-more-hop traversal against the external engines is unmeasured
  — the recursive CTEs here are depth-bounded to 4 hops as of v0.4.0.
  Unlike the v0.2.0→v0.3.0 step (where the Postgres join-strategy
  finding reproduced cleanly), v0.3.0→v0.4.0 showed a trend genuinely
  breaking rather than continuing: SQLite's margin over `ProductionStore`
  narrowed every hop through hop 3, then went flat at hop 4, and Postgres
  overtook SQLite as the closest external engine at 1,000,000 records for
  the first time (see `RESULTS.md`'s `### Graph traversal, four-hop`
  subsection). Whether that crossover holds, widens, or reverses at hop
  5 is genuinely unknown — extrapolating from the hop-1-through-4 numbers
  alone would not be safe, per the hop-4 break itself.
- Whether `three_hop_neighbors`/`four_hop_neighbors` (or an N-hop
  generalization) belongs in `bench_support` alongside `two_hop_neighbors`
  is an open, deliberately unmade decision — see the v0.3.0/v0.4.0
  Non-goals bullet above. A second real call site (e.g. a genuine
  in-repo `neighbors_three_hop` benchmark) would be the natural trigger
  to revisit it.
- A controlled-for durability comparison (matching fsync/WAL/checkpoint
  behavior across all four systems rather than each engine's own
  defaults) is unmeasured — see ADR-0015's Consequences.
- Cache-miss/hardware-counter instrumentation (this crate's
  `perf-events`/`cache_events.rs` suite) was not extended to the external
  engines this round — wall-clock only, matching this spec's own scope.
- Concurrent/multi-client access to the external engines (a closer analog
  to `ProductionStore`'s own concurrency story, `STORAGE-010`) is
  unmeasured — this round is single-threaded, single-connection, matching
  every other workload benchmark in this crate before its own later
  concurrency pass.

## Change history

- 0.1.0 (2026-09-01): Initial accepted draft, alongside ADR-0015 and the
  real `benches/external_db.rs` implementation and `RESULTS.md` numbers.
- 0.2.0 (2026-09-01): Added `neighbors_two_hop` (`STORAGE-013-FR-008`) —
  the depth-1 recursive CTE extended one hop deeper, against
  `ProductionStore`'s own `bench_support::two_hop_neighbors`. Real numbers
  in `RESULTS.md`'s new `### Graph traversal, two-hop` subsection; a
  genuine, investigated finding (a cost-based Postgres join-strategy flip
  between table sizes, not a benchmark artifact) reported alongside them.
  No `src/` changes, no new ADR — completes v0.1.0's own already-accepted
  scope rather than opening a new decision.
- 0.3.0 (2026-09-01): Added `neighbors_three_hop` (`STORAGE-013-FR-009`)
  — the depth-2 recursive CTE extended one hop deeper, against a new
  private, bench-target-local `three_hop_neighbors` (not promoted into
  `bench_support`; no in-repo counterpart exists for this shape). Real
  numbers in `RESULTS.md`'s new `### Graph traversal, three-hop`
  subsection; the v0.2.0 Postgres join-strategy finding reproduced
  identically at this depth, confirming it as a real, repeatable
  Postgres planning behavior rather than a one-off. No `src/` changes,
  no new ADR — same "completes already-accepted scope" reasoning as
  v0.2.0.
- 0.4.0 (2026-09-01): Added `neighbors_four_hop` (`STORAGE-013-FR-010`)
  — the depth-3 recursive CTE extended one hop deeper, against a new
  private, bench-target-local `four_hop_neighbors`. Real numbers in
  `RESULTS.md`'s new `### Graph traversal, four-hop` subsection. Unlike
  v0.2.0/v0.3.0, this round found a genuinely new result rather than a
  bigger version of an existing one: the "SQLite's margin narrows every
  hop" trend from hops 1-3 broke down at hop 4 (essentially flat at
  1,000 records), and Postgres overtook SQLite as the closest external
  engine at 1,000,000 records for the first time — driven by the same
  join-strategy mechanism found at v0.2.0, confirmed again via
  `EXPLAIN (ANALYZE, BUFFERS)`. No `src/` changes, no new ADR — same
  "completes already-accepted scope" reasoning as v0.2.0/v0.3.0.
