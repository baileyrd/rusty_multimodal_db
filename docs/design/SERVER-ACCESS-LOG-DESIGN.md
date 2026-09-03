# Server Access Log Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, `ADR-0031` option (a); folding into
  `AuditKind` and closing declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit — see
  `ADR-0031`'s "Acceptance and implementation" section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-AUTH-AUDIT-DESIGN.md` / `ADR-0029` (the
  audit log, `SERVER-001` v0.19.0 / FR-029; its second revisit trigger
  — *access logging is wanted — a volume and privacy decision, a
  second sink or a second event family* — is what this document
  answers, choosing the second sink; its own Non-goals named access
  logging as "a line per successful request — a volume and privacy
  decision this crate has not made," which this document now makes),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.20.0 (its
  "structured logging/metrics" non-goal, which this does not reopen),
  `docs/design/SERVER-AUTH-RATE-LIMIT-DESIGN.md` / `ADR-0030`
  (Proposed; a sibling policy round on the same audit-adjacent
  surface, not depended on).

## Purpose and scope

The audit log answers "who was admitted, authenticated, or refused."
It deliberately does not answer "who asked for what" — every
*successful* request is invisible to it by design, because that is a
volume and privacy decision `SERVER-AUTH-AUDIT-DESIGN.md` declined to
make on the audit log's behalf. The owner picked that decision up as
the last of the second round of four.

**In scope:**

- A record of every request a connection's gates admit through to
  being answered — its kind, its outcome's shape, who asked, when —
  on a **separate sink family** from the audit log, so an operator who
  wants the audit trail is not forced to pay per-request volume, and
  an operator who wants the access log is not forced to reason about
  admission/authentication semantics to get it.
- **Never** a record id, a field, a value, or a token/certificate —
  the same secrecy invariant the audit log holds, extended to a
  request's *content*, not only its *credentials*.
- Off by default; one `AuthConfig` method to turn it on, matching
  every other opt-in server feature.
- `dog_server` configuration.

**Out of scope (see "Non-goals")**: request or response payloads,
timing/latency, sampling, a shared sink type with the audit log,
structured (JSON) output, log rotation, remote sinks, correlating a
request to the session or transaction it belongs to.

## Non-goals

- **Payloads.** No record id, field tag, or value, ever — a `GetById`
  is logged as `GetById`, not as which id. This is not a debugging or
  observability log; `SERVER-001`'s "structured logging/metrics"
  non-goal stands.
- **Latency or size.** No timing, no byte counts. A real observability
  story is the still-open non-goal this document does not reopen.
- **Sharing a type with `AuditSink`.** Named in `ADR-0029`'s trigger as
  the choice to make; this document chooses a second sink (see
  "Considered options") precisely so the two can be turned on
  independently.
- **Sampling or rate-based log reduction.** An operator who wants less
  volume turns the log off; a real sampling story is future work, not
  named as a trigger because nobody has asked for it yet.
- **Correlating requests within a session or transaction** (e.g. "these
  five `UpdateField`s belong to session X"). The log is a flat stream
  of independent events, like the audit log; a correlation id is a
  real feature with its own privacy question (it lets an observer
  reconstruct which requests came from the same connection even across
  reconnects if the id survives one) and is named as an open question,
  not decided here.
- **A fourth cross-cutting `AuthConfig` knob becoming a `ServeOptions`
  break.** `ADR-0029` already named this trigger for a *third* thing
  after audit and (if accepted) rate limiting; access logging would be
  the fourth thing hung on `AuthConfig`. This document keeps the
  precedent (`AuthConfig`, no `serve` signature change) and restates
  the trigger rather than pre-empting it — a real design, not a
  default this round should reach for.

## Context and terminology

- **Dispatched request**: one that reaches an answer past every gate
  (`Hello`, `Authenticate` when present, the unauthenticated gate, the
  `ReadOnly` gate, and, on a session-managing connection, the session
  intercepts) — the request the audit log's `Refused` events are the
  *complement* of. Concretely: every request `handle_connection`
  answers other than `Hello`/`Authenticate` themselves (already
  covered as `Admitted`/`Authenticated`/`AuthenticationFailed`) and a
  `Refused` case (already covered).
- **Outcome shape**: `Ok` or `Err(ErrorCode)` — the *kind* of answer,
  never its content. A `GetById` that finds nothing is `Ok` (`NotFound`
  is a normal outcome, per this crate's own convention, not an error);
  a `Transaction` that fails validation is `Err(code)` naming the
  code, not the index or the operations.
- **`AccessSink`**: the new trait, one method, mirroring `AuditSink`'s
  shape but a distinct type — `record(&AccessEvent)`.

### What the current code does, read from `main` `2cbdeda`

`handle_connection`'s main loop, after the auth/`ReadOnly` gates,
either intercepts a session request itself or calls
`dispatch(store, req)` and sends the result; nothing observes this
path. `AuthConfig` (v0.20.0) holds two tokens, an optional
`Arc<dyn AuditSink>`, and (if `ADR-0030` is accepted before this) an
optional rate-limit table; its `Debug` is hand-written. `audit.rs`'s
`RequestKind` already names every `Request` variant exhaustively and
is reused here rather than duplicated.

## Requirements

- `ACC-FR-001` — **A second sink family.** `src/server/access.rs`
  (new, public): `AccessEvent { at: u64, peer: Option<SocketAddr>,
  class: Option<TokenClass>, request: RequestKind, outcome: Outcome }`
  where `Outcome` is `Ok` or `Err(ErrorCode)` (the code alone, no
  message string — a message can echo request-specific text depending
  on the domain's `error_message` wording; the code is the closed,
  content-free part). `AccessSink` (one method); `NoAccessLog`
  (default); `StderrAccessLog`; `FileAccessLog` — the same shapes as
  `AuditSink`'s three, and independently instantiable (an operator may
  run both, either, or neither).
- `ACC-FR-002` — **Line format**, its own, distinct from the audit
  line so the two streams are never ambiguous even interleaved in one
  file: `access at=<unix> peer=<addr|-> class=<class|-> request=<Kind>
  outcome=<Ok|Err> [code=<ErrorCode>]`, built in one place,
  `AccessEvent::line`.
- `ACC-FR-003` — **Where it hangs.** `AuthConfig::with_access_log(Arc<dyn
  AccessSink>)` / `AuthConfig::access_log()` (defaulting to
  `NoAccessLog`) — the same object the audit sink and (if accepted)
  the rate limiter hang on, keeping `serve`'s signature unchanged
  (`ADR-0025`/`ADR-0029`'s precedent). `AuthConfig`'s `Debug` gains a
  third field, same shape as `audit`'s.
- `ACC-FR-004` — **Where it fires.** After a dispatched request (see
  "Context") is answered and the response is about to be sent — one
  record per request, in the connection's own thread, outside every
  lock, after the audit log's own recording for that path (if any) so
  the two streams stay in the order a reader would expect. `Hello`,
  `Authenticate`, and every gate-refused request are **not** logged
  here — they are the audit log's job, and logging them twice would
  make "access log on, audit log off" and "audit log on, access log
  off" behave inconsistently with each having its own complete story.
- `ACC-FR-005` — **Never a payload.** No code path constructs an
  `AccessEvent` from a `Request`'s or `Response`'s data fields — the
  `RequestKind`/`Outcome` types have no field capable of carrying one,
  the same structural guarantee `AuditKind` gives the audit log,
  checked by the same kind of test (no line for any variant can
  contain a marker id or value planted in the request).
- `ACC-FR-006` — **Fail-open, same as the audit log.** A write failure
  on `StderrAccessLog`/`FileAccessLog` drops the event, counts it, and
  prints one notice per process; never fails or blocks a connection.
- `ACC-FR-007` — **Configuration.** `dog_server` reads
  `SERVER_ACCESS_LOG` (`stderr` | path; unopenable is a startup
  error), independent of `SERVER_AUDIT_LOG`; the startup line reports
  both modes.
- `ACC-FR-008` — **Cost and compatibility.** With `NoAccessLog` (the
  default) every path is byte-for-byte v0.20.0 apart from one
  `Option` check per dispatched request. With a sink, cost is one
  formatted line **per request**, not per connection or per gate
  decision — the volume `SERVER-AUTH-AUDIT-DESIGN.md` deferred,
  named plainly here as the reason this is a second, independently
  switchable sink. No `Cargo.toml`, wire, `PROTOCOL_VERSION`, store,
  or `serve`-signature change. `SERVER-001` takes its next minor / FR.

## Considered options

**One sink family or two.**

1. **A second, independent sink and event type (proposed).** Turning on
   the access log costs nothing extra for the audit log and vice
   versa; an operator who wants only the security-relevant trail
   (most of them, per the audit design's own framing) is never forced
   to pay per-request volume to get it. Two small, parallel modules
   rather than one that conflates two different volumes and two
   different privacy postures.
2. **Extend `AuditKind` with a `Handled { class, request, outcome }`
   variant on the existing `AuditSink`.** Less new surface — one
   trait, one config method. Rejected: it makes "turn on the audit
   log" and "accept per-request volume" the same switch, which is
   exactly the coupling `ADR-0029`'s Non-goals refused to assume; an
   operator who audits admission/auth decisions for compliance reasons
   would involuntarily also get a request-volume log.
3. **One sink, a `verbosity` level.** A third option in the shape of
   option 2 with a knob; rejected for the same coupling, plus a
   config surface (levels) this crate has avoided elsewhere.

**What is logged per request.**

1. **Kind and outcome shape only (proposed).** Enough to answer "how
   many requests, of what kind, succeeded or failed, from where" —
   the access-log question — without becoming a data log.
2. **Kind, outcome, and the record id.** Rejected: a per-record access
   trail is a real, larger feature (who touched which record) with a
   privacy posture of its own (ids are frequently not secret, but
   *which ids one peer looked up* often is); not asked for, named as
   an open question.
3. **Full request/response.** Rejected outright — a debugging log, not
   an access log, and the crate's own non-goals already refuse it.

**Where it hangs.**

1. **`AuthConfig` (proposed).** Consistent with `AuditSink` and the
   (possible) rate limiter; `serve` unchanged.
2. **A new `ServeOptions`.** The break `ADR-0029` deferred; correctly
   deferred again here rather than triggered by convenience — three or
   four cross-cutting knobs on one struct is a real smell, but the
   fix is a real migration this round does not need to force.

## Proposed shape

```rust
// src/server/access.rs (new, pub)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { Ok, Err(ErrorCode) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEvent {
    pub at: u64,
    pub peer: Option<SocketAddr>,
    pub class: Option<TokenClass>,
    pub request: RequestKind,   // reused from super::audit
    pub outcome: Outcome,
}
impl AccessEvent {
    pub fn now(peer: Option<SocketAddr>, class: Option<TokenClass>, request: RequestKind, outcome: Outcome) -> Self;
    pub fn line(&self) -> String;   // ACC-FR-002
}
pub trait AccessSink: Send + Sync { fn record(&self, event: &AccessEvent); }
pub struct NoAccessLog;
pub struct StderrAccessLog { .. }   // same shape as audit::StderrAudit
pub struct FileAccessLog { .. }     // same shape as audit::FileAudit

// src/server/mod.rs
impl AuthConfig {
    pub fn with_access_log(mut self, sink: Arc<dyn AccessSink>) -> Self;
    pub fn access_log(&self) -> &dyn AccessSink;   // NoAccessLog by default
}
// handle_connection, after a dispatched request is answered:
auth.access_log().record(&AccessEvent::now(peer, Some(class), audit::RequestKind::of(&req), outcome_of(&resp)));

// src/bin/dog_server.rs: SERVER_ACCESS_LOG = "stderr" | <path>
```

`outcome_of(&Response) -> Outcome` is a small exhaustive match kept
next to `error_message` — `Response::Err{code,..}`/`TransactionFailed{code,..}`
→ `Err(code)`, everything else (including `NotFound`/`NoParent`) →
`Ok`.

## Data/state and invariants

- `AccessSink` and `AuditSink` are independent: neither's presence or
  absence changes the other's behavior or ordering guarantees.
- Exactly one `AccessEvent` per dispatched request (never for `Hello`/
  `Authenticate`/a gate refusal — the audit log's exclusive territory,
  `ACC-FR-004`).
- `class` is `None` only when `AuthConfig` is unconfigured *and* the
  request is answered before any class would apply — in practice
  every dispatched request has a class (unauthenticated ones are
  refused, hence audited, not logged here); the field stays
  `Option` for the unconfigured-server case (`Some(ReadWrite)`
  always, in fact) to mirror `AuditKind::Admitted`'s shape rather than
  assert something the type system need not.

## Errors, failure, recovery, and observability

- Fail-open, identical posture to the audit log (`ACC-FR-006`).
- A malformed `SERVER_ACCESS_LOG` path is a startup error, independent
  of `SERVER_AUDIT_LOG`'s.
- The log itself is the observability.

## Security, privacy, and compatibility

- No secret, id, field, or value in any line — enforced the same way
  the audit log's secrecy is, by a test with no field in the type to
  carry one.
- Peer address and class are identifying (which client, at what
  privilege, asked how often) — the same consent model as the audit
  log: an operator turns this on knowingly, and the design says so
  where it names the config method.
- Two independent switches mean an operator's choice to audit
  admission decisions does not imply a choice to log request volume,
  and vice versa — the privacy point this whole document exists to
  make precise.
- Backward compatible by construction: `NoAccessLog` is the default,
  takes no new branch beyond one `Option` check, and every existing
  test is unaffected.

## Acceptance criteria

1. `AccessEvent`/`Outcome`/`AccessSink`/`NoAccessLog`/`StderrAccessLog`/
   `FileAccessLog` exist as specified; `AccessEvent::line` produces the
   documented format, distinguishable from an audit line at a glance
   (different leading key, `access` vs `audit`); a test asserts no
   line for any `RequestKind`/`Outcome` combination can contain a
   marker id or value planted in a real request.
2. A collecting sink sees exactly one event per dispatched request —
   a `GetById`, an `UpdateField`, a `Transaction` that fails validation
   — each with the right `RequestKind` and `Outcome`, and **no** event
   for `Hello`, `Authenticate`, or a gate-refused request (those
   appear on the audit log's collecting sink instead, in the same
   test, showing the two streams are disjoint).
3. Turning on the access log with the audit log off, and the audit log
   with the access log off, each produce exactly their own stream —
   no request ever produces an audit event and no admission/auth
   decision ever produces an access event.
4. `FileAccessLog` appends one line per event across two connections;
   a sink whose file is made unwritable drops events, prints the
   notice once, and every connection still gets its responses.
5. `NoAccessLog` default: `AuthConfig::new`/`from_env` unchanged; every
   `Response` in every existing test is unchanged with both sinks
   configured; no `Cargo.toml`, wire, `PROTOCOL_VERSION`, store, or
   `serve`-signature change; `dog_server` honors `SERVER_ACCESS_LOG`
   independently of `SERVER_AUDIT_LOG` and refuses an unopenable path.

## Verification plan

- `src/server/access.rs` unit tests: the line format, the secrecy
  assertion, `FileAccessLog` append and failure (mirroring `audit.rs`'s
  own tests).
- `tests/server_auth_integration.rs`: criteria 2–3 with both a
  collecting `AccessSink` and a collecting `AuditSink` on the same
  server, asserting the two streams' contents are disjoint by kind.
- `src/bin/dog_server.rs`: `SERVER_ACCESS_LOG`'s table, mirroring
  `audit_sink_from`.

## Traceability

- → `SERVER-001` next minor / FR (`ACC-FR-001`–`008`), `ADR-0031`;
  resolves `ADR-0029`'s second revisit trigger and
  `SERVER-AUTH-AUDIT-DESIGN.md`'s access-logging non-goal by pointer.
- Roadmap: `SERVER-ACCESS-LOG-DESIGN` (this document), then
  `SERVER-ACCESS-LOG` as the implementation unit if accepted.

## Open questions

- Whether a per-record access trail (option 2 under "What is logged")
  is ever wanted — a real, larger feature with its own privacy
  question, named, not proposed.
- Whether a correlation id (same session, same connection) belongs in
  a later revision — named as a non-goal, not decided.
- Whether three or four `AuthConfig` knobs (tokens, audit, rate limit,
  access log) is the signal `ADR-0029`'s `ServeOptions` trigger meant
  — not decided here; restated as still open.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting the
  access-logging design round as the fourth of four next directions
  ("1, 2, 3, 4"), after the rusty_tls accessor (upstream, PR opened by
  a child session), rate limiting and lockout (Proposed), and
  stage-time validation (implemented). (PR #165.)
- 2026-09-03: Accepted as proposed; implemented as `SERVER-001`
  v0.23.0 / FR-033 (this PR), the owner's "Start Unit 28" immediately
  after Unit 27 (`ADR-0030`) landed on the same still-open PR. Every
  requirement `ACC-FR-001`–`008` delivered exactly as designed; every
  acceptance criterion 1–5 holds; no deviation.
