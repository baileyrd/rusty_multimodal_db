# ADR-0030: Bound failed authentication — a per-connection lockout on by default, a per-peer failure budget opt-in, both audited, nothing on the wire

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): a per-connection lockout
  at `MAX_AUTH_FAILURES = 5` on by default, an opt-in per-peer
  fixed-window budget, both answered `Unauthenticated` on the wire,
  both audited as `LockedOut`/`Throttled`, `AuditKind`/`RequestKind`
  marked `#[non_exhaustive]`; (b) both opt-in with a typed
  `ErrorCode::RateLimited` and (c) close as not warranted declined; no
  changes requested). Acceptance authorizes the design; implementation
  follows as its own unit — see "Acceptance and implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-AUTH-RATE-LIMIT-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0012` /
  `docs/design/SERVER-AUTH-DESIGN.md` (the token scheme; its named gap
  — *rate-limiting failed authentication attempts, locking out a
  connection after N failures, and any audit log of authentication
  attempts* — is what this answers the remaining two thirds of),
  `ADR-0029` / `docs/design/SERVER-AUTH-AUDIT-DESIGN.md` (the audit
  log; its first revisit trigger is this), `ADR-0025` and `ADR-0029`
  (the precedent against a fifth `serve` parameter),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.19.0.
- Supersedes/Superseded by: none. Adds a policy on top of `ADR-0012`'s
  gate; changes no response, no wire, no store.

## Context

Every security round since `ADR-0012` carried one line forward: no
rate limiting, no lockout, no audit log. `ADR-0029` took the audit
log; the owner picked the other two next.

Today a connection may fail `Authenticate` without limit and a peer
may open connections without limit, so an online guess against an
operator-chosen token is bounded only by network speed. The audit log
now records every failure by peer — the input a limiter reads.

Two mechanisms, deliberately unequal: a **per-connection lockout** is
free, stateless, and costs a legitimate client nothing (five wrong
tokens on one connection is a bug or an attack), so it is on by
default with a constant; a **per-peer budget** needs shared state, a
window, and an answer to NAT, so it is opt-in with the consequence
documented. Neither changes what the server says: a throttled or
locked-out attempt is `Unauthenticated`, the same as a wrong token,
extending `SERVER-AUTH-DESIGN.md`'s rule that the response must not
say which. Neither delays: a delay is a thread held on the attacker's
schedule.

The owner selected this as the second of four directions. This ADR
proposes a design and authorizes no implementation — the posture
`ADR-0016` through `ADR-0029` took.

## Decision drivers

- Bound online token guessing per connection and per address without
  depending on the operator's token strength.
- Cost a legitimate client nothing by default; make the one mechanism
  with a false-positive mode (NAT) opt-in and documented.
- Change no response, no wire, no `serve` signature; add no dependency;
  bound memory under an address flood.
- Record every lockout and throttle on the audit log, where the rest of
  the gates' decisions already are.

## Considered options

1. **Per-connection lockout at `MAX_AUTH_FAILURES = 5` (on by default,
   a constant) plus an opt-in per-peer fixed-window budget on
   `AuthConfig` with bounded state; `Unauthenticated` on the wire;
   `LockedOut`/`Throttled` audit kinds; `AuditKind`/`RequestKind`
   marked `#[non_exhaustive]`** — proposed.
2. **Both opt-in.** Leaves the single-connection brute force open on
   every server that did not read the docs, for no benefit to anyone;
   offered inside option (b) for an owner who wants no default-on
   policy.
3. **A typed `ErrorCode::RateLimited`** (protocol 6) so a NAT'd
   legitimate client can tell it is throttled. Tells an attacker the
   moment the limiter engaged; offered as option (b)'s other half.
4. **Delay before answering a failure.** A denial-of-service lever on
   a thread-per-connection server; rejected.
5. **Per-token or global budgets.** Denial of service against
   legitimate holders; rejected.
6. **Token bucket / backoff.** More state and explanation for a
   smoother curve; the fixed window's 2× boundary worst case is
   accepted and documented; rejected for the first version.
7. **State on `serve`.** The fifth-parameter question, declined twice
   already; rejected.
8. **Close** — the gap stays named. Offered as option (c).

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/mod.rs`: `MAX_AUTH_FAILURES`, `MAX_TRACKED_PEERS`,
  `RateLimit { failures, window }` with `parse`, `FailureTable`
  (`Mutex<HashMap<IpAddr, Window>>`, monotonic `Instant`s, expired-
  then-oldest eviction), `AuthConfig::{with_rate_limit, rate_limit}`;
  the `Authenticate` arm counts failures per connection, consults the
  table before comparing, and closes the connection after the response
  to the fifth failure (`RL-FR-001`–`003`, `007`).
- `src/server/audit.rs`: `AuditKind::{LockedOut, Throttled}`,
  `#[non_exhaustive]` on `AuditKind` and `RequestKind` (`RL-FR-004`).
- `src/bin/dog_server.rs`: `SERVER_AUTH_RATE_LIMIT="<failures>/<seconds>"`
  (`RL-FR-006`).
- No `Request`/`Response`/`ErrorCode`, `PROTOCOL_VERSION`, store,
  `serve`-signature, or `Cargo.toml` change (`RL-FR-005`).
- `SERVER-001`'s next minor / FR (`RL-FR-001`–`008`); the gap line in
  `SERVER-AUTH-DESIGN.md` and `SERVER-001` resolved; `ADR-0029`'s first
  trigger taken and its enums clarified; `SPEC-REGISTRY`,
  `TRACEABILITY`, `ROADMAP` (`SERVER-AUTH-RATE-LIMIT`), `PROJECT-STATUS`.
- Tests per the design's verification plan.

## Consequences

### Positive

- The single-connection brute force is closed on every server, by
  default, at no cost to a legitimate client.
- The per-address brute force is closed on every server whose operator
  asks, with bounded memory and a documented NAT consequence.
- The wire is unchanged; an attacker learns nothing from the response.
- Every lockout and throttle is in the audit log by peer.

### Negative / tradeoffs

- **Address-keyed limiting does not bound a distributed guess**; token
  strength still matters, and the design says so.
- **NAT false positives** when the budget is on: a misbehaving client
  can throttle its neighbors for a window. Opt-in for exactly this
  reason.
- **A legitimate client cannot tell throttled from wrong**, by design;
  option (b) exists for an owner who weighs that the other way.
- **A default-on policy** is a first for this server's gates (every
  other one is opt-in). The design's argument is the nil
  false-positive cost; the constant is the smallest thing that could
  become a knob.
- `#[non_exhaustive]` on two public enums is a source-level change for
  a downstream crate that matches exhaustively; none exists.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0029`; every claim about the `Authenticate` arm, `AuthConfig`,
  and the audit enums read from `main`. No probe: the mechanism is a
  counter and a map.
- Revisit if: connection or handshake floods are the real problem — an
  accept-level limiter on `serve`, a different design.
- Revisit if: `MAX_AUTH_FAILURES` is hit by a legitimate client in
  practice — the constant becomes a knob.
- Revisit if: a NAT'd deployment needs the budget — option 3 (a typed
  error) or a per-token-per-peer key becomes worth its cost.
- Revisit if: a second server process shares tokens — the table would
  need to be shared, a real design.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — the
  lockout on by default at a constant, the per-peer budget opt-in, both
  answered `Unauthenticated`, both audited; **(b)** accept with both
  opt-in and a typed `ErrorCode::RateLimited` at protocol 6 for
  throttled attempts; **(c)** close as not warranted — the gap stays
  named, the audit log alone stands. Proposed in PR #161.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-AUTH-RATE-LIMIT-DESIGN.md`. (This PR.)
