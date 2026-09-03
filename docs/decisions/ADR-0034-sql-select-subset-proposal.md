# ADR-0034: A read-only `SELECT` subset — client-side SQL over a new full-scan primitive

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): a real client-side SQL
  `SELECT` subset over a new `scan_all` full-scan primitive, protocol
  version 8; (b) skipping the parser for a structured compound filter
  and (c) closing as not warranted declined; no changes requested).
  Acceptance authorizes the design; implementation follows as its own
  unit — see "Acceptance and implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-SQL-SELECT-DESIGN.md` (the full design this
  ADR summarizes), `ADR-0010` (named "a query language" out of scope
  for v1 — this is the first round to revisit that), `ADR-0011` (schema
  discovery — the name→tag/kind resolution mechanism this design
  reuses), `ADR-0022`/`src/server/protocol.rs` (the wire-shape/version
  rules a new `Request`/`Response` pair must follow), `ADR-0027`/
  `ADR-0033` (the "only `GetById` is overlaid/read-set-tracked"
  precedent this design's new read follows), `ADR-0009`/
  `src/generic/query.rs` (the query-trait-per-capability pattern, each
  forwarded through `Reversed`/`Symmetric` — the precedent for this
  design's own new trait), `docs/FUTURE-GROWTH.md`.
- Supersedes/Superseded by: none. Adds one `ConnectionStore` method
  (every implementor updated) and one new `crate::generic::query` trait
  (forwarded through the existing composition layers); changes no
  existing store, no existing wire shape beyond one appended `Request`/
  `Response` variant pair.

## Context

The owner picked "SQL" from `docs/FUTURE-GROWTH.md` as the direction to
pursue. Read literally, that document names the real thing — a parser,
a query planner, a cost-based optimizer, and an execution engine — as
"roughly a multi-year effort on its own," the same scale of claim
`ADR-0033`'s own round found for full MVCC. This ADR narrows the ask
the identical way that one did: a bounded, read-only `SELECT` subset,
parsed client-side and compiled to a new full-table-scan primitive that
needs no rework of `src/store/**`'s four `research`-gated backends —
not the planner, the joins, or the aggregation engine the full ask
would need.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0033` took.

## Decision drivers

- Give "SQL" real, literal meaning — an actual tokenizer/parser and a
  `SELECT ... FROM ... WHERE ...` grammar — rather than quietly
  substitute a structured-request extension and call it done.
- Keep the server itself dumb: no parser, no new server-side
  dependency, no untrusted string the server must defend against — the
  same "typed wire, smart client" architecture `Session::update`'s own
  name→tag resolution already established.
- Reuse `ADR-0011`'s schema-discovery mechanism for name resolution
  rather than invent a second one.
- Minimize blast radius on `src/store/**`: the four `research`-gated
  `DogStore` backends never need to know this capability exists, since
  none of them is ever wrapped by a `ConnectionStore` adapter.
- Name the real cost plainly: every query is an unindexed full scan.
  No planner, no index consultation, no cost model — proportionate
  scope over an impressive one, the same discipline `ADR-0033` applied
  to MVCC.

## Considered options

1. **Client-side parser, structured wire (`Request::Query`/
   `Response::Rows`) — proposed.** No server-side parser or dependency;
   the wire carries typed, already-resolved data, the same posture
   every other request already takes. A non-Rust client has no SQL
   syntax until a server-side entry point is added later — named, not
   solved, and not blocked by this choice.
2. **Server-side text parser (`Request::Sql { text: String }`).** Every
   client gets SQL for free, at the cost of a real parser (or
   dependency) living in the server's trusted surface and an untrusted
   string it must defend against. Rejected this round on the "keep the
   server dumb" grounds `ADR-0010` already set; a real, additive future
   increment, not ruled out permanently.
3. **No parser at all — extend the wire with a structured compound
   filter (`AND` of `FilterEq`-shaped predicates) and call that "the
   query slice."** Smaller, but not actually SQL by any reasonable
   reading — considered as the fallback if option 1 turned out too
   large for one round; option 1 is not, so this is not proposed.
4. **Close as not warranted.** A legitimate, smaller choice, left to
   the owner.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/protocol.rs`: `Request::Query { select: Selection,
  filter: Vec<Predicate>, limit: Option<usize> }`, `Response::Rows {
  rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> }`, `Selection::
  {All, Fields(Vec<FieldRef>)}`, `Predicate { field, op: CompareOp,
  value }`, `CompareOp::{Eq, Ne, Lt, Le, Gt, Ge}`. `PROTOCOL_VERSION`
  moves to 8 (an appended variant, `ADR-0022`'s rule 2); no new
  `ErrorCode` — `UnknownField`/`Malformed` cover every rejection shape.
- `ConnectionStore` gains `scan_all(&self) -> Vec<(RecordId,
  Vec<(FieldRef, ScanValue)>)>` — every record, full field set,
  unspecified order, implemented identically by all three domain
  adapters and this crate's own `FixtureStore`.
- `crate::generic::query` gains a new trait (working name `AllIds<R>`)
  implemented on `GenericMmapStore` and forwarded through `Reversed`/
  `Symmetric` exactly as `GetById`/`FilterEq`/`ScanField`/`UpdateField`/
  `Neighbors` already are; `ProductionStore` gains the equivalent
  accessor directly. `src/store/**`'s four `research`-gated backends
  are untouched — none is ever wrapped by a `ConnectionStore` adapter.
- Filtering/projection/limiting is one new, centralized pure function
  in `src/server/mod.rs` (alongside `overlay_staged`/`record_read_set`)
  applied identically to every domain's `scan_all` result — not
  duplicated per adapter.
- `src/server/sql.rs` (new): a real tokenizer and recursive-descent
  parser for `SELECT (* | cols) FROM ident [WHERE cond (AND cond)*]
  [LIMIT n]`; used only by `SchemaDrivenClient::query(sql)`, which
  resolves every name against the already-fetched `DomainSchema`
  (`ADR-0011`), gates on `server_protocol_version() >= 8`
  (`ClientError::Unsupported("sql query")`, no frame sent below it),
  and translates `Response::Rows`'s tags back to names.
- `Request::Query` is read-only, unauthenticated-gated like any other
  read, never overlaid by read-your-writes and never tracked into a
  snapshot-isolated session's read set — the same "only `GetById`" line
  `RYW-FR`/`ISO-FR-002` already draw, for the identical reason (a
  set-shaped read has no fixed identity to re-check/overlay cheaply).
- Every described field is queryable regardless of its `filter_eq`/
  `scan`/`update` capability flags — `scan_all` needs no index for any
  field, so there is no cost-based reason to refuse one.
- `SERVER-001`'s next minor / FR (`SQL-FR-001`–`010`); `SPEC-REGISTRY`,
  `TRACEABILITY`, `ROADMAP` (`SERVER-SQL-SELECT`), `PROJECT-STATUS`.
- Tests per the design's verification plan: parser unit tests, a
  centralized `evaluate_query` unit test, one adapter `scan_all` unit
  test, a real client-to-server integration suite
  (`tests/server_sql_integration.rs`) across all three domains, and the
  protocol-version pin/gate/golden-vector updates.
- No `Cargo.toml`, storage-format, or `serve`-signature change.

## Consequences

### Positive

- "SQL" gets a real, literal answer — an actual grammar and parser,
  not a renamed structured-filter request — while staying proportionate
  to one bounded round: no planner, no joins, no aggregation, no
  server-side parsing surface.
- Closes a real, previously-named gap on both sides of
  `docs/FUTURE-GROWTH.md`: "the query language itself" (server/query
  layer section) and the SQL third of the "big three" (SQLite/DuckDB
  parity section) both move from "doesn't exist at all" to "a real,
  if intentionally small, first slice exists."
- The new full-scan primitive (`scan_all`/`AllIds<R>`) is itself a
  small, reusable, honestly-scoped increment — the kind of thing a
  future query-planner round could build a cheaper path *on top of*,
  not something that needs to be redone.
- `src/store/**`'s four `research`-gated backends stay completely
  untouched — the blast radius is exactly the two production-facing
  store types plus the server layer, verified directly, not assumed.

### Negative / tradeoffs

- **Every `Query` is an unindexed full scan.** A `WHERE id = ...` or
  `WHERE <indexed-field> = ...` predicate pays the same O(n) cost as
  any other `Query`, even though `GetById`/`FilterEq` could answer the
  identical question far more cheaply through their existing index —
  named plainly, not hidden; a query planner choosing the cheaper path
  is real, separate, later work.
- **Two new pieces of surface at once**: a real parser (a new source of
  bugs a project this size has never had before — malformed input
  handling, not just a schema-driven method call) and a new
  storage-adjacent trait. Larger than most prior rounds' single-piece
  additions; mitigated by keeping the parser entirely client-side, out
  of the server's trusted surface, and by the new trait's small,
  precedented shape.
- **A field's exposure surface becomes inconsistent by design.** A
  field with every `filter_eq`/`scan`/`update` flag `false` becomes
  queryable via `Query` where it wasn't via `FilterEq`/`ScanField` —
  deliberate (see the design's own "Considered options"), but a real
  behavior change worth an operator's attention if they were relying
  on those flags as an access boundary rather than an index-availability
  descriptor (they were never documented as the former).
- **No SQL syntax for a non-Rust client** until a server-side entry
  point is added — named as an explicit, deferred gap, not solved here.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0033`. Every claim about the current code (the exact shape of
  `Request`/`Response`/`ConnectionStore` today, that `MmapAgeStore`/
  `GenericMmapStore` already hold every record in a `HashMap` keyed by
  id, that neither `DogStore` nor `src/generic/query.rs` exposes an
  "every id" primitive today, and the exact `GetById`-forwarding
  pattern `Reversed`/`Symmetric` already use) read from `main`
  `cd46017` by direct inspection of `src/server/protocol.rs`,
  `src/durability/mmap_store.rs`, `src/generic/mmap_store.rs`,
  `src/store/mod.rs`, `src/generic/query.rs`, and `src/generic/store.rs`
  — not assumed from memory. No probe: the new primitive is a `HashMap`
  read and a forwarding trait impl in an already-exercised shape, not
  new concurrency or durability machinery needing empirical validation
  before committing to it.
- Revisit if: a query planner choosing an index-backed path over a full
  scan becomes worth the complexity — the design's own first "Open
  question."
- Revisit if: a non-Rust client needs SQL syntax — the server-side
  `Request::Sql { text }` entry point option 2 named but did not
  propose.
- Revisit if: `OR`/parenthesized boolean logic, `ORDER BY`, or
  aggregation are ever wanted — each is `docs/FUTURE-GROWTH.md`'s own
  named, larger, separate requirement, not a small extension of this
  design.
- Revisit if: this round's full-scan cost proves unacceptable at real
  data sizes — the honest answer then is the query planner above, not
  a smaller tweak to `evaluate_query`.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — a real
  client-side SQL `SELECT` subset over a new `scan_all` full-scan
  primitive, protocol version 8; **(b)** accept option 3 instead — skip
  the parser, ship only a structured compound-filter request
  (`AND`-of-`FilterEq`-shaped predicates), smaller but not literally
  SQL; **(c)** close as not warranted — restate "the query language
  itself" as still open, build nothing. Proposed in PR #176.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-SQL-SELECT-DESIGN.md`. (PR #176.)
