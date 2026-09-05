# ADR-0040: `Entity` aliases and case-insensitive name lookup

- Status: **Proposed**
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-ALIASES-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0039` (named `aliases` and
  case/whitespace-insensitive name resolution as paired Non-goals —
  this ADR closes both), `SERVER-001-FR-041` (`Entity` v2's own real,
  merged shape this round builds on), `ADR-0022` (the append-only
  wire-compatibility rules confirmed, directly, to not require a
  version bump for this round's own headline capability).
- Supersedes: none. Additive to `Entity` v2's existing shape.

## Context

`ADR-0039` accepted revising `Entity` to match `rusty_remind_me`'s
real shape but explicitly declined to build two of Unit 41's own
confirmed findings that round: `aliases` (real, first-class, multiple
alternate names per entity) and case/whitespace-insensitive `name`
resolution. Both were named plainly as deferred gaps, not built.

Investigating the current, real, merged `Entity` v2 shape directly
found: `label` (the field `ADR-0039`'s own accepted text called
`name`, never actually renamed — `FR-041`'s own recorded deviation)
carries every capability flag `false` specifically because
`GenericMmapStore<R, IndexMarker, ScanMarker>` structurally admits
exactly one `IndexedField` marker, and `kind` already occupies it.
Making `label`-or-`alias` lookups real needs a second index structure,
not a change to `kind`'s own role. Separately, `GenericMmapStore`'s
`records: HashMap<R::Id, R>` already holds the *full* struct, and the
record blob is `Vec<R>` serialized whole — so a new plain field
(`aliases: Vec<String>`) needs no mmap or blob-format change to become
durable. The one genuinely new wire limitation: `ScanValue` has no
list variant, so `aliases` cannot appear in any `GetById`/`Query`
response at all this round — a different, smaller kind of gap than
`label`'s own "every capability flag `false` but still representable"
shape.

The concrete design (`docs/design/SERVER-ENTITY-ALIASES-DESIGN.md`)
proposes a new, domain-agnostic secondary-index primitive, `NameIndex`
(the `MultiSymmetric`/`MultiNeighbors` naming and placement precedent,
one round later, applied to indices instead of relations), normalizing
both `label` and every alias by lowercasing and trimming, and wiring
`EntityConnectionStore::filter_eq` on `FIELD_LABEL` to a real lookup
through it. This needs **no `PROTOCOL_VERSION` change** — the entire
capability reuses `Request::FilterEq`/`ScanValue::Str`/`Response::
RecordList` exactly as they exist today; only a data value inside
`DomainSchema` (`label`'s `filter_eq` capability flag, `false` →
`true`) changes, not any struct's own shape.

## Decision

Adopt the design document's mechanism: `Entity` gains `aliases:
Vec<String>` (durable, no wire representation this round); a new
`NameIndex<S, R>` primitive plus `NameIndexed`/`FindByName` traits
(`src/generic/{store,query}.rs`) provide a normalized, case/
whitespace-insensitive secondary index rebuilt from the wrapped
store's own records (no new blob file — index keys are fully
derivable from records, unlike relation edges); `EntityProductionStack`
becomes `NameIndex<MultiSymmetric<GenericMmapStore<Entity, KindField,
MentionCountField>, Entity>, Entity>`; `EntityConnectionStore::
filter_eq` on `FIELD_LABEL` becomes real, matching `label` or any
alias, normalized; `DomainSchema` reports `label`'s `filter_eq` as
`true`. `aliases`' own wire-readability (a future `ScanValue::StrList`
variant or a relation-based remodeling) stays explicitly deferred.

## Consequences

- Positive: closes both of `ADR-0039`'s own named non-goals together,
  since they are the same underlying caller-facing capability
  ("resolve an id from a name, however it's spelled or capitalized").
- Positive: needs no `PROTOCOL_VERSION` bump — a genuinely smaller
  wire footprint than `Entity` v2's own protocol-10 round, found by
  investigating the real constraint (`ScanValue::Str` already
  suffices for the query side) rather than assumed to need one because
  the prior round did.
- Positive: `NameIndex` is a real, reusable, domain-agnostic primitive
  in `crate::generic`, not `Entity`-specific plumbing — any future
  domain that needs normalized secondary-key lookup can adopt it
  directly.
- Named, not hidden: `aliases` gains no wire representation at all
  this round — a genuinely new "durable but not wire-representable"
  category for this crate, distinct from `label`'s own prior
  "wire-representable, every capability flag `false`" shape. A caller
  cannot yet read an entity's alias list back over the protocol.
- Named, not hidden: normalization is ASCII-oriented
  (`to_lowercase`/`trim`), not full Unicode case folding — real,
  scoped narrowing, not silently glossed over.
- Named, not hidden: two entities sharing a normalized name or alias
  is not an error or a validation failure — both come back from
  `filter_eq`, collision handling left entirely to the caller.
- No change to `Reminder`/`Order`/`Employee`/`Dog`, `RecordId`,
  `PROTOCOL_VERSION`, or any pre-existing `Request`/`Response`
  variant's own field layout.

## Considered options

The design document's own "Considered options" section covers three
real forks: **where to normalize** — (a) **(proposed)** server-side,
always, so a caller may pass raw text; (b) client-side, server exact-
match only [rejected, pushes correctness-critical logic to every
caller independently]; (c) both [redundant]. **How to add the second
index** — (a) widen `GenericMmapStore`'s own type parameters to admit
a second index marker [rejected, a materially larger structural
rework of the mmap-slot file layout than this round's mandate]; (b)
**(proposed)** a new, independent wrapper primitive, `NameIndex`,
mirroring `MultiSymmetric`'s own precedent; (c) client-side full-table
scan, no server-side index [rejected, real `O(records)` cost this
crate has never accepted elsewhere an index is possible]. **What to
build for `aliases`' own readability this round** — (a) a new
`ScanValue::StrList` variant now, protocol 11 [real, small, but not
needed for this round's actual load-bearing capability]; (b)
**(proposed)** defer readability entirely, ship lookup-only with zero
protocol change; (c) remodel `aliases` as edges to synthetic
string-keyed nodes [rejected, this crate's relations connect two
`Record`-typed values, not a record to a bare `String` — a new,
disproportionate primitive].

## Acceptance and implementation

- Options offered at proposal: (a) accept as proposed — `aliases` as
  a new, durably-stored-but-not-wire-readable field, `NameIndex` as
  the new secondary-index primitive, `label`/alias lookup made real
  via the existing `FilterEq`/`ScanValue::Str` shapes, no protocol
  bump; (b) accept the lookup capability but defer `aliases` entirely
  (case/whitespace-insensitive `label` lookup only this round, no new
  field at all) — smaller still, `aliases` stays a pure future item
  rather than a durably-stored-but-invisible one; (c) close as not
  warranted — the two-round-trip cost (`FilterEq` then `GetById`) that
  a name-based caller already pays today is judged acceptable as-is,
  and neither gap is pursued further absent new evidence they matter.
  Proposed in this PR.
