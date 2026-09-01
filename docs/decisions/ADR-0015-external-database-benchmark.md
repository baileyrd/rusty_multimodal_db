# ADR-0015: Benchmark `ProductionStore` against real external databases (SQLite, Postgres, DuckDB) on the three fixed access patterns already used in-repo

- Status: Accepted
- Date: 2026-09-01
- Deciders: baileyrd
- Related: `docs/charter/CHARTER.md`, `docs/FUTURE-GROWTH.md`, ADR-0001, ADR-0008, `RESULTS.md`, `benches/workloads.rs`, `benches/external_db.rs`
- Supersedes/Superseded by: none

## Context

Every comparison in `RESULTS.md` so far has been internal — `ProductionStore`
against the other backends/durability variants/concurrency strategies this
crate itself implements. That answers "which of *our own* designs is best,"
not "how does the winner compare to what a real project would actually
reach for instead." The owner asked for the second question, specifically
scoped to the three access patterns this crate has always measured (`get`,
`scan_ages`, one-hop `littermate_of` traversal), against three real,
general-purpose engines: SQLite, Postgres, DuckDB.

`docs/FUTURE-GROWTH.md`'s "Path to SQLite/DuckDB parity" already drew the
line this ADR has to stay behind: full SQL (a parser, planner, optimizer,
arbitrary joins) is a multi-year, different-tier effort, explicitly out of
scope. This is not that. This is three fixed, hand-written queries against
three real engines' actual query execution — the same kind of narrow,
falsifiable comparison ADR-0001 ran for AoS/SoA/Canonical, aimed outward
instead of inward.

## Decision drivers

- Stay inside the three access patterns already benchmarked — `get`,
  `scan_ages`, one-hop `neighbors` — not a broader SQL capability
  comparison. Matching `docs/FUTURE-GROWTH.md`'s own scope line keeps this
  from silently becoming the multi-year effort that document already
  named and declined.
- Reuse this crate's own benchmark conventions (`bench_support::build_dataset`,
  `SIZES`, `SEED`, the 200-UUID `RoundRobin` sample pool) so the external
  numbers are built from the *exact same generated dataset* as every
  existing `RESULTS.md` table for that size — not a separately-seeded,
  only-approximately-comparable dataset.
- Each engine gets a fair, idiomatic setup: an indexed primary key for
  `get`, a real aggregate query for `scan_ages`, and a genuine recursive
  CTE over an adjacency table for the graph shape — not a query
  deliberately written to lose.
- Report honestly, including the results that don't favor this crate.
  `ProductionStore` losing a workload to a mature, general-purpose engine
  is a legitimate, useful finding, not something to route around — same
  standard the charter already sets for the in-repo comparisons.
- Keep this fully additive: no source file under `src/` changes, no new
  production dependency. Everything lands in a new dev-only-gated bench
  target.

## Considered options

1. **Embed all three engines in-process (`:memory:` SQLite, DuckDB
   in-memory, and skip Postgres entirely since it has no pure-in-process
   mode).** Rejected — dropping Postgres from the comparison because it
   doesn't fit the other two's deployment model would quietly answer a
   different, easier question ("two embedded engines vs. this crate") than
   the one asked. An all-`:memory:` setup also isn't how SQLite or DuckDB
   are actually deployed in most real use, and would hand them a
   best-case that `ProductionStore`'s own mmap-backed file doesn't get
   either.
2. **Run all three engines on-disk (SQLite/DuckDB as real files under a
   scratch directory; Postgres as a real, separately-run server this
   benchmark connects to but does not manage) and report Postgres's
   client/server round trip honestly as part of what it costs.** Chosen.
   This is how all three are actually used in practice, keeps the
   comparison apples-to-apples with `ProductionStore`'s own on-disk
   (page-cache-warmed) mmap file rather than giving the embedded
   competitors an unrealistic edge, and doesn't misrepresent Postgres by
   silently benchmarking it in a deployment shape (in-process, no network)
   it's never actually run in.
3. **Shell out to each engine's own CLI (`sqlite3`, `psql`, `duckdb`)
   instead of a Rust client library.** Rejected for the same reason
   `rcgen` was chosen over shelling out to `openssl` for
   `SERVER-TLS-DESIGN`'s test certificates (see ADR-0014): process-spawn
   and text-parsing overhead per call would dominate point-workload
   timings, and per-call CLI invocation isn't how a real embedding
   application would use SQLite or DuckDB either. A native Rust client per
   engine (`rusqlite`, `duckdb`, `postgres`) measures the actual query
   execution cost, prepared once and reused, the same standard every
   in-repo backend already gets.
4. **A plain, non-recursive `SELECT` for the one-hop graph shape instead of
   a recursive CTE.** Considered, but the task explicitly asked for a
   recursive CTE (SQLite/DuckDB) or a recursive CTE/`LATERAL` join
   (Postgres) — the standard SQL idiom for graph traversal, depth-bounded
   to 1 in this benchmark's case (`WHERE hop.depth < 1` in the recursive
   term, so it never actually iterates past the first hop). Implemented as
   asked, with the caveat named plainly in `RESULTS.md`: this measures the
   recursive-CTE machinery's cost at one hop, not multi-hop traversal —
   two-hop wasn't part of this round's scope, matching how
   `benches/workloads.rs`'s own `neighbors_two_hop` group is a separate,
   later addition (`STORAGE-006`) rather than assumed by this one.

## Decision

Add a new Criterion bench target, `benches/external_db.rs`, gated behind a
new Cargo feature (`external-db-bench`, additionally requiring `research`
for `bench_support`) so it never compiles into a normal `cargo
build`/`cargo test` run. It benchmarks `ProductionStore` (re-run fresh in
the same process as the external engines, not stitched together from
older `RESULTS.md` numbers measured on a different day/machine) against:

- **SQLite** via `rusqlite` (`bundled` feature — compiles SQLite from
  vendored source, no system `libsqlite3` dependency), a real on-disk
  file, `dogs(id TEXT PRIMARY KEY, breed TEXT, age INTEGER)` plus a
  `littermates(dog_id, littermate_id)` adjacency table (edges inserted in
  both directions, indexed on `dog_id`, since `DogStore::neighbors`
  treats `littermate_of` symmetrically — see `src/store/canonical.rs`).
- **Postgres** via the sync `postgres` crate, against a real, separately
  managed local server (see "Setup" below) — schema and index shape
  identical to SQLite's, using Postgres's native `UUID` column type and
  `COPY ... FROM STDIN` for the (untimed) bulk load.
- **DuckDB** via the `duckdb` crate (`bundled` feature, for the same
  reproducibility reason as `rusqlite`'s), same schema shape, loaded via
  DuckDB's `Appender` API for the (untimed) bulk load.

Every engine's three queries — `SELECT breed, age FROM dogs WHERE id = ?`,
`SELECT AVG(age) FROM dogs`, and a depth-1 `WITH RECURSIVE` traversal over
`littermates` — are prepared once per (engine, dataset size) outside the
timed `b.iter()` closure and reused across iterations, mirroring how a
`HashMap` lookup or array scan in the in-repo backends never re-parses
anything per call either.

**Setup**: this benchmark does not start, stop, or otherwise manage a
Postgres server — it expects one already reachable via
`RUSTY_BENCH_POSTGRES_URL` (default:
`host=127.0.0.1 port=5432 user=postgres dbname=rusty_bench`). A minimal
local setup (no Docker required, matching this crate's own "minimal
dependencies" bar): `initdb`, then `pg_ctl start`, then
`createdb rusty_bench` — see the Postgres project's own documentation for
the exact commands on a given platform. SQLite and DuckDB need no such
setup; both are fully embedded.

## Consequences

**Positive**:

- A real, external comparison point exists for the first time — every
  prior `RESULTS.md` verdict was "best of what this crate itself builds,"
  not "competitive with what you'd actually reach for instead."
- Stays fully inside `docs/FUTURE-GROWTH.md`'s existing scope line: no SQL
  parser, no query planner, no arbitrary joins were built or are implied
  by this comparison — three fixed, hand-written queries against three
  real engines' own execution, nothing more.
- Zero production-code or default-build impact: no `src/` file changes.
  `rusqlite`/`duckdb`/`postgres` are declared as `optional = true` entries
  under `[dependencies]` (not `[dev-dependencies]`), `dep:`-gated behind
  `external-db-bench`, kept out of anything a normal `cargo build`/`cargo
  test`/`cargo doc` compiles — confirmed directly, not assumed (see the
  "A real methodology finding" bullet below for why this needed a real
  fix, not just the obvious-looking approach).

**Negative**:

- **A real methodology finding, caught and fixed before landing, not
  glossed over**: the first version of this dependency wiring put
  `rusqlite`/`duckdb`/`postgres` under `[dev-dependencies]` without
  `optional = true` (the seemingly obvious choice, since none of the
  three are ever used by this crate's own library code) — reasoning that
  the `external_db` bench target's own `required-features` gate would be
  enough to keep them out of a default build. It wasn't: a plain
  `cargo test --release` with **zero features enabled** still compiled
  all three, including DuckDB's full bundled build, confirmed directly by
  watching `cc1plus` compile `libduckdb-sys`'s C++ sources during that
  run. Cargo's `required-features` gates whether a *target* gets built,
  not whether an unconditional (non-optional) dependency of the package
  gets compiled — a non-optional `[dev-dependencies]` entry is available
  to the whole package's dev-target graph regardless of which specific
  target a given `cargo` invocation selects. The fix: move all three to
  `[dependencies]` with `optional = true`, `dep:`-gated behind
  `external-db-bench` — exactly the mechanism `rusty_tls` already uses
  for the identical reason (Cargo has no way to make a *dev*-dependency
  optional at all). Verified after the fix: `cargo tree -e normal` shows
  none of the three without the feature enabled, and a default
  `cargo check --release` finishes in ~6s touching none of them.
- **Three new dependencies, one with a real build-time cost.**
  `duckdb`'s `bundled` feature compiles DuckDB's own (large, C++)
  amalgamation from source — a genuinely heavier one-time build than
  anything else this crate's dev-dependency graph has taken on, including
  `rusty_tls`'s Rust-only `rustls`/`ring` chain. This is compiled (not
  run) by `cargo clippy --all-targets --all-features` on every CI run
  going forward, adding real CI time — flagged plainly here rather than
  minimized, matching how ADR-0014 named `rusty_tls` as "a real,
  meaningfully larger dependency addition than `subtle` was for auth."
  Accepted because the alternative (skipping DuckDB, or requiring a
  preinstalled system DuckDB) either drops one of the three engines the
  task asked for or reintroduces the exact "depends on what happens to be
  on a given machine" non-reproducibility this crate's own `rcgen`
  precedent already rejected.
- **Postgres needs a real, separately-run server this benchmark doesn't
  manage.** Unlike `perf-events` (Linux-only, but self-contained once the
  feature is on) or `server`'s `TlsConfig` (opt-in, no external process),
  running `benches/external_db.rs`'s Postgres portion requires a
  contributor to have `initdb`/`pg_ctl`'d a local instance first — a real,
  environment-specific setup step this benchmark deliberately doesn't
  paper over with an embedded/in-memory substitute (see "Considered
  options" above for why).
- **Not a durability or concurrency comparison.** All three external
  engines default to their own durability guarantees (SQLite's WAL,
  Postgres's WAL, DuckDB's checkpointing) without this benchmark tuning or
  disabling them — a fully controlled-for durability comparison (matching
  fsync behavior, transaction guarantees, etc. across all four systems)
  is out of scope for this round; the numbers below measure query cost
  under each engine's own real default behavior, not a normalized
  "durability turned off everywhere" baseline. Named explicitly, not
  glossed over.
- **Adjacency modeled as a doubled, indexed table**, not the single-row,
  `OR`-predicate shape `DogStore::neighbors`'s own in-memory adjacency
  index uses internally. This is the standard way to model a symmetric
  relationship in SQL for a single-column indexed equality lookup, but it
  means the external engines' `littermates` table holds roughly twice as
  many rows as the minimum a directed-edge model would need — a real,
  intentional data-modeling choice, not an oversight.

## Validation and revisit triggers

See `RESULTS.md`'s `## External database comparison` section for the
actual numbers and per-workload verdicts. Revisit this ADR's scope
specifically if:

- A future round wants two-hop (or deeper) traversal against the external
  engines — the recursive CTEs here are depth-bounded to 1 on purpose (see
  "Considered options" #4) and would need real changes, not just a
  parameter tweak, to go deeper.
- A future round wants durability/crash-safety parity checked across all
  four systems, not just query cost under each engine's own defaults.
- DuckDB's `bundled` CI build-time cost becomes a real friction point —
  the mitigation (a system-DuckDB feature variant, or moving this bench
  to a separate, non-`--all-features`-swept CI job) wasn't pursued this
  round since the cost was judged acceptable, not because no mitigation
  exists.
