# ADR-0031: A per-request access log — a second, independent sink family, kind and outcome only, never a payload

- Status: **Proposed** (not yet accepted; authorizes no implementation)
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-ACCESS-LOG-DESIGN.md` (the full design
  this ADR summarizes), `ADR-0029` /
  `docs/design/SERVER-AUTH-AUDIT-DESIGN.md` (the audit log; its second
  revisit trigger — *access logging is wanted — a volume and privacy
  decision, a second sink or a second event family* — is what this
  answers, and its Non-goals named the same gap), `ADR-0025` (the
  precedent against a fifth `serve` parameter), `ADR-0030` (Proposed;
  a sibling policy round, independent),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.20.0.
- Supersedes/Superseded by: none. Adds a second observer alongside the
  audit sink; changes no gate's decision, no response, no wire.

## Context

`ADR-0029` built the audit log and, on purpose, did not log successful
requests — a volume and privacy decision it declined to make for the
design that would come after it. The owner picked that design as the
last of the second four-item round.

The real choice is the trigger's own phrasing: a second sink, or a
second event family on the existing one. Coupling them (one switch,
two very different volumes and two different privacy postures) means
an operator who wants the compliance-relevant audit trail — the more
common ask, per the audit design's own framing — is forced to also
accept a per-request log, or forced to leave both off. Two independent
switches cost one more small module and answer that cleanly.

The owner selected this as the fourth of four directions. This ADR
proposes a design and authorizes no implementation — the posture
`ADR-0016` through `ADR-0030` took.

## Decision drivers

- Answer "who asked for what kind of request, how often, with what
  outcome shape" without becoming a data log — no id, field, value,
  token, or certificate, ever.
- Let the audit log and the access log be turned on independently, so
  neither operator's choice implies the other's cost or exposure.
- Change no gate decision, no response, no wire, no `serve` signature.
- Reuse what exists (`RequestKind`, the fail-open sink pattern) rather
  than inventing a second vocabulary.

## Considered options

1. **A second sink family (`AccessSink`/`AccessEvent`), hung on
   `AuthConfig` alongside `AuditSink`, firing once per dispatched
   request with kind and outcome shape only, never a payload** —
   proposed.
2. **Extend `AuditKind` with a `Handled` variant on the existing
   `AuditSink`.** Couples the two switches; rejected for exactly the
   reason `ADR-0029`'s Non-goals gave for deferring this in the first
   place.
3. **A verbosity level on one sink.** The same coupling with a knob;
   rejected.
4. **Log the record id or the full request.** A data log, not an
   access log; rejected — named as an open question for whoever wants
   it as its own, larger, privacy-reviewed feature.
5. **A new `ServeOptions` struct instead of another `AuthConfig`
   method.** The break `ADR-0029` deferred; still deferred — a real
   migration this round does not need to force. Restated as an open
   question, not decided.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/access.rs` (new, public): `Outcome::{Ok, Err(ErrorCode)}`,
  `AccessEvent { at, peer, class, request: RequestKind, outcome }`,
  `AccessEvent::line` (its own documented format, distinct from the
  audit log's), `AccessSink`, `NoAccessLog`, `StderrAccessLog`,
  `FileAccessLog` — the same fail-open shapes `audit.rs` already has.
- `src/server/mod.rs`: `AuthConfig::{with_access_log, access_log}`;
  `handle_connection` records one `AccessEvent` per dispatched request
  (never for `Hello`/`Authenticate`/a gate refusal — the audit log's
  territory), after the response is decided, outside every lock.
- `src/bin/dog_server.rs`: `SERVER_ACCESS_LOG` (`stderr` | path;
  independent of `SERVER_AUDIT_LOG`).
- `SERVER-001`'s next minor / FR (`ACC-FR-001`–`008`); `ADR-0029`'s
  second trigger and `SERVER-AUTH-AUDIT-DESIGN.md`'s access-logging
  non-goal resolved by pointer; `SPEC-REGISTRY`, `TRACEABILITY`,
  `ROADMAP` (`SERVER-ACCESS-LOG`), `PROJECT-STATUS`.
- Tests per the design's verification plan, including a test that
  turns both sinks on together and shows their streams are disjoint by
  kind.
- No `Cargo.toml`, wire, `PROTOCOL_VERSION`, store, or `serve`-signature
  change.

## Consequences

### Positive

- The volume and privacy decision `ADR-0029` deferred is made
  explicitly, with the coupling it warned against avoided by
  construction.
- An operator gets exactly the trail they asked for: audit-only,
  access-only, both, or neither.
- No new secret, id, or value ever appears; the guarantee is
  structural (the types have no field for one) and tested the same way
  the audit log's is.

### Negative / tradeoffs

- **A third module with a similar fail-open sink pattern** — some
  duplication with `audit.rs` (deliberately not factored into a shared
  base type this round, to keep the two families independent and
  simple to read each on its own).
- **A fourth cross-cutting `AuthConfig` knob** (after tokens, audit,
  and — if `ADR-0030` lands — rate limiting). `ADR-0029` already named
  the `ServeOptions` trigger for this kind of growth; this design adds
  to the count without pulling that trigger, on the judgment that one
  more opt-in method is not yet the real migration.
- **Real volume, when on**: one line per request, unlike the audit
  log's per-decision rate. Named plainly as the reason this is a
  second switch, not a reason to avoid building it.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0030`; every claim about the current code (`handle_connection`'s
  dispatch path, `AuthConfig`'s shape, `RequestKind`'s exhaustiveness)
  read from `main` `2cbdeda`. No probe: the mechanism is a struct and
  a fail-open write, the same shape `ADR-0029` already validated in
  production code.
- Revisit if: a per-record access trail is wanted — a real, larger,
  privacy-reviewed feature.
- Revisit if: a correlation id across a session's requests is wanted —
  its own privacy question.
- Revisit if: a fourth or fifth cross-cutting `AuthConfig` knob
  appears after this — the `ServeOptions` trigger `ADR-0029` named.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — a second,
  independent sink family, kind and outcome shape only, off by
  default; **(b)** accept but fold into `AuditKind` as a `Handled`
  variant on the existing `AuditSink` (option 2 above), coupling the
  two switches for a smaller API surface; **(c)** close as not
  warranted — the audit log alone stands, `SERVER-AUTH-AUDIT-DESIGN.md`'s
  non-goal stays a non-goal. Proposed in PR #165.
