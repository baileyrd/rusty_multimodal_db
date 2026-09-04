# Server Entity Domain Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved the design as proposed, `ADR-0037` option (a); (b)
  also fixing `Symmetric`'s forwarding gap this round and (c) closing
  as not warranted both declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit — see
  `ADR-0037`'s "Acceptance and implementation" section.
- Date: 2026-09-04
- Related: `ADR-0036`/`docs/design/SERVER-REMINDER-DOMAIN-DESIGN.md`
  (the `Reminder` domain — the immediately preceding round in this same
  `rusty_remind_me`-motivated line of work, and the precedent for a
  design not sourced from `docs/FUTURE-GROWTH.md`), `SERVER-001`
  `FR-004`/`FR-012` (`Dog`'s `SymmetricRelation`-only shape and
  `Employee`'s dual-relation combination — the two adapters this
  design's own relation choice is measured against), `ADR-0018`/
  `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (the durable
  edge-blob machinery `Symmetric` already provides, reused unchanged),
  `ADR-0034` (`SchemaDrivenClient::query` — the precedent for a new
  client-side-only capability compiling to existing wire primitives,
  the same shape `entity_traverse` below follows).
- Supersedes/Superseded by: none. Adds one new domain (`Entity`) and
  one new client-side-only `SchemaDrivenClient` method
  (`traverse`); changes no existing `Request`/`Response` variant, no
  `PROTOCOL_VERSION`, no existing domain's behavior.

## Purpose and scope

The immediately preceding round (`ADR-0036`, `Reminder`) closed the
first, cheapest slice of the owner's own question — could this crate
back `rusty_remind_me`? — by adding the one piece that needed zero new
engine capability: a fixed-schema record. That round's own "Open
questions" named the second slice explicitly: `entity`/
`entity_upsert`/`entity_traverse`, an entity graph, as the next
candidate, expected to reuse this crate's existing relation/edge
machinery (`ChildOf`/`SymmetricRelation`, `Symmetric<>`/`Reversed<>`)
rather than build something new.

This document is that investigation, done rather than assumed. It
finds a real, load-bearing gap in the existing machinery — named
below, not glossed over — and proposes a bounded slice that avoids
needing the fix this round: a single self-referential symmetric
relation (`Entity relates_to Entity`), the identical shape `Dog`'s own
`littermate_of` already has, plus one genuinely new capability this
crate has never had at any layer — bounded multi-hop graph
traversal — built entirely client-side over the existing one-hop
`Neighbors` primitive, the same "new client-side capability, zero new
wire request" shape `ADR-0034`'s SQL parsing already established.

**This document does not add multiple named relation types on one
record type** (e.g. `Entity` having both a `relates_to` and a
`mentions` relation simultaneously) — see "Considered options" for the
real gap that blocks it cheaply today, and "Open questions" for what
closing that gap would take.

## Non-goals

- Not multiple relation *kinds* on one domain beyond the one this
  design adds — `Employee` already proves one `ChildOf` plus one
  `SymmetricRelation` together; two independent `SymmetricRelation`s
  on the same record type is a different, currently-unsupported
  combination (verified directly — see "Considered options"), and not
  attempted here.
- Not per-edge metadata (a relation "kind" label, a weight, a
  timestamp) — `Symmetric`'s adjacency is an unlabeled `HashMap<Id,
  Vec<Id>>`; every edge is interchangeable once stored. Any future
  need to distinguish *why* two entities are related is a real,
  separate, larger data-model question, not decided here.
- Not server-side traversal — `entity_traverse` is a client-side loop
  over the existing `Request::Neighbors` RPC, one round trip per newly
  discovered node (see "Proposed shape"); a server-side batched
  traversal request would cut round trips but is real, additional
  engine work, named as a future option, not built here.
- Not entity deduplication, merging, or the `entity_upsert`-implied
  "update if present, insert if not" semantics specifically — this
  design gives `Entity` the same `GetById`/`UpdateField`/`Request::
  Transaction` primitives every domain already has; whatever upsert
  logic `rusty_remind_me` wants is a client-side (or future
  server-side) concern layered on top, not solved here.
- Not `rusty_remind_me`'s actual field/relation shape verified against
  its real source — `rusty_remind_me`'s own repository was not
  attached to this session, matching `ADR-0036`'s own identical,
  explicitly named limitation. `Entity`'s three fields and one
  relation below are this document's own reasoned guess, flagged as
  an open question for a later integration unit, not a confirmed fact.

## Context and terminology

- **`ChildOf<Marker>`/`SymmetricRelation<Marker>`**: this library's two
  relation traits (`crate::generic::traits`). Both are generic over a
  marker type, so in principle a record type could implement either
  trait more than once with different markers — nothing in the trait
  definitions themselves forbids it.
- **What actually forwards a second relation today**: verified
  directly against `src/generic/store.rs`. `Reversed<S, P, C, Marker>`
  has a `Neighbors<R, RelMarker>` forwarding impl generic over an
  *independent* `R`/`RelMarker` pair (not tied to `Reversed`'s own
  `P`/`C`/`Marker`) — this is exactly `FR-012`'s fix, added for
  `Employee`'s `reports_to` (`Reversed`) stacked over `collaborates_with`
  (`Symmetric`). `Symmetric<S, R, Marker>` has **no equivalent**: its
  only `Neighbors` impl is for its own `Marker`, with no forwarding
  impl re-exposing a *different* marker's `Neighbors` from an inner
  layer. So `Reversed` wrapping `Symmetric` (a directed relation over a
  symmetric one — `Employee`'s exact shape) works today; `Symmetric`
  wrapping `Symmetric` (two independent undirected relations, no
  directed one) does not, without a `FR-012`-shaped fix this design
  does not propose making.
- **`entity_traverse`**: inferred, like every `rusty_remind_me` field
  name this line of work has used, from the MCP tool name alone — a
  reasonable read is "walk the relation graph from a starting entity,
  some number of hops out," which this crate has never had at any
  layer: `Neighbors`/`Children`/`Parent` are all exactly one hop: even
  `benches/external_db.rs`'s `two_hop_neighbors`/`three_hop_neighbors`/
  `four_hop_neighbors`/`five_hop_neighbors` helpers are throwaway
  bench-only functions, private to that bench target, not reusable
  library capability.

## Requirements

- `ENT-FR-001` — **The `Entity` record**, `src/generic/entity.rs` (new,
  front-door — not behind `research`, matching `Reminder`'s own
  precedent): `id: Uuid`, `label: String`, `kind: EntityKind` where
  `EntityKind::{Person, Place, Organization, Concept, Event}`,
  `mention_count: i64`. `Record`, `SchemaTag`
  (`"entity::Entity"`), `Serialize`/`Deserialize` implemented the same
  way every existing generic-schema record already is.
- `ENT-FR-002` — **`kind` is the `IndexedField`** (marker `KindField`),
  encoded as its `u32` discriminant (the `server::order`/`server::
  reminder`-established fixed-mapping shape) — equality-filterable via
  `Request::FilterEq`, matching every existing domain's own convention
  (an enum in the index slot) — unlike `Reminder`'s deliberate
  inversion, there is no equally strong forcing function here to
  invert it: `kind` is expected to change rarely if ever, so the usual
  assignment is kept.
- `ENT-FR-003` — **`mention_count` is the `ScannableField`** (marker
  `MentionCountField`), `i64`, durably mutable via
  `Request::UpdateField`/`Session::update` — the field expected to
  change over an entity's lifecycle (incremented each time something
  references it), the identical "pick the field that actually
  changes" reasoning `Reminder`'s own `status` choice already used.
- `ENT-FR-004` — **`label` is read-only over the wire** — present in
  every `GetById`/`Query` result, never independently `scan`/`update`/
  `filter_eq`-able, the identical "every capability flag `false`"
  shape `Reminder::title`/`Order::created_at_unix_ms` already have.
- `ENT-FR-005` — **One self-referential `SymmetricRelation`,
  `RelatesTo`** — `Entity relates_to Entity`, the identical shape
  `Dog`'s own `littermate_of` already has (`Neighbors` only; no
  `ChildOf`, so `parent`/`children` report `ErrorCode::Unsupported`
  unconditionally, matching `Order`'s own missing `neighbors` half).
  `EntityProductionStack = Symmetric<GenericMmapStore<Entity,
  KindField, MentionCountField>, Entity, RelatesTo>` — one composition
  layer, the same shape `Dog`'s own `ProductionStore` uses for
  `littermate_of` (structurally; `Dog` is the bespoke, non-generic
  store, `Entity` the generic-library equivalent). `Symmetric::create`/
  `open`/`open_portable` (`STORAGE-016`) give the edge list real
  file-portability for free, unchanged.
- `ENT-FR-006` — **`EntityConnectionStore`** (`src/server/entity.rs`,
  gated by `server` alone — matching `Reminder`'s own front-door
  precedent, not `Order`/`Employee`'s `server` + `research`) implements
  `ConnectionStore` exactly as `ReminderConnectionStore` already does:
  `get`/`scan_all` reconstruct every field per id; `filter_eq` supports
  `kind` only; `scan_field`/`update_field` support `mention_count`
  only; `neighbors` real (`RelatesTo`); `parent`/`children`
  `Unsupported`; `validate_op`/`apply_transaction`/`with_journal`
  mirror every existing adapter's own shape exactly.
- `ENT-FR-007` — **`SchemaDrivenClient::traverse`** (new,
  client-side-only, no new `Request`/`Response`): `pub fn
  traverse(&mut self, id: RecordId, max_depth: usize, max_nodes:
  usize) -> Result<Vec<(RecordId, usize)>, ClientError>` — breadth-
  first from `id` (included at depth `0`), calling the existing
  `Request::Neighbors` once per newly-discovered id, stopping at
  `max_depth` hops or `max_nodes` total visited ids, whichever comes
  first (`ENT-FR-008` names why both bounds are required). A
  `visited: HashSet<RecordId>` guard is required for correctness, not
  just efficiency — a symmetric relation trivially cycles (`A`
  relates to `B` relates to `A`), so an unguarded walk never
  terminates. `ClientError::Unsupported("traverse")` locally,
  no round trip, if the connected domain's schema reports
  `relations.neighbors: false` — the identical client-side gate
  `SchemaDrivenClient::neighbors` already uses.
- `ENT-FR-008` — **Both `max_depth` and `max_nodes` are caller-supplied,
  with no crate-side default** — unlike `MAX_STAGED_OPS`/
  `MAX_TRACKED_READS`/`MAX_TRACKED_PEERS` (each a fixed `pub const`),
  a traversal's right bound depends entirely on the caller's own graph
  and use case; a client library forcing one fixed constant on every
  domain this method might ever traverse would be the wrong kind of
  bound. Cost is real and named, not hidden: one `Request::Neighbors`
  round trip per newly-discovered node, so `traverse(id, 3, 500)`
  over a densely connected graph can mean hundreds of round trips —
  the same "unconditional full scan, no index" honesty `Request::
  Query`'s own design already modeled.

## Considered options

**Whether to support multiple named relation types this round.** This
is the real architectural question a `rusty_remind_me`-style entity
graph actually wants (`relates_to` and `mentions` and `part_of`, not
one undifferentiated edge kind). Investigated directly, not assumed:
`Symmetric<S, R, Marker>` has no forwarding `Neighbors` impl for a
*different* marker the way `Reversed` does (`Context and terminology`
above) — stacking two `Symmetric` layers on the same record type would
compile (nothing stops writing `Symmetric<Symmetric<Base, Entity,
RelatesTo>, Entity, Mentions>`), but the outer layer's own `Neighbors`
impl only ever answers for its own marker, so the inner relation's
`Neighbors<Entity, RelatesTo>` would simply not be reachable through
the composed stack — a real, silent-if-unfixed gap, not a cosmetic
one. Three options: **(a) propose this round anyway, and fix the
forwarding gap as part of it** — the most complete answer, but a real
`crate::generic::store` change (the same class of fix `FR-012` made),
a bigger unit than a single new domain; **(b) (proposed) one relation
only this round, name the gap and the fix it would need, revisit if a
second relation type is ever actually wanted** — mirrors this
project's own repeated proportionality calls (`ADR-0033` rejecting
full MVCC, `ADR-0034` rejecting a query planner, `ADR-0035` rejecting
`HAVING`) and keeps this round's engine-capability cost at zero, the
same shape `Reminder` itself had; **(c) close entity support entirely
as not warranted this round** — rejected, since a single-relation
entity graph is still real, useful capability and a natural sequel to
`Reminder`.

Option (b) proposed.

**Where `entity_traverse` lives: client-side loop vs. a new server
request.** A new `Request::Traverse { id, max_depth }` /
`Response::RecordList`-with-depths pair would cut round trips to one,
but is real new wire surface — a `PROTOCOL_VERSION` bump, a new
`Response` variant, server-side BFS logic duplicated across every
domain's `ConnectionStore` the same way `evaluate_query`/
`evaluate_aggregate` centralize filter/reduce logic today. Proposed
instead: a client-side loop over the already-existing `Request::
Neighbors`, the identical "new capability, zero new wire primitive"
shape `ADR-0034`'s SQL parsing already used for `Request::Query`
compilation. Cost, named plainly: more round trips than a batched
server-side walk would need — acceptable for a first cut, revisit only
if that cost is ever shown to matter in practice.

## Proposed shape

```rust
// src/generic/entity.rs (new, front-door)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityKind { Person, Place, Organization, Concept, Event }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub label: String,
    pub kind: EntityKind,
    pub mention_count: i64,
}

impl Record for Entity { type Id = Uuid; fn id(&self) -> Uuid { self.id } }
impl SchemaTag for Entity { const SCHEMA_TAG: &'static str = "entity::Entity"; }

pub struct KindField;
impl IndexedField<KindField> for Entity {
    type IndexValue = EntityKind;
    fn indexed_value(&self) -> &EntityKind { &self.kind }
}

pub struct MentionCountField;
impl ScannableField<MentionCountField> for Entity {
    type ScanValue = i64;
    fn scannable_value(&self) -> i64 { self.mention_count }
    fn set_scannable_value(&mut self, value: i64) { self.mention_count = value; }
}

// RelatesTo — ENT-FR-005, one self-referential SymmetricRelation, the
// Dog::littermate_of shape.
pub struct RelatesTo;
impl SymmetricRelation<RelatesTo> for Entity {}

pub type EntityProductionStack =
    Symmetric<GenericMmapStore<Entity, KindField, MentionCountField>, Entity, RelatesTo>;

pub fn create_entity_production_stack(
    entities: Vec<Entity>,
    relates_to_edges: &[(Uuid, Uuid)],
    path: &Path,
) -> Result<EntityProductionStack, DurabilityError> {
    let core = GenericMmapStore::<Entity, KindField, MentionCountField>::create(entities, path)?;
    Symmetric::<_, Entity, RelatesTo>::create(core, relates_to_edges, &edges_path(path))
}
```

```rust
// src/server/entity.rs (new, `server`-gated only — ENT-FR-006)

pub const FIELD_LABEL: FieldRef = 0;
pub const FIELD_KIND: FieldRef = 1;
pub const FIELD_MENTION_COUNT: FieldRef = 2;

pub struct EntityConnectionStore {
    store: GenericProductionStore<EntityProductionStack>,
    journal: Option<CommitGroup>, // ENT-FR-006, identical shape to Reminder/Order/Employee
}

impl ConnectionStore for EntityConnectionStore {
    // get/scan_all: all three fields per id.
    // filter_eq: FIELD_KIND only (Ok(self.store.filter_eq::<Entity, KindField>(&kind))),
    //            FIELD_LABEL/FIELD_MENTION_COUNT => Unsupported.
    // scan_field/update_field: FIELD_MENTION_COUNT only.
    // parent/children: Unsupported — no ChildOf.
    // neighbors: real, Ok(self.store.neighbors::<Entity, RelatesTo>(id)).
    // describe: relations { parent_children: false, neighbors: true } — Dog's own shape.
}
```

```rust
// src/server/client.rs — ENT-FR-007, no new Request/Response

impl SchemaDrivenClient {
    pub fn traverse(
        &mut self,
        id: RecordId,
        max_depth: usize,
        max_nodes: usize,
    ) -> Result<Vec<(RecordId, usize)>, ClientError> {
        if !self.schema.relations.neighbors {
            return Err(ClientError::Unsupported("traverse"));
        }
        let mut visited: HashMap<RecordId, usize> = HashMap::new();
        let mut frontier = vec![id];
        visited.insert(id, 0);
        for depth in 0..max_depth {
            if visited.len() >= max_nodes {
                break;
            }
            let mut next_frontier = Vec::new();
            for &node in &frontier {
                for neighbor in self.neighbors(node)? {
                    if visited.len() >= max_nodes {
                        break;
                    }
                    if let std::collections::hash_map::Entry::Vacant(e) = visited.entry(neighbor) {
                        e.insert(depth + 1);
                        next_frontier.push(neighbor);
                    }
                }
            }
            frontier = next_frontier;
        }
        let mut result: Vec<(RecordId, usize)> = visited.into_iter().collect();
        result.sort_by_key(|(_, depth)| *depth);
        Ok(result)
    }
}
```

`src/server/mod.rs`'s `dispatch` needs no change at all — a fourth (now
fifth, counting `Reminder`) `ConnectionStore` implementor is invisible
to it, the identical claim `ADR-0036` already proved true. No
`Request`/`Response` variant, `PROTOCOL_VERSION`, or `ErrorCode`
changes anywhere in this proposal.

## Data/state and invariants

- No new persistent format — `EntityProductionStack` reuses
  `GenericMmapStore` (`STORAGE-015`) plus `Symmetric`'s own durable
  edge blob (`STORAGE-016`), the identical mechanism `Dog`'s
  `littermate_of` and `Employee`'s `collaborates_with` already use;
  `Entity`'s own `SchemaTag` (`"entity::Entity"`) is the only new
  on-disk-format value.
- `kind`/`mention_count` live in the mmap file's index/scan slots
  exactly as every generic-schema domain's own fields do;
  `relates_to` edges live in `<path>.edges`, rebuildable from that
  file alone (`Symmetric::open_portable`).
- `traverse`'s `visited` map is per-call, client-side, in-memory
  only — no server-side state of any kind, the identical "no
  cross-request server-side state" invariant this spec has held since
  `SERVER-001`'s own v0.1.0 (the one pre-existing session-state
  exception, `Begin`/`Commit`/session flags, is untouched by this
  design).

## Errors, failure, recovery, and observability

- No new `ErrorCode`. `UnknownField`/`Unsupported`/`Malformed`/
  `RecordNotFound` cover every rejection shape, identically to every
  existing domain.
- `traverse` surfaces the first `Request::Neighbors` failure it
  encounters as `ClientError`, aborting the walk with whatever nodes
  it had already collected discarded (a partial BFS result is not
  returned as if complete) — the same "fail the whole operation, don't
  silently return a partial answer" posture `Request::Transaction`
  already established for writes, applied here to a multi-round-trip
  read.

## Security, privacy, and compatibility

- No wire-protocol change of any kind — a connection negotiated at any
  version behaves identically whether or not it ever talks to an
  `Entity`-wrapping server or calls `traverse`; `PROTOCOL_VERSION` is
  untouched.
- `traverse` makes no new authorization decision — each `Request::
  Neighbors` it issues is gated exactly as a direct call would be
  (authentication only, the same posture every read-only request
  already has); a caller who could already walk the graph one hop at
  a time gains no new capability, only convenience.
- `label` is free-text and travels in plain `ScanValue::Str`, the same
  posture `Reminder::title`/`Dog::breed` already have — no new PII
  handling introduced or claimed.

## Acceptance criteria

1. `Entity`/`EntityKind`/`KindField`/`MentionCountField`/`RelatesTo`/
   `EntityProductionStack` exist exactly as specified, front-door (not
   behind `research`); `EntityConnectionStore` exists behind `server`
   alone.
2. `GetById`/`Query`/`Aggregate` against an `entity_server` return
   every field correctly, including a `GROUP BY kind` count matching a
   hand-computed tally.
3. `FilterEq` on `kind` returns exactly the matching ids; `FilterEq`
   on `label`/`mention_count` is `Unsupported`.
4. `UpdateField`/`Session::update` on `mention_count` succeeds and is
   immediately visible; `UpdateField` on `label`/`kind` is
   `Unsupported`.
5. `Neighbors` returns exactly the entities connected by a
   `relates_to` edge, both directions (symmetric); `parent`/`children`
   are `Unsupported` unconditionally.
6. `SchemaDrivenClient::traverse` from a starting entity, over a real
   multi-node graph with at least one cycle, returns every entity
   reachable within `max_depth` hops, each paired with its true
   shortest-path hop distance, with no duplicate and no infinite loop;
   stops at `max_nodes` when that bound is hit first; returns
   `ClientError::Unsupported("traverse")` with no round trip against a
   domain whose schema reports `relations.neighbors: false`.
7. `Request::Transaction`, every session kind, and journaled crash-
   atomicity all work against `Entity` with the same acceptance shape
   every existing domain's own tests already establish.
8. Every existing test in `tests/server_*.rs` — including every
   `Reminder` test — is unchanged; adding `Entity` costs nothing to a
   caller that never uses it.

## Verification plan

- `src/generic/entity.rs` unit tests: `IndexedField`/`ScannableField`
  round trips, `create`/`open` round-tripping a small fixture set with
  real `relates_to` edges, edge-list portability (`open_portable`
  matches `open`'s own `neighbors` results).
- `src/server/entity.rs` unit tests (the `ReminderConnectionStore`
  precedent): `get`, `filter_eq` by `kind`, `scan_field`/`update_field`
  on `mention_count`, `neighbors` reflecting `relates_to`, `describe`'s
  reported capabilities, `parent`/`children`'s `Unsupported`.
- `src/server/client.rs` unit/integration-adjacent tests for
  `traverse`: a real multi-hop graph (at least 4 nodes, one cycle) —
  correct hop distances, the `max_depth` bound, the `max_nodes` bound,
  the no-`neighbors`-capability client-side rejection, a failure
  mid-walk surfaced rather than swallowed.
- `tests/server_entity_integration.rs` (new, `required-features =
  ["server"]` only): a real client round trip covering acceptance
  criteria 2–7 above over a real socket, including `traverse` against
  a real running server.

## Traceability

- → `SERVER-001` next minor / FR (`ENT-FR-001`–`008`), a new ADR — the
  identical "domain adapter, same spec" shape `FR-004`/`FR-005`/
  `FR-012`/`FR-039` already established for `Dog`/`Order`/`Employee`/
  `Reminder`.
- Not sourced from `docs/FUTURE-GROWTH.md` — the second round in the
  line `ADR-0036` started, recorded the same way there.

## Open questions

- Whether a second, independently-named relation type is ever wanted
  badly enough to justify the `Symmetric`-forwarding fix named in
  "Considered options" — named, not decided; the natural next
  question once `Entity`'s single-relation shape proves useful in
  practice.
- Whether `rusty_remind_me`'s real `entity`/`entity_upsert`/
  `entity_traverse` shape actually matches this document's three-field,
  one-relation guess — unverified, since that repository was not read
  this session (the identical open question `ADR-0036` already named
  for `Reminder`, now asked again for `Entity`).
- Whether `traverse` should ever grow a server-side batched variant
  (`Request::Traverse`) once real usage shows the per-hop round-trip
  cost actually matters — named, not solved (see "Considered
  options").
- Whether per-edge metadata (a relation kind/weight/timestamp) is ever
  wanted — would need a real change to `Symmetric`'s own adjacency
  shape (`HashMap<Id, Vec<Id>>` has no room for a payload today), a
  separate, larger question from anything this document proposes.

## Change history

- 2026-09-04: Initial proposal, the second round in the
  `rusty_remind_me`-motivated line `ADR-0036` started — an `Entity`
  domain with one self-referential `SymmetricRelation` (`Dog`'s own
  shape) plus a new client-side-only `SchemaDrivenClient::traverse`
  for bounded multi-hop graph walking, after investigating and finding
  a real gap (`Symmetric` doesn't forward `Neighbors` for a second,
  independent marker the way `Reversed` does) that would have been
  needed for multiple named relation types, and deliberately not
  fixed this round.
- 2026-09-04: Accepted as proposed, `ADR-0037` option (a); (b) also
  fixing `Symmetric`'s forwarding gap this round and (c) closing as
  not warranted both declined.
- 2026-09-04: Implemented as `SERVER-001` v0.30.0 / FR-040, exactly as
  designed — see `ADR-0037`'s own implementation log entry for the
  full account. The `Symmetric`-forwarding gap stays named but
  unfixed, exactly as accepted.
