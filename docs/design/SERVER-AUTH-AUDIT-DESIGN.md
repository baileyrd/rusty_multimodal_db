# Server Authentication Audit Log Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, `ADR-0029` option (a); fail-closed
  and closing declined; no changes requested). Acceptance authorizes
  the design; implementation follows as its own unit, after `ADR-0027`'s
  — see `ADR-0029`'s "Acceptance and implementation" section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-AUTH-DESIGN.md` / `ADR-0012` (the token
  scheme; its "Out of scope, named rather than silently assumed
  solved: … any audit log of authentication attempts" is what this
  document answers, and `ADR-0012`'s "no audit log of who did what"
  consequence), `docs/design/SERVER-MTLS-DESIGN.md` / `ADR-0023`
  (admission by certificate — the third kind of decision this records),
  `docs/design/SERVER-MTLS-CLASS-DESIGN.md` / `ADR-0028` (Proposed;
  the eager handshake that gives the server a typed rejection reason,
  which this design records — and adopts on its own if `ADR-0028` is
  declined), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0 (its "structured logging/metrics" non-goal, which this does
  *not* reopen), `src/bin/dog_server.rs` (the one `eprintln!` the
  server has today).

## Purpose and scope

`SERVER-AUTH-DESIGN.md` closed the "anyone who can connect can do
anything" gap and, in the same breath, named what it did not do:
"rate-limiting failed authentication attempts, locking out a
connection after N failures, and any audit log of authentication
attempts. A real gap for a genuinely adversarial network." Every
later security round (`SERVER-TLS`, `SERVER-MTLS`) carried the line
forward unchanged. The owner picked the audit log as the fourth of
four directions.

An audit log answers one question after the fact: *who was let in,
who was turned away, and what was refused, when.* It is not diagnostic
logging (`SERVER-001` keeps "structured logging/metrics" out of scope
and this document does not reopen it), and it is not access logging
(a line per successful request — a volume and privacy decision this
crate has not made and this document does not make — *made in
`SERVER-ACCESS-LOG-DESIGN.md` / `ADR-0031` (Proposed): a second,
independent sink*). It is the record
of every decision the server's three gates take:

- **admission** — the TLS handshake, with or without a client
  certificate (`ADR-0014`, `ADR-0023`);
- **authentication** — `Authenticate`, and a certificate-derived class
  if `ADR-0028` lands;
- **authorization** — every request refused `Unauthenticated` or
  `Unauthorized`.

**In scope:**

- `AuditEvent`, the closed set of records, each carrying the peer
  address and a Unix timestamp and never a token, a certificate, a
  record id, or a value.
- `AuditSink`, a trait with one method, plus three implementations:
  `NoAudit` (the default), `StderrAudit`, `FileAudit` (append-only,
  one line per event).
- Where the sink hangs (`AuthConfig::with_audit`), where it is called
  (`handle_connection`'s existing gates — no new gate), and what
  happens when it fails (nothing, visibly).
- The eager server-side handshake, so a rejected admission has a
  reason to record — shared with `ADR-0028`, adopted here regardless.
- `dog_server`'s `SERVER_AUDIT_LOG`.

**Out of scope (see "Non-goals")**: rate limiting and lockout,
access logging, a log facade dependency, log rotation, remote sinks.

## Non-goals

- **Rate limiting and lockout.** The other half of the named gap, a
  separate policy design (what to count, per what key, for how long,
  and what "locked out" answers). An audit log is what such a policy
  would be tuned from; it comes first. Named as `ADR-0029`'s first
  revisit trigger.
- **Access logging.** No line per successful request: volume and
  privacy (record ids, values) are a different decision.
- **A logging facade** (`log`, `tracing`). A new dependency
  (`AGENTS.md`), and the wrong shape: an audit trail must not be
  filterable away by a log level or swallowed by an unset global
  logger. The sink is explicit and typed.
- **Rotation, remote sinks, structured formats beyond one line per
  event.** `FileAudit` appends; the operator rotates. A JSON sink is a
  trait impl anyone can write.
- **Any change to what the gates decide.** Every `Response` is
  byte-identical; the sink observes.

## Context and terminology

- **Gate**: a point in `handle_connection` that can refuse: the TLS
  handshake (`accept` + the handshake, lazy today), the
  `Authenticate` arm (`auth.check`), the unauthenticated gate (every
  request before a class is known), the `ReadOnly` gate
  (`UpdateField`/`Transaction`/`Commit`).
- **Peer**: `TcpStream::peer_addr()`, taken once at accept; `None` if
  the OS cannot say (recorded as such, never fabricated).
- **Sink**: an object every connection thread shares (`Arc<dyn
  AuditSink>`), called synchronously on the connection's own thread at
  the gate, after the decision and before the response is written.
- **Fail-open**: a sink that cannot write drops the event and the
  connection proceeds exactly as it would have; the drop is counted
  and reported once.

### What the current code does, read from `main` `97da28c`

`handle_connection` sets `TCP_NODELAY`, wraps the stream in TLS if
configured (`accept`, no I/O; the handshake runs under the first
read), initializes `authenticated` from `auth.is_configured()`, and
loops: `Hello` first-frame handling; the `Authenticate` arm calling
`auth.check(token)` and answering `Ok` or `Unauthenticated`; the
unauthenticated gate answering `Unauthenticated` for any other request
before a class is known; the `ReadOnly` gate answering `Unauthorized`
for the three write shapes; then the session intercepts and
`dispatch`. Nothing is recorded anywhere; the binary's only output is
one startup `eprintln!`. `AuthConfig` is two `Option<String>`s and a
constant-time `check`. `serve` takes `(listener, store, auth, tls)`.

## Requirements

- `AUD-FR-001` — **The events.** `pub enum AuditEvent`, each variant
  carrying `peer: Option<SocketAddr>` and `at: u64` (Unix seconds,
  `SystemTime::now()`, no dependency):
  `Admitted { transport: Transport, initial_class: Option<TokenClass> }`
  (a connection past the handshake — `Plain`, `Tls`, or `MutualTls`;
  the class it starts at, `None` meaning "must authenticate");
  `HandshakeFailed { reason: String }` (the `rusty_tls::Error`'s
  `Display`, e.g. `NoCertificatesPresented`);
  `Authenticated { class: TokenClass }`;
  `AuthenticationFailed` (a token that matched nothing — the token is
  not recorded, nor its length);
  `Refused { class: Option<TokenClass>, request: RequestKind, code: ErrorCode }`
  (`Unauthenticated` or `Unauthorized`; `RequestKind` is the variant
  name only, a small `Copy` enum — never a payload);
  `Disconnected`. Nothing else: no successful request is recorded.
- `AUD-FR-002` — **The sink.** `pub trait AuditSink: Send + Sync {
  fn record(&self, event: &AuditEvent); }`. `NoAudit` (the default;
  `record` is empty and the call is a no-op the compiler can see
  through), `StderrAudit`, `FileAudit::open(path)` (append, create,
  one line per event, `write_all` + `flush` under a `Mutex`, no
  `fsync`). The line format is fixed and documented:
  `audit at=<unix> peer=<addr|-> event=<Variant> [field=value ...]`,
  space-separated, values without spaces (addresses and enum names),
  so `grep`/`awk` suffice.
- `AUD-FR-003` — **Where it hangs.** `AuthConfig::with_audit(self,
  sink: Arc<dyn AuditSink>) -> Self`; `AuthConfig::audit(&self) ->
  &dyn AuditSink` (defaulting to `NoAudit`). `serve`'s signature is
  unchanged (`ADR-0025`'s precedent against a fifth parameter): every
  decision this records is one `handle_connection` takes with `auth`
  in hand, and `AuthConfig` is the policy object for admission and
  authorization already.
- `AUD-FR-004` — **Where it is called.** Exactly at the existing
  gates, after the decision, before the response: the eager handshake
  (`HandshakeFailed` or `Admitted`), the `Authenticate` arm
  (`Authenticated`/`AuthenticationFailed`), the unauthenticated gate
  and the `ReadOnly` gate (`Refused`), and the loop's exit
  (`Disconnected`). No new gate, no reordering, no change to any
  `Response`.
- `AUD-FR-005` — **Eager handshake.** On a TLS connection
  `handle_connection` calls `complete_handshake()` after `accept` and
  before the frame loop, so a rejected admission has a typed reason
  (`ADR-0028`'s `CLS-FR-002`, identical; whichever lands first carries
  it, the other inherits it). Client-visible behavior is unchanged
  (`TLS-FR-003`).
- `AUD-FR-006` — **Failure.** A sink is never allowed to fail a
  connection or block it beyond its own write: `FileAudit` and
  `StderrAudit` ignore write errors, count them, and print one line to
  stderr at the first (`audit sink failing: <err>; events are being
  dropped`), never again per process. A panicking sink is a bug in
  the sink; `record` is called outside every lock this crate holds.
  `ADR-0029` offers fail-closed as option (b).
- `AUD-FR-007` — **Secrecy and privacy.** No event carries a token,
  any part of one, a certificate or any part of one, a record id, a
  field, or a value. The peer address is the one identifying datum,
  and the design says so where the operator turns the log on. A sink
  configured on a server with no tokens and no TLS still records
  `Admitted`/`Refused`/`Disconnected` — a plaintext, open server has
  an audit trail too.
- `AUD-FR-008` — **Cost and compatibility.** With `NoAudit` (the
  default) every path is byte-for-byte v0.16.0; `AuthConfig::new`,
  `from_env`, and every existing test, bench, and binary are
  unchanged. With a sink, cost is one formatted line per *decision*
  (per connection, per `Authenticate`, per refusal) on the
  connection's thread — never per successful request. A hostile peer
  that hammers refused requests produces a line each; that is the
  log doing its job, and the rate-limiting revisit is where the
  volume gets bounded. `dog_server` reads `SERVER_AUDIT_LOG`:
  unset → `NoAudit`; `stderr` → `StderrAudit`; anything else → a
  `FileAudit` path, an unopenable one a startup error naming the
  variable. `SERVER-001` takes its next minor / FR.

## Considered options

**Where the sink hangs.**

1. **`AuthConfig::with_audit` (proposed).** The object every gate
   already consults; `serve` untouched.
2. **A fifth `serve` parameter.** Rejected: a signature change for
   every caller, and `ADR-0025` already declined the shape.
3. **A `ServeOptions` struct replacing `auth` and `tls`.** The right
   shape if a *fourth* cross-cutting option ever appears; premature
   for one, and a breaking change to `serve`.
4. **A global (`log`/`tracing`).** Rejected: a dependency, and the
   wrong guarantee (filterable, swallowable).

**What is recorded.**

1. **Decisions only (proposed).** Admission, authentication,
   authorization, disconnect.
2. **Every request.** Access logging — volume and privacy decisions
   this crate has not made; rejected here, nameable later.
3. **Failures only.** Cheaper, but "who was admitted" is half of what
   an audit answers; rejected.

**Failure posture.**

1. **Fail-open with one visible notice (proposed).** The server keeps
   serving; the operator is told once.
2. **Fail-closed** — a sink write failure ends the connection (the
   posture of systems where an unrecorded action must not happen).
   Offered as option (b): a real choice, with a real cost (a full disk
   takes the server down).
3. **Silent drop.** Rejected: an audit log that fails silently is
   worse than none.

**Handshake timing.**

1. **Eager (proposed; shared with `ADR-0028`).** The only way to
   record *why* an admission failed.
2. **Lazy, record the read error.** Every failure reads as
   `Disconnected`; rejected.

## Proposed shape

```rust
// src/server/audit.rs (new, pub)
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Transport { Plain, Tls, MutualTls }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum RequestKind { GetById, FilterEq, /* … one per Request variant */ }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent { pub at: u64, pub peer: Option<SocketAddr>, pub kind: AuditKind }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditKind {
    Admitted { transport: Transport, initial_class: Option<TokenClass> },
    HandshakeFailed { reason: String },
    Authenticated { class: TokenClass },
    AuthenticationFailed,
    Refused { class: Option<TokenClass>, request: RequestKind, code: ErrorCode },
    Disconnected,
}
impl AuditEvent { pub fn line(&self) -> String; }        // AUD-FR-002's format, one place
pub trait AuditSink: Send + Sync { fn record(&self, event: &AuditEvent); }
pub struct NoAudit;
pub struct StderrAudit;                                  // Mutex<()> + one-time failure notice
pub struct FileAudit { .. }                              // Mutex<BufWriter<File>>, open(path) -> io::Result<Self>

// src/server/mod.rs
impl AuthConfig {
    pub fn with_audit(mut self, sink: Arc<dyn AuditSink>) -> Self;
    pub fn audit(&self) -> &dyn AuditSink;                // NoAudit by default
}
// handle_connection: `let peer = stream.peer_addr().ok();` at the top; at each gate,
// `auth.audit().record(&AuditEvent::now(peer, AuditKind::…))` after the decision.

// src/bin/dog_server.rs: SERVER_AUDIT_LOG = "stderr" | <path>
```

`RequestKind::of(&Request)` is a `match` with one arm per variant,
kept exhaustive (no `_`) so a new `Request` cannot skip the decision,
the same discipline `dispatch` uses.

## Data/state and invariants

- The sink observes; no gate's decision or `Response` depends on it.
- Every event is emitted after its decision and before the response
  is sent, on the connection's thread, outside every store lock.
- Exactly one `Admitted` or `HandshakeFailed` per connection; exactly
  one `Disconnected` per admitted connection (the loop's single exit
  path emits it; a write failure on the response path is a
  disconnect too).
- `at` is wall-clock Unix seconds; it can go backwards with the
  clock, as any audit timestamp can. No monotone sequence number:
  ordering within a file is write order.

## Errors, failure, recovery, and observability

- Sink write failure: counted, one notice, connection unaffected
  (`AUD-FR-006`).
- `FileAudit::open` failure: the library returns the `io::Error`;
  `dog_server` refuses to start, naming `SERVER_AUDIT_LOG`.
- The log itself is the observability this design adds; there is no
  metric of dropped events beyond the one-time notice (a counter
  accessor on `FileAudit` is cheap and may be added at
  implementation).

## Security, privacy, and compatibility

- Peer addresses are the only identifying datum and are exactly what
  an operator needs to act on a refusal; the operator turning the log
  on is the consent. Nothing secret is ever in a line — the format
  function is the one place that builds lines, and its test asserts
  the absence of a known token and a known certificate in every
  variant's output.
- `FileAudit` writes with the process's umask; the operator owns
  permissions and rotation. No `fsync`: an audit line lost to a crash
  is accepted; the alternative (an `fsync` per refusal on a hostile
  peer's schedule) is a denial-of-service lever.
- Compatible by construction: `NoAudit` is the default and takes no
  new branch; every existing suite passes unchanged.

## Acceptance criteria

1. `AuditEvent`/`AuditKind`/`AuditSink`/`NoAudit`/`StderrAudit`/`FileAudit`
   exist as specified; `AuditEvent::line` produces the documented
   format; a test asserts no line contains a configured token or a
   certificate's bytes for any variant.
2. A collecting sink (an integration test's own `Mutex<Vec<AuditEvent>>`
   impl) sees, for one connection that authenticates with a wrong
   token, then the right one, then is refused a write as `ReadOnly`,
   then disconnects: `Admitted{initial_class: None}`,
   `AuthenticationFailed`, `Authenticated{ReadOnly}`,
   `Refused{Some(ReadOnly), UpdateField, Unauthorized}`,
   `Disconnected` — in that order, each with the client's peer address.
3. An unauthenticated request is `Refused{None, <kind>, Unauthenticated}`;
   a server with no tokens records `Admitted{initial_class:
   Some(ReadWrite)}`.
4. A TLS client without a certificate on an mTLS server yields
   `HandshakeFailed` with a non-empty reason and no `Admitted`; an
   admitted mTLS client yields `Admitted{transport: MutualTls, ..}`.
5. `FileAudit` appends one line per event across two connections; a
   sink whose file is made unwritable drops events, prints the notice
   once, and every connection still gets its responses.
6. Every `Response` in every existing test is unchanged with a
   collecting sink configured (the suites run once with `NoAudit` and
   the auth suite once more with a sink).
7. `NoAudit` default: `AuthConfig::new`/`from_env` unchanged; no
   `Cargo.toml`, wire, `PROTOCOL_VERSION`, store, or `serve`-signature
   change; `dog_server` honors `SERVER_AUDIT_LOG` and refuses an
   unopenable path.

## Verification plan

- `src/server/audit.rs` unit tests: the line format per variant, the
  secrecy assertion, `FileAudit` append and the failure notice (an
  unwritable path).
- `tests/server_auth_integration.rs`: criteria 2–3 with a collecting
  sink; `tests/server_tls_integration.rs`: criterion 4.
- `src/bin/dog_server.rs`: the `SERVER_AUDIT_LOG` decision factored
  for a unit test, as `TlsConfig::from_env_values` is.

## Traceability

- → `SERVER-001` next minor / FR (`AUD-FR-001`–`008`), `ADR-0029`;
  resolves `SERVER-AUTH-DESIGN.md`'s "any audit log of authentication
  attempts" and `ADR-0012`'s "no audit log" consequence by pointer;
  leaves rate limiting and lockout named, now with the record they
  would be tuned from.
- Roadmap: `SERVER-AUTH-AUDIT-DESIGN` (this document), then
  `SERVER-AUTH-AUDIT` as the implementation unit if accepted.

## Open questions

- Whether `Disconnected` should carry a reason (`ClientClosed`,
  `FramingError`, `WriteFailed`) — cheap, since the loop knows;
  proposed at implementation if it costs no new branch.
- Whether `Refused` should include `Malformed` (a `Hello` out of place,
  a `BeginWith` with unknown bits) — those are protocol errors, not
  authorization decisions; proposed no.
- If `ADR-0028` lands, `Admitted.initial_class` already says whether
  a certificate classed the connection; whether to add
  `certificate_classed: bool` explicitly is a one-field question for
  then.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting the
  authentication audit-log design round as the fourth of four next
  directions ("1, 2, 3, 4"). (PR #151.)
- 2026-09-03: Accepted as proposed. No content change. Implementation
  after `ADR-0027`'s unit, as `SERVER-001`'s next minor / FR. (PR #153.)
- 2026-09-03: Implemented as `SERVER-001` v0.19.0 / FR-029 (PR #159),
  per the verification plan: acceptance criteria 1–7 hold as written,
  no deviation. `Disconnected` is emitted by a drop guard, so the
  "exactly one per admitted connection" invariant is structural; the
  eager handshake shared with `ADR-0028` landed here first. `ADR-0029`'s
  acceptance log carries the same note.
