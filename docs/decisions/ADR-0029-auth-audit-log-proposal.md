# ADR-0029: An audit log of admission, authentication, and authorization decisions — a sink on `AuthConfig`, off by default, fail-open with one notice

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): decisions-only events,
  the sink on `AuthConfig`, `NoAudit` by default, `StderrAudit` and
  `FileAudit`, fail-open with one notice, the eager handshake; (b)
  fail-closed and (c) close as not warranted declined; no changes
  requested). Acceptance authorizes the design; implementation follows
  as its own unit, after `ADR-0027`'s — see "Acceptance and
  implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-AUTH-AUDIT-DESIGN.md` (the full design
  this ADR summarizes), `ADR-0012` / `docs/design/SERVER-AUTH-DESIGN.md`
  (the token scheme; its named gap — *rate-limiting failed
  authentication attempts, locking out a connection after N failures,
  and any audit log of authentication attempts* — is what this answers
  the last third of), `ADR-0014` / `ADR-0023` (admission by handshake
  and by certificate, the first decision recorded), `ADR-0028`
  (Proposed; the eager handshake both designs need, adopted by
  whichever lands first), `ADR-0025` (the precedent against a fifth
  `serve` parameter), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0.
- Supersedes/Superseded by: none. Adds an observer to decisions
  `ADR-0012`/`ADR-0014`/`ADR-0023` make; changes none of them.

## Context

Since `ADR-0012` every security round has carried one line forward
unchanged: no rate limiting, no lockout, no audit log — "a real gap for
a genuinely adversarial network." The owner picked the audit log.

An audit log is the part of that gap that comes first: it is what a
rate-limiting policy would be tuned from, and it is useful without
one. It is also the part with the fewest decisions in it, provided
three are made plainly: *what* is recorded (decisions, never requests
or secrets), *where* the sink hangs (on the policy object the gates
already consult, so `serve` does not change), and *what happens when
the sink fails* (the server keeps serving, and says so once).

The one mechanism the design needs that the server lacks is a reason
for a rejected handshake: today the handshake runs under the first
read and a rejection is indistinguishable from a disconnect.
`ADR-0028`'s probe showed an eager `complete_handshake()` returns a
typed reason; this design adopts the same two-line change, so
whichever of the two lands first carries it.

The owner selected this as the fourth of four directions. This ADR
proposes a design and authorizes no implementation — the posture
`ADR-0016` through `ADR-0028` took.

## Decision drivers

- Answer "who was admitted, who was turned away, what was refused,
  when" from a file, after the fact.
- Record no secret and no data — tokens, certificates, ids, values
  never appear; the peer address is the one identifier.
- Change nothing the gates decide; change no signature; add no
  dependency; cost nothing when off.
- Never let the audit path take the server down.

## Considered options

1. **Decisions-only events, `AuditSink` on `AuthConfig`, `NoAudit`
   default, `StderrAudit`/`FileAudit`, fail-open with one notice,
   eager handshake** — proposed.
2. **Fail-closed** (a sink write failure ends the connection). The
   posture where an unrecorded action must not happen; a full disk
   then stops the server. Offered as option (b).
3. **A fifth `serve` parameter** or a `ServeOptions` struct. A
   signature change; `ADR-0025` declined the shape; rejected for one
   option.
4. **A `log`/`tracing` facade.** A dependency, and filterable — the
   wrong guarantee for an audit trail; rejected.
5. **Access logging** (a line per request). Volume and privacy
   decisions not made here; rejected, nameable later.
6. **Failures only.** Half an audit; rejected.
7. **Close** — the gap stays named. Offered as option (c).

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/audit.rs` (new, public): `AuditEvent { at, peer, kind }`,
  `AuditKind::{Admitted, HandshakeFailed, Authenticated,
  AuthenticationFailed, Refused, Disconnected}`, `Transport`,
  `RequestKind` (exhaustive over `Request`), `AuditEvent::line`,
  `AuditSink`, `NoAudit`, `StderrAudit`, `FileAudit::open`.
- `src/server/mod.rs`: `AuthConfig::with_audit(Arc<dyn AuditSink>)` and
  `audit()`; `handle_connection` takes the peer address once, completes
  the handshake eagerly, and records at its existing gates and on
  exit. No `Response` changes.
- `src/bin/dog_server.rs`: `SERVER_AUDIT_LOG` (`stderr` | path;
  unopenable is a startup error).
- `SERVER-001`'s next minor / FR (`AUD-FR-001`–`008`); the auth gap
  line in `SERVER-AUTH-DESIGN.md`, `ADR-0012`, and `SERVER-001`
  resolved for the audit third and restated for rate limiting and
  lockout; `SPEC-REGISTRY`, `TRACEABILITY`, `ROADMAP`
  (`SERVER-AUTH-AUDIT`), `PROJECT-STATUS`.
- Tests per the design's verification plan, including the secrecy
  assertion over every variant's line and the ordered event sequence
  for one connection.
- No `Cargo.toml`, wire, `PROTOCOL_VERSION`, store, or `serve`
  signature change.

## Consequences

### Positive

- The operator can answer the audit question from a file, with the
  exact peer, decision, and time — the first record this server has
  ever kept of anything.
- Nothing secret can leak through it, by construction and by test.
- Off by default and free when off; on, it costs a line per decision
  and never a line per successful request.
- Rate limiting, when it comes, has data.

### Negative / tradeoffs

- **Fail-open means an unrecorded admission is possible** when the
  sink fails; the notice is the mitigation, not a guarantee. Option
  (b) exists for operators who need the guarantee and accept the
  outage.
- **A hostile peer writes lines** — one per refusal, on its own
  schedule; the log does its job, and the volume is unbounded until
  rate limiting exists. No `fsync`, so it is a disk-space lever, not
  a latency one.
- **The peer address is identifying.** An operator turning the log on
  is choosing to keep it; the design says so.
- **One more `Arc<dyn>` on `AuthConfig`** and a `peer_addr` call per
  connection; negligible, but `AuthConfig` is no longer two strings.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0028`; every claim about the gates and the loop read from
  `main` `97da28c`; the eager-handshake mechanism was run in
  `ADR-0028`'s probe.
- Revisit if: rate limiting or lockout is wanted — the other two
  thirds of the named gap, a policy design that reads this log's
  event kinds.
- Revisit if: access logging is wanted — a volume and privacy
  decision, a second sink or a second event family.
- Revisit if: a second cross-cutting `serve` option appears —
  `ServeOptions` (option 3) becomes worth its breaking change.
- Revisit if: `ADR-0028` lands — `Admitted` may gain an explicit
  "classed by certificate" field.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed —
  decisions-only events, the sink on `AuthConfig`, `NoAudit` by
  default, `StderrAudit` and `FileAudit`, fail-open with one notice,
  the eager handshake; **(b)** accept fail-closed — the same, but a
  sink write failure ends the connection rather than dropping the
  event; **(c)** close as not warranted — the gap stays named as it
  has been since `ADR-0012`. Proposed in PR #151.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implemented after `ADR-0027`'s unit and before `ADR-0028`'s
  crate-side unit (which waits upstream), as `SERVER-001`'s next minor
  / FR, per `docs/design/SERVER-AUTH-AUDIT-DESIGN.md`. (PR #153.)
- 2026-09-03: implemented as `SERVER-001` v0.19.0 (FR-029) in PR #159
  — `src/server/audit.rs` (the event types, `AuditSink`, `NoAudit`,
  `StderrAudit`, `FileAudit`, the documented line), `AuthConfig::with_audit`
  / `audit()`, the eager TLS handshake, the records at the existing
  gates, a drop guard for `Disconnected`, fail-open with one notice,
  `SERVER_AUDIT_LOG` in `dog_server`. Three unit tests, one binary
  test, four integration tests; every acceptance criterion 1–7 holds;
  no deviation. Full sweep green (356 lib tests, 353 + 3; auth suite
  10/10, TLS suite 13/13).
