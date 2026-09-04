# ADR-0038: `Entity`/`traverse` vs. `rusty_remind_me`'s real shape — verification findings

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved option (a): revise `Entity`/`traverse` to match the
  real shape now, including the `Symmetric`-forwarding fix; (b)
  accepting the findings as informational with no change and (c)
  deprecating `Entity` both declined). Acceptance authorizes the
  redesign's *direction*; the concrete mechanism (exact schema,
  identity model, the `Symmetric`-forwarding fix's shape, migration
  posture for the already-shipped `Entity` domain) is real,
  additional design work, out of proportion to fold into this
  acceptance — see "Acceptance and implementation" below for the
  follow-up design round this authorizes.
- Date: 2026-09-04
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-INTEGRATION-VERIFICATION-DESIGN.md`
  (the full findings this ADR summarizes), `ADR-0036`/`ADR-0037` (the
  two preceding rounds whose own identically-named open question —
  "does `rusty_remind_me`'s real shape match this guess?" — this ADR
  answers).
- Supersedes/Superseded by: none. Proposes no engine, wire, or schema
  change of any kind — a decision about what (if anything) follows,
  not an implementation.

## Context

`ADR-0036` (`Reminder`) and `ADR-0037` (`Entity`) each built real
engine capability against a three-field, one-relation guess at
`rusty_remind_me`'s `entity`/`entity_upsert`/`entity_traverse` shape,
inferred only from those MCP tool *names* — the `rusty_remind_me`
repository itself was never attached to either session, and both
documents named this precisely as their own open question rather than
glossing over it.

This session gained a channel neither prior one had: the
`rusty-remind-me` MCP server itself is now connected, exposing its
real tool schemas directly. Reading them (`docs/design/SERVER-ENTITY-
INTEGRATION-VERIFICATION-DESIGN.md`'s Findings 1–8) found the guess
right in *kind* — a bounded, bidirectional relation-edge traversal
over a name-addressable entity — but wrong in several concrete
particulars: entities are addressed by `name`/`aliases`, not a `Uuid`;
`kind` is an open string, not a fixed enum; there is no real
`mention_count`, only linked-memory lists; and, most consequentially,
`remind_me_entity_traverse`'s own `relation` filter parameter proves
multiple named relation types are real, already-exercised capability
in the target system — exactly the gap `ADR-0037`'s own "Considered
options" named and deliberately left unfixed (`Symmetric`'s missing
`Neighbors`-forwarding for a second marker).

## Decision

Accept the findings as informational. Make no change to `Entity`,
`traverse`, or any engine code this round — option (b) in the design
document's own "Considered options". `Entity`/`traverse` stand as
real, tested, useful capability in their own right (this crate's fifth
domain, its first `SymmetricRelation` outside `research`-gated
material, a genuinely new bounded-traversal client capability); they
are simply no longer positioned as `rusty_remind_me`'s literal,
unmodified backing store — that positioning was always this line of
work's own unverified assumption, now verified partially false. A real
integration, if ever pursued, is its own future design round working
directly from this document's findings (most likely a redesign closer
to the rejected option (a): `name`-keyed entities, an open `kind`
string, real `aliases`, dropping `mention_count`, and actually closing
the `Symmetric`-forwarding gap so labeled multi-relation traversal
becomes possible) — not an incremental patch onto the current
`Entity`.

## Consequences

- Positive: the open question both `ADR-0036` and `ADR-0037` each
  explicitly named and left unresolved is now answered with real
  evidence — the actual tool contract, not a name-only guess — closing
  a piece of technical debt this line of work has carried since its
  first round.
- Positive: confirms, with real evidence rather than assumption, that
  the `Symmetric`-forwarding gap `ADR-0037` named is not speculative —
  `rusty_remind_me`'s own `relation`-filtered traversal already uses
  the capability this crate deliberately doesn't have. Any future
  integration round can point to Finding 6 directly rather than
  re-investigating from scratch.
- Named, not hidden: choosing option (b) means `Entity`, exactly as
  shipped, remains a poor direct fit for backing `rusty_remind_me` —
  real integration work, if ever wanted, is not merely "point a client
  at it," it is a real redesign.
- No change to any existing domain, `Request`/`Response`, `ErrorCode`,
  `PROTOCOL_VERSION`, or store code — this ADR authorizes no
  implementation.
- Real, unresolved limitation carried forward: the comparison rests on
  tool *schemas* alone, not `rusty_remind_me`'s real source (still
  unattached) or live entity/relation data (blocked this round by a
  `remind_me_stats` protocol error and the absence of any entity-
  enumeration tool) — named as the design document's own "Open
  questions," not claimed as exhaustive.

## Considered options

**(a) Revise `Entity`/`traverse` to match the real shape now** — the
complete answer (name-keyed entities, open `kind`, real `aliases`, a
real multi-relation fix to `Symmetric`, matched traversal bound units)
but a substantial redesign of already-shipped, already-tested capacity
plus the real `crate::generic::store` engine work `ADR-0037` twice
already declined, premature without confirming real integration is
actually wanted. **(b) (proposed) accept as informational, no change
this round** — verify first, redesign only if the verified need is
then actually acted on, the same proportionality shape `ADR-0037`
itself used at one level up. **(c) deprecate/remove `Entity`** —
rejected; nothing in these findings makes `Entity` broken on its own
terms, only mismatched to one external system's real shape.

## Acceptance and implementation

- Options offered at proposal: (a) revise `Entity`/`traverse` to match
  the real shape now, including the `Symmetric`-forwarding fix; (b)
  accept as informational, no schema/engine change this round; (c)
  deprecate/remove `Entity`. Proposed in this PR.
- 2026-09-04: accepted, option (a) — revise `Entity`/`traverse` to
  match the real shape, including the `Symmetric`-forwarding fix; (b)
  and (c) declined. The concrete redesign (schema, identity model, the
  `Symmetric` fix's mechanism, migration posture) is real, additional
  design work this acceptance authorizes but does not itself specify
  — follows as its own design round, `SERVER-ENTITY-V2-REDESIGN` (or
  equivalently named), before any implementation.
