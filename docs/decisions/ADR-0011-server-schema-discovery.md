# ADR-0011: Add schema discovery (`DescribeSchema`) to the server/query-layer protocol

- Status: **Accepted**
- Date: 2026-08-31
- Deciders: baileyrd
- Related: `docs/decisions/ADR-0010-server-query-layer-proposal.md` (the
  server/query layer this amends), `docs/specifications/server/SERVER-001-query-layer.md`
  (bumped to 0.2.0 for this decision), `docs/design/SERVER-QUERY-LAYER-DESIGN.md`
  ("Field addressing" — the original considered-and-deferred option this
  revisits)
- Supersedes/Superseded by: none. Amends `SERVER-001`, not a new spec —
  this is an additive extension of the same server capability
  (`docs/specifications/SPEC-REGISTRY.md`'s own convention: "IDs remain
  stable... link superseding artifacts rather than reusing an ID for a new
  meaning" — extending, not superseding, so the ID stays `SERVER-001`).

## Context

`ADR-0010`'s original design considered string field names via a
schema-description sub-protocol and explicitly deferred it: "it needs a
real design for how a client discovers valid field names for a domain it
didn't compile against... which this proposal's non-goals exclude,"
choosing compile-time-fixed integer tags for v1 instead. `ADR-0010`'s own
"Validation and revisit triggers" named this as a live revisit trigger,
not a closed question: "Whether a schema-description RPC... is ever worth
building is explicitly deferred to a future decision, not ruled out
permanently."

The owner has now asked for it directly, after being offered a short list
of concrete next directions for the (already-implemented) server layer.
This ADR records that decision. Per `adr-cadence.md`'s Regime 1 ("active
major development... establishing or changing a public interface, data
format, or protocol" is an explicit trigger), this gets its own ADR —
but unlike `ADR-0009`/`ADR-0010`, it is not staged as a separate
design-only round: it is a bounded, additive extension of an
already-accepted, already-implemented protocol, not a new capability axis
comparable to "should this crate have a generic schema system" or "should
this crate have a network server" — so it follows this project's more
common pattern (`ADR-0001` through `ADR-0008`) of writing the ADR and
implementing it in the same delivery cycle.

## Decision drivers

- **Resolve the named revisit trigger, don't reopen the original
  decision.** Field *tags* remain the wire addressing scheme for
  `FilterEq`/`ScanField`/`UpdateField` — unchanged. This adds a way to
  *discover* what a compile-time client already knows, not a new
  addressing scheme, a query language, or a replacement for tags.
- **No new dependency, no framing change.** `DescribeSchema`/`Response::Schema`
  are ordinary additions to the existing `Request`/`Response` enums, carried
  over the existing length-prefixed `bincode` framing — nothing about the
  protocol's transport or encoding changes.
- **Prove discovery is actually usable, not just descriptive.** A schema
  response a client can't act on is decoration. The acceptance bar is a
  real client that starts with zero compile-time field-tag knowledge,
  calls `DescribeSchema`, and completes a real `UpdateField`/`FilterEq`
  using only what it discovered.

## Considered options

### Response shape: a flat field list vs. a structured `DomainSchema`

1. **A flat `Vec<(FieldRef, String)>`** (tag → name only). Rejected — a
   client would still have to guess a field's value type and which
   operations it supports, defeating the point of discovery.
2. **A structured `DomainSchema`** (`fields: Vec<FieldDescriptor>` — tag,
   name, `ValueKind`, and per-field `FieldCapabilities` — `filter_eq`/
   `scan`/`update` — plus `RelationCapabilities` — `parent_children`/
   `neighbors`). Chosen — lets a client construct any request this
   protocol supports without hardcoded per-domain knowledge, and directly
   exposes the real, sometimes-partial capability shape each domain
   already has (e.g. `Order::created_at_unix_ms` is a real, named field
   but supports none of the three operations, since it was never part of
   the durable stack the server wraps — see `SERVER-001`'s own
   non-goals). Reporting that partial shape honestly, rather than hiding
   unsupported fields from the schema, was itself a real choice: hiding
   them would make the schema simpler but would mean a client can't tell
   "this field doesn't exist" from "this field exists but you can't touch
   it," the same not-found/no-parent distinction this project already
   cares about for `Parent`.

### Request shape: a per-field lookup vs. one whole-domain call

1. **A per-field `DescribeField { name: String }` lookup.** Rejected — one
   server instance serves exactly one domain (no multi-domain routing
   exists or is planned), so there is no meaningful "look up one field
   without knowing the others" case; a client discovering a domain wants
   the whole shape at once.
2. **One `Request::DescribeSchema` (no arguments) returning the whole
   `DomainSchema`.** Chosen — matches the one-server-one-domain shape the
   rest of this protocol already assumes.

## Decision

- `Request::DescribeSchema` / `Response::Schema(DomainSchema)` added to
  `src/server/protocol.rs`. `DomainSchema { fields: Vec<FieldDescriptor>, relations: RelationCapabilities }`;
  `FieldDescriptor { tag, name, value_kind, capabilities }`.
- `ConnectionStore` gains one new, infallible method: `fn describe(&self) -> DomainSchema`.
  Both existing adapters implement it: `server::dog::DogConnectionStore`
  (2 fields — `breed` read-only, `age` scan+update; `neighbors: true`,
  `parent_children: false`) and `server::order::OrderConnectionStore` (4
  fields — `amount_cents` scan+update, `status` filter-only,
  `created_at_unix_ms`/`discount_cents` fully read-only;
  `parent_children: true`, `neighbors: false`).
- `dispatch` handles the new request with one new match arm; no change to
  any existing arm.
- Verified end to end, not just unit-tested: `tests/server_dog_integration.rs`/
  `tests/server_order_integration.rs` each gained a schema-driven test — a
  real client calls `DescribeSchema`, finds a field by name (`"age"` /
  `"status"`), and completes a real `UpdateField`/`FilterEq` using only the
  discovered tag, proving discovery is actually load-bearing.
- No new dependency. No framing/transport change. No existing `Request`/
  `Response` variant's shape changed.

## Consequences

### Positive

- Closes the one item `ADR-0010` explicitly left open as a named,
  non-permanent deferral, with a real, tested implementation rather than
  leaving it as a standing "someday" gap.
- A client library could now be schema-driven — build request/response
  handling generically from `DescribeSchema`'s answer instead of needing
  per-domain generated code, though no such client exists yet (this ADR
  adds the server-side capability, not a client library).
- The capability-per-field reporting (`FieldCapabilities`) surfaces a real
  asymmetry (`Order`'s `created_at_unix_ms`/`discount_cents` being
  fully read-only over the wire despite being real, named, in-memory
  `ScannableField`s) explicitly, rather than a client discovering it only
  by trial and error against `ErrorCode::Unsupported`.

### Negative / tradeoffs

- `DomainSchema`'s shape is itself now part of the wire protocol's
  compatibility surface — a future field-descriptor shape change is a
  breaking change for any schema-driven client, the same versioning gap
  `ADR-0010`'s own Consequences already named for the protocol as a whole
  (no version negotiation exists).
- Does not reduce the field-*tag* addressing this protocol already
  chose — a schema-driven client still issues `FilterEq`/`ScanField`/
  `UpdateField` by tag, just a tag it looked up by name instead of
  hardcoding. A client wanting to address by name directly (no lookup
  step) would need a different protocol shape, not attempted here.
- Adds one more request/response variant pair to maintain per domain
  adapter (`describe()`) — a real, small, ongoing cost for any future
  third domain, not free.

## Validation and revisit triggers

- Real implementation, same delivery cycle: `cargo test --features server`
  and `cargo test --features server,research` both green, including the
  two new schema-driven integration tests and three new unit tests
  (`server::tests::describe_schema_returns_the_fixture_store_own_shape`,
  `server::dog::tests::describe_names_both_fields_and_reports_neighbors_only`,
  `server::order::tests::describe_names_all_four_fields_and_reports_parent_children_only`).
- Revisit if: a real schema-driven client library is built and finds
  `DomainSchema`'s shape insufficient (e.g. needing nested/composite field
  types this crate's fixed `ValueKind` set doesn't cover); a third domain
  surfaces a capability shape `FieldCapabilities`'s three booleans can't
  express; or the protocol ever adds wire-format versioning, at which
  point `DomainSchema`'s own compatibility story should be revisited
  alongside it.
