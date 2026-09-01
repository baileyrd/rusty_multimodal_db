# STORAGE-013 — External database benchmark comparison (SQLite, Postgres, DuckDB)

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-001`, `STORAGE-002`, `STORAGE-003`, `STORAGE-006`, ADR-0015
- Supersedes: none

## Purpose and scope

Every benchmark in this crate so far compares `ProductionStore` (and the
backends it grew from) against other designs *this crate itself
implements*. This spec adds one external comparison point: the same three
access-pattern shapes already benchmarked in-repo — `get` (full-record
read by UUID), `scan_ages` (whole-table average-age aggregate), and
`neighbors_one_hop` (`littermate_of` one-hop traversal) — run against
three real, general-purpose databases: SQLite, Postgres, and DuckDB. See
ADR-0015 for why these three engines, why these three shapes, and the
full accounting of what this comparison does and does not claim.

## Non-goals

- Not full SQL parity. No query parser, planner, optimizer, or arbitrary
  join support is built or implied — `docs/FUTURE-GROWTH.md`'s "Path to
  SQLite/DuckDB parity" already scoped that out as a separate, multi-year
  effort. This spec is three fixed, hand-written queries against three
  real engines, nothing more.
- Not a durability or crash-safety comparison across the four systems —
  each external engine runs under its own default durability behavior,
  untuned; see ADR-0015's Consequences.
- Not two-hop (or deeper) graph traversal against the external engines —
  the recursive CTEs here are depth-bounded to one hop, matching this
  round's scope (`neighbors_one_hop`, not `neighbors_two_hop`).
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
  `SELECT AVG(age) FROM dogs`; the graph shape is a `WITH RECURSIVE`
  traversal over `littermates`, depth-bounded to 1 hop, identical in
  structure across all three engines modulo each engine's own parameter
  placeholder syntax (`?1` for SQLite/DuckDB, `$1` for Postgres).
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
  runs all three workload groups against all four systems (`production`,
  `sqlite`, `postgres`, `duckdb`) at all three sizes, completing without
  panics, given a locally reachable Postgres server per ADR-0015's
  "Setup" section.
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

- Two-hop (or deeper) traversal against the external engines is
  unmeasured — the recursive CTEs here are depth-bounded to 1 hop on
  purpose (see ADR-0015's "Considered options").
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
