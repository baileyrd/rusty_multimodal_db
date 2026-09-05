# ADR-0042: `Entity` verified against `rusty_remind_me`'s source — findings and two follow-ups

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved option (a): the ten findings accepted, and both
  follow-ups (`ENT5-FR-001` normalization, `ENT5-FR-002` derived
  `entity_id`, adds `sha2`) taken as one implementation unit; (b)
  normalization-only and (c) informational-only both declined.
  Acceptance authorizes the follow-ups; implementation follows as its
  own unit — see "Acceptance and implementation" below.)
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-SOURCE-VERIFICATION-DESIGN.md`
  (the full findings this ADR summarizes, every one cited to a file:line
  in `baileyrd/rusty_remind_me@29602f1`), `ADR-0038` (Unit 41 — the
  same question asked of the tool schemas), `ADR-0039`/`ADR-0040`/
  `ADR-0041` (each carried the "source unread" caveat this resolves),
  `SERVER-001-FR-041`/`FR-042` (the merged shape measured against).
- Supersedes: none. Findings plus two bounded proposals.

## Context

Every `Entity` round since `ADR-0036` has been built against
`rusty_remind_me`'s MCP tool *schemas* and has said so plainly: the
source was never read, `MentionedWith` is a placeholder label, the
relation vocabulary is unknown. The owner's ordered pick "1b" was to
finally read it. The public repository was shallow-cloned at `29602f1`
and `crates/remind_me_core/src/entity.rs` (908 lines), the SQL schema,
the traverse/id tests, the MCP handler, and the owner's own graph ADR
were read directly.

Ten findings. The three that matter most reframe earlier inferences
rather than confirm them:

- **There is no relation vocabulary.** `entity_relations.relation` is
  free-form `TEXT`, written from each memory's own SPO predicate when
  both sides resolve to known entities. Unit 41's "multiple named
  relation types are real" was right; `ADR-0039`'s inference that they
  form a *fixed set knowable at construction* — the model
  `MultiSymmetric`/`RELATION_LABELS` implements — was not. The
  placeholder *strings* were never the mismatch; the fixed-set model is.
- **Edges are directed triples, walked bidirectionally, returned with
  direction, label, hop, and timestamps.** `MultiSymmetric` stores
  symmetric adjacency and `traverse` returns bare ids with depths.
- **Identity is a derived content-hash id**, `sha256(normalized
  name)[..12]`, so canonical-name lookup is one indexed hit. This
  crate's arbitrary `Uuid` ids make it two round trips — the open
  question `ADR-0039`/`ADR-0040` left standing has a cheap answer that
  keeps `RecordId = Uuid`.

And one small, unambiguous delta: their normalization collapses
*internal* whitespace; `FR-042`'s only trims — the exact earlier version
their own doc comment says split one person into two entities.

## Decision

Accept the ten findings as the record of what `rusty_remind_me` actually
does. Take two bounded follow-ups as one implementation unit:
`ENT5-FR-001` — `NameIndex::normalize` collapses internal whitespace
(`split_whitespace().join(" ").to_lowercase()`), in-process only, zero
migration; `ENT5-FR-002` — a `pub fn entity_id(name) -> Uuid` deriving
a full 128-bit `Uuid` from `sha256(normalize(name))`, a convention for
minting `Entity` ids so name→record is one `GetById`, `RecordId`
untouched, one new dependency (`sha2`). Name the three larger
divergences — open-label edges, directed edges with metadata, an
edges-with-hops traverse result — as real and now fully informed, for a
future design round, not this one. Retire the "source unread" caveat
from every doc that carries it.

## Consequences

- Positive: the standing caveat is gone — every claim about
  `rusty_remind_me` in this crate's docs is now traceable to a file:line
  in its source, not a schema reading.
- Positive: two cheap, high-value fixes with no wire, schema, or
  on-disk change — one prevents an identity split the reference already
  suffered and fixed; the other collapses canonical-name lookup to one
  round trip.
- Positive: the open-label/directed-edge question is now *decidable*
  rather than speculative — the owner can weigh a `MultiSymmetric`
  redesign against the real shape, not a guess.
- Named, not hidden: `ADR-0039`'s fixed-relation-set model is the wrong
  model for the target system. It was a reasonable inference from a
  schema that shows a `relation` filter parameter and nothing else; it
  is not what the source does. `MultiSymmetric` remains a correct,
  useful primitive for a domain that *does* have a fixed relation set
  (`Dog`, `Employee`) — it is simply not `rusty_remind_me`'s shape.
- Named, not hidden: `ENT5-FR-002` adds `sha2`, the first new
  `Cargo.toml` dependency since `rusty_tls`. Option (b) below avoids it.
- Named, not hidden: `mention_count` stays synthetic (F9); `kind` stays
  a required `String` (F6); `FilterEq` on `label` keeps returning every
  match rather than one (F7) — each a deliberate, now-informed keep.
- Nothing in `sync/graph.rs` or the hub/API crates was read — sync and
  HTTP transport are outside `SERVER-001`'s scope.

## Considered options

The design document's own "Considered options" covers two forks.
**What to do with the findings** — (a) **(proposed)** accept, take
`ENT5-FR-001` + `ENT5-FR-002` as one small unit, name F3/F4/F5 for a
future round; (b) accept, take only `ENT5-FR-001` — the normalization
fix alone, no new dependency, no id convention; (c) accept as
informational only, retire the caveat, change nothing. **Whether to copy
the 12-hex id truncation** — (a) **(proposed)** no, full 128-bit `Uuid`
from the same digest (byte interop is impossible anyway; the 48-bit
domain is one the reference's author calls inherited, not chosen); (b)
pad a 48-bit prefix [rejected — inherits a collision domain for no
gain].

## Acceptance and implementation

- Options offered at proposal: (a) accept the findings and take both
  follow-ups (`ENT5-FR-001` normalization + `ENT5-FR-002` derived
  `entity_id`, adds `sha2`) as one implementation unit; (b) accept the
  findings and take only the normalization fix (`ENT5-FR-001`), no new
  dependency; (c) accept the findings as informational — retire the
  "source unread" caveat everywhere, no engine change. In every option
  the open-label/directed-edge/traverse-result divergences (F3/F4/F5)
  are named for a future design round, not built. Proposed in PR #189.
- 2026-09-05: accepted as proposed (option (a); (b) and (c) declined).
  Implementation of `ENT5-FR-001`/`002` follows as `SERVER-001`'s next
  minor / FR, per `docs/design/SERVER-ENTITY-SOURCE-VERIFICATION-
  DESIGN.md`. F3/F4/F5 remain named, not built. (PR #189.)
