# Server Entity/Entity-Traverse Integration Verification (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved option (a), `ADR-0038`: revise `Entity`/`traverse` to
  match the real shape found here, including the `Symmetric`-
  forwarding fix; (b) accepting as informational and (c) deprecating
  `Entity` both declined). Acceptance authorizes the redesign's
  direction; the concrete mechanism follows as its own design round —
  see `ADR-0038`'s "Acceptance and implementation" section.
- Date: 2026-09-04
- Related: `ADR-0036`/`docs/design/SERVER-REMINDER-DOMAIN-DESIGN.md`,
  `ADR-0037`/`docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md` (both
  explicitly named the same open question this document closes:
  "whether `rusty_remind_me`'s real `entity`/`entity_upsert`/
  `entity_traverse` shape actually matches this document's own guess
  — unverified, since that repository was not read this session").

## Purpose and scope

Both preceding rounds in this `rusty_remind_me`-motivated line
(`ADR-0036`, `ADR-0037`) built real, tested engine capability
(`Reminder`, `Entity`) against a **guess** at `rusty_remind_me`'s real
shape, inferred only from its MCP tool *names* — `entity`,
`entity_upsert`, `entity_traverse` — because the `rusty_remind_me`
repository itself was never attached to either session. Both design
docs named this precisely as their own open question, not glossed
over.

This round closes that question — not by reading the `rusty_remind_me`
repository (still not attached to this session), but by a channel that
became available in this session and was not in either prior one: the
`rusty-remind-me` MCP server itself is now connected, exposing its
real tool *schemas* — parameter names, types, bounds, and descriptions
— which are the real, authoritative contract `rusty_remind_me` commits
to for any caller, not this project's own reasoned guess against a
name alone. `mcp__rusty-remind-me__remind_me_entity`,
`remind_me_entity_upsert`, and `remind_me_entity_traverse`'s schemas
were read directly (via `ToolSearch`), not assumed or reconstructed
from memory.

**Scope**: report the real shape found, compare it field-by-field and
capability-by-capability against what `Entity`/`traverse` (`ADR-0037`)
actually built, and let the owner decide what — if anything — follows.
This document proposes no engine change itself.

## Non-goals

- Not a redesign of `Entity`'s schema or relation model — that would
  be real, additional engine work, proportionate only if the owner
  picks option (a) below; this document's own job is the comparison,
  not the fix.
- Not reading `rusty_remind_me`'s source code — still not attached to
  this session. The tool *schema* is a real, authoritative contract
  (what any caller, including a future integration, must actually
  conform to), but it does not show internal storage, validation
  logic, or behavior beyond what the schema and description state.
  Real field *semantics* beyond what a parameter's name/type/
  description says are still inference, named as such below, not
  claimed as verified fact.
- Not live data from a real `rusty_remind_me` instance — `remind_me_
  entity`/`remind_me_entity_traverse` need a `name` to look up, and no
  enumeration tool exists in this server's exposed surface to list
  real entities/kinds/relations first (see Finding 5); a live
  `remind_me_stats` call was attempted and returned a server-side
  protocol error unrelated to this investigation, not retried. Every
  finding below rests on the tool schema/description alone, not on
  observed real records.

## Context and terminology

- **Tool schema vs. tool name**: `ADR-0036`/`ADR-0037` each inferred a
  reasonable field guess from the bare strings `entity`/`entity_
  upsert`/`entity_traverse` alone — no parameter list, no description,
  no bounds. This round instead has the real, machine-checked
  parameter schema `rusty_remind_me`'s own MCP server publishes for
  each call — a strictly stronger source, though still once removed
  from the server's actual internal storage.
- **This crate's `Entity`** (`ADR-0037`, `src/generic/entity.rs`):
  `Entity { id: Uuid, label: String, kind: EntityKind, mention_count:
  i64 }`, `EntityKind::{Person, Place, Organization, Concept, Event}`
  (a fixed 5-variant enum), one self-referential, unlabeled
  `RelatesTo` `SymmetricRelation`, addressed by `Uuid` (`GetById`/
  `FilterEq`/etc. all take a `RecordId`).

## Findings

1. **`rusty_remind_me`'s real entity key is `name`, not a `Uuid`.**
   `remind_me_entity`'s only required parameter is `name: string`
   ("resolved case- and whitespace-insensitively"); `remind_me_entity_
   upsert`'s only required parameter is likewise `name`. Nothing in
   either schema exposes or accepts an opaque id. This crate's `Entity`
   is addressed exclusively by `Uuid` (`GetById<Entity>`, every
   `ConnectionStore` method) — the identity model itself doesn't
   match, before any field is even compared.
2. **`kind` is a free-form string in `rusty_remind_me`, not a fixed
   enum.** `remind_me_entity_upsert`'s `kind` parameter is typed
   `string` with no enum constraint in its schema, and its own
   description ("Create a knowledge-graph entity, or update its
   kind") treats `kind` as an open, evolvable classification, not a
   closed set. `EntityKind::{Person, Place, Organization, Concept,
   Event}` is a fixed five-variant Rust enum — adding a sixth kind
   `rusty_remind_me` might use (`Tool`, `Repository`, whatever its
   real taxonomy is) would need a code change and a migration on this
   crate's side, something the real system's own `kind: string` never
   requires.
3. **`aliases` is a real, first-class field this crate's `Entity` has
   no analogue for at all.** `remind_me_entity_upsert` accepts
   `aliases: string[]`, and `remind_me_entity`'s own `name` parameter
   is explicitly resolved against aliases too ("Entity name or alias").
   Multiple names resolving to one entity is real, exercised
   capability in the target system; `Entity::label` is a single
   `String`, one name only.
4. **`mention_count` has no real counterpart.** Nothing in `remind_me_
   entity`'s or `remind_me_entity_upsert`'s schema exposes a mention
   count. What `remind_me_entity` *does* return instead is "the
   memories mentioning it" — a list of linked memory records, not a
   running integer tally. `mention_count: i64` (`ENT-FR-003`, the
   field this crate chose as `Entity`'s durably-mutable
   `ScannableField`) was this project's own invention, not observed
   in the real target at all — the real system tracks *which* memories
   mention an entity, not merely *how many*.
5. **No entity-enumeration tool exists in this server's exposed
   surface.** `remind_me_list` lists *memories* (filterable by
   category/tags/source), not entities; there is no `remind_me_
   entity_list`. Every entity operation this server exposes needs a
   `name` up front. This crate's `Entity` domain leans on `AllIds`/
   `scan_all` for `Query`/`Aggregate` (`GROUP BY kind`, `SELECT *`) —
   real, tested capability with no counterpart the real target's own
   exposed tool surface offers at all; a real integration would need
   its own separate enumeration mechanism (or none), not `AllIds`.
6. **Multiple named relation types are real, already-exercised
   capability in `rusty_remind_me` — not a hypothetical future need.**
   `remind_me_entity_traverse` accepts an optional `relation: string`
   parameter, "Optional: only follow edges whose relation label
   matches exactly," and its own description names the backing store
   `entity_relations` (plural edges, implicitly plural relation
   *kinds*, each labeled). This is exactly the capability `ADR-0037`'s
   own "Considered options" investigated and *deliberately did not
   build* — `Symmetric<S, R, Marker>` has no forwarding `Neighbors`
   impl for a second, independent marker, so this crate's `Entity` has
   exactly one undifferentiated `relates_to` edge kind, unable to
   distinguish, say, a `works_at` edge from a `collaborates_with` edge
   the way `remind_me_entity_traverse`'s own `relation` filter already
   can. This is the single most consequential finding: the gap
   `ADR-0037` named and left open is not a speculative "might matter
   someday" — the real target this whole line of work is motivated by
   already uses it.
7. **`traverse`'s bounding shape differs from `remind_me_entity_
   traverse`'s.** The real tool bounds `hops` at a hard maximum of 3
   (`maximum: 3` in its schema) and `cap` bounds the number of
   *relation edges* returned (`"Max number of relation edges to
   return"`, `maximum: 100`). `SchemaDrivenClient::traverse`
   (`ENT-FR-007`/`008`) bounds `max_depth` and `max_nodes` with no
   crate-side maximum at all (caller-supplied, unbounded), and
   `max_nodes` counts distinct *nodes* visited, not edges returned —
   a different unit entirely. Neither bound is wrong on its own terms,
   but they don't compose: a caller porting real `rusty_remind_me`
   traversal parameters onto this crate's `traverse` has no direct
   translation for either `hops`'s hard cap or `cap`'s edge-count
   semantics.
8. **What *did* hold up**: the traversal *shape itself* — "follow
   relation edges outward from a starting entity, bounded, both
   directions" — is qualitatively right; `remind_me_entity_traverse`'s
   own description ("follows entity_relations edges in both
   directions") matches `RelatesTo`'s symmetric, bidirectional nature
   and `traverse`'s own breadth-first walk exactly in kind, just not
   in the specific bound units or relation-type plurality above. The
   basic instinct — reuse this crate's relation/traversal machinery
   for an entity graph — was directionally correct; the specific
   three-field, one-relation guess was not.

## Considered options

**What to do with these findings.** Three options, the same
proportionality shape `ADR-0033`–`ADR-0037` have each already used
against a larger available scope:

**(a) Revise `Entity`/`traverse` to match the real shape** — `name`
(not `Uuid`) as the addressable key, `kind` as an open string not a
fixed enum, a real `aliases` field, drop `mention_count` for a real
memory-linkage concept, and — the real, load-bearing piece — actually
close the `Symmetric`-forwarding gap `ADR-0037` named so multiple
labeled relation types are possible, with `traverse` gaining a
`relation` filter and hop/edge-count bounds matching the real tool's
own units. This is the complete answer, but it is a substantial
redesign of an already-implemented, already-tested domain, plus the
real `crate::generic::store` engine work `ADR-0037` twice declined —
disproportionate to a verification round's own job, and premature
without knowing whether real integration is ever actually pursued.

**(b) (proposed) Accept the findings as informational; make no schema
or engine change now.** `Entity`/`traverse` stand as they are: real,
useful, fully tested reference capability for this crate's generic
schema library (a domain with a symmetric relation, a client-side
bounded-traversal capability neither existed before `ADR-0037`), but
explicitly **not** repositioned as `rusty_remind_me`'s literal backing
store — that was always this line of work's own named, unverified
assumption, now verified false in several concrete respects. A real
`rusty_remind_me` integration, if one is ever actually pursued, is its
own future design round working from this document's findings
directly (most likely closer to option (a)'s shape), not an
incremental patch layered onto the current `Entity`. This mirrors
`ADR-0037`'s own "one relation only, name the rest" proportionality
call, one level up: verify first, redesign only if the verified need
is then actually acted on.

**(c) Deprecate or remove `Entity`** — rejected: nothing about these
findings makes `Entity` broken or wrong on its own terms; it is
real, working, tested capability (a fifth domain, this library's first
`SymmetricRelation` outside `research`-gated material, a genuinely new
`traverse` capability) that simply does not, as built, match one
specific external system's real shape closely enough to serve as that
system's backing store unmodified. Removing working capability over a
mismatched *motivating* example, rather than the capability's own
merit, would be a worse call than either (a) or (b).

Option (b) proposed.

## Data/state and invariants

No change — this round is investigation only; every invariant
`ADR-0037`'s own design already established stands unmodified whether
(a), (b), or (c) is chosen (choosing (a) would revisit them in that
future round, not this document).

## Errors, failure, recovery, and observability

Not applicable — no code changes proposed by this document.

## Security, privacy, and compatibility

No change — no wire, `PROTOCOL_VERSION`, `ErrorCode`, or existing
domain's behavior is touched by this document itself, regardless of
which option is chosen (a future option-(a) round would need its own
"Security, privacy, and compatibility" analysis).

## Acceptance criteria

1. `mcp__rusty-remind-me__remind_me_entity`/`remind_me_entity_upsert`/
   `remind_me_entity_traverse`'s real parameter schemas — names,
   types, bounds, descriptions — are quoted or paraphrased accurately
   in this document's Findings, verifiable against the live tool
   definitions this session loaded.
2. Every divergence claimed (id/key model, `kind`'s openness,
   `aliases`, `mention_count`, enumeration, multiple relation types,
   traversal bound units) is traceable to a specific schema field or
   description string named in Findings, not asserted without a
   citation.
3. Every claim about this crate's own `Entity`/`traverse` (the
   comparison's other side) matches `ADR-0037`/`src/generic/entity.rs`/
   `src/server/client.rs` exactly, re-verified against the merged code,
   not assumed from memory of writing it.
4. The three options are genuinely distinct in scope and consequence,
   each naming its own real cost, mirroring this project's own
   established "named proportionality call" shape.
5. Docs-only: no source file under `src/**` touched; `cargo fmt --all
   -- --check` clean; `cargo test` (default features) unchanged.

## Verification plan

- Every Finding cross-checked against the actual tool schema JSON this
  session loaded via `ToolSearch` for `remind_me_entity`/`remind_me_
  entity_upsert`/`remind_me_entity_traverse` — quoted fields match
  verbatim.
- Every claim about `Entity`/`traverse`'s own current shape
  cross-checked against `src/generic/entity.rs`, `src/server/
  entity.rs`, `src/server/client.rs`, and `ADR-0037` on `main` post-
  merge, not assumed.
- `cargo fmt --all -- --check` clean; `cargo test` (default features)
  unchanged (docs-only round).

## Traceability

- Closes the open question `ADR-0036`'s own "Open questions" and
  `ADR-0037`'s own "Open questions" each named identically: whether
  `rusty_remind_me`'s real shape matches this line of work's own
  three-field, one-relation guess. Answer: partially — the traversal
  *shape* (bounded, bidirectional, relation-edge-based) was right in
  kind; the entity *schema* (key model, `kind`'s openness, `aliases`,
  `mention_count`) and the *plurality* of relation types were not.
- Not sourced from `docs/FUTURE-GROWTH.md` — the third round in the
  line `ADR-0036` started, recorded the same way there and in
  `ADR-0037`.
- No new `SERVER-001` FR — this document proposes no engine change.

## Open questions

- Whether option (a)'s full redesign is ever actually warranted —
  left entirely to the owner's own call on option (b), not decided
  here.
- Whether `rusty_remind_me`'s real internal storage/validation goes
  beyond what its tool schemas expose (unverifiable without reading
  its source, still not attached to this session) — the schema is a
  real contract, not necessarily the complete picture.
- Whether a live `remind_me_stats`/real-entity-data pass (blocked this
  round by a server-side protocol error on `remind_me_stats` and the
  absence of any entity-enumeration tool) would surface further
  divergences beyond what the schemas alone show — named, not
  resolved.

## Change history

- 2026-09-04: Initial proposal — verification findings against the
  now-connected `rusty-remind-me` MCP server's real tool schemas,
  closing the open question `ADR-0036`/`ADR-0037` each named. Option
  (b) proposed: accept as informational, no schema/engine change this
  round.
- 2026-09-04: Accepted, option (a) — revise `Entity`/`traverse` to
  match the real shape, including the `Symmetric`-forwarding fix; (b)
  and (c) declined. A follow-up design round specifies the concrete
  redesign before any implementation.
