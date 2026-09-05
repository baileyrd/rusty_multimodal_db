# ADR-0045: More than one table on one connection — `Request::Use`, `serve_tables`, cross-table `Join`

- Status: **Proposed** — awaiting the owner's decision; options (a)–(c)
  below. Part B of `docs/design/SERVER-SQL-JOIN-DESIGN.md`; independent
  of `ADR-0044`'s acceptance, but its cross-table half presupposes
  `Request::Join`.
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-SQL-JOIN-DESIGN.md` (the full design),
  `ADR-0010` (one `ConnectionStore` per `serve` call — the architecture
  this extends rather than replaces), `ADR-0044` (Part A; its
  `right_table`/`target_table` fields exist for this ADR), `ADR-0022`
  (per-connection state precedent: `Hello`'s negotiated version,
  `Authenticate`'s class, the session), `docs/design/GENERIC-SCHEMA-
  DESIGN.md` §4.3 / `src/generic/store.rs` `Reversed` (why the parent
  record type is not readable today), `ADR-0036`/`SERVER-REMINDER-
  DOMAIN-DESIGN.md` ("no schema-less memory content" — why the
  consumer's second table does not exist here).

## Context

Every join round in this crate has stopped at the same sentence: a
connection serves exactly one domain, so there is no second table to
join against. `ADR-0044` shows that a *relation join within one table*
needs no second table and delivers the consumer's one-hop query. This
ADR is about the other half — the cross-type joins (`Order → Customer`,
and every `memory_entities` join `rusty_remind_me` runs).

Two facts bound it. First, the wire has no table concept: `Request::
Query`, `GetById`, `DescribeSchema` name no table, and rule 1 forbids
adding a field to any of them. Second — and this is the finding of the
round — **the two-type domain does not store its second type.**
`Reversed<S, P, C, Marker>` holds `inner: S` and `children_of:
HashMap<P::Id, Vec<C::Id>>`; it implements `GetById<C>` for the child,
never `GetById<P>`. `Customer { id, name }` exists only in the fixture's
`Vec<Customer>` at construction; no store, no blob, no adapter. A
`Customer` table is a new store, not a new wire variant. And the
consumer's second table, `memories`, is a domain this crate has
explicitly not modeled.

So Part B has a clean shape and no instance. This ADR records the shape
so that Part A's wire is a pure append when an instance appears, and
gates the implementation on that instance.

## Decision

Adopt Part B of the design document as the **direction**, with
implementation gated on a second table someone needs:

- **`serve_tables(listener, tables: Vec<(String, Arc<dyn ConnectionStore>)>,
  primary, options)`**; `serve` becomes the one-table case, every call
  site unchanged. `ConnectionStore` gains `table_name()` with a default.
- **`Request::Use { table }`** — per-connection table selection,
  answered `Ok` or `Malformed`; `SessionOpen` inside a session. State
  kept in `handle_connection` beside `negotiated`/`authenticated`/
  `session`; default = primary. **A connection that never sends `Use`
  is byte-for-byte today's connection.** Every table-less request
  routes to the selected table.
- **`Request::ListTables`** → `Response::Tables { names, primary }`.
- **Cross-table `Join`**: `JoinSpec::right_table: Some(name)` fetches
  right rows from that table's adapter; the relation stays the left
  adapter's; `RelationDescriptor::target_table` must match. `Order`
  then lists `parent` with `target_table: Some("customer")`.
- One `ServeOptions` per server shared by every table; per-table
  authorization is a non-goal.
- Its own protocol version and table row when built; `Malformed` below
  it; the client never sends `Use`/`ListTables` below it.

**Gate**: no implementation unit is scheduled until one of (i) a
`Customer` store exists (research-gated; a read-only in-memory adapter
over the fixture's `Vec<Customer>` would suffice for the reference
domain), or (ii) a front-door second table is designed (a `Memory`
domain is the obvious candidate and a domain round of its own).

## Consequences

- Positive: the table concept is one per-connection variable and two
  appended requests — no existing request changes, the `Hello`/
  `Authenticate` precedent applied a third time. Every existing client,
  test, bench, and bin is untouched.
- Positive: `ADR-0044`'s `right_table`/`target_table` have a written
  reason to exist.
- Positive: a client wanting two tables at once opens two connections
  — cheap under thread-per-connection — or `Use`s back and forth.
- Named, not hidden: **no instance exists.** `Order → Customer` needs a
  store this crate has never had; `memories` needs a domain this crate
  has declined to model. Accepting this ADR schedules nothing; it
  records a shape.
- Named, not hidden: `Use` inside a session is proposed as `Session
  Open`; the alternative — a multi-table journal batch — is real, larger,
  and not needed by any instance.
- Named, not hidden: per-table authorization does not exist; a token's
  class applies to every table on the server.

## Considered options

- **(a) (proposed)** Per-connection `Use`, one state variable, two
  appended requests; implementation gated.
- (b) A table field on every request — rejected outright, rule 1.
- (c) Table-qualified duplicates of every request — rejected, doubles
  the surface for per-connection state.
- (d) One process per table, joins federated client-side — what exists
  today; the consumer's joins would cost N round trips across two
  sockets.
- (e) No ADR; defer silently — rejected, because then `ADR-0044`'s two
  `Option` fields have no justification on record.

## Acceptance and implementation

- Options offered at proposal: **(a) accept as direction, gated** — the
  shape above is the plan; `ADR-0044` carries `right_table`/`target_
  table` from day one; no implementation unit until an instance exists;
  **(b) accept and schedule the `Order → Customer` instance now** — a
  read-only `CustomerConnectionStore` over the fixture (`research`-
  gated), `serve_tables`, `Use`/`ListTables`, and the cross-table join
  test, as one unit after `ADR-0044`'s; **(c) decline** — drop
  `right_table`/`target_table` from `ADR-0044`'s shapes (they become
  a later variant if ever needed) and leave the one-table architecture
  as the design.
- Sizing: (a) none now; (b) about three days including the store and
  the two-table fixture.
