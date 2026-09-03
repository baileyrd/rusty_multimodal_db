# Server Authentication Rate Limit and Lockout Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, `ADR-0030` option (a); both opt-in
  with a typed error and closing declined; no changes requested).
  Acceptance authorizes the design; implementation follows as its own
  unit — see `ADR-0030`'s "Acceptance and implementation" section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-AUTH-DESIGN.md` / `ADR-0012` (the token
  scheme; its named gap — *rate-limiting failed authentication
  attempts, locking out a connection after N failures, and any audit
  log of authentication attempts* — is what this document answers the
  remaining two thirds of), `docs/design/SERVER-AUTH-AUDIT-DESIGN.md` /
  `ADR-0029` (the audit log, `SERVER-001` v0.19.0 / FR-029, whose first
  revisit trigger — *rate limiting or lockout is wanted — a policy
  design that reads this log's event kinds* — this takes),
  `docs/design/SERVER-MTLS-DESIGN.md` / `ADR-0023` (admission by
  certificate, which this does not touch),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.19.0.

## Purpose and scope

`SERVER-AUTH-DESIGN.md` named three things it did not do. The audit
log (`ADR-0029`) took the third. This document takes the first two:
**rate-limiting failed authentication attempts** and **locking out a
connection after N failures**.

What the gap costs today, read from `main`: a connection may send
`Authenticate` as often as it likes; each wrong token costs the server
one constant-time comparison per configured token and answers
`Unauthenticated` with the connection open; a client may also open as
many connections as the OS allows. So an online guess against a token
is bounded only by network speed. The tokens are operator-chosen
shared secrets (`AUTH-FR-005`) — a 128-bit random one is not guessable
at any rate this server could serve, a short human-chosen one is — and
the design's posture is that the server should not depend on the
operator having chosen well.

**In scope:**

- A **per-connection lockout**: after a fixed number of failed
  `Authenticate`s on one connection, the server records the fact and
  closes the connection. On by default; the number is a constant.
- An **opt-in per-peer failure budget**: failed `Authenticate`s are
  counted per peer IP over a sliding window; once a peer exceeds its
  budget, every further `Authenticate` from that IP is refused without
  a comparison until the window passes. Bounded memory.
- Both recorded on the audit log as their own event kinds, so the log
  answers "who was locked out or throttled, when."
- `dog_server` configuration for the budget.

**Out of scope (see "Non-goals")**: connection or handshake floods,
delays, CAPTCHAs, distributed state, a typed "throttled" error on the
wire.

## Non-goals

- **Connection and handshake floods.** A peer that opens connections
  and never authenticates costs a thread and, with TLS, a handshake;
  bounding that is an accept-level concern (a listener limiter, a
  handshake budget) — a different design with a different owner
  (`serve`, not `AuthConfig`). Named as `ADR-0030`'s first revisit
  trigger.
- **Slowing the client down** (a delay before answering a failure).
  Rejected: a delay holds a server thread on the attacker's schedule —
  a denial-of-service lever this thread-per-connection server cannot
  afford. Refusing is free; delaying is not.
- **Lockout of the *token*** (disabling a token after N failures from
  anywhere). Rejected: lets any peer deny service to every legitimate
  holder of that token.
- **A new `ErrorCode`.** A throttled or locked-out `Authenticate` is
  answered `Unauthenticated`, the same shape as a wrong token —
  deliberately, extending `SERVER-AUTH-DESIGN.md`'s own rule that the
  response must not distinguish "wrong token" from "no token yet." No
  wire change, no `PROTOCOL_VERSION` change. `ADR-0030` offers a typed
  `ErrorCode::RateLimited` as option (b).
- **Shared state across processes.** One server, one table.
- **Counting mTLS handshake failures.** A certificate is not guessable
  by retrying; a failed handshake is an operator problem, already
  audited. Not counted; named as an open question.

## Context and terminology

- **Failure**: an `Authenticate` on a server with tokens configured
  that matches no token (`auth.check` returns `None`) — the event the
  audit log records as `AuthenticationFailed`.
- **Lockout** (per connection): the connection is closed by the server
  after `MAX_AUTH_FAILURES` failures. Stateless beyond the connection's
  own counter.
- **Budget** (per peer): `RateLimit { failures: u32, window: Duration }`
  — at most `failures` failures per `window` per peer IP. State: a
  table keyed by `IpAddr` holding a failure count and a window start,
  shared by every connection thread.
- **Throttled**: an `Authenticate` refused because its peer is over
  budget — no comparison is made, `Unauthenticated` is answered, the
  connection stays open (and the per-connection counter still advances,
  so lockout follows).
- **Peer**: `TcpStream::peer_addr()`'s IP, the same datum the audit log
  keys on. A NAT puts many clients behind one IP; the design names the
  consequence.

### What the current code does, read from `main` `fce0762`

`handle_connection`'s `Authenticate` arm: with tokens configured,
`auth.check(token)` sets `authenticated` on a match and records
`Authenticated`, or records `AuthenticationFailed` and answers
`Unauthenticated`; the loop continues either way. `AuthConfig` holds
two optional tokens and an optional audit sink; it is `Clone`, and
`serve` shares one `Arc<AuthConfig>` across connection threads. The
audit log's `AuditKind` and `RequestKind` are plain public enums
without `#[non_exhaustive]`.

## Requirements

- `RL-FR-001` — **Per-connection lockout, on by default.**
  `pub const MAX_AUTH_FAILURES: u32 = 5`. On a server with tokens
  configured, the fifth failed `Authenticate` on one connection is
  answered `Unauthenticated` as today, then the server records
  `LockedOut { failures }` and closes the connection. A successful
  `Authenticate` does not reset the counter (a guess that eventually
  succeeds still spent its budget; the connection is simply
  authenticated, and the counter is moot). No configuration knob: a
  constant, on purpose — the first real report decides whether it
  becomes one (the `MAX_STAGED_OPS` precedent).
- `RL-FR-002` — **Per-peer budget, opt-in.**
  `AuthConfig::with_rate_limit(RateLimit { failures, window })`. Each
  failure from a peer IP is counted in that peer's current window; the
  window starts at the first failure and resets when `window` has
  elapsed. While a peer's count is at or over `failures`, every
  `Authenticate` from that IP is refused before any comparison —
  answered `Unauthenticated`, recorded as `Throttled { failures }` —
  and counts as a failure toward `RL-FR-001`. A successful
  `Authenticate` from a peer under budget does not clear its count.
- `RL-FR-003` — **Bounded state.** The table holds at most
  `MAX_TRACKED_PEERS` (`pub const`, proposed 4096) entries; on insert,
  entries whose window has expired are dropped first, then the oldest
  window start is evicted if still full. Memory is therefore bounded
  regardless of how many addresses fail; a peer evicted early simply
  gets a fresh window — the budget degrades toward "no budget" under an
  address flood, never toward "no service."
- `RL-FR-004` — **Audit.** `AuditKind` gains `LockedOut { failures: u32 }`
  and `Throttled { failures: u32 }`; both carry the peer as every event
  does; their lines follow `AUD-FR-002`. `AuditKind` and `RequestKind`
  are marked `#[non_exhaustive]` at the same time — they were designed
  to grow with the gates, and this is the first growth — a
  clarification of `ADR-0029`, recorded there.
- `RL-FR-005` — **Wire.** No new `Request`, `Response`, or `ErrorCode`;
  `PROTOCOL_VERSION` stays 5. A throttled or locked-out attempt is
  indistinguishable on the wire from a wrong token, by design
  (`SERVER-AUTH-DESIGN.md`'s own rule).
- `RL-FR-006` — **Configuration.** `dog_server` reads
  `SERVER_AUTH_RATE_LIMIT` as `<failures>/<seconds>` (e.g. `10/60`);
  malformed is a startup error naming the variable; unset is no budget.
  The lockout has no variable. The startup line reports the mode.
- `RL-FR-007` — **Cost and compatibility.** With no budget configured
  the only new work is one counter per connection and one comparison
  per failed `Authenticate`; the success path is unchanged. With a
  budget, each `Authenticate` takes the table's mutex once (a
  `HashMap` lookup); no work on any other request. A server with no
  tokens configured (`AUTH-FR-007`) has no failures and takes no new
  branch. Every existing test passes unchanged: none fails
  `Authenticate` five times on one connection.
- `RL-FR-008` — `SERVER-001` takes its next minor / FR;
  `SERVER-AUTH-DESIGN.md`'s gap line and `SERVER-001`'s non-goals line
  are resolved for lockout and rate limiting; `ADR-0029`'s first
  trigger taken.

## Considered options

**Lockout.**

1. **Close the connection after a constant number of failures, on by
   default (proposed).** Free, stateless, exactly the gap's words. The
   cost to a legitimate client is nil: five wrong tokens on one
   connection is a bug or an attack, and a reconnect is one round trip.
2. **Configurable count.** A knob nobody has asked for; the constant
   becomes one at the first real report.
3. **Opt-in.** Weighed; the design recommends on-by-default because the
   false-positive cost is nil and the alternative leaves the
   single-connection brute force open on every server that did not
   read the docs. Offered inside `ADR-0030`'s option (b).

**Rate limit.**

1. **Per-peer failure budget over a fixed window, opt-in (proposed).**
   Simple to explain (`10/60`), bounded, and enough: with a budget of
   ten per minute an eight-character alphanumeric token takes longer
   than the universe to guess from one address. Opt-in because the
   window and the NAT question are the operator's, and because it
   needs shared state the lockout does not.
2. **Token bucket / exponential backoff per peer.** Smoother, more
   state, more to explain; rejected for the first version — the fixed
   window's worst case (2× the budget across a window boundary) is
   acceptable and documented.
3. **Per-token budget.** Rejected (denial of service against the
   token's legitimate holders — see Non-goals).
4. **Global budget.** Rejected for the same reason at larger scale.

**What a refused attempt says.**

1. **`Unauthenticated`, unchanged (proposed).** The existing rule.
2. **A typed `ErrorCode::RateLimited`** at protocol 6, so a legitimate
   client behind a NAT can tell it is throttled rather than wrong.
   Offered as option (b); rejected as the default because it tells an
   attacker the exact moment the limiter engaged, which is the one
   thing a limiter should not say.

**Where the state lives.**

1. **`AuthConfig` (proposed)** — the policy object every gate consults;
   `Clone`s share the table through an `Arc`. `serve` unchanged.
2. **`serve`** — the same fifth-parameter question `ADR-0025` and
   `ADR-0029` declined.

## Proposed shape

```rust
// src/server/mod.rs
pub const MAX_AUTH_FAILURES: u32 = 5;
pub const MAX_TRACKED_PEERS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit { pub failures: u32, pub window: Duration }
impl RateLimit { pub fn parse(s: &str) -> Result<Self, RateLimitParseError>; } // "<failures>/<seconds>"

pub struct AuthConfig {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
    audit: Option<Arc<dyn AuditSink>>,
    rate_limit: Option<Arc<FailureTable>>,   // RL-FR-002/003
}
impl AuthConfig {
    pub fn with_rate_limit(self, limit: RateLimit) -> Self;
    pub fn rate_limit(&self) -> Option<RateLimit>;
    /// RL-FR-002: record a failure for `peer`; `true` if over budget.
    fn note_failure(&self, peer: Option<IpAddr>) -> bool;
    /// Whether `peer` is currently over budget (no comparison should run).
    fn is_throttled(&self, peer: Option<IpAddr>) -> bool;
}
struct FailureTable { limit: RateLimit, peers: Mutex<HashMap<IpAddr, Window>> }
struct Window { started: Instant, failures: u32 }

// handle_connection — the Authenticate arm
let mut failures: u32 = 0;
// ...
if auth.is_throttled(peer_ip) { failures += 1; record Throttled; resp = Unauthenticated }
else match auth.check(token) { Some(class) => ..., None => { failures += 1; auth.note_failure(peer_ip); record AuthenticationFailed; resp = Unauthenticated } }
send resp;
if failures >= MAX_AUTH_FAILURES { record LockedOut { failures }; return; }   // RL-FR-001

// src/server/audit.rs
#[non_exhaustive] pub enum AuditKind { ..., LockedOut { failures: u32 }, Throttled { failures: u32 } }
#[non_exhaustive] pub enum RequestKind { ... }

// src/bin/dog_server.rs: SERVER_AUTH_RATE_LIMIT="10/60"
```

`Instant` (monotonic) for windows, never wall-clock; a peer with no
address (`peer_addr` failed) is neither counted nor throttled — it is
still locked out per connection.

## Data/state and invariants

- The per-connection counter is the connection's own; nothing else
  reads it.
- The table is the only shared state; it is touched only on the
  `Authenticate` path of a server with tokens *and* a budget
  configured, under one mutex, for one lookup or insert.
- `|table| ≤ MAX_TRACKED_PEERS` after every insert.
- A throttled attempt never runs a token comparison — the limiter is
  before the check, so an over-budget peer's cost per attempt is the
  lookup, not the comparisons.
- Responses are unchanged in shape and content; only *when the
  connection closes* changes (after the response to the fifth
  failure).

## Errors, failure, recovery, and observability

- A malformed `SERVER_AUTH_RATE_LIMIT` is a startup error. The library
  constructor takes a typed `RateLimit`; `RateLimit::parse` is the
  binary's helper and is unit-tested.
- Lockout closes the connection after the response is written, so the
  client sees `Unauthenticated` then EOF — the same thing it sees if the
  server restarts; a client library needs no new error kind.
- The audit log is the observability: `Throttled` and `LockedOut`
  lines by peer.
- A poisoned table mutex is treated as "not throttled" (fail-open for
  availability; the lockout still holds per connection). Named.

## Security, privacy, and compatibility

- Online guessing from one connection is bounded at
  `MAX_AUTH_FAILURES` per connect; from one address, at the budget per
  window when configured. Neither bounds a distributed guess across
  many addresses — the honest limit of address-keyed limiting, and the
  reason token strength (`AUTH-FR-005`'s advice) still matters.
- No new secret, no new data at rest; the table holds peer IPs and
  counters in memory only.
- **NAT**: many clients behind one address share a budget; a
  misconfigured client can throttle its neighbors for a window. The
  budget is opt-in and documented with this consequence; the lockout
  has no such effect.
- Backward compatible: no wire change; clients that never fail five
  times on one connection see nothing new; the existing auth suite
  passes unchanged.
- `#[non_exhaustive]` on `AuditKind`/`RequestKind` is a source-level
  change for a downstream crate matching exhaustively; none exists, and
  the design says so.

## Acceptance criteria

1. The fifth failed `Authenticate` on one connection is answered
   `Unauthenticated` and the connection then closes; the audit log
   shows `AuthenticationFailed` ×5 then `LockedOut { 5 }` then
   `Disconnected`; four failures then a success leaves the connection
   open and authenticated.
2. With `RateLimit { failures: 3, window: 60s }`, three failures from
   one peer over two connections are counted; the fourth attempt — a
   *correct* token — is refused `Unauthenticated`, recorded `Throttled`,
   and no comparison runs (a test-only counter on `check`, or the
   observation that the correct token was refused).
3. A different peer address is unaffected (loopback offers only one
   address; the unit test drives `FailureTable` directly with two
   addresses).
4. After the window elapses (a unit test with a shortened window), the
   peer is under budget again and a correct token authenticates.
5. `MAX_TRACKED_PEERS` holds: inserting more addresses than the cap
   evicts expired then oldest entries; the table never exceeds the cap.
6. `RateLimit::parse` accepts `10/60` and rejects `10`, `0/60`, `10/0`,
   `a/b`; `dog_server` refuses a malformed variable and reports the
   mode.
7. `Throttled` and `LockedOut` lines follow the documented format; no
   line carries a token.
8. Every `Response` in every existing test is unchanged; no
   `Cargo.toml`, wire, `PROTOCOL_VERSION`, store, or `serve`-signature
   change.

## Verification plan

- `src/server/mod.rs` unit tests on `FailureTable` (criteria 3–5) and
  `RateLimit::parse` (6); `src/server/audit.rs` line tests extended
  (7).
- `tests/server_auth_integration.rs`: the lockout sequence (1) and the
  throttle-with-correct-token case (2) on the collecting sink.
- `src/bin/dog_server.rs`: the variable's table test extended.

## Traceability

- → `SERVER-001` next minor / FR (`RL-FR-001`–`008`), `ADR-0030`;
  resolves the lockout and rate-limit thirds of `SERVER-AUTH-DESIGN.md`'s
  named gap; takes `ADR-0029`'s first revisit trigger; clarifies
  `ADR-0029` (`#[non_exhaustive]`).
- Roadmap: `SERVER-AUTH-RATE-LIMIT-DESIGN` (this document), then
  `SERVER-AUTH-RATE-LIMIT` as the implementation unit if accepted.

## Open questions

- Whether mTLS handshake failures should count toward a peer's budget
  (they are audited but not guessable). Proposed no.
- Whether `MAX_AUTH_FAILURES` should be higher for `ReadOnly`-token
  deployments where a client library might retry — proposed no; a
  library that retries a wrong token five times has a bug.
- Whether a `Throttled` event should be emitted once per window rather
  than per attempt (log volume under attack). Proposed per attempt: the
  log's job; the window bounds nothing about volume, and the
  rate-limiting of the *log* is the access-logging design's question.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting the
  rate-limiting / lockout design round as the second of four next
  directions ("1, 2, 3, 4"), after the audit log landed as `SERVER-001`
  v0.19.0. (PR #161.)
