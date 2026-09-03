# Server SQL SELECT Subset Design (Proposed)

- Status: **Proposed**
- Date: 2026-09-03
- Related: `ADR-0010`/`docs/design/SERVER-QUERY-LAYER-DESIGN.md` (named "a
  query language" as explicitly out of scope for v1 — fixed, server-
  assigned field tags only; this design is the first round that revisits
  that), `ADR-0011` (schema discovery — the mechanism a client-side
  parser leans on for name→tag/kind resolution), `ADR-0022`/
  `src/server/protocol.rs` (the append-only wire-shape/version rules a
  new `Request`/`Response` variant pair must follow), `ADR-0027`/
  `ADR-0033` (`RYW-FR`/`ISO-FR`'s own precedent that only `GetById` is
  overlaid/read-set-tracked — the reason this design's new read is
  neither), `ADR-0009`/`src/generic/query.rs` (the query-trait pattern —
  `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Neighbors`, each with a
  forwarding impl through `Reversed`/`Symmetric` — the precedent for
  this design's own new trait), `docs/FUTURE-GROWTH.md`'s "Path to a
  server / query layer" (*"the query language itself — real parser and
  language design, not a small extension"* — genuinely new, not
  incremental) and "Path to SQLite/DuckDB parity" (*"SQL. A parser, a
  query planner, a cost-based optimizer, and an execution engine... a
  multi-year effort on its own"* — the owner's "SQL" pick this round
  narrows from).

## Purpose and scope

The owner picked "SQL" from `docs/FUTURE-GROWTH.md` as the direction to
pursue. Read literally, that document names the full thing — parser,
planner, cost-based optimizer, execution engine — as "a multi-year
effort on its own," the same scale of claim `ADR-0033`'s own design
round found for full MVCC and, exactly as that round did, this one
narrows the ask to the bounded slice that is actually buildable now
without rebuilding anything: a **read-only `SELECT` subset**, compiled
entirely to primitives this crate already has or can add without
touching `src/store/**`'s four `research`-gated backends.

**In scope:**

- A minimal `SELECT <columns> FROM <ident> [WHERE <cond> [AND <cond>]*]
  [LIMIT <n>]` grammar — real tokenizing and parsing, not string
  splitting — parsed **client-side**, in `SchemaDrivenClient`, using the
  schema already fetched via `DescribeSchema` for name→tag/kind
  resolution (`ADR-0011`'s own mechanism, reused rather than
  reinvented).
- A new, structured wire request/response pair, `Request::Query`/
  `Response::Rows`, carrying the *parsed* query (field tags, typed
  literals) — the wire never carries raw SQL text. The server has no
  parser at all.
- One new `ConnectionStore` method, `scan_all`, returning every record's
  full field set — the one genuinely new storage-adjacent capability
  this design needs, and the one this document accounts for most
  carefully (see "Considered options").
- Comparators `=`, `!=` (every field kind) and `<`, `<=`, `>`, `>=`
  (`U32`/`I64` only) — no `LIKE`, no `IN`, no `IS NULL`, no `BETWEEN`.
- `AND`-only conjunction of predicates — no `OR`, no parentheses, no
  nested boolean logic.
- `LIMIT <n>` — a trivial, real safety valve on an otherwise-unbounded
  full scan's response size.

**Out of scope (see "Non-goals")**: `ORDER BY`, aggregation (`COUNT`/
`SUM`/`AVG`/`GROUP BY`), joins, subqueries, `INSERT`/`UPDATE`/`DELETE`
via SQL, prepared statements, a query planner or cost-based optimizer
(every `Query` is unconditionally a full scan — no index is ever
consulted, even when one exists for a predicate's field), and a
server-side SQL parser.

## Non-goals

- **A query planner or cost-based optimizer.** `docs/FUTURE-GROWTH.md`
  names this as its own separate, real requirement for DuckDB-style
  parity, distinct from the parser itself. Every `Request::Query` this
  design answers is unconditionally a full scan via the new `scan_all`
  — even a `WHERE id = ...` predicate that `Request::GetById` could
  answer in O(1), or a `WHERE <indexed-field> = ...` predicate
  `Request::FilterEq` could answer via its existing index, pays the
  same O(n) cost as every other `Query`. Choosing a cheaper path when
  one exists is real, valuable future work, not attempted here.
- **Joins.** Every server instance serves exactly one domain
  (`ADR-0010`'s own architecture — one `ConnectionStore` per `serve`
  call); there is no second table on the same connection to join
  against. Cross-connection federation is a different, larger,
  unaddressed problem.
- **Aggregation and `GROUP BY`.** `docs/FUTURE-GROWTH.md` names this
  explicitly as DuckDB's own "core identity," not a small extension.
- **`ORDER BY`.** `Response::Rows` carries rows in whatever unspecified
  order `scan_all` enumerates them in — the same "unspecified order"
  convention `ScanField`'s own doc comment already establishes for a
  full-column read. `LIMIT` truncates that same unspecified order, not
  a meaningful top-N.
- **Writes via SQL.** `INSERT`/`UPDATE`/`DELETE` are not parsed; writes
  stay exactly where they are — `Request::UpdateField`/`Transaction`/
  the session API, addressed by field tag as always. `Request::Query`
  is read-only end to end.
- **A server-side SQL parser.** The server never sees SQL text, only
  the already-parsed, already-typed `Request::Query`. A future round
  could add a server-side entry point (a non-Rust client would need
  one), but nothing about this design blocks that — it would be a new,
  additive `Request::Sql { text: String }` translating to the same
  `Request::Query` shape server-side, not a redesign.
- **Prepared statements / parameterized queries.** Each call to the new
  client method parses its SQL string fresh; no plan or parse tree is
  cached across calls.
- **Respecting the old per-field capability flags
  (`filter_eq`/`scan`/`update`).** Considered and rejected — see
  "Considered options."

## Context and terminology

**What the wire protocol can already do**, read from
`src/server/protocol.rs`: `Request::FilterEq { field, value }` is a
single-field equality lookup, `Unsupported` unless that field carries
an in-process index; `Request::ScanField { field }` returns every
record's value for one field, **with no id attached** — the values
come back correlated to nothing, useful only for a column-level
aggregate, not a row-shaped read; `Request::GetById { id }` is the only
request that returns a full row, and only for one, already-known id.
There is no way today to ask "every row where X," project a subset of
fields, or read more than one field per record across more than one
record in a single request.

**What every server-facing store already holds**, verified directly
rather than assumed: `MmapAgeStore` (`src/durability/mmap_store.rs`,
the type `ProductionStore` wraps) holds `records: HashMap<Uuid,
DogRecord>` — every record, in memory, keyed by id. `GenericMmapStore`
(`src/generic/mmap_store.rs`, the base layer under `OrderProductionStack`/
`EmployeeProductionStack` through `Reversed`/`Symmetric`) holds the
identical shape, `records: HashMap<R::Id, R>`. Enumerating every id (or
every full record) is not a new storage capability — the data is
already resident — but no trait exposes it: `DogStore` has `get`/
`scan_ages`/`same_breed`/`neighbors`, none of which return a full,
id-correlated record set; `src/generic/query.rs`'s `GetById`/
`FilterEq`/`ScanField`/`UpdateField`/`Neighbors` have the identical gap.

**What `crate::generic`'s query-trait pattern already establishes**,
read from `src/generic/query.rs` and `src/generic/store.rs`: each
capability is its own small trait (`GetById<R>`, `FilterEq<R, Marker>`,
...), implemented on the base layer (`BaseStore`/`Indexed`/`Scanned`
for in-memory, `GenericMmapStore` for durable) and re-exposed through
each composition layer (`Reversed<S, P, C, Marker>`, `Symmetric<S, R,
Marker>`) by its own small forwarding `impl` block — `store.rs`'s own
comments label these "Forwarding impl" and there is one per trait per
layer already. Adding a new trait in this shape is mechanical, not
speculative — the pattern exists and is exercised by five capabilities
already.

## Requirements

- `SQL-FR-001` — **A minimal `SELECT` grammar, client-side.**
  `SELECT (* | ident (, ident)*) FROM ident [WHERE cond (AND cond)*]
  [LIMIT number]`, where `cond` is `ident op literal` and `op` is one of
  `= != < <= > >=`. Real tokenizing (identifiers, string/number/bool
  literals, operators, keywords case-insensitive) and recursive-descent
  parsing in a new module, not string splitting or a regex. The `FROM`
  identifier is required by the grammar but not semantically checked
  against anything — a connection serves exactly one domain, so there
  is nothing to look it up against; a deliberate simplification, named
  here rather than silently assumed.
- `SQL-FR-002` — **Name/kind resolution against the fetched schema.**
  Every column name in `SELECT`/`WHERE` is resolved to a `FieldRef` via
  the `DomainSchema` `SchemaDrivenClient` already fetched at connect
  time (`ADR-0011`) — an unknown name is `ClientError::UnknownField`,
  client-side, no round trip, the same posture every other
  name-addressed client method already takes. Each `WHERE` literal's
  `ScanValue` variant is checked against the resolved field's
  `ValueKind` client-side too.
- `SQL-FR-003` — **A structured wire request, not SQL text.**
  `Request::Query { select: Selection, filter: Vec<Predicate>, limit:
  Option<usize> }`, `Selection::{All, Fields(Vec<FieldRef>)}`,
  `Predicate { field: FieldRef, op: CompareOp, value: ScanValue }`,
  `CompareOp::{Eq, Ne, Lt, Le, Gt, Ge}`. Introduced at protocol version
  8 (`PROTOCOL_VERSION` 7 → 8, the table gains row 8) — a new variant,
  gated exactly as every prior appended variant has been (`Malformed`
  is not applicable here since an old server cannot decode an unknown
  variant index at all; the client checks `negotiated >= 8` before ever
  sending one, compatibility rule 4).
- `SQL-FR-004` — **One new `ConnectionStore` method.**
  `fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>` —
  every record, full field set, unspecified order (the `ScanField`
  precedent). Implemented by all three domain adapters plus this
  crate's own `FixtureStore` test double, matching `apply_transaction`'s
  own "every implementor updated, no external implementor exists"
  precedent (`STV-FR`, `ISO-FR-006`).
- `SQL-FR-005` — **A new "list every id" primitive underneath
  `scan_all`**, since neither `DogStore` nor `src/generic/query.rs`
  exposes one today (see "Considered options" for why this is a new,
  small, server-facing-only trait rather than a change to `DogStore`
  or the four `research`-gated backends). `ProductionStore` gains a
  `pub(crate)` or inherent accessor; `crate::generic` gains a new query
  trait (working name `AllIds<R>`) implemented on `GenericMmapStore`
  and forwarded through `Reversed`/`Symmetric` exactly as `GetById`/
  `FilterEq`/etc. already are.
- `SQL-FR-006` — **Filtering and projection are centralized, not
  per-adapter.** `scan_all`'s result is unfiltered and unprojected; one
  new pure function (working name `evaluate_query`, alongside
  `overlay_staged`/`record_read_set` in `src/server/mod.rs`) applies
  `filter`, then `select`, then `limit`, identically for every domain —
  the filter/project/limit logic is written once, not duplicated three
  times.
- `SQL-FR-007` — **Validation before any scan.** An unknown field tag in
  `select` or `filter` is `ErrorCode::UnknownField`; an ordering
  comparator (`<`/`<=`/`>`/`>=`) against a `Str`/`Bool` field, or a
  predicate's value kind not matching its field's real type, is
  `ErrorCode::Malformed` — both existing codes, reused, not a new wire
  addition. Validated against the schema alone (`describe()`), before
  `scan_all` ever runs, so a malformed query costs nothing beyond one
  `describe()`-shaped check.
- `SQL-FR-008` — **Every described field is queryable, independent of
  its `filter_eq`/`scan`/`update` capability flags.** A field with
  every capability flag `false` today (e.g. `Dog::breed`) is fully
  selectable and filterable via `Query`, because `scan_all` needs no
  index and returns every field regardless — see "Considered options"
  for why restricting `Query` to the old flags was rejected.
- `SQL-FR-009` — **Not overlaid, not read-set-tracked.** `Request::Query`
  is a set-shaped read, the same category `RYW-FR`/`ISO-FR-002`
  already draw the line at: only `GetById` is read-your-writes-overlaid
  or snapshot-isolation-tracked, for the identical reason both designs
  already gave (no fixed identity to re-check/overlay cheaply — see
  each design's own "Non-goals"). `Query` inside a session — of either
  kind, or plain — always reads committed state, unconditionally.
- `SQL-FR-010` — **Backward and cost compatible.** A pre-8 connection
  cannot construct or send `Request::Query` (the client library gates
  it, `ClientError::Unsupported("sql query")`, no frame sent); a server
  that never receives one runs no new code path beyond the version
  table and one new match arm; no `Cargo.toml`, storage-format, or
  `serve`-signature change.

## Considered options

**Where the query text is parsed.**

1. **Client-side, structured wire — proposed.** Keeps the server dumb
   (no parser, no new server-side dependency), matches this crate's
   whole "typed wire, schema-driven client does the smart part"
   architecture already established by `Session::update(id,
   field_name, value)`'s own name→tag resolution. A non-Rust client
   loses SQL syntax until a server-side entry point is added — named,
   not solved, and explicitly not blocked by this choice (see
   "Non-goals").
2. **Server-side text parser (`Request::Sql { text: String }`).**
   Every client, including a hand-rolled one, gets SQL syntax for
   free; costs a real parser dependency or hand-written one living in
   the server binary's trusted surface, and a wire request carrying an
   untrusted string the server must parse defensively. Rejected this
   round on the same "keep the server dumb" grounds `ADR-0010` set for
   the whole project — not ruled out permanently, named as a real,
   additive future increment.

**Where "list every id"/"every full record" is exposed.**

1. **A new, small, server-facing-only trait — proposed.** Mirrors
   `TransactionalStore`'s own precedent (`ADR-0013`): a capability only
   the server layer needs, added directly to the two production-facing
   types (`ProductionStore`, and a new `crate::generic::query` trait
   forwarded through `Reversed`/`Symmetric`) without touching the four
   `research`-gated `DogStore` backends (`AosStore`/`SoaStore`/
   `CanonicalStore`/`CanonicalCachedStore`) at all — none of them is
   ever wrapped by a `ConnectionStore` adapter, so none needs this.
2. **Grow `DogStore`/`src/generic/query.rs`'s existing traits
   directly.** Would force all four benchmarked backends to implement
   "list every id" too, for a capability only the server-facing types
   will ever use — a real, avoidable blast-radius increase over
   option 1, rejected on the same "no rework of the storage layer"
   grounds `docs/FUTURE-GROWTH.md` itself sets as the bar for
   "genuinely additive."

**Whether `Query` respects the existing per-field capability flags.**

1. **No — every described field is queryable — proposed.** The
   `filter_eq`/`scan`/`update` flags describe what the *old*,
   index-shaped requests can do for a field, not an access-control
   decision; `scan_all` needs no index for any field, so artificially
   refusing to filter or select a field with every flag `false` would
   be a restriction invented for this design alone, with no
   correctness or cost justification once the primitive exists.
2. **Yes — `Query` is refused for a field whose relevant flag is
   `false`.** Keeps a field's exposure surface identical across every
   request kind, a real, defensible consistency argument — the
   trade-off this option buys is a `Query` that can do meaningfully
   less than the schema's own fields would otherwise allow, for no
   engine-side reason. The owner may prefer this if field-level
   exposure symmetry matters more than the extra reach.

## Proposed shape

```rust
// src/server/protocol.rs
pub const PROTOCOL_VERSION: u32 = 8;   // SQL-FR-003

pub enum Selection {
    All,
    Fields(Vec<FieldRef>),
}

pub enum CompareOp { Eq, Ne, Lt, Le, Gt, Ge }

pub struct Predicate {
    pub field: FieldRef,
    pub op: CompareOp,
    pub value: ScanValue,
}

pub enum Request {
    // ...unchanged variants...
    Query {
        select: Selection,
        filter: Vec<Predicate>,
        limit: Option<usize>,
    },
}

pub enum Response {
    // ...unchanged variants...
    Rows {
        rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>,
    },
}

// src/server/mod.rs
pub trait ConnectionStore: Send + Sync {
    // ...unchanged methods...
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>;   // SQL-FR-004
}

// dispatch's new arm (SQL-FR-006/007), validated against `describe()`
// before `scan_all` runs:
// Request::Query { select, filter, limit } => {
//     if let Err(code) = validate_query(&store.describe(), &select, &filter) {
//         return err_response(code);
//     }
//     let rows = evaluate_query(store.scan_all(), &select, &filter, limit);
//     Response::Rows { rows }
// }

// src/generic/query.rs — new trait, forwarded through Reversed/Symmetric
// exactly as GetById/FilterEq/ScanField/UpdateField/Neighbors already are
pub trait AllIds<R: Record> {
    fn all_ids(&self) -> Vec<R::Id>;
}
```

```rust
// src/server/sql.rs (new) — client-side only
pub(crate) struct ParsedQuery {
    pub table: String,             // named, not looked up against anything
    pub columns: Vec<String>,      // empty means `*`
    pub conditions: Vec<(String, CompareOp, ScanValue)>,
    pub limit: Option<usize>,
}

pub(crate) fn parse(sql: &str) -> Result<ParsedQuery, SqlParseError>;

// src/server/client.rs
impl SchemaDrivenClient {
    pub fn query(&mut self, sql: &str)
        -> Result<Vec<(RecordId, Vec<(String, ScanValue)>)>, ClientError> {
        // 1. parse(sql) — SQL-FR-001
        // 2. resolve every column/condition name to (FieldRef, ValueKind)
        //    via self.schema() — SQL-FR-002, ClientError::UnknownField
        //    locally, no round trip, on an unknown name
        // 3. check server_protocol_version() >= 8 — SQL-FR-010,
        //    ClientError::Unsupported("sql query"), no frame sent
        // 4. build Request::Query, round-trip
        // 5. translate Response::Rows's tags back to names — the same
        //    "answer named, not tagged" convention `get`/`Session::get`
        //    already follow
    }
}
```

## Data/state and invariants

- `Request::Query` is stateless and read-only — no session, no lock
  held across anything but the single `scan_all` call, exactly the
  same shape `Request::ScanField`/`FilterEq` already have.
- `scan_all`'s result and `evaluate_query`'s filtering are both pure —
  no adapter-specific logic beyond `scan_all` itself, so the filter/
  project/limit behavior is provably identical across all three
  domains by construction, not by three independently-written copies
  agreeing.
- `AllIds<R>`'s forwarding through `Reversed`/`Symmetric` follows the
  same trait-per-layer shape every existing query trait already uses —
  no new state, no new index, purely a read over `records: HashMap<R::Id,
  R>`, which both `GenericMmapStore` and `MmapAgeStore` already
  maintain for every other reason.
- `limit` bounds the *response*, not the *work*: `scan_all` still
  builds every row before `evaluate_query` filters and truncates —
  named plainly as the real cost, not hidden. A future round could push
  `limit`/`filter` down into `scan_all` itself to short-circuit early;
  not attempted here, since it would blur the "filtering is
  centralized, not per-adapter" property `SQL-FR-006` is built on.

## Errors, failure, recovery, and observability

- No new `ErrorCode` — `UnknownField`/`Malformed` cover every rejection
  shape (`SQL-FR-007`).
- A parse error (bad SQL syntax) never reaches the wire at all — it is
  `ClientError`, client-side, the same posture `SchemaDrivenClient`
  already takes for a bad field name.
- `Request::Query`'s outcome flows through the existing access-log
  machinery unmodified — `outcome_of` already maps every non-`Err`/
  `TransactionFailed` response, `Response::Rows` included, to
  `Outcome::Ok`; no new sink, no new gate.

## Security, privacy, and compatibility

- No new secret crosses the wire; `Response::Rows` carries exactly the
  fields a plain `GetById`/`FilterEq`/`ScanField` on the same
  connection could already read, just more of them per request.
- The one real widening (`SQL-FR-008`): a field with `filter_eq`/
  `scan`/`update` all `false` today becomes readable via `Query` where
  it wasn't via `FilterEq`/`ScanField` before. Every such field was
  already readable via plain `GetById` (a full-record read has never
  been capability-gated per field), so this closes an inconsistency
  rather than opening a new one — named explicitly, not glossed over.
- Backward compatible by construction: `PROTOCOL_VERSION` 7 → 8 is an
  appended variant, unreachable below 8; every existing golden vector,
  request, and response is untouched.

## Acceptance criteria

1. `Request::Query`/`Response::Rows`/`Selection`/`CompareOp`/
   `Predicate` exist exactly as specified; `PROTOCOL_VERSION = 8`; the
   version table and golden vectors updated; a pre-8 client gets
   `ClientError::Unsupported("sql query")` with no frame sent.
2. `ConnectionStore::scan_all` is implemented identically in shape by
   all three domain adapters and `FixtureStore`; `AllIds<R>` is
   implemented on `GenericMmapStore` and forwarded through `Reversed`/
   `Symmetric`; `src/store/**`'s four `research`-gated backends are
   untouched.
3. `SchemaDrivenClient::query("SELECT age FROM dog WHERE age > 3")`
   (and the equivalent for `*`, multiple columns, `!=`/`<=`/`>=`/`<`,
   two `AND`-ed conditions, and `LIMIT`) returns exactly the rows a
   hand-written scan-and-filter over `GetById` on every id would.
4. An unknown column name in `SELECT` or `WHERE` is a client-side
   `ClientError::UnknownField`, no round trip; an ordering comparator
   against a `Str`/`Bool` field, and a literal whose kind doesn't match
   its field, are each a clean parse-or-resolve-time client error, not
   a server round trip either.
5. A field with every capability flag `false` (`Dog::breed`) is both
   selectable and filterable via `Query`.
6. `Query` inside a read-your-writes and/or snapshot-isolation session
   sees committed state only — a staged write to a field `Query` reads
   is never reflected, and a `Query`'s own reads are never added to a
   snapshot-isolated session's read set.
7. `LIMIT` truncates the row count and nothing else; omitting it
   returns every matching row.
8. With `Request::Query` never sent, every existing test in
   `tests/server_*.rs` is unchanged — the new method's cost is one
   `Option`/match branch per dispatched request, matching every prior
   opt-in addition's own "no branch, no cost" bar for anyone not using
   it.

## Verification plan

- `src/server/sql.rs` unit tests: the parser's own grammar — `*`, named
  columns, each comparator, `AND` of two-plus conditions, `LIMIT`,
  every documented syntax error (unknown keyword, missing `FROM`, bad
  literal, trailing garbage).
- `src/server/mod.rs` unit tests: `evaluate_query`'s filter/project/
  limit behavior directly, independent of any adapter — an empty
  filter, a filter matching nothing, `Selection::All` vs. a field
  subset, `limit` shorter than and longer than the match count.
- Each adapter (`dog.rs` at minimum): a unit test driving `scan_all`
  directly, confirming every record and field comes back.
- `tests/server_sql_integration.rs` (new): a real client round trip on
  each of the three domains — `*`/named columns, each comparator kind,
  two `AND`-ed conditions, `LIMIT`, an unknown column, a kind-mismatched
  literal, a field with no other capability flag set, `Query` inside a
  read-your-writes and a snapshot-isolation session each reading
  committed state.
- `tests/server_protocol_version.rs`: the version pin (8), the
  unsupported-below-8 client gate, a golden vector for `Request::Query`/
  `Response::Rows`.

## Traceability

- → `SERVER-001` next minor / FR (`SQL-FR-001`–`010`), a new ADR;
  narrows `docs/FUTURE-GROWTH.md`'s "SQL" item (the owner's "SQL" pick)
  and its "the query language itself" item from "Path to a server /
  query layer" to the bounded read-only slice this document scopes —
  a parser, a planner, a cost-based optimizer, joins, and aggregation
  all remain open, named, not solved.
- Roadmap: `SERVER-SQL-SELECT-DESIGN` (this document), then
  `SERVER-SQL-SELECT` as the implementation unit if accepted.

## Open questions

- Whether a server-side `Request::Sql { text }` entry point (for a
  non-Rust client) is ever wanted — named, not solved; this design's
  own client-side choice does not block it.
- Whether pushing `filter`/`limit` down into `scan_all` (an early-exit
  scan instead of build-then-filter) is worth the adapter-side
  duplication it would cost against `SQL-FR-006`'s "centralized, not
  per-adapter" property — an open efficiency question, not a
  correctness one.
- Whether `Query` should ever consult an existing index (`FilterEq`'s
  own index, or `GetById` for an `id =` predicate) as a cheap-path
  optimization — the first real step toward the query planner
  `docs/FUTURE-GROWTH.md` itself names as a separate, later
  requirement; not decided here.
- Whether `OR`/parenthesized boolean logic is ever wanted enough to
  justify a real expression tree instead of a flat `AND`-list — open,
  matching the scale of every other non-goal above.

## Change history

- 2026-09-03: Initial proposal, in response to the owner's "SQL" pick
  from `docs/FUTURE-GROWTH.md`, scoped down from that document's own
  "parser, planner, cost-based optimizer, execution engine" framing to
  a bounded, read-only `SELECT` subset compiled to a new full-scan
  primitive — no query planner, no joins, no aggregation, no writes.
