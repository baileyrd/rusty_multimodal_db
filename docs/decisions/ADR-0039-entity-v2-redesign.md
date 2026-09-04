# ADR-0039: `Entity` v2 redesign — the concrete mechanism for `ADR-0038` option (a)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved option (a): the full redesign as proposed, including
  `PROTOCOL_VERSION` 10's wire additions; (b) deferring the
  wire-protocol half and (c) closing as not warranted both declined).
  Acceptance authorizes the design; implementation follows as its own
  unit — see "Acceptance and implementation" below.
- Date: 2026-09-04
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0038` (accepted option (a) — this
  ADR specifies its concrete mechanism, which that acceptance
  explicitly deferred), `ADR-0037` (`FR-012`, the `Reversed`-forwards-
  `Neighbors` precedent this design's own `Symmetric` fix mirrors),
  `ADR-0022` (the append-only wire-compatibility rules this design's
  two new `Request`/`Response` variants follow).
- Supersedes: `ADR-0037`'s `Entity` field/relation shape — a straight
  revision, not an additive extension, since `Entity` has zero real
  deployed instances. Does not change any other domain, `RecordId`'s
  `Uuid` invariant, or any pre-existing wire variant's field layout.

## Context

`ADR-0038` accepted revising `Entity`/`traverse` to match
`rusty_remind_me`'s real shape (Unit 41's findings) but authorized
only the direction, since the concrete mechanism was unknown at
acceptance time. Investigating it directly found real architectural
facts `ADR-0038` did not have: `RecordId = Uuid` is a crate-wide
invariant stated in `protocol.rs`'s own doc comment, not an
`Entity`-local choice, so literally adopting `rusty_remind_me`'s
`name`-keyed identity would be a far larger break than "revise one
domain"; and `bincode`'s positional struct encoding means the only
wire-compatible way to add relation-type filtering is new, appended
`Request`/`Response` variants (`ADR-0022`'s own established pattern),
never new fields on `Request::Neighbors`/`DomainSchema` — a real
constraint, not a style preference.

The concrete design (`docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md`)
keeps `Uuid` as `Entity`'s identity, adds `name` as a real equality-
filterable field (the closest compatible approximation to real
name-based lookup), makes `kind` an open string filling both the
`IndexedField` and `ScannableField` roles `GenericMmapStore` structurally
requires (removing the need for `mention_count`'s synthetic role
entirely), and — the genuinely new, load-bearing piece — actually
closes the `Symmetric`-forwarding gap `ADR-0037` twice named, proven
against a second, honestly-labeled example relation (`MentionedWith`,
not a claimed real `rusty_remind_me` label), reachable over the wire
through two new, protocol-10, append-only `Request`/`Response`
variants (`NeighborsByRelation`/`ListRelationKinds`).

## Decision

Adopt the design document's concrete mechanism in full:
`Entity { id: Uuid, name: String, kind: String }`; two self-referential
`SymmetricRelation`s (`RelatesTo`, `MentionedWith`) over a nested
`Symmetric<Symmetric<..>, ..>` stack; the `Symmetric`-forwarding
`Neighbors` impl (`ENT2-FR-003`); `PROTOCOL_VERSION` moved to 10 with
`Request::NeighborsByRelation`/`ListRelationKinds` and `Response::
RelationKinds` appended; `SchemaDrivenClient::traverse` gaining an
optional relation filter. `aliases`, case-insensitive name resolution,
and server-side multi-hop traversal all stay explicitly deferred, not
built this round.

## Consequences

- Positive: `Entity` moves from "a reasonable guess, now known
  partially wrong" to a shape that matches `rusty_remind_me`'s real
  behavior in every respect this crate's own `Uuid`-keyed architecture
  can accommodate without breaking a crate-wide invariant.
- Positive: closes the `Symmetric`-forwarding gap for real — the same
  class of fix `FR-012` made for `Reversed`, now proven for `Symmetric`
  too, removing a previously-named architectural limitation from this
  library entirely, not just from `Entity`.
- Positive: `PROTOCOL_VERSION` 10's two new variants are genuinely
  reusable by any future domain that ever needs more than one relation
  type — not `Entity`-specific plumbing.
- Named, not hidden: `Entity` stays `Uuid`-keyed, not `name`-keyed —
  a real, permanent divergence from `rusty_remind_me`'s own identity
  model, not fully closed by this round and not closable without a
  crate-wide identity-model change this ADR does not propose.
- Named, not hidden: `MentionedWith` is an honest placeholder relation
  label, not a confirmed real `rusty_remind_me` vocabulary term — the
  mechanism is proven, the specific label is not yet verified.
- A real breaking change to `Entity`'s own already-merged shape (field
  set, on-disk record format, `traverse`'s signature) — justified only
  because zero real callers exist yet; the last round this reasoning
  is available without a real migration story.
- No change to `Reminder`/`Order`/`Employee`/`Dog`, `RecordId`, or any
  pre-existing `Request`/`Response` variant's own field layout.

## Considered options

Both real design forks are the design document's own "Considered
options" section: **identity** — (a) break `RecordId` to `String` for
`Entity` alone [rejected, crate-wide-precedent-breaking, larger than
this round's mandate], **(b) (proposed)** keep `Uuid`, add `name` as a
real field, **(c)** do nothing about identity [rejected, `name`-lookup
is real, load-bearing capability]. **Multi-relation wire reachability**
— (a) add a field to the existing `Neighbors`/`DomainSchema` [rejected
outright, breaks `bincode`'s positional encoding for every existing
connection], **(b) (proposed)** new, appended, version-gated `Request`/
`Response` variants, **(c)** a server-side batched multi-relation
traversal request [real additional engine work beyond this round's
own scope, `ADR-0037`'s client-side-traversal call left unrevisited].

## Acceptance and implementation

- Options offered at proposal: (a) accept as proposed — `Entity` v2
  exactly as designed (`name`/open `kind`, `MentionedWith` as a second
  example relation, the `Symmetric` fix, `PROTOCOL_VERSION` 10's two
  new variants); (b) accept but defer the wire-protocol half
  (`NeighborsByRelation`/`ListRelationKinds`/protocol 10) — ship the
  field/identity changes and the `Symmetric`-forwarding fix in-process
  only, reachable via `neighbors::<Entity, MentionedWith>` directly but
  not over the wire, closing the engine gap without a protocol bump
  this round; (c) close as not warranted — the `Uuid`-vs-`name`
  divergence named in Non-goals means even this redesign never fully
  matches `rusty_remind_me`, so stop here rather than invest further.
  Proposed in this PR.
- 2026-09-04: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR (protocol
  10), per `docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md`. (This
  PR.)
- 2026-09-04: **implemented** as `SERVER-001-FR-041` (v0.31.0, this
  PR). Two real architectural blockers surfaced by direct
  investigation before any implementation code was written, neither
  assumed away, both resolved with the owner via `AskUserQuestion`
  rather than silently reinterpreted — recorded here in full, since
  both are real deviations from this ADR's own accepted text.

  **Blocker 1 — `kind` cannot fill both structural roles.** This
  ADR's own "Decision" proposed `kind: String` filling both
  `IndexedField` and `ScannableField` (retiring `mention_count`'s
  synthetic role). That does not compile:
  `ScannableField::ScanValue: Copy`, and `GenericMmapStore`'s own mmap
  slot mechanism (`src/generic/mmap_field.rs`) is fixed-width only by
  `ADR-0009`'s own design — `String` is neither `Copy` nor
  fixed-width. Three options were put to the owner: `kind` becomes
  read-only (stays the `IndexedField`, `mention_count` stays,
  unretired, as the `ScannableField`); split `kind` into two fields
  (an indexed enum-like tag plus a separate free-text field); or
  revisit whether `kind` needs to be `String` at all. The owner picked
  the first, recommended option. `kind` stays exactly where `ADR-0037`
  had it structurally — the equality-filterable `IndexedField` — now
  open-ended rather than a fixed enum, but not durably updatable over
  the wire, the same always-read-only shape every other domain's
  `IndexedField` already has (`Order::status`, `Employee::department`).
  `mention_count` was not retired.

  **Blocker 2 — the `Symmetric`-forwarding fix does not compile.**
  This ADR's own "Decision" and "Context" (mirroring `ADR-0037`, Unit
  41 Finding 6, and `ADR-0038` before it) all named a generic
  `Neighbors`-forwarding impl for `Symmetric` — the same shape
  `Reversed` used for its own `Neighbors` forwarding fix (`FR-012`) —
  as the mechanism `MentionedWith` would prove real. It does not
  compile. Verified directly with `rustc` on two isolated test files
  before writing any real implementation code: a direct
  `Neighbors<R, Marker>` impl (which `Symmetric` needs for every
  existing single-relation domain, e.g. `Dog::littermate_of`) produces
  `E0119` (conflicting implementations) against any additional generic
  forwarding impl for a second, independent marker on the same struct.
  `Reversed` never faces this conflict, because its own relation
  (`ChildOf`) is a different trait entirely from the one it forwards
  (`Neighbors`) — there is no direct `Neighbors` impl on `Reversed` to
  conflict with. `Symmetric` has no such luxury: a direct `Neighbors`
  impl is exactly what makes it usable for every domain that has only
  one relation today. **This is a genuine, previously-unverified-and-
  wrong claim that had propagated across four prior documents**
  (`ADR-0037`, Unit 41 Finding 6, `ADR-0038`, and this ADR's own
  original "Decision" text) — none of them had actually compiled the
  fix; each cited the prior document's own claim. Caught here, before
  implementation, and corrected rather than perpetuated. Three options
  were put to the owner: a new runtime-keyed multi-relation primitive
  sidestepping the type-level conflict entirely; a nested
  `Symmetric<Symmetric<..>, ..>` nominal-type trick (rejected as
  investigated further — it does not actually resolve the same
  conflict, since both layers still compete for the same `Neighbors<R,
  ..>` impl surface on the outer type); or dropping the second
  relation from this round's scope entirely. The owner picked the
  first, recommended option. `MultiSymmetric<S, R>`/`MultiNeighbors<R>`
  (`src/generic/{store,query}.rs`, new) key relations by a runtime
  `String` label held in a `HashMap<String, HashMap<R::Id, Vec<R::Id>>>`
  rather than by a compile-time `Marker` — which also happens to match
  `Request::NeighborsByRelation`'s own wire shape exactly, since that
  field was always going to be a runtime string, never a compile-time
  type. Each relation's edge list stays independently durable via
  `STORAGE-016`'s own `EdgeBlob` mechanism, reused directly, one blob
  per label.

  **A third, smaller, real deviation, named here for the first time**:
  this ADR's own "Decision" prose described `label` being renamed to
  `name`. The field's identifier is unchanged — `src/generic/entity.rs`
  still declares `pub label: String`; its own doc comment states the
  field fulfills the "name" role concept this ADR describes, but the
  identifier itself was never renamed. Not caught via `AskUserQuestion`
  like the two blockers above (it is cosmetic, not structural), but
  recorded here rather than left silently inconsistent with this ADR's
  own text.

  Everything else in this ADR's "Decision" landed as proposed:
  `PROTOCOL_VERSION` 10 with `Request::NeighborsByRelation`/
  `ListRelationKinds`/`Response::RelationKinds`, all append-only, gated
  entirely client-side; `MentionedWith` as the second relation proving
  the (corrected) mechanism; `traverse`'s optional relation filter;
  `aliases`, case-insensitive name resolution, and server-side
  multi-hop traversal all still deferred. Full validation sweep green
  — see `docs/PROJECT-STATUS.md` item 117 and `SERVER-001-FR-041` for
  the complete test/tooling account.
