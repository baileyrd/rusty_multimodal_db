# rusty_multimodal_db

A durable, concurrency-safe key-value record store for Rust: mmap-backed
persistence and a `RwLock` for safe multi-threaded access, either as a
fixed `Dog`-shaped store (`ProductionStore`) or generically, for your own
record type (`GenericProductionStore`). Internal use only — not published
to crates.io.

## Getting started

This repo isn't on crates.io, so depend on it by git (or by local path if
you already have it checked out):

```toml
[dependencies]
rusty_multimodal_db = { git = "https://github.com/baileyrd/rusty_multimodal_db" }
# or, from a local checkout:
# rusty_multimodal_db = { path = "../rusty_multimodal_db" }
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
`ProductionStore`/`GenericProductionStore` — a `Request`/`Response` wire
protocol over length-prefixed `bincode` framing, thread-per-connection,
reusing whichever `RwLock` the wrapped store already manages (no new lock
at this layer). Off by default, distinct from `research` (this is new,
additive capability, not a benchmarked alternative):

```sh
cargo build --features server
cargo test --features server               # Dog domain only
cargo test --features server,research       # + Order/Customer and Employee, the second and third validation domains
cargo run --features server --bin dog_server   # a minimal local server, Dog domain
```

A client that doesn't know a domain at compile time can send
`Request::DescribeSchema` first — the `Response::Schema` it gets back
names every field, its wire type, and which operations it supports (see
`ADR-0011`), so it can drive `GetById`/`FilterEq`/`ScanField`/
`UpdateField`/`Parent`/`Children`/`Neighbors` from discovered field tags
instead of hardcoded ones. `server::client::SchemaDrivenClient` is a
real, reusable client built exactly this way — addresses every field by
name, never a domain's own `FIELD_*` constant, and checks capabilities
client-side before sending.

Three domain adapters validate the protocol: `Dog` (`Neighbors` only),
`Order`/`Customer` (`Parent`/`Children` only), and `Employee` — the third,
purpose-built to combine both relation kinds on one self-referential
record type (`reports_to`/`ChildOf`, `collaborates_with`/
`SymmetricRelation`), the first domain where every relation-kind request
is a real operation, none `Unsupported`.

**No transport encryption, no transaction semantics, no query language
beyond fixed field-tag addressing** — see `src/server`'s own module docs
and `docs/decisions/ADR-0010-server-query-layer-proposal.md` (Accepted)
before using it. Do not expose a server built from this module beyond a
trusted, localhost/development network unless paired with an external
TLS-terminating proxy/tunnel. **Authentication/authorization is now
implemented** — `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012, Accepted —
`server::serve` takes an `AuthConfig` naming which token(s), if any, a
server instance accepts and the `ReadOnly`/`ReadWrite` class each grants;
`AuthConfig::default()` (no tokens configured) reproduces today's
unauthenticated behavior exactly, so this is purely opt-in. It closes the
"anyone who can open a TCP connection can do anything" gap, not the
transport-encryption one — tokens and every record value are still
plaintext on the wire. **A design for atomic multi-operation
transactions is proposed, not yet accepted or implemented** —
`docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013, Proposed — see that
document for what it would and wouldn't deliver (atomicity/isolation
with respect to concurrent access, explicitly not crash-atomicity or a
multi-round-trip interactive session) before assuming any of it exists.

```sh
cargo bench --features server,research --bench server   # real-socket round-trip latency + thread-per-connection throughput sweep, all three domains
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
  multi-operation transactions on the server/query layer, **Proposed**,
  not yet reviewed or accepted — no implementation exists.
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
    server/query layer (**Proposed** — awaiting owner review, no
    implementation yet)
- **`docs/specifications/SPEC-REGISTRY.md`** + **`docs/specifications/storage/`**/**`docs/specifications/server/`**
  — the `STORAGE-0xx`/`SERVER-0xx` requirement/spec tree each round implemented against.
- **`docs/roadmap/ROADMAP.md`** — status vocabulary and what's next.
- **`docs/FUTURE-GROWTH.md`** — unplanned, unscheduled directions this
  project could grow in (a server/query layer, SQLite/DuckDB-style
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
