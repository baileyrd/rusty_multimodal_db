# ADR-0044: `JOIN` over a declared relation within one table — `Request::Join` at protocol 12

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved option (a): the design as proposed — `Request::Join`/
  `DescribeRelations`, `Response::JoinedRows`/`Relations` at protocol
  12, the server-side index nested loop, `describe_relations()` with
  `Employee`'s override, the SQL `JOIN … ON <relation>` grammar with
  alias-qualified names, and `QueryResult::Joined`; (b) wire and server
  half only, (c) adding indexed equi-joins, and (d) closing as not
  warranted all declined). Acceptance authorizes the design;
  implementation follows as its own unit — see "Acceptance and
  implementation" below. Part A of `docs/design/SERVER-SQL-JOIN-
  DESIGN.md`; Part B is `ADR-0045`, accepted the same day as gated
  direction.
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-SQL-JOIN-DESIGN.md` (the full design),
  `ADR-0034`/`ADR-0035` (both named joins as a Non-goal — "no second
  table on the same connection to join against"; `ADR-0035`'s revisit
  trigger *multi-table joins are ever taken up* fires here), `ADR-0037`/
  `ADR-0039` (`SchemaDrivenClient::traverse`, the N+1-round-trip
  composition this replaces for one hop), `ADR-0042` (the source
  reading: every join `rusty_remind_me` runs is a relation join),
  `ADR-0022` (rules 1–4; `Join`/`DescribeRelations` are gated requests
  — the `Query` precedent, not the `StrList` one), `ADR-0045` (Part B,
  the table concept `right_table`/`target_table` leave room for),
  `docs/FUTURE-GROWTH.md` ("Arbitrary joins" — not this).

## Context

`docs/FUTURE-GROWTH.md` files "arbitrary joins" under the multi-year
path to SQLite/DuckDB parity: "SQL lets you join any two tables on any
predicate at query time; nothing like that exists here." Two accepted
SQL rounds declined joins with the same sentence — a connection serves
one `ConnectionStore`, so there is nothing to join against.

Reading the consumer changes the ask. `rusty_remind_me` runs nine
`JOIN`s (`entity.rs`, `contradictions.rs`); every `ON` is a foreign key
or a link table, never a computed predicate. Its `traverse_entities`
query — `entity_relations r JOIN entities s ON s.id = r.subject_entity_
id JOIN entities o ON o.id = r.object_entity_id`, returning both
endpoints' *names* — is exactly what this crate stores as `Multi
Symmetric` adjacency and returns from `traverse` as bare ids, so today
the same result costs one `traverse` plus one `GetById` per id.

Reading the crate confirms the join key already exists as an index:
`ChildOf` (`Order → Customer`, `Employee → Employee`), `Symmetric
Relation` (`Dog`, `Employee`, `Entity` with two labels), and every
adapter's `parent`/`children`/`neighbors`/`neighbors_by_relation`. A
one-hop relation join is an index nested loop over those methods — the
work `traverse` already does, moved server-side into one round trip.
What does *not* exist is a second table: `Reversed` stores only a
child→parent index, never the parent records, so `Customer`'s `name`
cannot be read by anyone (see `ADR-0045`).

## Decision

Adopt Part A of the design document: **a relation join within one
table, server-side, one round trip, protocol 12.**

- `Request::Join(JoinSpec)` (index 19) with `relation: JoinRelation`
  (`Neighbors(Option<label>)` / `Parent` / `Children`), `right_table:
  Option<String>` (`None` in Part A; present from day one so Part B
  never needs a second variant), two `Selection`s, two filter lists,
  `limit`; answered by `Response::JoinedRows { rows: Vec<JoinedRow> }`
  (index 15) where a `JoinedRow` carries both ids and both projected
  field lists.
- `Request::DescribeRelations` (index 20) → `Response::Relations {
  relations: Vec<RelationDescriptor { name, kind, target_table }> }`
  (index 16), backed by a new `ConnectionStore::describe_relations()`
  with a conservative default (symmetric labels listed; `parent`/
  `children` omitted unless an adapter says they target its own table —
  `Employee` does, `Order` does not). `DomainSchema` cannot grow a field
  (`STORAGE-018`), so relation detail is a request, as `ListRelation
  Kinds` was.
- Evaluation in `dispatch`: validate both sides like `validate_query`
  and the relation against `describe_relations()`; then for each
  filtered left row, related ids through the adapter's own relation
  method, `get` each, apply the right filter, emit; `limit` truncates
  and stops. Symmetric edges appear in both orientations (SQL
  semantics). Never overlaid, never read-set tracked — `Query`'s line.
- SQL: `FROM t [AS] a JOIN t [AS] b ON <relation>`, alias-qualified
  columns and conditions required in a `JOIN` query, no `GROUP BY`/
  aggregates with `JOIN`; compiled client-side to `Request::Join`;
  `QueryResult::Joined(Vec<JoinedRowNamed>)` as a third variant, names
  alias-qualified. `ClientError::Unsupported("sql join")` below 12 with
  no frame sent.
- No new `ErrorCode`; no `downgrade_for_version` arm (`JoinedRows` only
  answers a gated request); `PROTOCOL_VERSION` 11 → 12, table row 12,
  four golden vectors, the three pins moved.

Theta joins — `ON` over arbitrary columns — are declined: without a
planner they are O(|L|·|R|), the only indexed fields are one per
domain plus `NameIndex`, and no consumer issues one. An indexed
equi-join (`ON a.x = b.<IndexedField>`) is named as a bounded later
extension, not built.

## Consequences

- Positive: the consumer's one-hop-with-names query becomes one round
  trip against `Entity` — and the same statement works on `Dog` and
  `Employee` today, with no new domain.
- Positive: the join key is always an existing index; cost is
  Σ degree `get`s over the filtered left side, never |L|·|R|; `limit`
  bounds work as well as response.
- Positive: no new `ErrorCode`, no content-rewriting downgrade, no
  on-disk change, no adapter change beyond one `describe_relations()`
  override.
- Named, not hidden: **this is not "arbitrary joins."** `docs/FUTURE-
  GROWTH.md`'s multi-year item stays open; this ADR narrows to the
  join shape the consumer actually uses and says so. The owner may
  judge the narrowing wrong (option (c)) or the whole thing premature
  (option (d)).
- Named, not hidden: `QueryResult` gains a third variant — a public
  client API change every `match` on it must absorb (the `Groups`
  precedent, `AGG-FR-003`).
- Named, not hidden: a `JOIN` query requires alias-qualified names
  everywhere, even where unambiguous. Simpler than SQL's ambiguity
  rules; a real ergonomic cost.
- Named, not hidden: `right_table`/`target_table` are fields whose only
  Part A value is `None` — carried so Part B (`ADR-0045`) is a pure
  append. If Part B is never built they are dead weight on the wire
  (one byte each per `JoinSpec`/`RelationDescriptor`).
- The default `describe_relations()` omits `parent`/`children` rather
  than guessing; an adapter that wants them must say so. Conservative
  by design — a wrong "same table" claim would return wrong rows
  silently.

## Considered options

The design document's own "Considered options" covers four forks.
**Where the join runs** — (a) **(proposed)** server-side index nested
loop, one round trip; (b) client-side like `traverse` [what exists
today; N round trips; cannot filter the right side before fetching];
(c) a generic `Request::Batch` of `GetById`s [amortizes round trips
only]. **What `ON` may name** — (a) **(proposed)** a declared relation;
(b) an equi-join on any field [O(|L|·|R|) without an index; bounded
indexed form named for later]; (c) arbitrary predicates [the multi-year
item]. **Result shape** — (a) **(proposed)** `JoinedRows` with two ids
and two field lists; (b) reuse `Rows` with synthetic tags [loses the
right id, tag collisions]; (c) right rows only [a projection of (a)].
**How the server knows a relation is self-joinable** — (a)
**(proposed)** `describe_relations()` + `DescribeRelations`, conservative
default; (b) infer from `get(parent_id) == None` [indistinguishable
from a dangling id]; (c) grow `RelationCapabilities` [a format change].

## Acceptance and implementation

- Options offered at proposal: **(a) accept as proposed** — `Request::
  Join`/`DescribeRelations`, `Response::JoinedRows`/`Relations`,
  protocol 12, the SQL `JOIN … ON <relation>` grammar with
  alias-qualified names, `QueryResult::Joined`, `describe_relations()`
  with `Employee`'s override; **(b) accept the wire and server half
  only** — `Request::Join`/`DescribeRelations` and the evaluator, no SQL
  grammar change (a client builds `JoinSpec` directly, as it can build
  `Request::Query`); **(c) accept with equi-joins on the indexed field
  added** — `JoinRelation::IndexedEq { left_field, right_field }` via
  `filter_eq`, a second join kind in the same round; **(d) close as not
  warranted** — `traverse` + `GetById` is the consumer's path today and
  no consumer of this crate exists yet.
- Sizing: (b) about a day; (a) about two days (the grammar, aliases,
  and `QueryResult::Joined` are the other half); (c) about three.
- 2026-09-05: accepted as proposed (option (a); (b), (c), and (d)
  declined). Implementation follows as `SERVER-001`'s next minor / FR
  (protocol 12), per Part A of `docs/design/SERVER-SQL-JOIN-DESIGN.md`.
  Proposed and accepted in PR #193.
- 2026-09-05: **implemented** as `SERVER-001-FR-045` (v0.35.0). Landed
  as proposed: `Request::Join`/`DescribeRelations` (19/20), `Response::
  JoinedRows`/`Relations` (15/16), `PROTOCOL_VERSION` 12, four golden
  vectors; `ConnectionStore::describe_relations()` with the conservative
  default (`default_relation_descriptors`) and `Employee`'s one override;
  `validate_join`/`evaluate_join` in `dispatch` — the index nested loop
  over each adapter's existing relation methods, `limit` bounding work
  and response, both orientations of a symmetric edge; `Malformed`
  below 12 in `handle_connection` (the session precedent, per this
  ADR's text; no `downgrade_for_version` arm, pinned by the `Hello
  { 11 }`/silent-client tests); SQL aliases, `alias.field`, `JOIN … ON
  <relation>`, the four parse-time rejections; `SchemaDrivenClient::
  relations()`, `query_join`, `QueryResult::Joined`/`JoinedRowNamed`,
  `Unsupported("sql join")` below 12. Proven over real sockets on
  `Entity` (the consumer's one-hop-with-names query in one round trip,
  `aliases` riding as a `StrList`), `Employee` (`parent`/`children`/
  `collaborates_with`), and `Dog`; `Order` and `Reminder` refuse as
  designed. **No deviation from the accepted text.** Two consequences
  recorded, not folded in: `QueryResult`'s third variant broke three
  exhaustive `match`es in this crate's own integration suites (the
  public-API cost named above — each gained a `Joined` arm); and four
  unplanned protocol-11 literals in Unit 49's tests broke on the bump
  and were rewritten against `PROTOCOL_VERSION` (or `>= 11` where the
  claim is "since 11"), so only `tests/server_protocol_version.rs` and
  `protocol.rs`'s own test pin the literal. Wire-and-client only: no
  `Cargo.toml`, `src/generic/**`, or on-disk change; `traverse`
  untouched. Tests: four new unit tests in `src/server/mod.rs`, six in
  `src/server/sql.rs`, seven new integration tests across the six
  server suites, the three named pins moved. Full sweep green — see
  `docs/PROJECT-STATUS.md` item 131 and `SERVER-001-FR-045` for the
  counts.
