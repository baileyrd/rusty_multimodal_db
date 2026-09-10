# rusty_multimodal_db

> **This repo's development has moved.** `rusty_multimodal_db` now lives
> at [`crates/rusty_multimodal_db`](https://github.com/Rusty-Mill/rusty_mill/tree/main/crates/rusty_multimodal_db)
> in the [`Rusty-Mill/rusty_mill`](https://github.com/Rusty-Mill/rusty_mill)
> monorepo, merged in via `git subtree` with full history preserved (see
> that repo's `RELEASE_NOTES.md`/`CHANGELOG.md` for the merge, and its
> `docs/adr/0001-consolidate-crates-into-workspace.md` for why). This
> repo's own `AGENTS.md`/`WORKFLOW.md`/`docs/` stay the crate's
> governance of record per that ADR's remit — only where the code lives
> and how it's built/tested changed. Open new work against the monorepo
> copy; this repo is not actively developed going forward.

A durable, concurrency-safe key-value record store for Rust: mmap-backed
persistence and a `RwLock` for safe multi-threaded access, either as a
fixed `Dog`-shaped store (`ProductionStore`) or generically, for your own
record type (`GenericProductionStore`). Internal use only — not published
to crates.io.

## Getting started

This repo isn't on crates.io. As of the monorepo migration above, prefer
depending on the maintained copy — a workspace path dependency if you're
already inside `rusty_mill`, otherwise git pinned to a commit under
`crates/rusty_multimodal_db`:

```toml
[dependencies]
rusty_multimodal_db = { path = "../rusty_multimodal_db" } # from inside crates/ in rusty_mill
# or, from outside that workspace:
# rusty_multimodal_db = { git = "https://github.com/Rusty-Mill/rusty_mill", rev = "<commit>" }
```

This repo's own history (frozen at the point of migration) still resolves
by git if you need it:

```toml
[dependencies]
rusty_multimodal_db = { git = "https://github.com/baileyrd/rusty_multimodal_db" }
```

A complete, minimal example — create a store, read a record, update it:

```rust
use rusty_multimodal_db::{DogRecord, DogStore, ProductionStore};
use uuid::Uuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new("/tmp/dogs.mmap");
    let rex = Uuid::from_u128(1);
    let records = vec![DogRecord::new(rex, "Corgi", 3)];

    let mut store = ProductionStore::create(records, Vec::new(), path)?;
    assert_eq!(store.get(rex).unwrap().age, 3);

    store.update_age(rex, 4)?;
    assert_eq!(store.get(rex).unwrap().age, 4);

    Ok(())
}
```

`ProductionStore::open` reopens an existing file the same way; both
implement `DogStore` (single-owner, `&mut self`) and `ConcurrentStore`
(`&self`, share across threads via `Arc`). See
[`ProductionStore`'s own rustdoc](#rustdoc) for the runnable version of
this example and every method's contract.

### Your own record type: `GenericProductionStore`

`ProductionStore` is fixed to one record shape (`DogRecord`).
`GenericProductionStore<S>` is the same recipe — mmap durability, `RwLock`
concurrency — generalized to any type implementing this crate's `Record`
trait (plus `IndexedField`/`ScannableField` for whichever fields need
equality lookup or scan/update access). See
[`GenericProductionStore`'s own rustdoc](#rustdoc) for a complete, minimal
worked example implementing a custom domain from scratch, and
`src/generic/order_customer.rs` (behind the `research` feature, see below)
for a larger real reference domain with a directed relation
(`Order belongs_to Customer`).

### The `research` feature: seeing the alternatives this recommendation is built on

`ProductionStore`/`GenericProductionStore` are the *recommended* defaults,
not the only backends in this repo — they're the winners of a long
empirical comparison against other storage layouts, durability
strategies, and concurrency strategies. That comparison code is real and
kept, but off by default so a normal build doesn't compile it in. Enable
it with the `research` Cargo feature:

```sh
cargo build --features research
cargo test --all-features
cargo bench --features research          # or just `cargo bench` — see below
```

This unlocks: the other three `Dog` storage backends (row-oriented,
column-oriented, and the plain non-durable canonical store), the other
seven durability variants (WAL/snapshot combinations, an embedded
transactional store, `redb`), the other three concurrency strategies
(sharded, `dashmap`, an actor-style channel), the `Order`/`Customer`
reference domain (`generic::order_customer`), and every historical
spike/comparison module. See `src/lib.rs`'s own top-level doc comment for
the full front-door/research split, and `RESULTS.md` for the numbers that
justified each pick.

### The `server` feature: a network server/query layer

`server::serve` puts a thin, real TCP listener in front of
`ProductionStore`/`GenericProductionStore` — a versioned
`Request`/`Response` wire protocol (currently version 18) over
length-prefixed `bincode` framing, thread-per-connection, reusing whichever
`RwLock` the wrapped store already manages (no new lock at this layer).
Off by default, distinct from `research` (this is new, additive
capability, not a benchmarked alternative):

```sh
cargo build --features server
cargo test --features server                # Dog, Reminder, Entity, Memory — the front-door domains
cargo test --features server,research       # + Order/Customer and Employee, the reference domains
cargo run --features server --bin dog_server      # a minimal local server, Dog domain
cargo run --features server --bin entity_server   # Entity: a labeled graph with open relation labels
cargo run --features server --bin memory_server   # Memory and Entity, two tables on one listener
```

The client half stands alone behind the `client` feature (`server`
implies it): `SchemaDrivenClient`, the framing, the protocol types, and
TLS via `rusty_tls`, with no listener, no adapters, and no
`ConnectionStore` compiled in — for a process that only talks to a
server. The wire itself is specified byte-for-byte for non-Rust readers
in `docs/specifications/server/SERVER-002-wire-format.md`, pinned by
`tests/fixtures/wire-vectors.txt` (written by and checked against every
golden-vector test), and implemented from that document by a stdlib-only
Python reference client in `clients/python/` (ADR-0043):

```sh
cargo test --features client                  # the client half alone
python3 -m unittest discover -s clients/python/tests -v   # Python client vs. the wire fixture, offline
cargo test --features server --test server_python_client  # Python client vs. a live Entity server
```

A client that doesn't know a domain at compile time sends
`Request::DescribeSchema` first — the `Response::Schema` it gets back
names every field, its wire type, and which operations it supports (see
`ADR-0011`) — and `DescribeRelations` for what a `JOIN` may name.
`server::client::SchemaDrivenClient` is a real, reusable client built
exactly this way — addresses every field by name, never a domain's own
`FIELD_*` constant, and checks capabilities client-side before sending.
It also parses a bounded, read-only SQL subset (`SELECT … WHERE`,
`GROUP BY` with `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, and `JOIN … ON
<relation>`, within a table or across two) and compiles it to the wire
primitives client-side — the server has no query language of its own.

**What the wire can do today** (`SERVER-001`, every item its own accepted
ADR): read a record, filter on an indexed field, scan and update a
scannable field, walk a directed or symmetric relation one hop;
per-connection transaction sessions with read-your-writes, stage-time
validation, and snapshot isolation; an opt-in crash-atomic redo journal
with group commit; a protocol version negotiated by an optional first
`Hello` frame, append-only with four compatibility rules; and — since the
`rusty_remind_me` line of rounds (ADR-0036 through ADR-0052) — runtime
**insertion**, **linking** under open relation labels, **whole-record
replacement**, **deletion** with cascading edge cleanup, **more than one
table on one connection** (`Use`, `ListTables`, cross-table `JOIN`), and
an operator's **compaction** request. Every runtime write is append-only
to a log beside the file and folded at the next open or compaction.

Six domain adapters validate the protocol. Three are front-door, built as
a real backend for the owner's `rusty_remind_me` memory service:
`Reminder` (a fixed-schema record), `Entity` (a labeled graph with
name lookup, aliases, and relation labels created at runtime), and
`Memory` (the consumer's `memories` table, with a `mentions` relation
whose far end lives in the `entity` table). Three are reference material:
`Dog` (`Neighbors` only), `Order`/`Customer` (`Parent`/`Children` only),
and `Employee` (both relation kinds on one self-referential record).

**Security.** Authentication/authorization (`ADR-0012`), native TLS via
`rusty_tls` (`ADR-0014`), mutual TLS with class-from-certificate, rate
limiting and lockout, and audit and access logs are all implemented and
all opt-in through one `ServeOptions` value (`ADR-0032`);
`ServeOptions::default()` reproduces the original unauthenticated,
plaintext behavior exactly. **Do not expose a server built from this
module beyond a trusted, localhost/development network unless both
authentication and TLS are configured together** — either alone leaves
the other half of the gap open. See `src/server`'s own module docs and
`docs/decisions/ADR-0010-server-query-layer-proposal.md` before using it.

```sh
cargo bench --features server,research --bench server   # real-socket round-trip latency + thread-per-connection throughput sweep
```

## Running the suite

```sh
cargo test --all-features            # unit tests, including ProductionStore's and GenericProductionStore's flagship integration tests
cargo bench                          # wall-clock Criterion suite (cross-platform), ProductionStore listed first in every workload
cargo bench --bench concurrency      # concurrency throughput sweep, ProductionStore included alongside every strategy
cargo bench --bench generic_production   # GenericProductionStore (Order/Customer) get/scan/filter/parent/children sweep
cargo bench --features perf-events --bench cache_events   # cache-miss counts, Linux bare-metal only — see ADR-0002
```

See `AGENTS.md` for the full canonical command set and change rules.

## Where to go deeper

This repo accumulated its design and evidence across many rounds of
empirical work — this README won't re-explain any of it, just point you
at the right file:

- **`docs/charter/CHARTER.md`** — the original hypothesis under test (can
  one canonical, UUID-keyed store serve row/column/graph access as
  derived views?) and the repo's naming history.
- **`docs/architecture/SYSTEM-ARCHITECTURE.md`** — how the pieces fit
  together today, starting from `ProductionStore`.
- **`docs/design/GENERIC-SCHEMA-DESIGN.md`** — the original design
  proposal for the generic record/schema/query library (`crate::generic`),
  now Accepted and implemented.
- **`docs/design/SERVER-QUERY-LAYER-DESIGN.md`** — the design proposal for
  the network server/query layer (`server` feature), now Accepted and
  implemented.
- **`docs/design/SERVER-AUTH-DESIGN.md`** — the design for
  authentication/authorization on the server/query layer, now **Accepted
  and implemented** (`AuthConfig`, `server` feature, `SERVER-001` v0.6.0).
- **`docs/design/SERVER-TRANSACTION-DESIGN.md`** — the design for atomic
  multi-operation transactions on the server/query layer, now **Accepted
  and implemented** (`Request::Transaction`, `server` feature, `SERVER-001`
  v0.7.0).
- **`docs/design/SERVER-TLS-DESIGN.md`** — the design for native
  transport encryption (TLS via `rusty_tls`, this owner's own
  ecosystem-wide `rustls` wrapper) on the server/query layer, now
  **Accepted and implemented** (`TlsConfig`, `server` feature, `SERVER-001`
  v0.9.0).
- **`docs/design/SERVER-*-DESIGN.md`**, the rest — one design per server
  round after that, each Accepted and implemented: protocol versioning,
  sessions, the journal, SQL `SELECT`/`GROUP BY`/`JOIN`, the `Reminder`,
  `Entity`, and `Memory` domains, runtime insertion/linking/replacement/
  deletion, tables on one connection, compaction, and the wire
  specification with its Python client. `docs/specifications/server/
  SERVER-001-query-layer.md` is the one place every round's requirement
  lives, in order.
- **`docs/decisions/`** — one ADR per accepted architectural decision, in
  order:
  - `ADR-0001` — the three-backend (AoS/SoA/canonical) empirical comparison
  - `ADR-0002` — cache-miss instrumentation platform
  - `ADR-0003` — eager write-through cache invalidation
  - `ADR-0004` — one-hop `neighbors` as a trait method
  - `ADR-0005` — WAL/snapshot hybrid durability
  - `ADR-0006` — the Tier 2 durability architectures (mmap, `redb`, etc.)
  - `ADR-0007` — the concurrency strategies compared
  - `ADR-0008` — `ProductionStore` as the production default
  - `ADR-0009` — the generic schema design proposal (now Accepted)
  - `ADR-0010` — the server/query layer proposal (now Accepted)
  - `ADR-0011` — schema discovery for the server/query layer (now Accepted)
  - `ADR-0012` — authentication/authorization for the server/query layer
    (now Accepted and implemented)
  - `ADR-0013` — atomic multi-operation transactions for the
    server/query layer (now Accepted and implemented)
  - `ADR-0014` — native transport encryption (TLS), via `rusty_tls`, for
    the server/query layer (now Accepted and implemented)
  - `ADR-0015` — benchmarking `ProductionStore` against real external
    databases (SQLite, Postgres, DuckDB) on the same three fixed access
    patterns already used in-repo (now Accepted and implemented)
  - `ADR-0016`–`ADR-0035` — the server hardened round by round:
    portable record and edge blobs, schema tags, multi-field mmap
    durability, the pinned `bincode` codec, protocol versioning
    (`Hello`), mutual TLS, transaction sessions, the redo journal and its
    group commit, read-your-writes, class-from-certificate, audit and
    access logs, rate limiting, stage-time validation, snapshot
    isolation, `ServeOptions`, and the SQL `SELECT`/`GROUP BY` subset
  - `ADR-0036`–`ADR-0052` — the `rusty_remind_me` line: the `Reminder`,
    `Entity`, and `Memory` domains; aliases and name lookup; the wire
    specification and Python client (`ADR-0043`); relation `JOIN`
    (`ADR-0044`) and tables on one connection (`ADR-0045`/`ADR-0050`);
    runtime insertion (`ADR-0046`), linking (`ADR-0047`), replacement
    (`ADR-0049`), deletion (`ADR-0051`), and compaction (`ADR-0052`) —
    every one proposed and implemented in one cycle, then accepted
- **`docs/specifications/SPEC-REGISTRY.md`** + **`docs/specifications/storage/`**/**`docs/specifications/server/`**
  — the `STORAGE-0xx`/`SERVER-0xx` requirement/spec tree each round implemented against.
- **`docs/roadmap/ROADMAP.md`** — status vocabulary and what's next.
- **`docs/FUTURE-GROWTH.md`** — what was once unplanned growth (a
  server/query layer, now built — the document records what of it
  happened) and what remains genuinely out of scope (SQLite/DuckDB-style
  parity) and what each would actually require.
- **`docs/traceability/TRACEABILITY.md`** — the requirement → decision →
  implementation → verification mapping tying the above together.
- **`docs/PROJECT-STATUS.md`** — the current checkpoint (last verified
  commit, what's merged).
- **`RESULTS.md`** — the actual benchmark numbers and verdict behind every
  pick above.
- **`AGENTS.md`** / **`WORKFLOW.md`** — contributor/agent conventions:
  canonical commands, branch/PR rules, and what needs an ADR.

## Rustdoc

`cargo doc --all-features --open` builds and opens full API documentation,
including the `research`-gated modules. Start at `ProductionStore` and
`GenericProductionStore` — both have a complete, runnable example in
their own doc comment.

## Repo name

This repo is named `rusty_multimodal_db` on GitHub. `docs/charter/CHARTER.md`
records a naming discrepancy worth knowing about: the task that seeded
this repo suggested `rusty_multimodel_bench` (multi-**model** access —
row/column/graph — not multimodal *data*) as a less ambiguous name once
the benchmark's shape was clear.

## License

MIT — see `LICENSE`.
