//! Compares [`ProductionStore`] against three real external databases —
//! SQLite (`rusqlite`), Postgres (`postgres`, against a locally-run
//! server), and DuckDB (`duckdb`) — on `get` (full-record read by UUID),
//! `scan_ages` (a whole-table aggregate), and `littermate_of` graph
//! traversal at one through four hops deep. `get`/`scan_ages`/
//! `neighbors_one_hop`/`neighbors_two_hop` mirror `benches/workloads.rs`'s
//! own in-repo groups exactly; `neighbors_three_hop`/`neighbors_four_hop`
//! (`STORAGE-013` v0.3.0/v0.4.0) do not — no in-repo three-or-more-hop
//! group exists (`bench_support` only composes up to two hops, see
//! ADR-0004) — so this one benchmark's own `three_hop_neighbors`/
//! `four_hop_neighbors` below are the only place those exact shapes are
//! measured at all, external or in-repo. See
//! `docs/decisions/ADR-0015-external-database-benchmark.md` for why these
//! three engines, why these query shapes, and what this comparison does
//! and does not claim to answer — most importantly, this is *not* the
//! full-SQL-parity comparison `docs/FUTURE-GROWTH.md` already scoped out;
//! it is the same fixed access patterns this crate has always
//! benchmarked (plus these external-only extra hops), aimed at three
//! more (external, general-purpose) targets. `neighbors_two_hop`,
//! `neighbors_three_hop`, and `neighbors_four_hop` are later additions
//! (`STORAGE-013` v0.2.0, v0.3.0, v0.4.0) — the original round (v0.1.0)
//! deliberately depth-bounded every graph query to one hop.
//!
//! Each dataset is built once per size via
//! [`rusty_multimodal_db::bench_support::build_dataset`] — the exact same
//! generator output (records, `littermate_of` edges, sample UUIDs) every
//! other benchmark in this crate uses for that size — then loaded into
//! each engine before any timing starts (schema creation and bulk load
//! are explicitly *not* part of the measured `b.iter()` closure, matching
//! how every in-repo backend's own construction happens outside the timed
//! portion too). Every query is prepared once per (engine, size) and
//! reused across iterations, the same "no per-call parse/plan cost"
//! standard the in-repo backends already get for free from being plain
//! Rust method calls.
//!
//! Postgres is the one real asymmetry: it's the only engine of the three
//! that isn't embedded-in-process. This benchmark does not start or stop
//! the Postgres server itself (see the ADR's "Setup" section for how to
//! provide one) — a real client/server round trip over a real socket is
//! part of what Postgres actually costs, not an oversight to normalize
//! away.

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion,
};
use rusty_multimodal_db::bench_support::{
    build_dataset, two_hop_neighbors, Dataset, RoundRobin, SIZES,
};
use rusty_multimodal_db::{DogStore, ProductionStore};

use duckdb::Connection as DuckConn;
use postgres::{Client as PgClient, NoTls};
use rusqlite::Connection as SqliteConn;
use std::collections::HashSet;
use std::path::PathBuf;
use uuid::Uuid;

/// A third hop composed the same way `bench_support::two_hop_neighbors` composes its own two —
/// generically, via repeated `DogStore::neighbors` calls, no trait method (see ADR-0004): the
/// deduplicated union of `neighbors(n)` for every `n` in `two_hop_neighbors(id)`. Lives here,
/// private to this bench target, rather than in `bench_support` alongside `two_hop_neighbors` —
/// this benchmark is (so far) its only call site, and this project's own convention is not to
/// generalize before a second real one exists (see `docs/PROJECT-STATUS.md`'s repeated
/// "no abstraction before two real call sites" reasoning).
fn three_hop_neighbors<S: DogStore>(store: &S, id: Uuid) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    for two_hop in two_hop_neighbors(store, id) {
        for three_hop in store.neighbors(two_hop) {
            seen.insert(three_hop);
        }
    }
    seen.into_iter().collect()
}

/// A fourth hop, composed the identical way one more time: the deduplicated union of
/// `neighbors(n)` for every `n` in `three_hop_neighbors(id)`. Same rationale as
/// `three_hop_neighbors` for staying private to this bench target rather than moving into
/// `bench_support` — still this benchmark's only call site.
fn four_hop_neighbors<S: DogStore>(store: &S, id: Uuid) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    for three_hop in three_hop_neighbors(store, id) {
        for four_hop in store.neighbors(three_hop) {
            seen.insert(four_hop);
        }
    }
    seen.into_iter().collect()
}

/// Connection string for the locally-run Postgres server this benchmark
/// expects but does not manage — see the ADR's "Setup" section for how to
/// start one. Overridable via `RUSTY_BENCH_POSTGRES_URL` so a contributor
/// running this outside the exact setup documented there isn't stuck.
fn postgres_conn_string() -> String {
    std::env::var("RUSTY_BENCH_POSTGRES_URL")
        .unwrap_or_else(|_| "host=127.0.0.1 port=5432 user=postgres dbname=rusty_bench".into())
}

/// A fresh, process-unique scratch path for an embedded engine's on-disk
/// file — real disk-file engines (not `:memory:`), matching how both
/// SQLite and DuckDB are actually deployed in practice, and letting the
/// OS page cache behave the same way it does for `ProductionStore`'s own
/// mmap-backed file after warmup, rather than giving the embedded
/// competitors an unrealistic all-RAM best case `ProductionStore` doesn't
/// get either.
fn scratch_path(engine: &str, n: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rusty_multimodal_db-external-db-bench-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("creating this benchmark's own scratch dir failed");
    dir.join(format!("{engine}_{n}.db"))
}

// ---------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------

fn sqlite_load(dataset: &Dataset, n: usize) -> SqliteConn {
    let path = scratch_path("sqlite", n);
    let _ = std::fs::remove_file(&path);
    let mut conn = SqliteConn::open(&path).expect("opening a fresh SQLite file failed");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE dogs (id TEXT PRIMARY KEY, breed TEXT NOT NULL, age INTEGER NOT NULL);
         CREATE TABLE littermates (dog_id TEXT NOT NULL, littermate_id TEXT NOT NULL);
         CREATE INDEX idx_littermates_dog_id ON littermates (dog_id);",
    )
    .expect("SQLite schema creation failed");

    let tx = conn.transaction().expect("SQLite transaction begin failed");
    {
        let mut insert_dog = tx
            .prepare("INSERT INTO dogs (id, breed, age) VALUES (?1, ?2, ?3)")
            .expect("preparing SQLite dog insert failed");
        for r in &dataset.records {
            insert_dog
                .execute(rusqlite::params![r.id.to_string(), r.breed, r.age])
                .expect("SQLite dog insert failed");
        }
        let mut insert_edge = tx
            .prepare("INSERT INTO littermates (dog_id, littermate_id) VALUES (?1, ?2)")
            .expect("preparing SQLite edge insert failed");
        for (a, b) in &dataset.edges {
            insert_edge
                .execute(rusqlite::params![a.to_string(), b.to_string()])
                .expect("SQLite edge insert failed");
            insert_edge
                .execute(rusqlite::params![b.to_string(), a.to_string()])
                .expect("SQLite edge insert failed");
        }
    }
    tx.commit().expect("SQLite transaction commit failed");
    conn
}

fn run_get_sqlite(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = sqlite_load(dataset, n);
    let mut stmt = conn
        .prepare("SELECT breed, age FROM dogs WHERE id = ?1")
        .expect("preparing SQLite get failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let row: (String, u32) = stmt
                .query_row(rusqlite::params![id.to_string()], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .expect("SQLite get query failed");
            black_box(row)
        });
    });
}

fn run_scan_ages_sqlite(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = sqlite_load(dataset, n);
    let mut stmt = conn
        .prepare("SELECT AVG(age) FROM dogs")
        .expect("preparing SQLite scan failed");
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let avg: f64 = stmt
                .query_row([], |row| row.get(0))
                .expect("SQLite scan query failed");
            black_box(avg)
        });
    });
}

fn run_neighbors_sqlite(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = sqlite_load(dataset, n);
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 1
             )
             SELECT id FROM hop",
        )
        .expect("preparing SQLite neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params![id.to_string()], |row| row.get(0))
                .expect("SQLite neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("SQLite neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_two_hop_sqlite(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = sqlite_load(dataset, n);
    // Same recursive shape as the one-hop query above, extended one hop deeper: `depth < 2`
    // lets the recursive term fire once more (from the depth-1 base rows), producing depth-2
    // rows; the final `WHERE depth = 2` keeps only those, matching
    // `bench_support::two_hop_neighbors`'s own semantics exactly (the deduplicated union of
    // `neighbors(n)` for every `n` in `neighbors(id)` — not the 1-hop set itself, unless a
    // record is also reachable at depth 2, e.g. through a return edge).
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 2
             )
             SELECT id FROM hop WHERE depth = 2",
        )
        .expect("preparing SQLite two-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params![id.to_string()], |row| row.get(0))
                .expect("SQLite two-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("SQLite two-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_three_hop_sqlite(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = sqlite_load(dataset, n);
    // Same recursive shape, extended one hop deeper again: `depth < 3` lets the recursive term
    // fire once more, from the depth-2 rows, producing depth-3 rows; the final `WHERE depth = 3`
    // keeps only those, matching this file's own `three_hop_neighbors` (the deduplicated union
    // of `neighbors(n)` for every `n` in `two_hop_neighbors(id)`).
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 3
             )
             SELECT id FROM hop WHERE depth = 3",
        )
        .expect("preparing SQLite three-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params![id.to_string()], |row| row.get(0))
                .expect("SQLite three-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("SQLite three-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_four_hop_sqlite(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = sqlite_load(dataset, n);
    // Same recursive shape, extended one hop deeper yet again: `depth < 4` lets the recursive
    // term fire once more, from the depth-3 rows, producing depth-4 rows; the final
    // `WHERE depth = 4` keeps only those, matching this file's own `four_hop_neighbors`.
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 4
             )
             SELECT id FROM hop WHERE depth = 4",
        )
        .expect("preparing SQLite four-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("sqlite", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params![id.to_string()], |row| row.get(0))
                .expect("SQLite four-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("SQLite four-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

// ---------------------------------------------------------------------
// DuckDB
// ---------------------------------------------------------------------

fn duckdb_load(dataset: &Dataset, n: usize) -> DuckConn {
    let path = scratch_path("duckdb", n);
    let _ = std::fs::remove_file(&path);
    let conn = DuckConn::open(&path).expect("opening a fresh DuckDB file failed");
    conn.execute_batch(
        "CREATE TABLE dogs (id UUID PRIMARY KEY, breed VARCHAR NOT NULL, age INTEGER NOT NULL);
         CREATE TABLE littermates (dog_id UUID NOT NULL, littermate_id UUID NOT NULL);
         CREATE INDEX idx_littermates_dog_id ON littermates (dog_id);",
    )
    .expect("DuckDB schema creation failed");

    {
        let mut appender = conn.appender("dogs").expect("DuckDB dogs appender failed");
        for r in &dataset.records {
            appender
                .append_row(duckdb::params![r.id.to_string(), r.breed, r.age])
                .expect("DuckDB dog append failed");
        }
        appender.flush().expect("DuckDB dogs appender flush failed");
    }
    {
        let mut appender = conn
            .appender("littermates")
            .expect("DuckDB littermates appender failed");
        for (a, b) in &dataset.edges {
            appender
                .append_row(duckdb::params![a.to_string(), b.to_string()])
                .expect("DuckDB edge append failed");
            appender
                .append_row(duckdb::params![b.to_string(), a.to_string()])
                .expect("DuckDB edge append failed");
        }
        appender
            .flush()
            .expect("DuckDB littermates appender flush failed");
    }
    conn
}

fn run_get_duckdb(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare("SELECT breed, age FROM dogs WHERE id = ?1")
        .expect("preparing DuckDB get failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let row: (String, i32) = stmt
                .query_row(duckdb::params![id.to_string()], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .expect("DuckDB get query failed");
            black_box(row)
        });
    });
}

fn run_scan_ages_duckdb(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare("SELECT AVG(age) FROM dogs")
        .expect("preparing DuckDB scan failed");
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let avg: f64 = stmt
                .query_row([], |row| row.get(0))
                .expect("DuckDB scan query failed");
            black_box(avg)
        });
    });
}

fn run_neighbors_duckdb(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 1
             )
             SELECT id FROM hop",
        )
        .expect("preparing DuckDB neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(duckdb::params![id.to_string()], |row| row.get(0))
                .expect("DuckDB neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("DuckDB neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_two_hop_duckdb(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 2
             )
             SELECT id FROM hop WHERE depth = 2",
        )
        .expect("preparing DuckDB two-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(duckdb::params![id.to_string()], |row| row.get(0))
                .expect("DuckDB two-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("DuckDB two-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_three_hop_duckdb(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 3
             )
             SELECT id FROM hop WHERE depth = 3",
        )
        .expect("preparing DuckDB three-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(duckdb::params![id.to_string()], |row| row.get(0))
                .expect("DuckDB three-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("DuckDB three-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

fn run_neighbors_four_hop_duckdb(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let conn = duckdb_load(dataset, n);
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = ?1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 4
             )
             SELECT id FROM hop WHERE depth = 4",
        )
        .expect("preparing DuckDB four-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("duckdb", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows: Vec<String> = stmt
                .query_map(duckdb::params![id.to_string()], |row| row.get(0))
                .expect("DuckDB four-hop neighbors query failed")
                .collect::<Result<_, _>>()
                .expect("DuckDB four-hop neighbors row decode failed");
            black_box(rows)
        });
    });
}

// ---------------------------------------------------------------------
// Postgres
// ---------------------------------------------------------------------

fn postgres_load(dataset: &Dataset, n: usize) -> PgClient {
    let mut client = PgClient::connect(&postgres_conn_string(), NoTls).expect(
        "connecting to the local Postgres server failed — see \
         docs/decisions/ADR-0015-external-database-benchmark.md's \"Setup\" section for how to \
         start one, or set RUSTY_BENCH_POSTGRES_URL to point at an existing instance",
    );
    let schema = format!("bench_{n}");
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE;
             CREATE SCHEMA {schema};
             CREATE TABLE {schema}.dogs (id UUID PRIMARY KEY, breed TEXT NOT NULL, age INTEGER NOT NULL);
             CREATE TABLE {schema}.littermates (dog_id UUID NOT NULL, littermate_id UUID NOT NULL);
             CREATE INDEX idx_littermates_dog_id ON {schema}.littermates (dog_id);"
        ))
        .expect("Postgres schema creation failed");

    {
        let mut writer = client
            .copy_in(&format!(
                "COPY {schema}.dogs (id, breed, age) FROM STDIN (FORMAT csv)"
            ))
            .expect("Postgres dogs COPY IN failed to start");
        for r in &dataset.records {
            use std::io::Write;
            writeln!(writer, "{},{},{}", r.id, r.breed, r.age)
                .expect("Postgres dogs COPY IN write failed");
        }
        writer
            .finish()
            .expect("Postgres dogs COPY IN commit failed");
    }
    {
        let mut writer = client
            .copy_in(&format!(
                "COPY {schema}.littermates (dog_id, littermate_id) FROM STDIN (FORMAT csv)"
            ))
            .expect("Postgres littermates COPY IN failed to start");
        for (a, b) in &dataset.edges {
            use std::io::Write;
            writeln!(writer, "{a},{b}").expect("Postgres littermates COPY IN write failed");
            writeln!(writer, "{b},{a}").expect("Postgres littermates COPY IN write failed");
        }
        writer
            .finish()
            .expect("Postgres littermates COPY IN commit failed");
    }
    client
        .batch_execute(&format!("SET search_path TO {schema}"))
        .expect("Postgres search_path set failed");
    // Without this, `pg_class.reltuples` stays at its post-CREATE-TABLE default (-1, "unknown")
    // — COPY doesn't update planner statistics itself. A recursive CTE's cost estimate against
    // an un-ANALYZEd table is wildly wrong (measured ~93,000 vs. ~260 real cost on a 100K-row
    // table), enough to cross Postgres's default `jit_above_cost` threshold and trigger LLVM JIT
    // compilation on every single execution — a ~270× real slowdown (53 ms vs. 0.2 ms, measured
    // directly via EXPLAIN ANALYZE) that has nothing to do with the query itself. Skipping this
    // would be an unrepresentative gap in this benchmark's own setup, not a fair measurement of
    // Postgres: any real deployment runs ANALYZE after a bulk load.
    client
        .batch_execute(&format!(
            "ANALYZE {schema}.dogs; ANALYZE {schema}.littermates;"
        ))
        .expect("Postgres post-load ANALYZE failed");
    client
}

fn run_get_postgres(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let mut client = postgres_load(dataset, n);
    let stmt = client
        .prepare("SELECT breed, age FROM dogs WHERE id = $1")
        .expect("preparing Postgres get failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let row = client
                .query_one(&stmt, &[&id])
                .expect("Postgres get query failed");
            black_box((row.get::<_, String>(0), row.get::<_, i32>(1)))
        });
    });
}

fn run_scan_ages_postgres(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let mut client = postgres_load(dataset, n);
    // Postgres's AVG() over an integer column returns `numeric`, not `float8` — an explicit
    // cast keeps the wire type a plain f64, matching SQLite's/DuckDB's own AVG() (both already
    // return a float for an integer column, no cast needed there).
    let stmt = client
        .prepare("SELECT AVG(age)::float8 FROM dogs")
        .expect("preparing Postgres scan failed");
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let row = client
                .query_one(&stmt, &[])
                .expect("Postgres scan query failed");
            black_box(row.get::<_, Option<f64>>(0))
        });
    });
}

fn run_neighbors_postgres(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let mut client = postgres_load(dataset, n);
    let stmt = client
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = $1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 1
             )
             SELECT id FROM hop",
        )
        .expect("preparing Postgres neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows = client
                .query(&stmt, &[&id])
                .expect("Postgres neighbors query failed");
            black_box(
                rows.iter()
                    .map(|row| row.get::<_, Uuid>(0))
                    .collect::<Vec<_>>(),
            )
        });
    });
}

fn run_neighbors_two_hop_postgres(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let mut client = postgres_load(dataset, n);
    let stmt = client
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = $1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 2
             )
             SELECT id FROM hop WHERE depth = 2",
        )
        .expect("preparing Postgres two-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows = client
                .query(&stmt, &[&id])
                .expect("Postgres two-hop neighbors query failed");
            black_box(
                rows.iter()
                    .map(|row| row.get::<_, Uuid>(0))
                    .collect::<Vec<_>>(),
            )
        });
    });
}

fn run_neighbors_three_hop_postgres(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let mut client = postgres_load(dataset, n);
    let stmt = client
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = $1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 3
             )
             SELECT id FROM hop WHERE depth = 3",
        )
        .expect("preparing Postgres three-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows = client
                .query(&stmt, &[&id])
                .expect("Postgres three-hop neighbors query failed");
            black_box(
                rows.iter()
                    .map(|row| row.get::<_, Uuid>(0))
                    .collect::<Vec<_>>(),
            )
        });
    });
}

fn run_neighbors_four_hop_postgres(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let mut client = postgres_load(dataset, n);
    let stmt = client
        .prepare(
            "WITH RECURSIVE hop(id, depth) AS (
                 SELECT littermate_id, 1 FROM littermates WHERE dog_id = $1
                 UNION
                 SELECT l.littermate_id, hop.depth + 1
                 FROM littermates l JOIN hop ON l.dog_id = hop.id
                 WHERE hop.depth < 4
             )
             SELECT id FROM hop WHERE depth = 4",
        )
        .expect("preparing Postgres four-hop neighbors query failed");
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("postgres", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            let rows = client
                .query(&stmt, &[&id])
                .expect("Postgres four-hop neighbors query failed");
            black_box(
                rows.iter()
                    .map(|row| row.get::<_, Uuid>(0))
                    .collect::<Vec<_>>(),
            )
        });
    });
}

// ---------------------------------------------------------------------
// ProductionStore (re-run fresh here, same process/session as the
// external engines above, so the comparison isn't stitched together from
// numbers measured on different days/machines — see the ADR).
// ---------------------------------------------------------------------

fn run_get_production(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(store.get(black_box(id)))
        });
    });
}

fn run_scan_ages_production(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| black_box(store.scan_ages()));
    });
}

fn run_neighbors_production(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, dataset: &Dataset) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(store.neighbors(black_box(id)))
        });
    });
}

fn run_neighbors_two_hop_production(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(two_hop_neighbors(&store, black_box(id)))
        });
    });
}

fn run_neighbors_three_hop_production(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(three_hop_neighbors(&store, black_box(id)))
        });
    });
}

fn run_neighbors_four_hop_production(
    group: &mut BenchmarkGroup<'_, WallTime>,
    n: usize,
    dataset: &Dataset,
) {
    let store = ProductionStore::from((dataset.records.clone(), dataset.edges.clone()));
    let mut cursor = RoundRobin::new(dataset.sample_ids.len());
    group.bench_with_input(BenchmarkId::new("production", n), &n, |b, _| {
        b.iter(|| {
            let id = dataset.sample_ids[cursor.advance()];
            black_box(four_hop_neighbors(&store, black_box(id)))
        });
    });
}

// ---------------------------------------------------------------------
// Criterion wiring — one group per workload, mirroring
// `benches/workloads.rs`'s own shape, `production` listed first.
// ---------------------------------------------------------------------

fn bench_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_get_production(&mut group, n, &dataset);
        run_get_sqlite(&mut group, n, &dataset);
        run_get_postgres(&mut group, n, &dataset);
        run_get_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

fn bench_scan_ages(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_ages_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_scan_ages_production(&mut group, n, &dataset);
        run_scan_ages_sqlite(&mut group, n, &dataset);
        run_scan_ages_postgres(&mut group, n, &dataset);
        run_scan_ages_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

fn bench_neighbors_one_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_one_hop_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_production(&mut group, n, &dataset);
        run_neighbors_sqlite(&mut group, n, &dataset);
        run_neighbors_postgres(&mut group, n, &dataset);
        run_neighbors_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

fn bench_neighbors_two_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_two_hop_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_two_hop_production(&mut group, n, &dataset);
        run_neighbors_two_hop_sqlite(&mut group, n, &dataset);
        run_neighbors_two_hop_postgres(&mut group, n, &dataset);
        run_neighbors_two_hop_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

fn bench_neighbors_three_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_three_hop_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_three_hop_production(&mut group, n, &dataset);
        run_neighbors_three_hop_sqlite(&mut group, n, &dataset);
        run_neighbors_three_hop_postgres(&mut group, n, &dataset);
        run_neighbors_three_hop_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

fn bench_neighbors_four_hop(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbors_four_hop_vs_external_db");
    for &n in &SIZES {
        let dataset = build_dataset(n);
        run_neighbors_four_hop_production(&mut group, n, &dataset);
        run_neighbors_four_hop_sqlite(&mut group, n, &dataset);
        run_neighbors_four_hop_postgres(&mut group, n, &dataset);
        run_neighbors_four_hop_duckdb(&mut group, n, &dataset);
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_get,
    bench_scan_ages,
    bench_neighbors_one_hop,
    bench_neighbors_two_hop,
    bench_neighbors_three_hop,
    bench_neighbors_four_hop
);
criterion_main!(benches);
