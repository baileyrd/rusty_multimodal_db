# ADR-0041: `Entity::aliases` on the wire — `ScanValue::StrList` at protocol 11

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved option (a): the design as proposed, `ScanValue::
  StrList`/`ValueKind::StrList` at protocol 11 with rule-3 content
  stripping in `downgrade_for_version`; (b) the separator-joined `Str`
  fallback and (c) closing as not warranted both declined). Acceptance
  authorizes the design; implementation follows as its own unit — see
  "Acceptance and implementation" below.
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-ENTITY-ALIASES-WIRE-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0040` (shipped `aliases` as durable
  but not wire-representable and deferred exactly this), `ADR-0035`
  (the `F64` precedent — the last `ScanValue` addition, and the one
  point this ADR diverges from it), `ADR-0022` (rule 3, "nearest older
  shape," is the load-bearing clause), `ADR-0025`/`ADR-0033` (the two
  existing `downgrade_for_version` cases this extends).
- Supersedes: none. Additive; wire-only; no on-disk change.

## Context

`ADR-0040` gave `Entity` a durable `aliases: Vec<String>` and made
`FilterEq` on `label` resolve any alias — but a client cannot read the
list back, because `ScanValue` has no list variant. That round named
the gap as a genuinely new category ("durable but not
wire-representable") and deferred it with two candidate mechanisms.

Investigating the real merged shape directly found that adding the
variant itself is routine — `ScanValue` derives everything
`Vec<String>` satisfies, an append at index 5 changes no existing bytes
(rule 1), and every exhaustive-looking match over `ScanValue`/
`ValueKind` in the codebase already has a safe fallthrough. What is
*not* routine, and is the actual subject of this decision, is
backward compatibility: every prior appended variant was either a new
request (never sent below its version, rule 4) or rode only inside a
gated response (`F64` inside `Response::Groups`, protocol 9). A list
inside `Response::Record` is neither — `GetById` is a protocol-1
request a silent client can send, and its answer would carry a variant
that client cannot decode. The same holds for `Response::Rows`
(`SELECT *`, protocol 8) and `Response::Schema` (`DescribeSchema`,
protocol 1, which would carry an undecodable `ValueKind`).

Rule 3 already says what to do, and `downgrade_for_version`
(`src/server/mod.rs:716`) is already where it happens, on every
dispatched response, through one funnel (`mod.rs:2109`). Today it
remaps two `ErrorCode`s. This round is the first time it must rewrite a
response's *content*: strip the field a pre-11 client never knew
existed.

One further divergence from the `F64` precedent, reasoned rather than
copied: `F64` deliberately got no `ValueKind` because it "never
describes a stored field's real type." `aliases` *is* a stored field
with a `FieldDescriptor`, so `ValueKind::StrList` must exist — a schema
that names a field's kind wrongly (option (b) below) is worse than none.

## Decision

Adopt the design document's mechanism: append `ScanValue::StrList(Vec<
String>)` (index 5) and `ValueKind::StrList` (index 4); `PROTOCOL_
VERSION` 10 → 11 with version-table row 11. `Entity::aliases` gains
`FIELD_ALIASES = 3`, an all-capabilities-`false` `FieldDescriptor`, and
appears in `GetById`/`Query` results as a raw, un-normalized
`StrList`. `downgrade_for_version` gains three rule-3 arms for
`negotiated < 11`: strip `StrList` pairs from `Response::Record` and
each `Response::Rows` row, strip `StrList` descriptors from `Response::
Schema` — a pre-11 client sees exactly the three-field `Entity` it saw
at `FR-042`. No new `Request`, `ErrorCode`, or client API; every
write/filter/scan path on `aliases` reuses `Unsupported`/`Malformed`.
`NameIndex`, normalization, and the on-disk format are untouched.

## Consequences

- Positive: closes the one piece `ADR-0040` deferred; a client can now
  read an entity's aliases, not only resolve by them.
- Positive: zero new client API — `get`/`query` already return
  `(name, ScanValue)` pairs; a version-11 client gets the list for free,
  and every existing client-side kind/capability check already refuses
  the operations `aliases` doesn't support.
- Positive: `StrList` is general — any future list-of-strings field on
  any domain reuses it — while being proven on exactly one field, the
  single-instance precedent every prior primitive followed.
- Named, not hidden: **this is the first content-rewriting downgrade in
  this protocol's history**, and the first appended value variant that
  can reach a client which never negotiated for it. `downgrade_for_
  version`'s job widens from "remap an error code" to "remove a field
  the old client never had." The single funnel keeps it to one function;
  the design names the generalization (a per-kind introduced-at table)
  as not warranted for one instance.
- Named, not hidden: a `PROTOCOL_VERSION` bump for a read-only
  projection of one field. The owner may reasonably judge read-back not
  worth it — option (c).
- No on-disk change; no change to `Dog`/`Order`/`Employee`/`Reminder`;
  no change to any existing `Request`/`Response` variant's bytes.

## Considered options

The design document's own "Considered options" covers three forks.
**Protecting a sub-11 client** — (a) **(proposed)** rule-3 strip in
`downgrade_for_version`; (b) gate `GetById`/`Query`/`DescribeSchema`
below 11 [rejected outright — breaks every existing client of every
domain]; (c) document that old clients must avoid `Entity` [rejected —
a silent client cannot know its domain before `DescribeSchema`, which
itself would be undecodable]. **Whether `ValueKind` gains `StrList`** —
(a) **(proposed)** yes, it describes a stored field; (b) lie and call it
`Str` [rejected — the schema-driven client would build predicates the
server calls `Malformed`]; (c) omit the descriptor [rejected — a pair
with no schema entry has no name and cannot be projected]. **Whether to
build this at all** — (a) **(proposed)** `StrList` at protocol 11; (b)
join aliases into one `Str` with a separator [no bump, but lossy and
escaping-fraught — the same false economy ADR-0035 rejected for `AVG`];
(c) close as not warranted [real — lookup already works, no consumer of
the list itself is demonstrated].

## Acceptance and implementation

- Options offered at proposal: (a) accept as proposed — `ScanValue::
  StrList`/`ValueKind::StrList`, protocol 11, rule-3 content stripping
  in `downgrade_for_version`, `aliases` readable via `GetById`/`Query`
  with every capability flag `false`; (b) accept the separator-joined
  `Str` fallback instead — `aliases` exposed as one `Str` field, no
  protocol bump, lossy for any alias containing the separator; (c) close
  as not warranted — resolving *by* alias (`FR-042`) is enough until a
  real consumer of the list itself appears. Proposed in PR #188.
- 2026-09-05: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR (protocol
  11), per `docs/design/SERVER-ENTITY-ALIASES-WIRE-DESIGN.md`. (PR #188.)
