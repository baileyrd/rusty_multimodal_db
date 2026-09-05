# Server SQL JOIN: Relation Joins First, Tables Second (Accepted)

- Status: **Accepted** (both parts promoted from Proposed on 2026-09-05 —
  the owner approved each as proposed, option (a) of both `ADR-0044`
  and `ADR-0045`: **Part A**, a `JOIN` over a *declared relation*
  within one table, server-side at protocol 12, with the SQL grammar,
  `QueryResult::Joined`, and `describe_relations()`; **Part B**, more
  than one table on one connection, as gated direction — the shape is
  the plan, `right_table`/`target_table` ride in Part A's wire from day
  one, and no implementation unit is scheduled until a second table
  someone needs exists. `ADR-0044` (b)/(c)/(d) and `ADR-0045` (b)/(c)
  declined.) Acceptance authorizes each design; Part A's implementation
  follows as its own unit — see each ADR's "Acceptance and
  implementation" section. Design-only here: no code changes in this
  round.
- Date: 2026-09-05
- Related: `docs/FUTURE-GROWTH.md` ("Arbitrary joins," one of the three
  "different tier of project" items — this document deliberately does
  not build that, and says why), `ADR-0034`/`docs/design/SERVER-SQL-
  SELECT-DESIGN.md` and `ADR-0035`/`docs/design/SERVER-SQL-AGGREGATE-
  DESIGN.md` (both named joins as a Non-goal with the identical reason:
  "every server instance serves exactly one domain … there is no second
  table on the same connection to join against"; `ADR-0035`'s revisit
  trigger *multi-table joins are ever taken up* fires here),
  `ADR-0010` (one `ConnectionStore` per `serve` call — the architecture
  Part B extends), `ADR-0037`/`ADR-0039` (`SchemaDrivenClient::traverse`
  — the client-side, N-round-trip composition Part A replaces for the
  one-hop case), `ADR-0042` (the source reading that established what
  `rusty_remind_me` actually joins), `docs/design/GENERIC-SCHEMA-
  DESIGN.md` §4.3 (`Reversed` — why the parent record type is not
  readable today), `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`
  (the two-part, two-ADR precedent this document follows).
- Supersedes: none. Additive — every existing `Request`/`Response`
  variant's bytes, every blob, and every file are unchanged under either
  part.

## Purpose and scope

The owner's ordered pick "2," the last of the four: "arbitrary joins /
real SQL." `docs/FUTURE-GROWTH.md` describes it as "SQL lets you join
any two tables on any predicate at query time; nothing like that exists
here," and files it under the multi-year "path to SQLite/DuckDB parity."
Two accepted rounds (`ADR-0034`, `ADR-0035`) declined it with one
sentence each, and that sentence is the real constraint: a connection
serves one `ConnectionStore`, so **there is nothing to join against.**

This document does what every round in this line does first: reads the
merged shape and the motivating consumer, and asks what a join would
actually have to be here. Two findings reframe the ask:

1. **Every join `rusty_remind_me` runs is a relation join, not an
   arbitrary one.** Nine `JOIN`s in `crates/remind_me_core/src/
   entity.rs` and `contradictions.rs`; every `ON` clause is a foreign
   key or a link table (`me.memory_id = m.id`, `s.id = r.subject_entity_
   id`, `me2.entity_id = me1.entity_id`). Not one joins two tables on a
   computed predicate. The "arbitrary" in "arbitrary joins" is a
   capability nobody in the consumer landscape uses; the *relation* join
   is what they use nine times.
2. **This crate already has the relations — as traversal primitives.**
   `ChildOf` (`Order → Customer`, `Employee → Employee`), `Symmetric
   Relation` (`Dog`, `Employee`, `Entity` twice), `MultiNeighbors` with
   named labels. `SchemaDrivenClient::traverse` (ADR-0037) already
   composes a multi-hop walk client-side over `Neighbors`. A one-hop
   relation join is that walk with the *rows* attached — today N+1
   round trips (`traverse`, then a `GetById` per id), which is exactly
   `rusty_remind_me`'s `traverse_entities` query (`entity_relations r
   JOIN entities s … JOIN entities o …` returning subject and object
   *names*) done the slow way.

So the round splits into two parts with different prerequisites:

- **Part A (`ADR-0044`) — a relation self-join.** `SELECT a.label,
  b.label FROM entity a JOIN entity b ON relates_to` — one table, one
  declared relation, one round trip, evaluated server-side over the
  adapter's existing relation methods. Needs no table concept, so it is
  buildable against every domain today: `Entity` (two symmetric
  labels), `Dog` (`littermate_of`), `Employee` (`reports_to` in both
  directions, `collaborates_with`). Answers the consumer's real
  one-hop-with-names query in one round trip.
- **Part B (`ADR-0045`) — more than one table per connection.** The
  prerequisite for a *cross-type* relation join (`Order → Customer`) and
  for anything resembling the consumer's `memory_entities` link-table
  joins. Requires a table concept on the wire and a real second store
  behind it — and this document is explicit that the second store does
  not exist today (see "Context"), so Part B is proposed as the
  *direction* Part A's wire shape must leave room for, with its
  implementation gated on a second table anyone actually needs.

## Non-goals

- **Not arbitrary (theta) joins.** No `JOIN … ON a.x = b.y` over
  arbitrary columns, no cross joins, no join on a computed predicate.
  That is `docs/FUTURE-GROWTH.md`'s multi-year item — it needs a
  planner to be anything but O(n·m), and the consumer never does it.
  Named, declined with reasons in "Considered options."
- **Not a query planner or optimizer.** Part A's evaluation strategy is
  fixed — index nested loop over the relation — and stated. There is no
  cost model, no join reordering, no choice of strategy.
- **Not multi-way joins.** Exactly two sides. `a JOIN b JOIN c` is a
  later extension whose wire shape (a `Vec` of hops) Part A's request
  does *not* pre-empt — named as the obvious next step, not built.
- **Not `GROUP BY`/aggregates over a join**, no `HAVING`, no `ORDER
  BY`, no `DISTINCT`, no `OR`. Each is a `SELECT`/`Aggregate` non-goal
  today and stays one.
- **Not writes through a join.** Read-only, like `Query`/`Aggregate`.
- **Not session overlay or read-set tracking for joins** — the identical
  line `SQL-FR-009` and `AGG-FR-010` draw for `Query`/`Aggregate`.
- **Not a `Customer` store, a `Memory` domain, or any new domain.** Part
  B defines how a second table would be served; it does not create one.
- **Not `LEFT`/`RIGHT`/`FULL OUTER JOIN`.** Inner join only: a left row
  with no related right row produces no output row. `rusty_remind_me`'s
  one `LEFT JOIN` (`list_entities`'s mention count) is an aggregate over
  a join, already excluded above.

## Context and terminology

Everything below was read from `main` at `6fd4783` and the
`rusty_remind_me` clone at `29602f1`, not assumed.

### What a connection can see

- `serve<S: ConnectionStore>(listener, Arc<S>, options)` — one adapter
  per server process (`src/server/mod.rs:2202`). `DomainSchema { fields,
  relations: RelationCapabilities { parent_children, neighbors } }`
  describes exactly one record type. `Request::Query { select, filter,
  limit }` and `Request::Aggregate` name no table; `FieldRef` is a `u16`
  meaningful only within the connection's one domain.
- The SQL front end (`src/server/sql.rs`, 765 lines, client-side only)
  parses `FROM ident` and carries it as `ParsedQuery::table` — "never
  validated against anything" (its own doc). No aliases, no
  qualification, no `JOIN` keyword.
- **The two-type domain exposes one type.** `Order`/`Customer`: `Parent`
  on an `Order` id returns a `Customer` id; nothing can then *read* that
  customer. `Customer { id, name }`'s `name` never crosses the wire —
  and not because of a protocol gap: `Reversed<S, P, C, Marker>` holds
  `inner: S` and `children_of: HashMap<P::Id, Vec<C::Id>>` only
  (`src/generic/store.rs:1036`). It implements `GetById<C>` (the child),
  not `GetById<P>`. **No `Customer` record is stored anywhere in the
  production stack.** A `Customer` table needs a store, not just a wire
  variant.
- `Employee` is the self-referential directed case: `ChildOf<ReportsTo>`
  with `ParentId = Uuid` — an employee's manager is an employee, so
  `parent`/`children` ids *are* readable rows of the same table.
- Relation methods every adapter already has (`ConnectionStore`):
  `parent(id) -> ParentLookup`, `children(id) -> Vec<RecordId>`,
  `neighbors(id)`, `neighbors_by_relation(id, label)`,
  `list_relation_kinds()`. These are the join's inner loop, unchanged.

### What the consumer joins

From `crates/remind_me_core/src/entity.rs` and `contradictions.rs`:

| Query | Shape | Join key |
|---|---|---|
| entity profile: linked memories | `memory_entities me JOIN memories m ON m.id = me.memory_id WHERE me.entity_id = ?` | link table → FK |
| entity profile: memory count | same, `count(*)` | link table → FK |
| `list_entities` | `entities e LEFT JOIN memory_entities me ON me.entity_id = e.id GROUP BY e.id` | FK, aggregate |
| `traverse_entities` | `entity_relations r JOIN entities s ON s.id = r.subject_entity_id JOIN entities o ON o.id = r.object_entity_id WHERE subject/object IN (frontier)` | relation → both endpoints |
| contradiction pairs | `memory_entities me1 JOIN memory_entities me2 ON me2.entity_id = me1.entity_id …` | link-table self-join |
| shared entities | `memory_entities me1 JOIN me2 … JOIN entities e ON e.id = me1.entity_id` | link table → FK |

Every `ON` is an equality on a declared key. The fourth row is Part A
exactly: the relation table joined to the entity table on both
endpoints, returning the endpoints' names — what `MultiSymmetric`
stores as adjacency and `traverse` returns as bare ids. The others all
involve `memories`, a type this crate does not model (`Reminder` is a
different, smaller record — `SERVER-REMINDER-DOMAIN-DESIGN.md` names
"no schema-less memory content" as a Non-goal). They are what Part B is
*for*, and why Part B has no first instance yet.

### Terminology

- **Relation join**: an inner join whose `ON` names a *declared
  relation* of the left table (a symmetric label, `parent`, or
  `children`) rather than a column predicate. The join key is the
  relation's own index.
- **Index nested loop**: for each left row passing the left filter,
  look up its related ids through the relation (O(1) per row — every
  relation here is a hash index), fetch each right row by id, apply the
  right filter, emit the pair. Cost O(|L| + Σ degree) `get`s — the
  identical work `traverse` does today across N+1 round trips.
- **Table**: a `ConnectionStore` served under a name on a connection
  that may serve several. Today every connection has exactly one,
  unnamed.

## Part A — a relation join within one table (`ADR-0044`)

### Requirements

- `JOIN-FR-001` — **`Request::Join`**, protocol 12, appended at index
  19:
  ```rust
  pub struct JoinSpec {
      /// Which declared relation of the left row yields right ids.
      pub relation: JoinRelation,
      /// `None` = the connection's own table (Part A). `Some(name)` =
      /// a Part B table; `Malformed` until Part B lands. Present from
      /// day one so Part B never needs a second `Join` variant.
      pub right_table: Option<String>,
      pub left: Selection,
      pub right: Selection,
      pub left_filter: Vec<Predicate>,
      pub right_filter: Vec<Predicate>,
      pub limit: Option<usize>,
  }
  pub enum JoinRelation {
      /// Every symmetric relation (`neighbors`), or one named label.
      Neighbors(Option<String>),
      /// The left row's parent (`parent`) — one right row at most.
      Parent,
      /// The left row's children (`children`).
      Children,
  }
  Request::Join(JoinSpec)                                   // index 19
  Response::JoinedRows { rows: Vec<JoinedRow> }             // index 15
  pub struct JoinedRow {
      pub left_id: RecordId,
      pub left: Vec<(FieldRef, ScanValue)>,
      pub right_id: RecordId,
      pub right: Vec<(FieldRef, ScanValue)>,
  }
  ```
  `Selection`, `Predicate`, `FieldRef`, `ScanValue` reused unchanged.
  `Malformed` below 12 (rule 3); sent only after negotiating ≥ 12 (rule
  4). `JoinedRows` only ever answers `Join`, so no `downgrade_for_
  version` arm is needed — the `Query`/`Rows` precedent, not the
  `StrList` one.
- `JOIN-FR-002` — **`Request::DescribeRelations`** (index 20) →
  **`Response::Relations { relations: Vec<RelationDescriptor> }`**
  (index 16):
  ```rust
  pub struct RelationDescriptor {
      /// `"neighbors"`, a symmetric label (`"relates_to"`), `"parent"`,
      /// or `"children"` — what `ON` names.
      pub name: String,
      pub kind: JoinRelation,
      /// `None` = the related rows are this table's own (self-join
      /// legal). `Some(table)` = they live in another table (Part B);
      /// a `Join` on such a relation with `right_table: None` is
      /// `Unsupported`.
      pub target_table: Option<String>,
  }
  ```
  `DomainSchema` is a struct and cannot grow a field (`STORAGE-018`
  evolution rules), so relation detail is a new request, not a new
  schema field — the same reason `ListRelationKinds` (ADR-0039) was a
  request. `ConnectionStore` gains `fn describe_relations(&self) ->
  Vec<RelationDescriptor>` with a **default implementation derived from
  `describe()` and `list_relation_kinds()`**: `neighbors` plus one entry
  per label when `relations.neighbors`; `parent` and `children` with
  `target_table: Some("<unknown>")`-equivalent — concretely, `parent`/
  `children` are *omitted* from the default — when `relations.parent_
  children`, because the default cannot know whether the parent type is
  the same table. `Employee` overrides to list them with `target_table:
  None`; `Order` keeps the default (its parent is a `Customer`, not an
  `Order` row). An adapter that lies here produces wrong rows, the same
  trust `describe()` already carries (`FR-010`).
- `JOIN-FR-003` — **Server-side evaluation, index nested loop, in
  `dispatch`**: validate both `Selection`s and both filters against the
  schema exactly as `validate_query` does; validate `relation` against
  `describe_relations()` (`Malformed` if the name is not listed,
  `Unsupported` if it is listed with a `target_table` that is not this
  table and `right_table` is `None`); then for each `(id, fields)` in
  `scan_all()` passing `left_filter`, obtain right ids — `neighbors(id)`
  / `neighbors_by_relation(id, label)` / `parent(id)` (`Parent(pid)`
  → `[pid]`, `NoParent`/`ChildNotFound` → `[]`) / `children(id)` — and
  for each right id `get(right_id)`, apply `right_filter`, project both
  sides, push a `JoinedRow`. `limit` truncates the pair count after
  evaluation (the "bounds the response, not the work" posture `Query`
  has). Rows come out in `scan_all` order then relation order —
  unspecified, like every other row-returning response.
- `JOIN-FR-004` — **Symmetric relations produce both orientations.**
  `a JOIN b ON relates_to` yields `(x, y)` and `(y, x)` for one
  undirected edge — SQL semantics (each left row is joined), and what
  `traverse`'s callers already see from `neighbors`. Named, not
  de-duplicated; a caller wanting each edge once filters `left_id <
  right_id` client-side or waits for `DISTINCT`, a non-goal.
- `JOIN-FR-005` — **SQL grammar** (`src/server/sql.rs`), additive:
  ```text
  query      := "SELECT" columns "FROM" table_ref [join_clause] [where_clause] [group_by_clause] [limit_clause]
  table_ref  := ident [["AS"] ident]                       -- optional alias
  join_clause := "JOIN" table_ref "ON" ident               -- ident = a relation name
  column_item := qualified | ident | agg_call
  qualified  := ident "." ident                            -- alias.field
  condition  := (qualified | ident) comparator literal
  ```
  `JOIN` with `GROUP BY` or any `agg_call` is a parse-time error
  ("aggregation over a join is not supported"). Without `JOIN`, a
  qualified name whose alias is the `FROM` alias resolves as before; an
  unknown alias is a parse error. With `JOIN`, every column and
  condition **must** be qualified (no ambiguity resolution — both sides
  have the same field names in Part A). `FROM x JOIN x` with the same
  table name is the only legal form in Part A; a different right table
  name is `ClientError::Sql` until Part B (`right_table` is populated
  from it then).
- `JOIN-FR-006` — **Client compilation and result shape**:
  `SchemaDrivenClient::query` compiles a `JOIN` query to `Request::
  Join`, resolving each side's names against the *same* schema (Part
  A) and the `ON` name against a `DescribeRelations` fetched once at
  connect (alongside `DescribeSchema`, only when the negotiated version
  is ≥ 12); `WHERE` conditions are routed to `left_filter`/`right_
  filter` by their alias. Returns a new **`QueryResult::Joined(Vec<
  JoinedRowNamed>)`** where `JoinedRowNamed { left_id, right_id,
  fields: Vec<(String, ScanValue)> }` and every name is alias-qualified
  (`"a.label"`, `"b.label"`) in `SELECT`-list order — a third variant
  of the existing public enum, matching how `Groups` was added in
  `AGG-FR-003`. Below protocol 12, a `JOIN` query is `ClientError::
  Unsupported("sql join")` with no frame sent (rule 4), the identical
  gate `query_aggregate` has for version 9.
- `JOIN-FR-007` — **No new `ErrorCode`.** Unknown alias/field/relation
  → `UnknownField`/`Malformed`; a relation whose target is another
  table with `right_table: None` → `Unsupported`; `right_table: Some(_)`
  in Part A → `Malformed`; a `StrList`-kinded predicate → `Malformed`
  (existing `value_matches_kind`). `Join`/`DescribeRelations` are
  read-only, gated exactly like `Query` (authentication only).
- `JOIN-FR-008` — **Golden vectors and pins**: `Join`, `Describe
  Relations`, `JoinedRows`, `Relations` each pinned; `PROTOCOL_VERSION`
  12; table row 12; the three hardcoded pins moved.

### Considered options

**Fork 1 — where the join runs.**

- **(a) (proposed)** Server-side, one round trip, index nested loop
  over the adapter's existing relation methods. The relation index is
  already O(1) per lookup; the work is Σ degree `get`s, which is what
  the client would otherwise issue as N round trips.
- (b) Client-side, like `traverse`: `Query` the left side, then a
  `GetById` per related id. Zero wire change. Rejected as the *only*
  path: it is what exists today (`traverse` + `get`), it costs one
  round trip per right row, and it cannot apply a right-side filter
  before paying for the fetch. Named as the fallback a version-11
  client already has.
- (c) A generic server-side `Request::Batch` of `GetById`s to amortize
  round trips. Rejected: solves the round-trip cost but not the
  right-filter-before-fetch cost, and adds a primitive nothing else
  wants.

**Fork 2 — what `ON` may name.**

- **(a) (proposed)** A declared relation only (`neighbors`, a label,
  `parent`, `children`). The join key *is* the relation's index; no
  predicate evaluation on the join key at all.
- (b) Any `a.field = b.field` equality (an equi-join). Rejected for
  this round: without an index on `b.field` it is O(|L|·|R|) or a
  server-side hash build per query; the only indexed fields are the
  one `IndexedField` per domain and `NameIndex`. A bounded later round
  could allow `ON a.x = b.<indexed field>` via `filter_eq` — named, not
  built. The consumer never issues one.
- (c) Arbitrary predicates (theta joins). Rejected — `docs/FUTURE-
  GROWTH.md`'s own multi-year item; needs a planner to be sane.

**Fork 3 — result shape.**

- **(a) (proposed)** `Response::JoinedRows` with two ids and two field
  lists per row; the client qualifies names by alias. Honest about the
  two-record nature of a joined row; `RecordId` stays the row key on
  both sides.
- (b) Reuse `Response::Rows` with the left id as row id and the right
  fields appended under synthetic tags. Rejected: loses the right id,
  and synthetic `FieldRef`s collide with real ones.
- (c) Return only right rows (`SELECT b.* FROM a JOIN b …`), i.e. a
  filtered `Neighbors`. Rejected: that is a projection of (a), and the
  consumer's query wants both endpoints' names.

**Fork 4 — how the server learns which relations are self-joinable.**

- **(a) (proposed)** `ConnectionStore::describe_relations()` with a
  conservative default (symmetric labels listed; `parent`/`children`
  omitted unless the adapter says they target the same table) and a
  `DescribeRelations` request so the client can validate `ON`
  client-side. `Employee` overrides; `Order` does not.
- (b) Infer at `Join` time: `get(parent_id)` returning `None` means
  "other table." Rejected: indistinguishable from a dangling parent id
  (which `Reversed` tolerates), so it would silently return zero rows
  for a wrong query.
- (c) Grow `RelationCapabilities`. Rejected: a struct field is a format
  change for every `Schema` response ever pinned.

### Proposed shape

```sql
-- Entity: the consumer's traverse-with-names, one round trip
SELECT a.label, b.label FROM entity a JOIN entity b ON relates_to WHERE a.kind = 'person'
-- Entity: every relation at once
SELECT a.label, b.label, b.kind FROM entity a JOIN entity b ON neighbors LIMIT 100
-- Employee: manager's name next to each report (directed, self-referential)
SELECT e.name, m.name FROM employee e JOIN employee m ON parent
SELECT m.name, e.name FROM employee m JOIN employee e ON children WHERE e.salary > 100000
-- Dog: littermates
SELECT a.breed, b.age FROM dog a JOIN dog b ON littermate_of WHERE a.age < 2
```

```rust
// src/server/mod.rs — dispatch
Request::Join(spec) => match validate_join(&store.describe(), &store.describe_relations(), &spec) {
    Ok(()) => Response::JoinedRows { rows: evaluate_join(store, &spec) },
    Err(code) => err_response(code),
},
Request::DescribeRelations => Response::Relations { relations: store.describe_relations() },

fn evaluate_join<S: ConnectionStore + ?Sized>(store: &S, spec: &JoinSpec) -> Vec<JoinedRow> {
    let mut out = Vec::new();
    for (left_id, left_fields) in store.scan_all() {
        if !spec.left_filter.iter().all(|p| predicate_matches(&left_fields, p)) { continue; }
        let right_ids: Vec<RecordId> = match &spec.relation {
            JoinRelation::Neighbors(None) => store.neighbors(left_id).unwrap_or_default(),
            JoinRelation::Neighbors(Some(label)) => store.neighbors_by_relation(left_id, label).unwrap_or_default(),
            JoinRelation::Parent => match store.parent(left_id) { Ok(ParentLookup::Parent(p)) => vec![p], _ => vec![] },
            JoinRelation::Children => store.children(left_id).unwrap_or_default(),
        };
        for right_id in right_ids {
            let Some(right_fields) = store.get(right_id) else { continue };
            if !spec.right_filter.iter().all(|p| predicate_matches(&right_fields, p)) { continue; }
            out.push(JoinedRow {
                left_id, left: select_fields(left_fields.clone(), &spec.left),
                right_id, right: select_fields(right_fields, &spec.right),
            });
            if spec.limit.is_some_and(|n| out.len() >= n) { return out; }
        }
    }
    out
}
```

(`unwrap_or_default` on a relation error is safe only because
`validate_join` has already refused a relation the adapter does not
list; the `Err` arms are unreachable through `dispatch` and kept as a
non-panicking default, the `compare` precedent.) The early return on
`limit` is the one place a join bounds *work* as well as response —
deliberate, because the pair count is not knowable up front and the
consumer's `traverse` caps its own frontier the same way.

### Data/state and invariants

- No on-disk change. No change to any adapter's stored shape. The only
  adapter code is `describe_relations()` overrides where the default is
  wrong (`Employee`).
- Invariant: `JoinedRows` never appears except as `Join`'s answer, so
  no rule-3 content strip exists for it. A `StrList` may appear in
  either side of a `JoinedRow` (an `Entity` join carries `aliases`),
  and a connection below 12 cannot have sent a `Join`, so the `FR-044`
  strip is not needed here either — pinned by a test that a `Join` on
  `Entity` at 12 carries the list intact.
- Invariant: a join's row count is bounded by `Σ degree` over the
  filtered left side, and by `limit`; never by |L|·|R|.
- Invariant: `Join` never reaches the session intercepts in
  `handle_connection` (no overlay, no read-set), exactly as `Query`.

### Errors, failure, recovery, and observability

- Every rejection is one of the existing codes, listed in `JOIN-FR-007`.
- The access log records `Join`/`DescribeRelations` as their own request
  kinds (`access::RequestKind` gains two variants — an internal enum,
  not wire), with the same `Ok`/`Err(code)` outcome shape.
- A join over a large, dense relation is expensive by construction
  (Σ degree `get`s). `limit` bounds it; there is no timeout, matching
  `Query`'s own posture. Named.

### Security, privacy, and compatibility

- Read-only; authentication-gated like `Query`; no new class needed.
- Protocol 11 → 12, rules 1–4 applied: two appended requests, two
  appended responses, no existing byte changed; `Malformed` below 12;
  the client never sends either below 12. `SERVER-002` (when it lands
  per `ADR-0043`) gains the four shapes and a table row.
- Every existing test, client, and bench is unchanged.

### Acceptance criteria (Part A)

1. `Request::Join`/`DescribeRelations` at 19/20, `Response::JoinedRows`/
   `Relations` at 15/16; `PROTOCOL_VERSION == 12`; table row 12; every
   pre-existing golden vector unchanged; four new vectors.
2. Over a real socket against `entity_server`'s fixture: `SELECT
   a.label, b.label FROM entity a JOIN entity b ON relates_to` returns
   exactly the `relates_to` edge set in both orientations with the
   right labels; `ON mentioned_with` returns the disjoint edge; `ON
   neighbors` returns the union; a `WHERE a.kind = 'person'` halves it
   accordingly; a `WHERE b.mention_count > 4` filters the right side;
   `LIMIT 2` returns two rows; `aliases` rides in the row as a
   `StrList`.
3. Against `Employee` (`research`): `ON parent` returns each report
   paired with its manager (an employee with no manager produces no
   row); `ON children` returns the inverse pairs; `ON collaborates_with`
   returns the symmetric pairs.
4. Against `Order`: `DescribeRelations` lists no `parent`/`children`
   (the default omits them — the parent is a `Customer`); `SELECT …
   FROM order a JOIN order b ON parent` is `ClientError::Sql` client-
   side (the relation is not listed) and a raw `Request::Join` with
   `Parent` is `Malformed` server-side.
5. Against `Dog`: `ON littermate_of` works; against `Reminder`
   (`relations.neighbors: false`, no parent): `DescribeRelations` is
   empty and every `Join` is `Malformed`.
6. A `JOIN` with `GROUP BY` or an aggregate, an unqualified column in a
   `JOIN` query, an unknown alias, and a different right table name are
   each `ClientError::Sql` with no frame sent; a `JOIN` query on a
   connection negotiated at 11 is `ClientError::Unsupported("sql join")`
   with no frame sent; a raw `Join` on a `Hello { 11 }` connection is
   `Malformed` server-side.
7. `traverse` is unchanged and still passes every existing test — Part
   A adds to it, it does not replace it.

### Verification plan (Part A)

- `src/server/protocol.rs`: four golden vectors, the pin, round-trip
  tests for the new shapes.
- `src/server/mod.rs`: `validate_join`/`evaluate_join` unit tests
  against the fixture store (each `JoinRelation`, both filters, `limit`,
  both orientations of a symmetric edge, the unlisted-relation and
  other-table refusals).
- `src/server/sql.rs`: grammar tests — aliases, qualified names, `JOIN
  … ON`, every rejection in criterion 6.
- `src/server/client.rs`: compilation to `Request::Join`, `QueryResult::
  Joined` naming, the version gate.
- `tests/server_entity_integration.rs`, `tests/server_employee_
  integration.rs`, `tests/server_dog_integration.rs`, `tests/server_
  reminder_integration.rs`, `tests/server_protocol_version.rs`: criteria
  2–7 over real sockets, including the hand-negotiated `Hello { 11 }`.

## Part B — more than one table on one connection (`ADR-0045`)

### Requirements

- `TBL-FR-001` — **`serve_tables`**: `pub fn serve_tables(listener,
  tables: Vec<(String, Arc<dyn ConnectionStore>)>, primary: usize,
  options)`; `serve` becomes `serve_tables` with one table named by
  `store.table_name()` (a new `ConnectionStore` method with a default
  of the adapter's domain name — `"dog"`, `"entity"`, …). Every
  existing call site is unchanged.
- `TBL-FR-002` — **Per-connection table selection**: `Request::Use {
  table: String }` (next free index) answered `Response::Ok`, or `Err {
  Malformed }` for an unknown name; state kept per connection exactly
  like `negotiated`/`authenticated`/`session` in `handle_connection`;
  default = `primary`. Every table-less request (`GetById`, `Query`,
  `DescribeSchema`, `Join` with `right_table: None`, …) routes to the
  selected table. **A connection that never sends `Use` behaves exactly
  as today** — this is what keeps every existing client working
  (`Hello`/`Authenticate`'s per-connection-state precedent).
- `TBL-FR-003` — **`Request::ListTables`** → `Response::Tables { names:
  Vec<String>, primary: String }`.
- `TBL-FR-004` — **Cross-table `Join`**: `JoinSpec::right_table:
  Some(name)` fetches right rows from that table's adapter; the
  relation's `RelationDescriptor::target_table` must equal it
  (`Malformed` otherwise). The relation is still the *left* adapter's
  (`parent(id)` on `Order` yields a `Customer` id); the right adapter
  only needs `get`. `Order`'s `describe_relations()` then lists
  `parent` with `target_table: Some("customer")`.
- `TBL-FR-005` — **A real second store for the first instance.** `Reversed`
  holds no parent records, so `Order → Customer` requires a
  `CustomerConnectionStore` over a `GenericMmapStore<Customer, …>` (or
  a read-only in-memory adapter over the `Vec<Customer>` the fixture
  already has, for the `research` domain). This is the part with no
  motivating instance today: `Customer` is research-gated reference
  material, and the consumer's second table (`memories`) is a domain
  this crate does not model. Part B is therefore proposed as the wire
  and server shape that Part A must stay compatible with (`right_table`
  and `target_table` exist from day one), with its implementation
  **gated on a second table someone needs** — named, not scheduled.
- `TBL-FR-006` — **Sessions are per connection, not per table.** A
  `Begin` … `Commit` with a `Use` in between stages writes against two
  tables in one batch; Part B either forbids `Use` inside a session
  (`SessionOpen`) or makes the journal multi-table. Forbidding is
  proposed; the alternative is named.

### Considered options

- **(a) (proposed)** Per-connection `Use` — zero change to any existing
  request, one new state variable, the `Hello`/`Authenticate`/session
  precedent. A client that wants two tables at once opens two
  connections (cheap; the thread-per-connection model already assumes
  it) or `Use`s back and forth.
- (b) A table field on every request. Rejected outright: a struct-field
  or variant-field change to every pinned request — the one thing rule
  1 forbids.
- (c) Table-qualified duplicates of every request (`QueryIn { table,
  .. }`, `GetByIdIn { .. }`, …). Rejected: doubles the request surface
  for a state that is naturally per-connection.
- (d) One server process per table, joins federated client-side.
  Rejected as the *design*: it is what exists today; the consumer's
  joins would each cost N round trips across two sockets.
- (e) Defer Part B entirely with no ADR. Rejected in favor of an ADR
  that records the shape and the gate — otherwise Part A's `right_table`
  and `target_table` fields have no written justification.

### Proposed shape

```rust
// handle_connection — one more per-connection variable
let mut table: usize = primary;
// …
Request::Use { table: name } => match tables.iter().position(|(n, _)| *n == name) {
    Some(i) if session.is_none() => { table = i; Response::Ok }
    Some(_) => err_response(ErrorCode::SessionOpen),
    None => err_response(ErrorCode::Malformed),
},
Request::ListTables => Response::Tables { names: tables.iter().map(|(n, _)| n.clone()).collect(), primary: tables[primary].0.clone() },
other => dispatch_in(&tables, table, other),   // today's `dispatch(store, other)` with the table resolved
```

### Data/state and invariants

- No on-disk change. A table is an adapter; adapters are unchanged.
- Invariant: a connection with no `Use` is byte-for-byte today's
  connection.
- Invariant: `Join` never crosses tables unless the relation descriptor
  says it does.

### Errors, failure, recovery, and observability

- Unknown table → `Malformed`; `Use` inside a session → `SessionOpen`
  (proposed) — no new `ErrorCode`. The access log records the table
  name in the event (an internal field).

### Security, privacy, and compatibility

- One `ServeOptions` (tokens, TLS, rate limit, logs) per server, shared
  by every table — the authentication gate is per connection, before
  any `Use`. Per-table authorization is a non-goal; named.
- Protocol: `Use`/`ListTables`/`Tables` appended at the next free
  indices when built, its own version-table row; `Malformed` below it.

### Acceptance criteria (Part B, when built)

1. `serve` still works unchanged; `serve_tables` with two adapters
   serves both; a connection with no `Use` sees the primary.
2. `Use` switches every subsequent table-less request; `ListTables`
   lists both; an unknown name is `Malformed`; `Use` inside a session
   is `SessionOpen`.
3. `SELECT o.amount, c.name FROM order o JOIN customer c ON parent`
   returns each order with its customer's name in one round trip.
4. Every existing test, bench, and bin is unchanged.

### Verification plan (Part B, when built)

- The same suites as Part A plus a two-table server fixture in
  `tests/server_order_integration.rs` (`research`), and the
  `Hello { <prior> }` proof that `Use` is `Malformed` below its version.

## Traceability

- Part A → `SERVER-001` next minor / FR (`JOIN-FR-001`–`008`),
  `ADR-0044`. Part B → a later `SERVER-001` minor / FR (`TBL-FR-001`–
  `006`), `ADR-0045`, gated as stated.
- Answers `ADR-0035`'s revisit trigger ("multi-table joins are ever
  taken up") and reframes `docs/FUTURE-GROWTH.md`'s "arbitrary joins":
  the relation join is built; the arbitrary join is declined with
  reasons and stays that document's multi-year item.
- Sourced from `docs/FUTURE-GROWTH.md`, the owner's ordered pick "2" —
  the fourth and last of the ordered queue.

## Open questions

- Multi-way joins (`a JOIN b ON r1 JOIN c ON r2`): a `Vec<JoinHop>`
  request is the obvious shape; Part A's two-sided `JoinSpec` does not
  pre-empt it (a new variant, rule 1). Named.
- Equi-joins on an indexed field (`ON a.x = b.<IndexedField>`) via
  `filter_eq` — bounded, indexed, not arbitrary. Named for a later
  round if a consumer needs a join that is not a declared relation.
- `DISTINCT` (or an `a.id < b.id` idiom) to see each symmetric edge
  once. Named.
- Whether `traverse` should be re-expressed as repeated `Join`s
  server-side (a multi-hop join is a traversal). Not this round; the
  client-side walk stays.
- Part B's first instance: `Order → Customer` (needs a `Customer`
  store, research-gated) or a future `Memory` domain (front-door, but a
  domain design round of its own). The owner's call; neither is
  scheduled here.

## Change history

- 2026-09-05: Initial proposal, the owner's ordered pick "2" (arbitrary
  joins / real SQL JOIN) — the last of the four-item ordered queue.
  Reframed from "arbitrary joins" to "relation joins first, tables
  second" after reading the consumer's nine real `JOIN`s (all relation
  joins) and the crate's own two-type domain (the parent type is not
  stored, so a second table needs a store, not just a wire variant).
  Two parts, two ADRs, the `SERVER-TRANSACTION-SESSION-DESIGN`
  precedent; theta joins declined with reasons.
- 2026-09-05: **Accepted**, both parts as proposed (`ADR-0044` option
  (a), `ADR-0045` option (a) — gated direction). Stays design-only;
  Part A's implementation follows as `SERVER-001`'s next minor / FR
  (protocol 12), Part B's is gated on a second table.
- 2026-09-05: **Part A implemented** (`SERVER-001-FR-045`, v0.35.0),
  landed as this document's "Proposed shape" sketched it — see
  `ADR-0044`'s implementation log for the two consequences recorded
  (the `QueryResult` third-variant cost this document named; four
  unplanned version pins in the previous unit's tests). Acceptance
  criteria 1–7 (Part A) hold; the verification plan ran as written in
  every suite it named. Part B stays gated per `ADR-0045`.
