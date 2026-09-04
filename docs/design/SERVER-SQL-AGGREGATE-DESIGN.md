# Server SQL Aggregation / GROUP BY Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the owner
  approved the design as proposed, `ADR-0035` option (a); accepting
  without `AVG`/`ScanValue::F64` and closing as not warranted both
  declined; no changes requested). Acceptance authorizes the design;
  implementation follows as its own unit — see `ADR-0035`'s
  "Acceptance and implementation" section.
- Date: 2026-09-04
- Related: `ADR-0034`/`docs/design/SERVER-SQL-SELECT-DESIGN.md` (the
  `Request::Query`/`Response::Rows` read-only `SELECT` subset this
  design extends — named `GROUP BY`/aggregation as its own explicit
  "Non-goal," not attempted there), `ADR-0011` (schema discovery — the
  name→tag/kind resolution mechanism this design reuses unchanged),
  `ADR-0022`/`src/server/protocol.rs` (the append-only wire-shape/
  version rules a new `Request`/`Response` variant pair must follow),
  `ADR-0027`/`ADR-0033`/`ADR-0034` (`RYW-FR`/`ISO-FR-002`/`SQL-FR-009`'s
  own precedent that only `GetById` is overlaid/read-set-tracked — the
  reason this design's new read is neither), `docs/FUTURE-GROWTH.md`'s
  "Path to SQLite/DuckDB parity" (*"A query optimizer for aggregation
  (`GROUP BY`, `AVG`, multi-table joins) — DuckDB's core identity is
  vectorized execution over exactly this; nothing comparable exists
  here"* — the owner's "Aggregation / GROUP BY" pick this round narrows
  from, dropping the "multi-table joins" third explicitly, per
  `ADR-0034`'s own precedent of narrowing one item from a list rather
  than the whole list at once).

## Purpose and scope

The owner picked "Aggregation / GROUP BY" as this round's direction —
the natural next slice on top of the just-accepted `Request::Query`
(`ADR-0034`), which deliberately named `GROUP BY`/`COUNT`/`SUM`/`AVG`
as out of scope. `docs/FUTURE-GROWTH.md` bundles aggregation with
"multi-table joins" under one bullet; this design narrows to
aggregation alone — joins remain `docs/FUTURE-GROWTH.md`'s own separate,
still-open "big three" item, untouched here, for the identical reason
`ADR-0034` left them untouched (no second table exists on a
`ConnectionStore` connection to join against, `ADR-0010`'s own
one-domain-per-server architecture).

**In scope:**

- `SELECT`-list aggregate function calls — `COUNT(*)`, `SUM(<field>)`,
  `AVG(<field>)`, `MIN(<field>)`, `MAX(<field>)` — parsed **client-side**,
  extending `src/server/sql.rs`'s existing grammar rather than adding a
  second parser.
- `GROUP BY <field> (, <field>)*` — grouping by one or more fields, with
  the standard SQL rule that every non-aggregated column in the
  `SELECT` list must also appear in `GROUP BY`, checked client-side.
- A new, structured wire request/response pair, `Request::Aggregate`/
  `Response::Groups`, reusing `Request::Query`'s existing `Predicate`/
  `CompareOp` for `WHERE` filtering unchanged — the only genuinely new
  wire shapes are the group key and the aggregate function list
  themselves.
- Grouping/reduction computed **server-side**, over the exact same
  `ConnectionStore::scan_all` primitive `ADR-0034` already added — this
  design needs **zero new storage-adjacent primitives**; see "Considered
  options" for why client-side reduction was rejected.
- One new `ScanValue` variant, `F64(f64)`, carrying `AVG`'s result — the
  first non-integer numeric value this wire has ever carried.
- `LIMIT <n>` on the grouped result — truncates the number of groups
  returned, the same role it already plays for `Request::Query`'s rows.

**Out of scope (see "Non-goals")**: `HAVING` (post-aggregation
filtering), `ORDER BY` on grouped results (`ADR-0034`'s own `ORDER BY`
non-goal, unchanged), multi-table joins, nested/composite aggregate
expressions (`SUM(a) + SUM(b)`, `COUNT(*) * 2`), `DISTINCT` inside an
aggregate call (`COUNT(DISTINCT field)`), a query planner or
cost-based optimizer (still a full scan, exactly as `Request::Query`
already is).

## Non-goals

- **`HAVING`.** Real, standard SQL, and a real, separate piece of work
  — a second filter-shaped structure applied *after* reduction instead
  of before it, needing its own validation against the shape of
  `aggregates` rather than against `schema` directly (an aggregate's
  computed value has no `FieldRef`, so `HAVING`'s predicates cannot
  reuse `Predicate` unchanged the way `WHERE` does). Named as the most
  likely next slice, not attempted here — bundling it into this round
  would repeat the exact "two new pieces of surface at once" tradeoff
  `ADR-0034` already flagged as a real cost, this time three deep
  (`GROUP BY` + 5 aggregate functions + a new value kind + `HAVING`).
- **Multi-table joins.** `docs/FUTURE-GROWTH.md`'s own separate "big
  three" item; `ADR-0010`'s one-domain-per-connection architecture gives
  this design nothing to join against, unchanged from `ADR-0034`'s
  identical reasoning.
- **`ORDER BY` on grouped results.** `Response::Groups` carries groups
  in whatever unspecified order the grouping computation produces them
  in — the same "unspecified order" convention `ScanField`/
  `Request::Query`'s own `Response::Rows` already establish. `LIMIT`
  truncates that same unspecified order, not a meaningful top-N —
  unchanged from `ADR-0034`.
- **Nested or composite aggregate expressions.** `SUM(a) + SUM(b)`,
  `COUNT(*) * 100`, `AVG(a) / AVG(b)` — no expression evaluator exists
  or is proposed; each `SELECT`-list item is exactly one plain column
  or exactly one `agg_fn(column_or_star)` call, nothing more.
- **`DISTINCT` inside an aggregate (`COUNT(DISTINCT field)`).** A real,
  separate counting semantic (distinct-value cardinality, not row
  count) that would need its own accumulator shape; not attempted here.
- **`COUNT(<field>)` as distinct from `COUNT(*)`.** This crate's schema
  has no `NULL`/optional-field concept anywhere — every returned record
  carries every one of its fields, always (verified directly: no
  `ScanValue` variant represents an absent value, and `scan_all`'s
  result always pairs every field tag with a real value). Real SQL's
  `COUNT(field)` differs from `COUNT(*)` only by skipping `NULL`s, so
  with no `NULL`s to skip the two are unconditionally identical here —
  this design restricts `COUNT` to `COUNT(*)` only (`field: None`) as a
  deliberate simplification with no lost expressiveness, not an
  oversight. `SELECT COUNT(age) ...` is therefore a client-side parse
  restriction (an aggregate call's argument, if not `*`, is only valid
  for `SUM`/`AVG`/`MIN`/`MAX`), not a silently-accepted-then-ignored
  argument.
- **A query planner or cost-based optimizer.** Unchanged from
  `ADR-0034`: `Request::Aggregate` is unconditionally a full scan via
  the existing `scan_all`, same as `Request::Query`.
- **Writes, prepared statements, a server-side parser.** All three
  unchanged from `ADR-0034`'s own identical non-goals — this design
  adds no new answer to any of them.

## Context and terminology

**What `ADR-0034` already built, read from the current `main`**:
`Request::Query { select, filter, limit }` / `Response::Rows { rows }`
at protocol version 8; `Selection::{All, Fields(Vec<FieldRef>)}`;
`Predicate { field, op: CompareOp, value: ScanValue }`; `CompareOp::
{Eq, Ne, Lt, Le, Gt, Ge}` with `is_ordering()` restricting `Lt`/`Le`/
`Gt`/`Ge` to `U32`/`I64` fields; `ConnectionStore::scan_all(&self) ->
Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>` (`src/server/mod.rs`),
returning every record's full field set, unfiltered/unprojected;
`validate_query`/`evaluate_query`, the one centralized pure-function
pair that does `Request::Query`'s filter/project/limit work, identically
for every domain, over `scan_all`'s result. `src/server/sql.rs`
tokenizes and parses `SELECT (* | ident (, ident)*) FROM ident [WHERE
cond (AND cond)*] [LIMIT n]` entirely client-side;
`SchemaDrivenClient::query(sql)` resolves every name against the
`DomainSchema` already fetched at `connect` time (`ADR-0011`) and gates
on `server_protocol_version() >= 8`.

**What this means for aggregation, concretely**: `scan_all` already
returns everything a `GROUP BY`/aggregate computation needs — every
matched row's full field set. Nothing new needs to be read from the
store; the only new work is a different *reduction* over the same rows
`evaluate_query` already filters, done server-side instead of client-side
(see "Considered options" for why server-side, not client-side).

**`ScanValue` today** (`src/server/protocol.rs`): `U32(u32)`, `I64(i64)`,
`Bool(bool)`, `Str(String)` — every variant an exact, lossless
representation of a stored field's value. None is fractional; `AVG`
cannot be represented in any existing variant without either losing
precision (truncating to an integer) or silently rounding — the reason
this design proposes a new variant rather than reusing `I64`.

## Requirements

- `AGG-FR-001` — **Aggregate-function grammar, client-side.** Extends
  `src/server/sql.rs`'s existing `SELECT` grammar: a column-list item is
  either a plain `ident` (as today) or `agg_fn "(" ("*" | ident) ")"`,
  where `agg_fn` is one of `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`
  (case-insensitive, matching every other keyword). `*` is valid only as
  `COUNT`'s argument; `SUM`/`AVG`/`MIN`/`MAX` require a column `ident`.
  A trailing `GROUP BY <ident> (, <ident>)*` clause is added after the
  optional `WHERE` clause and before the optional `LIMIT` clause,
  matching the grammar's existing left-to-right clause order.
- `AGG-FR-002` — **Client-side compile-time routing.** A parsed query
  compiles to `Request::Aggregate` if it contains any aggregate-function
  column *or* a `GROUP BY` clause (a `GROUP BY` with no aggregate
  columns is valid — see `AGG-FR-004`); otherwise it compiles to the
  existing `Request::Query` exactly as `ADR-0034` already does, with
  zero change to that path.
- `AGG-FR-003` — **The standard SQL functional-dependency check,
  client-side.** Every plain (non-aggregate) column named in the
  `SELECT` list of a query compiling to `Request::Aggregate` must also
  appear in `GROUP BY` — checked against the *parsed* query, before any
  name resolution or round trip; a violation is `ClientError::Sql`, the
  same posture a syntax error already takes. `SELECT *` alongside any
  aggregate column or `GROUP BY` clause is rejected the same way — `*`
  has no meaning once rows collapse into groups.
- `AGG-FR-004` — **A new, structured wire request/response pair.**
  `Request::Aggregate { group_by: Vec<FieldRef>, filter: Vec<Predicate>,
  aggregates: Vec<AggregateSpec>, limit: Option<usize> }`,
  `AggregateSpec { func: AggregateFn, field: Option<FieldRef> }`,
  `AggregateFn::{Count, Sum, Avg, Min, Max}`, `Response::Groups { groups:
  Vec<AggregateGroup> }`, `AggregateGroup { key: Vec<(FieldRef,
  ScanValue)>, values: Vec<ScanValue> }` — `key` echoes back each
  `group_by` field's value for that group (empty when `group_by` is
  empty — see `AGG-FR-006`), `values` carries one result per
  `aggregates` entry, same order. Introduced at protocol version 9
  (`PROTOCOL_VERSION` 8 → 9, the table gains row 9); reuses `Predicate`/
  `CompareOp` unchanged for `filter`. `Request::Query`/`Response::Rows`
  are completely unaffected — a plain `SELECT` with no `GROUP BY`/
  aggregate column still only needs protocol 8, not 9.
- `AGG-FR-005` — **A new `ScanValue` variant, `F64(f64)`.** The only new
  wire value shape this design needs; carries exactly `AVG`'s result.
  Never appears in a `DomainSchema`/`FieldDescriptor` (`ValueKind` is
  unchanged — no stored field is ever `F64`-typed; see "Security,
  privacy, and compatibility" for why `ValueKind` and `ScanValue` are
  allowed to diverge here). `#[derive(PartialEq)]` already covers `f64`
  (no `Eq`/`Ord` needed anywhere this value is used); every existing
  `ScanValue` match/golden-vector test gains one new arm/case, no
  existing byte changes.
- `AGG-FR-006` — **Validation before any scan**, mirroring
  `validate_query`'s exact shape (`SQL-FR-007`). Checked against
  `schema` alone, before `scan_all` ever runs: an unknown `FieldRef` in
  `group_by` or an `AggregateSpec.field` is `ErrorCode::UnknownField`;
  a `Sum`/`Avg`/`Min`/`Max` field whose `ValueKind` is not `U32`/`I64`
  (the identical "orderable kind" rule `CompareOp::is_ordering()`
  already established) is `ErrorCode::Malformed`; a `Count` spec whose
  `field` is `Some(_)` is `ErrorCode::Malformed` too (`COUNT(*)` only —
  see "Non-goals"). No new `ErrorCode` — both codes already exist and
  are reused, matching `SQL-FR-007`'s own "no new wire addition" bar.
- `AGG-FR-007` — **Grouping and reduction are centralized, computed
  server-side over `scan_all`'s existing result — no new storage-adjacent
  primitive.** One new pure function (working name `evaluate_aggregate`,
  alongside `evaluate_query`/`overlay_staged`/`record_read_set` in
  `src/server/mod.rs`): filters `scan_all`'s rows with `filter` (reusing
  `predicate_matches` unchanged), buckets the survivors by their
  `group_by` field values (`group_by` empty ⇒ exactly one bucket holding
  every filtered row — the "whole-table aggregate, no `GROUP BY`" case
  real SQL also treats as one implicit group), reduces each bucket
  through every `AggregateSpec` in order, then truncates the group list
  by `limit` exactly as `evaluate_query` truncates rows. A group whose
  key matches zero filtered rows never appears — no null/zero-valued
  group is synthesized for a key nothing matched.
- `AGG-FR-008` — **Aggregate function semantics, computed from one pass
  per group.** `Count` → `ScanValue::I64` of the group's row count.
  `Sum` → `ScanValue::I64`, accumulating as `i64` regardless of whether
  the source field is `U32` or `I64` (a `U32` field's values are widened
  before summing, so the *sum* cannot overflow at `u32`'s range the way
  naively summing into a `u32` accumulator could — an `i64` sum can
  still itself overflow at extreme scale, the same inherent numeric
  limit this crate's other `i64` fields already live with, not a new
  gap this design introduces). `Avg` → `ScanValue::F64`, computed as
  that same group's accumulated sum divided by its row count (never a
  separately-tracked running average) — `Avg` therefore implicitly
  requires at least one matching row per group to be well-defined,
  which `AGG-FR-007`'s "a group with zero rows never appears" rule
  already guarantees. `Min`/`Max` → the actual observed `ScanValue`
  (`U32` stays `U32`, `I64` stays `I64`) — a passthrough of a real
  stored value, never promoted or converted.
- `AGG-FR-009` — **Not overlaid, not read-set-tracked.**
  `Request::Aggregate` is exactly as set-shaped a read as
  `Request::Query` already is — the identical line `RYW-FR`/
  `ISO-FR-002`/`SQL-FR-009` already draw: only `GetById` is
  read-your-writes-overlaid or snapshot-isolation-tracked, for the
  identical "no fixed identity to re-check/overlay cheaply" reasoning.
  `Aggregate` inside a session — of either kind, or plain — always reads
  committed state, unconditionally.
- `AGG-FR-010` — **Backward and cost compatible.** A connection
  negotiated below 9 cannot construct or send `Request::Aggregate` — the
  client library gates it (`ClientError::Unsupported("sql aggregate")`,
  no frame sent), the same posture `SQL-FR-010` already established for
  `Request::Query` at version 8; a plain `SELECT` with no aggregate
  content keeps working at version 8 unchanged, never forced to
  negotiate 9. A server that never receives `Request::Aggregate` runs no
  new code path beyond the version table and one new match arm; no
  `Cargo.toml`, storage-format, or `serve`-signature change.

## Considered options

**Where grouping/reduction is computed.**

1. **Server-side, over the existing `scan_all` — proposed.** The
   response carries only the reduced groups, not the underlying rows —
   the entire value proposition of `GROUP BY`/aggregation in the first
   place (a `COUNT`/`SUM` over a million rows should not cost a
   million-row response). Needs a new `Request`/`Response` pair, but
   zero new storage-adjacent primitive: `scan_all` already returns
   everything the reduction needs.
2. **Client-side, over `Request::Query`'s existing
   `Response::Rows`.** No new wire shapes at all — `SchemaDrivenClient::
   query` would fetch every matching row via the existing `Query`
   machinery and reduce them locally. Rejected: this ships every raw
   row over the wire to compute a result that is, by definition, far
   smaller than its input — the network cost `Request::Query` already
   pays honestly (a full unindexed scan) would be paid *again*, on top
   of transferring data the caller never wanted returned. It also
   silently reintroduces exactly the kind of "aggregate over everything
   the client can see" model this crate's whole client/server split was
   built to avoid.

**Whether `Request::Aggregate` is a new pair or an extension of
`Request::Query`/`Response::Rows`.**

1. **A new, separate pair — proposed.** `Response::Rows`'s row shape
   (`RecordId`, full field list) has no meaningful analogue once rows
   collapse into groups — there is no single record a group "is," so
   there is no id to attach. Overloading `Response::Rows` to sometimes
   carry a group's key instead of a record's id, or a synthetic/first
   id, would blur an existing, well-understood response shape for no
   real benefit. Mirrors `ADR-0034`'s own choice to add a new pair
   rather than overload `GetById`/`ScanField` for the identical reason —
   a fundamentally different response shape gets its own variant.
2. **Extend `Request::Query` with an optional `group_by`/`aggregates`
   field, reusing `Response::Rows` with `RecordId` left meaningless
   (e.g. the group's first matching id, or a zeroed placeholder) when
   grouping is active.** Fewer new top-level types, but a `RecordId`
   that sometimes means "a real record" and sometimes means "nothing in
   particular, ignore this field" is exactly the kind of overloaded,
   context-dependent shape this crate's wire protocol has consistently
   avoided (see `ParentLookup`'s own three-way enum, kept specifically
   to avoid a similar overload). Rejected.

**Whether to add `ScanValue::F64` for `AVG`, truncate to an integer, or
drop `AVG` this round.**

1. **Add `ScanValue::F64(f64)` — proposed.** Correct, standard SQL
   `AVG` semantics; `docs/FUTURE-GROWTH.md` names `AVG` explicitly
   alongside `GROUP BY` as this very item, so shipping `GROUP BY`
   without it would under-deliver the item as named. One new variant is
   a small, precedented cost — `ScanValue::Str` was itself added after
   the original design as "a necessary completion" (`src/server/
   protocol.rs`'s own module docs), the identical shape of decision.
2. **Truncate `AVG` to an integer, returning `ScanValue::I64`.** No new
   wire variant, but a real, permanent precision loss baked into the
   protocol for a value real SQL treats as fractional by definition —
   a false economy for what option 1 shows is a one-variant addition
   with zero cost to any existing code path. Rejected.
3. **Drop `AVG` this round; ship `COUNT`/`SUM`/`MIN`/`MAX` only,
   deferring `AVG` and the `F64` question to a later round.** The
   smallest possible footprint; considered as the fallback if `F64`
   turned out disproportionate to this round. It doesn't (see option
   1's cost accounting), so this is not proposed — named here as the
   fallback the owner may still prefer.

**Whether `GROUP BY` with zero aggregate functions (e.g. `SELECT breed
FROM dog GROUP BY breed`, effectively `DISTINCT breed`) is in scope.**

1. **Yes — proposed.** The natural degenerate case of the exact same
   `evaluate_aggregate` machinery (`aggregates: vec![]`); real SQL
   allows it; costs nothing extra once `group_by` exists, since
   `AGG-FR-007`'s bucketing step is required regardless of whether any
   aggregate function is also requested.
2. **No — require at least one aggregate function whenever `GROUP BY`
   is used.** Smaller client-side validation surface, but an arbitrary
   restriction invented for this design alone, with no engineering
   justification once the bucketing machinery exists — the same
   "restriction invented for this design alone" `SQL-FR-008` already
   rejected for the capability-flag question. Rejected on the identical
   grounds.

## Proposed shape

```rust
// src/server/protocol.rs
pub const PROTOCOL_VERSION: u32 = 9;   // AGG-FR-004

pub enum ScanValue {
    U32(u32),
    I64(i64),
    Bool(bool),
    Str(String),
    F64(f64),   // AGG-FR-005 — carries only AVG's result
}

pub enum AggregateFn { Count, Sum, Avg, Min, Max }

pub struct AggregateSpec {
    pub func: AggregateFn,
    pub field: Option<FieldRef>,   // None only for Count
}

pub enum Request {
    // ...unchanged variants, including Query...
    Aggregate {
        group_by: Vec<FieldRef>,
        filter: Vec<Predicate>,        // reused unchanged from Request::Query
        aggregates: Vec<AggregateSpec>,
        limit: Option<usize>,
    },
}

pub struct AggregateGroup {
    pub key: Vec<(FieldRef, ScanValue)>,   // this group's group_by values
    pub values: Vec<ScanValue>,            // one per `aggregates`, same order
}

pub enum Response {
    // ...unchanged variants, including Rows...
    Groups {
        groups: Vec<AggregateGroup>,
    },
}

// src/server/mod.rs
// dispatch's new arm (AGG-FR-006/007), validated against `describe()`
// before `scan_all` runs, mirroring Request::Query's own arm exactly:
// Request::Aggregate { group_by, filter, aggregates, limit } => {
//     if let Err(code) = validate_aggregate(&store.describe(), &group_by, &filter, &aggregates) {
//         return err_response(code);
//     }
//     let groups = evaluate_aggregate(store.scan_all(), &group_by, &filter, &aggregates, limit);
//     Response::Groups { groups }
// }
```

```rust
// src/server/sql.rs — grammar extension, client-side only
// query   := "SELECT" columns "FROM" ident [where_clause] [group_by_clause] [limit_clause]
// columns := "*" | column_item ("," column_item)*
// column_item := ident | agg_call
// agg_call := agg_fn "(" ("*" | ident) ")"
// agg_fn   := "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"
// group_by_clause := "GROUP" "BY" ident ("," ident)*

pub(crate) enum ParsedColumnItem {
    Plain(String),
    Aggregate { func: AggregateFn, arg: AggregateArg },
}

pub(crate) enum AggregateArg {
    Star,       // COUNT(*) only
    Field(String),
}

// ParsedQuery gains:
//     pub group_by: Vec<String>,
// and `columns` becomes `Vec<ParsedColumnItem>` in place of `ParsedColumns`
// (AGG-FR-001) — the plain-column-only path is unchanged in every other
// respect.

// src/server/client.rs
impl SchemaDrivenClient {
    // AGG-FR-002/003: if the parsed query has any aggregate column or a
    // non-empty group_by, resolve names/kinds and compile to
    // Request::Aggregate (checking server_protocol_version() >= 9,
    // ClientError::Unsupported("sql aggregate") otherwise); the
    // functional-dependency check (every plain column also in
    // group_by) runs before any resolution. Otherwise, behavior is
    // byte-for-byte SchemaDrivenClient::query as it exists today.
}
```

## Data/state and invariants

- `Request::Aggregate` is stateless and read-only — no session, no lock
  held across anything but the single `scan_all` call, exactly the same
  shape `Request::Query`/`ScanField`/`FilterEq` already have.
- `evaluate_aggregate` is a pure function, like `evaluate_query` — the
  reduction behavior is provably identical across all three domains by
  construction (one shared implementation over `scan_all`'s already-
  domain-agnostic result), not three independently-written copies
  agreeing.
- `AVG`'s result is always derived from the same accumulated sum/count
  pair a `Sum`/`Count` aggregate on the identical group would produce —
  never a separately maintained running average that could drift from
  what `Sum`/`Count` on the same request would report.
- `group_by`'s bucketing key is the tuple of a row's values for the
  named fields, compared by `ScanValue`'s existing `PartialEq` — the
  same equality this crate already uses for `Predicate::Eq`, no new
  comparison semantics.
- `limit` on `Request::Aggregate` bounds the number of *groups*
  returned, computed after the full reduction — the same "bounds the
  response, not the work" cost shape `Request::Query`'s own `limit`
  already has (`scan_all` still reads and reduces every filtered row
  before `limit` truncates the group list).

## Errors, failure, recovery, and observability

- No new `ErrorCode` — `UnknownField`/`Malformed` cover every rejection
  shape (`AGG-FR-006`), the identical bar `SQL-FR-007` already set.
- A syntax error, a functional-dependency violation (`AGG-FR-003`), or
  a below-9-connection attempt never reaches the wire — all three are
  `ClientError`, client-side, matching `SchemaDrivenClient`'s existing
  posture for a bad field name or a below-8 `Query`.
- `Request::Aggregate`'s outcome flows through the existing access-log
  machinery unmodified — `outcome_of` already maps every non-`Err`/
  `TransactionFailed` response to `Outcome::Ok`; `Response::Groups`
  needs no new arm beyond that existing catch-all, no new sink, no new
  gate.

## Security, privacy, and compatibility

- No new secret crosses the wire; `Response::Groups` carries strictly
  less raw data per matching row than `Response::Rows` already would —
  a group's `key` and reduced `values`, never the underlying rows
  themselves.
- `ValueKind` (the schema-describing enum in `DomainSchema`) is
  deliberately **not** extended with an `F64` case — no stored field is
  ever `F64`-typed, so no `FieldDescriptor` would ever need to report
  it; `F64` exists only as a `ScanValue` an `AGG-FR-008` reduction
  computes at request time, never a value a schema describes as
  storable. This is a deliberate, named asymmetry between the two
  enums, not an oversight — the same reasoning that already lets
  `ScanValue` carry values (like a `Query` row's projected subset) that
  no single `FieldDescriptor` alone determines.
- Backward compatible by construction: `PROTOCOL_VERSION` 8 → 9 is an
  appended request/response pair, unreachable below 9; every existing
  golden vector for versions 1–8, including `Request::Query`/
  `Response::Rows` themselves, is untouched. A client that never asks
  for `GROUP BY`/an aggregate function keeps negotiating and running at
  version 8 exactly as before — this design does not raise the floor
  for plain `SELECT`.

## Acceptance criteria

1. `Request::Aggregate`/`Response::Groups`/`AggregateSpec`/`AggregateFn`/
   `AggregateGroup`/`ScanValue::F64` exist exactly as specified;
   `PROTOCOL_VERSION = 9`; the version table and golden vectors updated;
   a client negotiated below 9 gets `ClientError::Unsupported("sql
   aggregate")` with no frame sent; a plain `SELECT` with no aggregate
   content still negotiates and runs at 8 unchanged.
2. `SchemaDrivenClient::query("SELECT breed, COUNT(*) FROM dog GROUP BY
   breed")` (and the equivalent for `SUM`/`AVG`/`MIN`/`MAX`, a `WHERE`
   clause combined with `GROUP BY`, multiple `group_by` fields, multiple
   aggregate columns in one query, and `LIMIT`) returns exactly the
   groups a hand-written filter-then-bucket-then-reduce over
   `Request::Query`'s own row set would.
3. `SELECT COUNT(*) FROM dog` with no `GROUP BY` returns exactly one
   group whose `key` is empty and whose `values` is the whole (filtered)
   table's count — the implicit single-group case.
4. A filter that matches zero rows for a given `GROUP BY` key produces
   zero groups for that key, not a group with a zero/null aggregate
   value; a filter matching nothing at all produces zero groups when
   `group_by` is non-empty, or exactly one group whose `Count` is `0`
   when `group_by` is empty (`SELECT COUNT(*) FROM dog WHERE false`-
   equivalent).
5. A plain column in the `SELECT` list that is not also in `GROUP BY` is
   a client-side `ClientError::Sql`, no round trip; `SELECT *` alongside
   `GROUP BY`/an aggregate column is rejected the same way;
   `SUM`/`AVG`/`MIN`/`MAX` against a `Str`/`Bool` field, and `COUNT`
   given an explicit field argument, are each a clean client-side or
   server-side (`ErrorCode::Malformed`) rejection, never silently
   accepted.
6. `AVG` returns `ScanValue::F64` equal to that same group's `Sum`
   divided by its `Count`, verified directly against a `SUM`+`COUNT`
   query on the identical `GROUP BY`/`WHERE`.
7. `Aggregate` inside a read-your-writes and/or snapshot-isolation
   session sees committed state only, and its own reads are never added
   to a snapshot-isolated session's read set — the identical criterion
   `SQL-FR-009`/`ADR-0034`'s own acceptance criterion 6 already
   establishes for `Query`.
8. With `Request::Aggregate` never sent, every existing test in
   `tests/server_*.rs` — including every `Request::Query` test — is
   unchanged, matching `SQL-FR-010`'s own "no branch, no cost" bar for
   anyone not using the new capability.

## Verification plan

- `src/server/sql.rs` unit tests: the extended grammar — `COUNT(*)`,
  each of `SUM`/`AVG`/`MIN`/`MAX` with a column argument, a mixed
  plain-and-aggregate column list, `GROUP BY` with one and with several
  fields, `GROUP BY` combined with `WHERE`/`LIMIT`, and every new syntax
  error this extension introduces (a bare `*` argument to `SUM`/`AVG`/
  `MIN`/`MAX`, a missing `BY` after `GROUP`, an aggregate call with no
  closing paren).
- `src/server/client.rs` unit/integration-adjacent tests: the
  functional-dependency check (a plain column missing from `GROUP BY`
  is `ClientError::Sql`), `SELECT *` rejected alongside `GROUP BY`, the
  below-9 gate.
- `src/server/mod.rs` unit tests: `evaluate_aggregate`'s behavior
  directly, independent of any adapter — `group_by` empty vs. one vs.
  several fields, each `AggregateFn` including the `Sum`/`Count`-vs-`Avg`
  cross-check from acceptance criterion 6, a filter matching nothing
  (both `group_by` empty and non-empty), `limit` shorter than and
  longer than the group count; `validate_aggregate`'s every rejection
  shape (unknown field in `group_by`/an `AggregateSpec`, a non-orderable
  field under `Sum`/`Avg`/`Min`/`Max`, a `Count` with `field: Some(_)`).
- `tests/server_sql_integration.rs` (extends the existing suite from
  `ADR-0034`): a real client round trip on at least the `Dog` domain —
  `COUNT(*)` with no `GROUP BY`, `GROUP BY` on one and on two fields,
  each aggregate function, a `WHERE`+`GROUP BY` combination, `LIMIT` on
  a grouped result, the functional-dependency and `SELECT *` client-side
  rejections, `Aggregate` inside a read-your-writes and a
  snapshot-isolation session each reading committed state.
- `tests/server_protocol_version.rs`: the version pin (9), the
  unsupported-below-9 client gate, a golden vector for
  `Request::Aggregate`/`Response::Groups`, and confirmation that a
  plain `Request::Query`/`Response::Rows` golden vector from version 8
  is byte-for-byte unchanged.

## Traceability

- → `SERVER-001` next minor / FR (`AGG-FR-001`–`010`), a new ADR;
  narrows `docs/FUTURE-GROWTH.md`'s "A query optimizer for aggregation
  (`GROUP BY`, `AVG`, multi-table joins)" item to the bounded
  aggregation-only slice this document scopes — multi-table joins, a
  query planner/cost-based optimizer, and `HAVING` all remain open,
  named, not solved.
- Extends `ADR-0034`/`SERVER-SQL-SELECT-DESIGN.md`'s own explicit
  `GROUP BY`/aggregation "Non-goal," resolving it (one bounded slice
  only) rather than leaving it standing.
- Roadmap: `SERVER-SQL-AGGREGATE-DESIGN` (this document), then
  `SERVER-SQL-AGGREGATE` as the implementation unit if accepted.

## Open questions

- Whether `HAVING` is wanted enough to justify its own round — named as
  the most likely next slice in "Non-goals," not decided here.
- Whether `Request::Aggregate` should ever consult an existing index as
  a cheap-path optimization — the identical open question `ADR-0034`
  already named for `Request::Query`, inherited unchanged since both
  requests share the same unconditional-full-scan execution model.
- Whether a `COUNT(DISTINCT field)`-shaped distinct-cardinality
  aggregate is ever wanted — named, not solved; would need its own
  accumulator shape beyond a running count.
- Whether `F64` should ever be usable outside an `AVG` result — e.g. a
  future fractional stored field type — open, and not something this
  design's narrow, output-only use of `F64` needs to anticipate.

## Change history

- 2026-09-04: Initial proposal, in response to the owner's "Aggregation
  / GROUP BY" pick from `docs/FUTURE-GROWTH.md`, narrowed from that
  document's combined "`GROUP BY`, `AVG`, multi-table joins" bullet to
  aggregation alone, extending `Request::Query`/`ADR-0034` with a new
  `Request::Aggregate`/`Response::Groups` pair at protocol version 9 and
  one new `ScanValue::F64` variant.
- 2026-09-04: Accepted as proposed. No content change. Implementation
  follows as `SERVER-001`'s next minor / FR.
- 2026-09-04: Implemented as `SERVER-001` v0.28.0 / FR-038, exactly this
  document's "Proposed shape" with one clarification found only during
  implementation, not a deviation from it: acceptance criterion 4's
  "exactly one group whose `Count` is `0`" case (the implicit
  whole-table bucket, `group_by` empty, a filter matching zero rows)
  needed the bucket to be seeded unconditionally rather than only when
  a row survives filtering, and `Min`/`Max` needed a `schema`-typed
  zero fallback for that same zero-row case, since no real observed
  value exists there to pass through — both caught by this round's own
  new unit tests, both consistent with, not contradicting, `AGG-FR-007`/
  `AGG-FR-008` as written. Zero new storage-adjacent primitives, as
  designed — `ConnectionStore::scan_all` (`ADR-0034`) reused entirely
  unchanged. Every acceptance criterion 1–8 holds.
