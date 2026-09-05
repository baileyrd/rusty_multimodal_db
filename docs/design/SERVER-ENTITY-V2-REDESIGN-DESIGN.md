# Server Entity v2 Redesign (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved the design as proposed, `ADR-0039` option (a),
  including `PROTOCOL_VERSION` 10's wire additions; (b) deferring the
  wire-protocol half and (c) closing as not warranted both declined).
  Acceptance authorizes the design; implementation follows as its own
  unit — see `ADR-0039`'s "Acceptance and implementation" section.
- Date: 2026-09-04
- Related: `ADR-0038`/`docs/design/SERVER-ENTITY-INTEGRATION-VERIFICATION-DESIGN.md`
  (the verification findings this design implements, option (a)
  accepted), `ADR-0037`/`docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md`
  (the `Entity` v1 domain this redesigns in place — no real deployed
  instance exists yet, so this is a straight revision, not a migration),
  `ADR-0022` (the append-only wire-compatibility rules this design's
  own wire additions follow exactly), `SERVER-001` `FR-012` (the
  `Reversed`-forwards-`Neighbors` precedent this design's own
  `Symmetric`-forwarding fix mirrors).
- Supersedes: `ADR-0037`'s `Entity` field/relation shape (`kind` as a
  fixed enum, `mention_count`, one undifferentiated relation) —
  revised, not additively extended, since no real caller depends on
  the v1 shape yet. Does not change `Reminder`/`Order`/`Employee`/
  `Dog`, `RecordId`'s crate-wide `Uuid` invariant, or any pre-existing
  `Request`/`Response` variant's own field layout.

## Purpose and scope

`ADR-0038` (accepted, option (a)) authorized revising `Entity`/
`traverse` to match `rusty_remind_me`'s real shape (Unit 41's
findings), including closing the `Symmetric`-forwarding gap, but
authorized only the *direction* — the concrete mechanism was left to
this follow-up round, since investigating it turned up real
architectural constraints ADR-0038 itself did not yet know about.

This document is that investigation and the concrete proposal it
produced: which parts of the real shape this crate's own architecture
can adopt directly, which parts collide with crate-wide invariants
that predate this whole line of work and are not this round's to
break, and — the genuinely new capability — a real, wire-compatible
way to reach more than one named relation type, which `ADR-0037`
deliberately deferred and Unit 41 confirmed is not speculative.

## Non-goals

- **Not changing `RecordId` from `Uuid`.** `protocol.rs`'s own doc
  comment states plainly: "every domain this crate has ever used is
  `Uuid`-keyed." This is a crate-wide invariant every existing domain
  and the wire protocol's own `TransactionOp`/`ParentLookup`/etc.
  assume, not an `Entity`-local choice. `rusty_remind_me`'s real
  `name`-as-primary-key model is real and confirmed (Unit 41 Finding
  1), but adopting it literally would mean `Entity` alone no longer
  being `Uuid`-addressable — the first domain ever to break that
  invariant, a far larger and more disruptive change than "revise one
  domain," and not what `ADR-0038`'s own Consequences named as this
  round's cost. `Entity` stays `Uuid`-keyed; `name` becomes a real,
  equality-filterable field instead (see `ENT2-FR-001`) — the closest
  compatible approximation, not a literal match. Named plainly, not
  hidden: a caller who has only a `name` still needs one `FilterEq`
  round trip to resolve an id before any other operation, where
  `remind_me_entity(name)` needs none.
- **Not adding `aliases` this round.** Unit 41 Finding 3 confirmed it
  is real, first-class capability in `rusty_remind_me` — but `ScanValue`
  (`src/server/protocol.rs`) has no list/array variant, and
  `IndexedField`/`ScannableField`'s generic machinery, while it could
  in principle carry a `Vec<String>` `IndexValue`/`ScanValue` for the
  *in-process* layer, has no wire representation to carry it over
  `Request`/`Response` at all without a new `ScanValue` variant (a real
  protocol addition, precedent: `F64`, ADR-0035) or remodeling aliases
  as a relation (a `HasAlias`-shaped edge to synthetic alias nodes) —
  either a real, separate design question, not decided here. Deferred
  explicitly, the same "name the gap precisely, don't build it this
  round" discipline `ADR-0037` itself used for the `Symmetric` gap.
- **Not case/whitespace-insensitive name resolution.** `rusty_remind_me`
  resolves `name` "case- and whitespace-insensitively"; this design's
  `FilterEq` on `name` is exact-string equality, the same mechanism
  every other `IndexedField` in this crate already uses. Real,
  accepted divergence, not silently glossed over.
- **Not server-side multi-hop traversal.** `remind_me_entity_traverse`
  runs its `hops`/`cap` bounds server-side; `ADR-0037`'s own choice —
  a client-side loop over one-hop `Neighbors` — stands unchanged here.
  This round only adds *which relation* a one-hop lookup follows; the
  bounded-walk mechanism itself (`SchemaDrivenClient::traverse`) is
  untouched apart from gaining an optional relation filter.
- **Not a real second relation label copied from `rusty_remind_me`'s
  own vocabulary.** Its real relation *labels* (what `remind_me_entity_
  traverse`'s `relation` parameter actually matches against in
  practice) are not knowable from a tool schema alone — that needs
  either real data or the source, neither available this round (Unit
  41's own "Open questions"). This design proves the *mechanism*
  (more than one named relation type, reachable and filterable) with
  one honestly-labeled example relation, `MentionedWith`, the same
  "structurally different validation, not a claimed real business
  shape" posture `Employee` itself was built under (`generic_spike::
  employee_impl`'s own module doc: "purpose-built for one specific
  untested combination... not a domain motivated by an external
  reference shape").
- **Not migrating any real deployed `Entity` data.** `Entity` merged
  this session with zero real external callers — every field/type
  change below is a straight in-place revision, the same "no
  deprecation shim, justified since nothing depends on the old shape"
  posture `FR-035`'s `AuthConfig`→`ServeOptions` rename and `FR-038`'s
  `QueryResult` widening both already used.

## Context and terminology

- **Why `kind` can be both `IndexedField` and `ScannableField`.**
  `GenericMmapStore<R, IndexMarker, ScanMarker>` structurally mandates
  exactly one `IndexedField` and one `ScannableField` per record type
  — but nothing requires them to be *different* struct fields. `kind`
  implementing both traits (different marker types, same underlying
  `self.kind` projection) is exactly what `Entity` v2's own real
  capabilities need: equality-filterable (`FilterEq`, a coarse
  "what kind is this" lookup) *and* durably mutable (`UpdateField`,
  matching `remind_me_entity_upsert`'s real "update kind" capability)
  — the same field, two roles, no synthetic third field required to
  satisfy the structural constraint the way v1's unrelated
  `mention_count` did.
- **Why the wire additions below are new `Request`/`Response`
  *variants*, never new fields on an existing one.** `ADR-0022`'s own
  append-only rules only ever add *variants*, appended at the end,
  each recording its introducing version — `Request::Query`/
  `Aggregate`, `Response::Rows`/`Groups` all followed this exactly.
  `bincode`'s struct encoding is positional, not name-tagged
  (`STORAGE-018`/`ADR-0021`): adding a *field* to an existing struct —
  `DomainSchema`, `RelationCapabilities`, or an existing `Request`/
  `Response` variant's own fields — changes that struct's byte layout
  for every version that already sends/expects it, which is exactly
  the class of change this crate has never made and this design does
  not either. `DomainSchema`/`RelationCapabilities` stay byte-for-byte
  unchanged; relation-kind discovery and filtering both go through
  brand new, independently-versioned variants instead (`ENT2-FR-004`/
  `005`).
- **`Symmetric`-forwarding gap** (Unit 41 Finding 6, `ADR-0037`'s own
  "Considered options"): `Symmetric<S, R, Marker>` has exactly one
  `Neighbors<R, Marker>` impl, tied to its own `Marker`; unlike
  `Reversed<S, P, C, Marker>`, which has a `Neighbors<R, RelMarker>`
  forwarding impl generic over an *independent* `R`/`RelMarker` pair
  (`FR-012`'s own fix), `Symmetric` has no equivalent. This design adds
  that missing forwarding impl, the same shape as `Reversed`'s.

## Requirements

- `ENT2-FR-001` — **`Entity` v2's fields**: `id: Uuid` (unchanged),
  `name: String` (renamed from `label`; `IndexedField<NameField>`,
  `IndexValue = String` — equality-filterable, matching `rusty_remind_
  me`'s own primary lookup key as closely as a `Uuid`-keyed domain
  can), `kind: String` (was `EntityKind`; `ScannableField<KindField>`,
  `ScanValue = String` — durably mutable via `UpdateField`, matching
  `entity_upsert`'s real "update kind" capability; open-ended, no
  fixed variant set, matching Unit 41 Finding 2 directly).
  `mention_count`/`EntityKind`/`kind_to_u32`/`kind_from_u32` all
  removed — no synthetic replacement field needed once `kind` fills
  both structural roles (see Context).
- `ENT2-FR-002` — **Two self-referential `SymmetricRelation`s**:
  `RelatesTo` (kept, unchanged meaning) and `MentionedWith` (new,
  explicitly a mechanism-validation example, not a claimed real
  `rusty_remind_me` label — see Non-goals). `EntityProductionStack`
  becomes a *nested* `Symmetric`: `Symmetric<Symmetric<
  GenericMmapStore<Entity, NameField, KindField>, Entity, RelatesTo>,
  Entity, MentionedWith>` — the shape `ADR-0037`'s own "Considered
  options" named as compiling today but silently unreachable for the
  inner relation; `ENT2-FR-003` is what makes it reachable.
- `ENT2-FR-003` — **The `Symmetric`-forwarding fix**
  (`src/generic/store.rs`, unconditional — `Entity` is front-door):
  a new `impl<S, R, Marker, R2, RelMarker> Neighbors<R2, RelMarker> for
  Symmetric<S, R, Marker> where R2: SymmetricRelation<RelMarker>, S:
  Neighbors<R2, RelMarker>` forwarding to the inner store — the exact
  shape `Reversed`'s own `Neighbors` forwarding impl already has,
  applied to `Symmetric` for the first time. Closes the real,
  previously load-bearing gap `ADR-0037`/Unit 41 both named.
- `ENT2-FR-004` — **`Request::NeighborsByRelation { id: RecordId,
  relation: String }` / reuses `Response::RecordList`** (protocol 10,
  new): a one-hop neighbor lookup filtered to one named relation —
  `RelatesTo`'s wire label `"relates_to"`, `MentionedWith`'s
  `"mentioned_with"`. `ErrorCode::UnknownField`-shaped rejection (a
  relation label this domain doesn't have) reuses `ErrorCode::
  Malformed`, the same "no new `ErrorCode` for a bounded slice"
  posture `FR-037`/`FR-038` both already used. `Malformed` on a
  connection negotiated below 10, the same append-only gate every
  prior protocol bump used.
- `ENT2-FR-005` — **`Request::ListRelationKinds` / `Response::
  RelationKinds { kinds: Vec<String> }`** (protocol 10, new): lets a
  client discover a domain's real relation labels without hardcoding
  one — `["relates_to"]` for a single-relation domain (`Dog`,
  unchanged), `["relates_to", "mentioned_with"]` for `Entity` v2,
  `[]` for a no-relation domain (`Reminder`, unchanged). Does not
  touch `DomainSchema`/`RelationCapabilities` at all (see Context) —
  `RelationCapabilities.neighbors` keeps meaning exactly what it
  always has, "at least one `SymmetricRelation` exists."
- `ENT2-FR-006` — **`SchemaDrivenClient::traverse` gains an optional
  relation filter**: `traverse(&mut self, id, max_depth, max_nodes,
  relation: Option<&str>)` — `None` behaves exactly as `ADR-0037`'s
  own `traverse` does today (plain `Request::Neighbors`, every
  relation kind followed); `Some(label)` routes each hop through
  `Request::NeighborsByRelation` instead. A deliberate, documented
  breaking signature change, no deprecation shim (see Non-goals'
  justification) — the same posture `FR-038`'s `QueryResult` widening
  already used.
- `ENT2-FR-007` — **`EntityConnectionStore` updated**: `FIELD_NAME`/
  `FIELD_KIND` (two fields, not three — `FIELD_LABEL`/`FIELD_MENTION_
  COUNT` retired); `filter_eq`/`get`/`scan_all` reflect `name`'s new
  `IndexedField` role and `kind`'s new `ScannableField` role (a
  straight swap of which field plays which role from v1, not a new
  shape); `update_field` on `kind` accepts any `String` — no
  discriminant validation, unlike v1's fixed-enum `kind_from_u32`
  check, since `kind` is open-ended now (`ENT2-FR-001`); `neighbors`/
  the new `neighbors_by_relation`/`list_relation_kinds` `ConnectionStore`
  methods all real.

## Considered options

**Whether to break `RecordId`'s `Uuid` invariant for `Entity` alone.**
Investigated directly: every existing domain, `TransactionOp`,
`ParentLookup`, and the wire protocol's own doc comment all assume
`RecordId = Uuid` crate-wide. **(a) Make `Entity` `String`-keyed,
breaking the invariant for the first time** — the most literal match
to `rusty_remind_me`'s real shape, but a far larger, crate-wide-
precedent-breaking change than "revise one domain," touching code
this round has no mandate to touch. **(b) (proposed) keep `Uuid`,
add `name` as a real equality-filterable field** — the closest
compatible approximation; named plainly as an approximation, not a
full match (see Non-goals). **(c) leave identity as-is, do nothing**
— rejected, since `name`-based lookup is real, load-bearing capability
`entity_upsert`/`remind_me_entity` both center on; not adding it at
all would leave the redesign incomplete on its own terms.

Option (b) proposed.

**How to reach a second relation type on the wire.** Investigated
directly against `ADR-0022`'s own compatibility rules: **(a) add a
`relation` field to the existing `Request::Neighbors`/`DomainSchema`**
— rejected outright; `bincode`'s positional struct encoding makes this
a real breaking change to every connection at every existing protocol
version, not merely a style choice. **(b) (proposed) new `Request`/
`Response` variants, `NeighborsByRelation`/`ListRelationKinds`,
appended and version-gated exactly like every prior protocol bump** —
the only wire-compatible way to add this capability, confirmed by
re-reading `ADR-0022`'s own rules rather than assumed. **(c) a
server-side batched multi-relation traversal request** — real,
additional engine work beyond what closing the *plurality* gap needs;
`ADR-0037`'s own "server-side vs. client-side traversal" call stands
unrevisited.

Option (b) proposed.

## Proposed shape

```rust
// src/generic/entity.rs (revised)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
}

pub struct NameField;
impl IndexedField<NameField> for Entity {
    type IndexValue = String;
    fn indexed_value(&self) -> &String { &self.name }
}

pub struct KindField;
impl ScannableField<KindField> for Entity {
    type ScanValue = String;
    fn scannable_value(&self) -> String { self.kind.clone() }
    fn set_scannable_value(&mut self, value: String) { self.kind = value; }
}

pub struct RelatesTo;
impl SymmetricRelation<RelatesTo> for Entity {}

// New: a second relation, purely to validate the forwarding fix
// (ENT2-FR-002) — not a claimed real rusty_remind_me label.
pub struct MentionedWith;
impl SymmetricRelation<MentionedWith> for Entity {}

pub type EntityProductionStack = Symmetric<
    Symmetric<GenericMmapStore<Entity, NameField, KindField>, Entity, RelatesTo>,
    Entity,
    MentionedWith,
>;
```

```rust
// src/generic/store.rs — ENT2-FR-003, the Symmetric-forwarding fix

impl<S, R, Marker, R2, RelMarker> Neighbors<R2, RelMarker> for Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    R2: SymmetricRelation<RelMarker>,
    S: Neighbors<R2, RelMarker>,
{
    fn neighbors(&self, id: R2::Id) -> Vec<R2::Id> {
        self.inner.neighbors(id)
    }
}
```

```rust
// src/server/protocol.rs — ENT2-FR-004/005, protocol 10

pub const PROTOCOL_VERSION: u32 = 10;

// Request, appended:
NeighborsByRelation { id: RecordId, relation: String },  // index 17
ListRelationKinds,                                       // index 18

// Response, appended:
RelationKinds { kinds: Vec<String> },                     // index 14
// NeighborsByRelation reuses Response::RecordList (index 1) unchanged.
```

`dispatch`/`serve` need no signature change — both new `Request`
variants route through the same `ConnectionStore` trait, gaining two
new methods (`neighbors_by_relation`, `list_relation_kinds`) every
existing adapter implements as `Ok(Vec::new())`/always-`Unsupported`
respectively (mirroring how every pre-`Employee` adapter answered
`parent`/`children` before that domain needed them for real).

## Data/state and invariants

- No new persistent format primitive — the nested `Symmetric<Symmetric<
  GenericMmapStore<..>, .., RelatesTo>, .., MentionedWith>` stack reuses
  `STORAGE-016`'s existing edge-blob machinery twice, once per relation
  layer, each at its own `<path>.edges`-shaped companion file (a second
  edges path needed — `edges_path`'s own convention extended to
  disambiguate by relation, not redesigned).
- `Entity`'s own `SchemaTag` (`"entity::Entity"`) is unchanged — the
  record type itself didn't change identity, only its field set;
  existing `<path>.records` blobs from a v1-shaped `Entity` are not
  binary-compatible with v2's own `Entity` struct (a real, accepted
  break, matching this document's own "zero real deployed data"
  Non-goal).

## Errors, failure, recovery, and observability

- No new `ErrorCode`. `NeighborsByRelation` naming an unknown relation
  label reuses `ErrorCode::Malformed`, the same "no new code for a
  bounded slice" posture `FR-037`/`FR-038` both used.
- `ListRelationKinds` never fails once a connection is negotiated at
  ≥ 10 — it always has an answer (possibly `[]`).

## Security, privacy, and compatibility

- `PROTOCOL_VERSION` moves to 10; the version table gains row 10.
  `NeighborsByRelation`/`ListRelationKinds`/`RelationKinds` are
  `Malformed` below 10 (rule 3) and sent only after negotiating ≥ 10
  (rule 4) — the identical append-only discipline every prior bump
  used, re-verified against `ADR-0022`'s own four rules directly, not
  assumed.
- `DomainSchema`/`RelationCapabilities`/every pre-existing `Request`/
  `Response` variant's own field layout is byte-for-byte unchanged —
  confirmed, not merely intended, by re-reading `bincode`'s positional
  struct-encoding behavior against `STORAGE-018`'s own codec.
- Both new requests are read-only, gated exactly like `GetById`/
  `Neighbors` (authentication only); neither is overlaid by a
  read-your-writes session nor tracked into a snapshot-isolation read
  set — the same "only `GetById`" line every prior read-only addition
  already drew.

## Acceptance criteria

1. `Entity`/`NameField`/`KindField`/`RelatesTo`/`MentionedWith`/
   `EntityProductionStack` exist exactly as specified; `Symmetric`'s
   new forwarding impl compiles and is exercised by both relation
   layers on the same nested stack.
2. `GetById`/`Query`/`Aggregate` against an `entity_server` return
   `name`/`kind` correctly; `FilterEq` on `name` returns exactly the
   matching id(s); `UpdateField` on `kind` accepts any string and is
   immediately visible.
3. `Request::Neighbors` (unfiltered) returns the union of both
   relations' neighbors, matching `traverse(.., relation: None)`'s own
   behavior; `Request::NeighborsByRelation` with `"relates_to"`/
   `"mentioned_with"` returns exactly that relation's own edges, not
   the other's — the real proof the forwarding fix works.
4. `Request::ListRelationKinds` against `entity_server` returns
   `["relates_to", "mentioned_with"]` (order unspecified); against
   `reminder_server` returns `[]`; against `dog_server` returns
   `["relates_to"]` — `littermate_of`'s own real wire label chosen to
   match `Dog`'s existing in-process relation name.
5. `SchemaDrivenClient::traverse(id, depth, nodes, Some("mentioned_
   with"))` over a graph with edges on both relations visits only
   `MentionedWith`-connected nodes; `None` visits both, matching
   today's behavior exactly (no regression for a caller that never
   passes a relation filter).
6. A connection negotiated below protocol 10 gets `Malformed` for
   either new request; `PROTOCOL_VERSION`'s own version table gains
   row 10; every pre-existing golden vector (`Request::Query`/
   `Aggregate`/etc.) is byte-for-byte unchanged.
7. `Request::Transaction`, every session kind, and journaled crash-
   atomicity all work against `Entity` v2 with the same acceptance
   shape every existing domain's own tests already establish.
8. Every existing test in `tests/server_*.rs` for every domain other
   than `Entity` is unchanged; `Entity`'s own tests are rewritten for
   the new shape, not merely patched around the old one.

## Verification plan

- `src/generic/store.rs` unit tests: the new `Symmetric`-forwarding
  impl exercised directly (a two-relation in-memory stack, `neighbors`
  for each marker independently), plus a regression check that
  `Reversed`-wrapping-`Symmetric` (`Employee`'s own shape) still works
  unchanged.
- `src/generic/entity.rs` unit tests: `name`/`kind` round trips,
  `create`/`open` with both relation layers, edge-list portability for
  both `<path>.edges` companions.
- `src/server/entity.rs` unit tests: `filter_eq` by `name`, `update_
  field` on `kind` with an arbitrary string, `neighbors_by_relation`
  per relation, `list_relation_kinds`.
- `src/server/protocol.rs`: new golden vectors for `NeighborsByRelation`/
  `ListRelationKinds`/`RelationKinds`; every pre-existing vector
  re-run unchanged.
- `tests/server_entity_integration.rs` (rewritten): every acceptance
  criterion above over a real socket, including the version-10 gate
  and a real two-relation graph.

## Traceability

- → `SERVER-001` next minor / FR (`ENT2-FR-001`–`007`), a new ADR —
  implements `ADR-0038`'s accepted direction concretely.
- Not sourced from `docs/FUTURE-GROWTH.md` — the fourth round in the
  line `ADR-0036` started.

## Open questions

- Whether `aliases` is ever added, and via which mechanism (new
  `ScanValue` variant vs. a relation-based remodeling) — named, not
  decided (see Non-goals).
- Whether `name`-based lookup ever needs to be a single round trip
  (a combined `FilterEq`-then-`GetById` server-side primitive) rather
  than two — named, not solved; real cost, not yet shown to matter.
- Whether `rusty_remind_me`'s real relation label vocabulary is ever
  confirmed (still unread source) — `MentionedWith` stays an honest
  placeholder, not a verified real label, until then. *Resolved by
  `ADR-0042` (`docs/design/SERVER-ENTITY-SOURCE-VERIFICATION-DESIGN.md`,
  Finding F3): the source was read at `29602f1` — there is no
  vocabulary; `entity_relations.relation` is free-form text per triple.
  The placeholder label was never the mismatch; this document's own
  fixed-set model is, named there for a future round.*

## Change history

- 2026-09-04: Initial proposal, the fourth round in the
  `rusty_remind_me`-motivated line `ADR-0036` started — concrete
  mechanism for `ADR-0038`'s accepted option (a): `name`/open `kind`
  fields, `mention_count` removed, a second example relation type
  (`MentionedWith`) proving the `Symmetric`-forwarding fix, and two
  new wire-compatible `Request`/`Response` variants (protocol 10) for
  relation-filtered neighbor lookup and relation-kind discovery.
- 2026-09-04: Accepted as proposed, `ADR-0039` option (a); (b) and (c)
  declined.
- 2026-09-04: **Implemented** (`SERVER-001-FR-041`, v0.31.0). Two of
  this document's own proposed mechanisms did not survive contact with
  the compiler, both caught before implementation code was written and
  resolved with the owner rather than assumed away — see
  `ADR-0039`'s own implementation-log entry for the full account of
  each. Summary: `kind` could not fill both `IndexedField` and
  `ScannableField` roles (`ScanValue: Copy`, `GenericMmapStore`'s
  fixed-width mmap slots) — it stays the read-only `IndexedField`
  instead, `mention_count` not retired; the `Symmetric`-forwarding fix
  this document's own "Considered options"/"Proposed shape" described
  (mirroring `Reversed`'s `FR-012` fix) does not compile —
  confirmed directly with `rustc`, `E0119` — so a genuinely new
  primitive, `MultiSymmetric`/`MultiNeighbors`, was built instead,
  keying relations by a runtime `String` label rather than a
  compile-time `Marker`. A third, smaller deviation: `label`'s field
  identifier was not renamed to `name` as this document's own
  "Proposed shape" showed — the field fulfills that role under its
  existing v1 name. Everything else — `PROTOCOL_VERSION` 10's two new
  variants, `MentionedWith` as the second relation, `traverse`'s
  optional relation filter, `aliases`/case-insensitive names/
  server-side traversal deferred — landed as proposed.
