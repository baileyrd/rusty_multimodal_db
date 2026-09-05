# ADR-0035: `GROUP BY` and aggregate functions on top of `Request::Query`

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the owner
  approved the design as proposed, option (a): `GROUP BY` and `COUNT`/
  `SUM`/`AVG`/`MIN`/`MAX` on top of `Request::Query`, a new
  `Request::Aggregate`/`Response::Groups` pair at protocol version 9,
  `ScanValue::F64` for `AVG`; (b) accepting without `AVG`/`F64` and
  (c) closing as not warranted both declined; no changes requested).
  Acceptance authorizes the design; implementation follows as its own
  unit — see "Acceptance and implementation" below.
- Date: 2026-09-04
- Deciders: baileyrd
- Related: `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md` (the full design
  this ADR summarizes), `ADR-0034`/`docs/design/SERVER-SQL-SELECT-
  DESIGN.md` (the `Request::Query`/`Response::Rows` `SELECT` subset this
  design extends — named `GROUP BY`/aggregation as its own explicit
  non-goal), `ADR-0011` (schema discovery — the name→tag/kind resolution
  mechanism this design reuses unchanged), `ADR-0022`/
  `src/server/protocol.rs` (the wire-shape/version rules a new
  `Request`/`Response` pair must follow), `ADR-0027`/`ADR-0033` (the
  "only `GetById` is overlaid/read-set-tracked" precedent this design's
  new read follows, same as `ADR-0034` already does), `docs/FUTURE-
  GROWTH.md`.
- Supersedes/Superseded by: none. Adds one new `Request`/`Response`
  variant pair (protocol version 9) and one new `ScanValue` variant;
  changes no existing store, no existing wire shape, and no existing
  behavior of `Request::Query` itself.

## Context

The owner picked "Aggregation / GROUP BY" from `docs/FUTURE-GROWTH.md`
as the direction to pursue this round — the natural next slice on top
of the just-accepted `Request::Query` (`ADR-0034`), which named
`GROUP BY`/`COUNT`/`SUM`/`AVG` as its own explicit non-goal.
`docs/FUTURE-GROWTH.md` bundles aggregation with "multi-table joins"
under one bullet ("A query optimizer for aggregation (`GROUP BY`,
`AVG`, multi-table joins)"); this ADR narrows to aggregation alone,
leaving joins as `docs/FUTURE-GROWTH.md`'s own separate, still-open
item — the identical narrowing move `ADR-0034` itself made against a
larger bundled ask.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0034` took.

## Decision drivers

- Deliver `GROUP BY`/aggregation with real, standard SQL semantics
  (`COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, the functional-dependency rule),
  not a smaller stand-in renamed to sound like it.
- Reuse everything `ADR-0034` already built rather than duplicate it:
  the same `scan_all` full-scan primitive, the same `Predicate`/
  `CompareOp` filter shape, the same client-side-parser/server-side-
  execution split, the same "not overlaid, not read-set-tracked" line.
- Keep the storage layer completely untouched — this design needs zero
  new storage-adjacent primitives, unlike `ADR-0034`'s own `AllIds`
  addition; `scan_all` already returns everything a reduction needs.
- Name the real cost plainly: `Request::Aggregate` is, like
  `Request::Query`, an unindexed full scan — no planner, no partial
  aggregation pushdown, no cost model.
- Give `AVG` correct semantics rather than a truncated-integer
  approximation, at the honest, small cost of one new wire value kind.

## Considered options

**Where grouping/reduction is computed.**

1. **Server-side, over the existing `scan_all` — proposed.** The
   response carries only the reduced groups, not the underlying rows —
   the entire point of `GROUP BY`/aggregation. Needs a new `Request`/
   `Response` pair; needs zero new storage-adjacent primitive.
2. **Client-side, over `Request::Query`'s existing `Response::Rows`.**
   No new wire shapes, but ships every raw matching row over the wire
   just to reduce it locally — defeating the entire value proposition
   of aggregation and paying the full-scan cost `Request::Query`
   already pays honestly a second time, on data the caller never
   wanted returned. Rejected.

**Whether `Request::Aggregate` is a new pair or an extension of
`Request::Query`/`Response::Rows`.**

1. **A new, separate pair — proposed.** A group has no single record
   it "is," so there is no `RecordId` to attach the way every
   `Response::Rows` entry has one — mirrors `ADR-0034`'s own choice to
   add a new pair rather than overload `GetById`/`ScanField` for the
   identical reason.
2. **Extend `Request::Query`, reusing `Response::Rows` with a
   meaningless/placeholder `RecordId` when grouping is active.** Fewer
   new types, at the cost of a field that means "a real record" in one
   response and "ignore this" in another — exactly the overloaded shape
   this crate's wire protocol has consistently avoided (`ParentLookup`'s
   own three-way enum exists to avoid the identical overload). Rejected.

**Whether to add `ScanValue::F64` for `AVG`, truncate to an integer, or
drop `AVG` this round.**

1. **Add `ScanValue::F64(f64)` — proposed.** Correct `AVG` semantics;
   `docs/FUTURE-GROWTH.md` names `AVG` explicitly alongside `GROUP BY`
   in the very item this ADR narrows from copy shipping without it
   would under-deliver. One new variant, zero cost to any existing code
   path — the same shape of addition `ScanValue::Str` itself already
   was.
2. **Truncate `AVG` to `ScanValue::I64`.** No new variant, but a real,
   permanent precision loss baked into the protocol. Rejected as a
   false economy.
3. **Drop `AVG` this round.** Smallest footprint; the fallback if `F64`
   proved disproportionate. It doesn't, so not proposed — named as the
   fallback the owner may still prefer.

**Whether `GROUP BY` with zero aggregate functions is in scope.**

1. **Yes — proposed.** The natural degenerate case of the same
   bucketing machinery; costs nothing extra; real SQL allows it.
2. **No — require at least one aggregate function.** An arbitrary
   restriction invented for this design alone, with no engineering
   justification once bucketing exists. Rejected on the same grounds
   `ADR-0034` already rejected the equivalent capability-flag
   restriction.

## Decision

Proposed: option 1 in each of the four questions above. Concretely, at
implementation:

- `src/server/protocol.rs`: `Request::Aggregate { group_by:
  Vec<FieldRef>, filter: Vec<Predicate>, aggregates: Vec<AggregateSpec>,
  limit: Option<usize> }`, `Response::Groups { groups:
  Vec<AggregateGroup> }`, `AggregateSpec { func: AggregateFn, field:
  Option<FieldRef> }`, `AggregateFn::{Count, Sum, Avg, Min, Max}`,
  `AggregateGroup { key: Vec<(FieldRef, ScanValue)>, values:
  Vec<ScanValue> }`, `ScanValue` gains `F64(f64)`. `PROTOCOL_VERSION`
  moves to 9 (an appended variant pair, `ADR-0022`'s rule 2); no new
  `ErrorCode` — `UnknownField`/`Malformed` cover every rejection shape,
  reused from `Request::Query`'s own precedent.
- `filter` reuses `Predicate`/`CompareOp` unchanged; `Request::Query`/
  `Response::Rows` and everything at protocol 8 and below are completely
  untouched — a plain `SELECT` still only needs version 8.
- Grouping/reduction is one new, centralized pure function
  (`evaluate_aggregate`) in `src/server/mod.rs`, alongside
  `evaluate_query`, computed over `ConnectionStore::scan_all`'s existing
  result — no new `ConnectionStore` method, no new storage-adjacent
  trait, unlike `ADR-0034`'s own `AllIds` addition.
- `src/server/sql.rs`'s grammar gains aggregate-function column items
  (`COUNT(*)`, `SUM`/`AVG`/`MIN`/`MAX(<field>)`) and a `GROUP BY <field>
  (, <field>)*` clause; `SchemaDrivenClient::query`'s existing parser
  entry point routes a parsed query to `Request::Aggregate` when it
  contains any aggregate column or `GROUP BY`, otherwise unchanged to
  `Request::Query` — the standard SQL functional-dependency rule (every
  plain `SELECT` column must also be in `GROUP BY`) is checked
  client-side before any resolution, `ClientError::Sql` on violation;
  gated on `server_protocol_version() >= 9`
  (`ClientError::Unsupported("sql aggregate")`, no frame sent below it).
- `COUNT` is restricted to `COUNT(*)` only (`field: None`) — this
  crate's schema has no `NULL` concept, so `COUNT(field)` and
  `COUNT(*)` are unconditionally identical; a deliberate simplification,
  not an oversight.
- `Request::Aggregate` is read-only, unauthenticated-gated like any
  other read, never overlaid by read-your-writes and never tracked into
  a snapshot-isolated session's read set — the same "only `GetById`"
  line `RYW-FR`/`ISO-FR-002`/`SQL-FR-009` already draw, unchanged.
- `SERVER-001`'s next minor / FR (`AGG-FR-001`–`010`); `SPEC-REGISTRY`,
  `TRACEABILITY`, `ROADMAP` (`SERVER-SQL-AGGREGATE`), `PROJECT-STATUS`.
- Tests per the design's verification plan: extended parser unit tests,
  a centralized `evaluate_aggregate`/`validate_aggregate` unit test
  suite, an extended `tests/server_sql_integration.rs`, and the
  protocol-version pin/gate/golden-vector updates.
- No `Cargo.toml`, storage-format, or `serve`-signature change.

## Consequences

### Positive

- `GROUP BY`/aggregation gets a real, standard-semantics answer —
  `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`, the functional-dependency rule, the
  implicit single-group case — not a smaller stand-in, while staying
  bounded to one round: no `HAVING`, no joins, no planner.
- Closes the `GROUP BY`/aggregation half of `docs/FUTURE-GROWTH.md`'s
  combined "aggregation, `GROUP BY`, `AVG`, multi-table joins" item and
  `ADR-0034`'s own explicit non-goal for the same — multi-table joins
  remain the one piece of that original bullet still fully open.
- Zero new storage-adjacent primitive: this design reuses `ADR-0034`'s
  `scan_all` unchanged, so the blast radius is exactly `src/server/
  protocol.rs`/`mod.rs`/`sql.rs`/`client.rs` — smaller than `ADR-0034`'s
  own footprint, which additionally touched `src/production.rs` and
  `src/generic/**`.
- `Request::Query`/`Response::Rows` and every protocol-8-and-below byte
  are provably untouched — verified by the version table and golden
  vectors, not merely asserted.

### Negative / tradeoffs

- **Every `Request::Aggregate` is an unindexed full scan**, exactly like
  `Request::Query` — a `GROUP BY`/aggregate over a large table pays the
  same O(n) cost regardless of how small its final response is; a query
  planner or partial-aggregation pushdown is real, separate, later work.
- **A second new wire value kind** (`ScanValue::F64`) in as many rounds
  — a real, if small, growth in the wire's surface area; named plainly,
  matching the discipline `ADR-0034` already applied to its own
  `AllIds` addition.
- **`GROUP BY` without `HAVING`** is a real, incomplete slice of what a
  user would expect from "aggregation" once they have `GROUP BY` at
  all — named explicitly as the most likely next revisit, not hidden
  behind "aggregation is done."
- **`COUNT(field)` is silently unavailable** (only `COUNT(*)` is
  supported) — a deliberate simplification given this schema's lack of
  `NULL`s, but a real surface-area restriction relative to standard SQL,
  worth an operator's attention if they expected the distinction to
  exist even though it is provably a no-op here.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0034`. Every claim about the current code (the exact shape of
  `Request::Query`/`Response::Rows`/`Predicate`/`CompareOp` today, that
  `ConnectionStore::scan_all` already returns every filtered row's full
  field set, the exact declaration indices `Request::Query` (15) and
  `Response::Rows` (12) occupy, and that no `ScanValue` variant
  represents an absent/`NULL` value) read from `src/server/protocol.rs`
  and `src/server/mod.rs` on `main` by direct inspection, not assumed
  from memory. No probe: the new primitive is a pure reduction over data
  `scan_all` already returns, the identical "already-exercised shape,
  no new concurrency or durability machinery" reasoning `ADR-0034`
  itself used to skip a probe.
- Revisit if: `HAVING` becomes worth its own round — the design's own
  first "Open question" and most-named non-goal.
- Revisit if: a query planner or partial-aggregation pushdown becomes
  worth the complexity — inherited unchanged from `ADR-0034`'s own
  identical open question.
- Revisit if: `COUNT(DISTINCT field)`-shaped distinct-cardinality
  aggregation is ever wanted — named, not solved.
- Revisit if: multi-table joins are ever taken up — `docs/FUTURE-
  GROWTH.md`'s own separate "big three" item, deliberately untouched
  by this ADR.
  *Fired: `ADR-0044` (relation joins within one table, implemented as
  `SERVER-001-FR-045`) and `ADR-0045` (multi-table connections, accepted
  as gated direction) — `docs/design/SERVER-SQL-JOIN-DESIGN.md`.*

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — `GROUP BY`
  and `COUNT`/`SUM`/`AVG`/`MIN`/`MAX` on top of the existing
  `Request::Query` machinery, a new `Request::Aggregate`/
  `Response::Groups` pair at protocol version 9, `ScanValue::F64` for
  `AVG`; **(b)** accept without `AVG`/`ScanValue::F64` — ship `COUNT`/
  `SUM`/`MIN`/`MAX` only this round, deferring `AVG` and the new value
  kind; **(c)** close as not warranted — restate aggregation as still
  open, build nothing. Proposed in PR #178.
- 2026-09-04: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md`. (PR #179.)
- 2026-09-04: implemented as `SERVER-001` v0.28.0 / FR-038 — exactly as
  designed, option (a) in full: `Request::Aggregate`/`Response::Groups`
  at protocol version 9, `AggregateFn::{Count,Sum,Avg,Min,Max}`,
  `ScanValue::F64` for `AVG`, `validate_aggregate`/`evaluate_aggregate`
  reusing `ConnectionStore::scan_all` (`ADR-0034`) unchanged — zero new
  storage-adjacent primitives, confirmed rather than merely carried
  over from the design. One real edge case this round's own
  implementation surfaced beyond the design's worked examples: the
  implicit whole-table bucket (`group_by` empty) must always exist,
  even holding zero rows, so `SELECT COUNT(*) FROM t WHERE false`-
  equivalent still returns one group whose `Count` is `0` (acceptance
  criterion 4) — the first cut of `evaluate_aggregate` did not do this,
  caught by this round's own new unit test; fixed by always seeding
  that one bucket, and by giving `Min`/`Max` a `schema`-typed zero
  fallback (rather than panicking on an empty iterator) for the one
  case a real observed value doesn't exist, since `Sum`/`Avg` were
  already well-defined there at `0`/`0.0`. Every acceptance criterion
  1–8 holds; no other deviation from the design. (This PR.)
