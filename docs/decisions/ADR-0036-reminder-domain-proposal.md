# ADR-0036: `Reminder` domain — the generic schema library's first front-door domain

- Status: **Proposed**
- Date: 2026-09-04
- Deciders: baileyrd
- Related: `docs/design/SERVER-REMINDER-DOMAIN-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0009`/`GENERIC-SCHEMA-DESIGN`
  (`crate::generic`, the engine this reuses unchanged), `SERVER-001`
  `FR-004`/`FR-005`/`FR-012` (`Dog`/`Order`/`Employee` — the three
  existing domain adapters this proposal follows exactly), `ADR-0034`/
  `ADR-0035` (`Request::Query`/`Request::Aggregate` — already fully
  domain-agnostic, needing no change here).
- Supersedes/Superseded by: none. Adds one new domain and its
  `ConnectionStore` adapter; changes no existing `Request`/`Response`
  variant, no `PROTOCOL_VERSION`, no `ErrorCode`, no existing domain's
  behavior.

## Context

The owner asked, in conversation, whether this crate could be used to
back a real external tool, `rusty_remind_me` (a separate MCP-exposed
reminder/memory system). A scoping discussion found that most of what
`rusty_remind_me` does — semantic search, schema-less memory content,
an entity graph, provenance/history — is a genuinely different kind of
database than this crate builds, and out of scope for one bounded
round. What does fit, with zero new storage-adjacent primitives: the
reminders themselves, a small fixed-schema record this crate's
existing generic schema library already handles the shape of.

This is the first design round in this project not sourced from
`docs/FUTURE-GROWTH.md` — recorded here rather than added there, since
it originates from a specific external integration ask, not a
previously-named future direction.

## Decision

Add `Reminder` as this crate's fourth domain, built on the already-
accepted generic schema library
(`crate::generic::production::GenericProductionStore`) and exposed
over the already-accepted server/query layer, with two choices that
depart from the three existing domains' own precedent:

1. **Front-door, not `research`-gated.** Every domain built on
   `crate::generic` so far (`Order`/`Customer`, `Employee`) has lived
   behind `research`, as reference/validation material for the library
   itself. `Reminder` is proposed as the library's first real,
   deployable appearance outside `research` — directly serving the
   round's own motivation: a domain an external tool's default build
   could actually reach.
2. **`status` (not a plain number) is the durably-mutable
   `ScannableField`; `due_at_unix_ms` is the equality-filterable
   `IndexedField`.** Every existing domain inverts this (an enum
   indexed, a number scanned). Since `Request::Query`/`Request::Aggregate`
   (`ADR-0034`/`ADR-0035`) already make range/ordering search on any
   field possible regardless of its `filter_eq` capability, there is no
   real cost to this inversion, and the benefit is concrete: marking a
   reminder done works via plain `UpdateField`, no SQL required.

No relation of either kind is proposed (`parent_children: false`,
`neighbors: false`) — the one combination no existing adapter has —
so `ReminderProductionStack` needs no `Symmetric`/`Reversed`
composition layer at all, just `GenericMmapStore` directly: the
simplest domain shape this library supports. A real, runnable
`reminder_server` binary (mirroring `dog_server`) is proposed
alongside the adapter, since a domain with no runnable server would
not actually be reachable — without it this round would not meet its
own stated motivation.

**Not decided by this document:** whether or how `rusty_remind_me`
itself is changed to talk to this crate. `rusty_remind_me`'s source
was not read this session; the reminder field shape here
(`title`/`due_at_unix_ms`/`status`) is inferred from its MCP tool
*names* alone and named explicitly as an unverified assumption. A
follow-on integration unit — checking that assumption against the real
target, then building whatever bridge is needed — is deliberately out
of scope here.

## Consequences

- Positive: a real domain reachable via a real binary, at essentially
  zero engine cost — no wire-protocol, `PROTOCOL_VERSION`, `ErrorCode`,
  or `dispatch` change; `Request::Transaction`, every session kind, and
  journaled crash-atomicity all work for `Reminder` for free, the
  fourth instance of an already-proven pattern.
- Positive: demonstrates the generic schema library is genuinely
  reusable outside `research` — a real question this crate's own
  architecture has left open since `ADR-0009`, now answered by
  construction rather than argument.
- Cost: the front-door placement and the inverted index/scan
  assignment are both new ground, not mechanical repeats of `Order`/
  `Employee`'s own wiring — each named plainly in the design document
  rather than glossed over as "just another domain."
- Real, named gap: the actual value of this work depends on an
  unverified assumption about `rusty_remind_me`'s real schema. If that
  assumption is wrong, `Reminder`'s three fields may need to change
  before any real integration is useful — a follow-on cost this ADR
  does not resolve, only names.
- No change to `Order`/`Employee`'s own `research` gating, `Dog`'s
  bespoke store, or any existing `Request`/`Response`/`ErrorCode`.

## Considered options

**(a) Accept as proposed** — `Reminder` as designed: front-door,
`status` scanned/`due_at_unix_ms` indexed, no relations, a real
`reminder_server` binary. **(b) Accept, but keep `Reminder` behind
`research`** — matches `Order`/`Employee`'s own precedent exactly,
cheaper to justify, but undercuts the round's own stated purpose (a
`research`-gated domain is not one an external consumer's default
build compiles in). **(c) Close as not warranted** — the
`rusty_remind_me` motivation doesn't justify even this bounded slice;
revisit only if a concrete need resurfaces.

## Acceptance and implementation

- Options offered at proposal: (a) accept as proposed — front-door
  `Reminder` domain, inverted index/scan assignment, no relations, a
  real `reminder_server` binary; (b) accept but keep it `research`-
  gated like `Order`/`Employee`; (c) close as not warranted. Proposed
  in this PR.
