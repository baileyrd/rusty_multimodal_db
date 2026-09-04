# ADR-0037: `Entity` domain and bounded client-side graph traversal

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved the design as proposed, option (a): `Entity` with one
  self-referential `SymmetricRelation`, client-side `traverse`, the
  `Symmetric`-forwarding gap named but not fixed; (b) also fixing the
  gap this round and (c) closing as not warranted both declined; no
  changes requested). Acceptance authorizes the design; implementation
  follows as its own unit — see "Acceptance and implementation" below.
- Date: 2026-09-04
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0036`/`docs/design/SERVER-
  REMINDER-DOMAIN-DESIGN.md` (the preceding round in this same
  `rusty_remind_me`-motivated line of work, and the precedent for a
  design not sourced from `docs/FUTURE-GROWTH.md`), `SERVER-001`
  `FR-004`/`FR-012` (`Dog`'s `SymmetricRelation`-only shape — this
  design's own relation precedent — and `Employee`'s dual-relation
  combination, the closest existing analogue to the gap this ADR
  names but does not close), `ADR-0034` (the "new client-side
  capability, zero new wire primitive" shape `entity_traverse`
  follows).
- Supersedes/Superseded by: none. Adds one new domain and one new
  client-side-only `SchemaDrivenClient` method; changes no existing
  `Request`/`Response` variant, no `PROTOCOL_VERSION`, no `ErrorCode`,
  no existing domain's behavior.

## Context

`ADR-0036` (`Reminder`) closed the first, cheapest slice of backing
`rusty_remind_me`: a fixed-schema record needing zero new engine
capability. That round's own "Open questions" named the natural
second slice: `entity`/`entity_upsert`/`entity_traverse` — an entity
graph, expected to reuse this crate's existing relation machinery
(`ChildOf`/`SymmetricRelation`) rather than invent something new.

Investigating that expectation directly (not assuming it) found a
real, previously-unexercised gap: `Symmetric<S, R, Marker>` — the
layer providing `Neighbors` — has no forwarding impl re-exposing a
*different* marker's `Neighbors` from an inner layer, unlike
`Reversed`, which gained exactly that forwarding for `FR-012`
(`Employee`'s directed-plus-symmetric combination). So a directed
relation stacked over a symmetric one (`Employee`'s shape) works
today; two independent symmetric relations on one record type does
not, without a `FR-012`-shaped fix to `Symmetric` itself.

## Decision

Add `Entity` as this crate's fifth domain (`title`/`kind`/
`mention_count`, one self-referential `SymmetricRelation` — `Dog`'s
own shape, not `Employee`'s dual-relation one), and add
`SchemaDrivenClient::traverse` — bounded breadth-first graph walking,
built entirely client-side over the already-existing `Request::
Neighbors` RPC, the identical "compile a new client capability to
existing wire primitives" shape `ADR-0034`'s SQL parsing already
established. Neither needs the `Symmetric`-forwarding fix named
above, since this round proposes exactly one relation, not two.

**Deliberately not decided by this document:** whether a second,
independently-named relation type (`relates_to` and `mentions`
together, say) is ever wanted enough to justify that fix. The gap is
named precisely, with the exact trait/impl it lives in, so a future
round does not have to rediscover it — but closing it is real,
additional `crate::generic::store` work, out of proportion to what
this round needs, the same proportionality call `ADR-0033`/`ADR-0034`/
`ADR-0035` each already made against a larger available shape.

## Consequences

- Positive: a real, queryable, traversable entity graph, reachable
  over the existing wire protocol with zero new `Request`/`Response`
  surface — the same low engine cost `ADR-0036` already demonstrated,
  now proven true for a relation-bearing domain too.
- Positive: `traverse` is genuinely new capability (this crate has
  never had multi-hop graph walking at any layer — even the
  `external_db.rs` bench helpers of that shape are throwaway,
  non-reusable code) delivered without touching the server at all,
  keeping the wire protocol's own stability record intact through a
  fifth domain and a new read shape both.
- Named, not hidden: `traverse`'s cost is one round trip per newly
  discovered node — real, and potentially large on a densely
  connected graph; `max_depth`/`max_nodes` bound it, but do not make
  it cheap.
- Named, not hidden: this round does not deliver what a real entity
  graph most likely actually wants — more than one relation kind. The
  gap that blocks it cheaply is now documented precisely (`Symmetric`'s
  missing forwarding impl); closing it is real, deferred work.
- Real, unresolved gap carried over from `ADR-0036`: `Entity`'s field
  and relation shape is this document's own reasoned guess against
  `rusty_remind_me`'s tool *names* alone, not its real source. If
  wrong, `Entity` may need to change before real integration is
  useful.
- No change to `Reminder`/`Order`/`Employee`/`Dog`'s own domains, no
  existing `Request`/`Response`/`ErrorCode`/`PROTOCOL_VERSION` change.

## Considered options

**(a) Accept as proposed** — `Entity` with one `SymmetricRelation`, a
client-side `traverse`, the `Symmetric`-forwarding gap named but not
fixed. **(b) Accept, and also fix `Symmetric`'s forwarding gap this
round** — the more complete answer for a real entity graph, but a
real `crate::generic::store` change beyond a single new domain's own
scope, the same class of unplanned-but-real work `FR-012` was inside
`Employee`'s own round — here it would be *planned*, which changes the
shape of the work (a deliberate two-part round) without changing that
it is real, additional engine work. **(c) Close as not warranted** —
the `rusty_remind_me` motivation doesn't justify even this bounded
slice; revisit only if a concrete need resurfaces.

## Acceptance and implementation

- Options offered at proposal: (a) accept as proposed — `Entity` with
  one `SymmetricRelation`, client-side `traverse`, the multi-relation
  gap named but not fixed; (b) accept and also fix `Symmetric`'s
  forwarding gap this round, delivering multiple relation types now;
  (c) close as not warranted. Proposed in this PR.
- 2026-09-04: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md`. (PR #183.)
- 2026-09-04: implemented as `SERVER-001` v0.30.0 / FR-040 — exactly
  as designed, option (a) in full: `Entity` (`src/generic/entity.rs`,
  front-door, this library's first `SymmetricRelation` outside
  `research`-gated reference material), `kind` as the equality-
  filterable `IndexedField` (not inverted, unlike `Reminder::status`),
  `mention_count` as the durably-mutable `ScannableField`, one
  self-referential `RelatesTo` relation (`Dog::littermate_of`'s own
  shape), `EntityConnectionStore` (`src/server/entity.rs`,
  `server`-gated alone), a real `entity_server` binary, and
  `SchemaDrivenClient::traverse` (`src/server/client.rs`) — bounded
  client-side breadth-first walking over the existing
  `Request::Neighbors`, no new wire primitive of any kind. The
  `Symmetric`-forwarding gap this document named precisely stays
  unfixed, exactly as accepted under option (a). Four new unit tests
  in `src/generic/entity.rs` (run even under default, zero-feature
  builds), five in `src/server/entity.rs`, nine in
  `tests/server_entity_integration.rs` (new); every acceptance
  criterion 1–8 holds; no deviation. Full sweep green: `cargo fmt`,
  `cargo clippy -- -D warnings`, `cargo test --all-features`,
  `cargo test` (default features, 143 lib tests confirming front-door
  status), `cargo doc --all-features --no-deps` (64-warning baseline
  held). (This PR.)
