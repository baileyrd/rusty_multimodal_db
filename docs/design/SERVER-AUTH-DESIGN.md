# Server Authentication/Authorization Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-01 — the owner
  approved the design as proposed, no changes requested). Acceptance
  authorizes the design; implementation still requires its own unit
  (extending `SERVER-001` with new FRs, or registering a dedicated spec)
  before any server code is written — see ADR-0012's "Decision" section.
  See `docs/decisions/ADR-0012-server-authentication-proposal.md` for the
  decision record this document backs.
- Date: 2026-09-01
- Related: `docs/specifications/server/SERVER-001-query-layer.md` (the
  spec this design would extend, once accepted), ADR-0010 (the
  server/query layer's own protocol/framing/concurrency-model decisions,
  unchanged by this proposal), ADR-0011 (schema discovery — this design's
  `Authenticate` request follows the same "add one `Request`/`Response`
  variant to the existing protocol" shape schema discovery used)

## Purpose and scope

`docs/FUTURE-GROWTH.md` names authentication/authorization as "genuinely
new" work for the server/query layer, not an incremental extension —
"doesn't exist in any form today; a network-exposed store needs it from
the start, not as an add-on." This document proposes the smallest real
slice of that: a shared-secret token, sent once per connection, checked
before any other request is served, with two coarse authorization
classes (read-only, read-write).

**In scope for this proposal:**

- A new `Request::Authenticate` request kind, checked once per
  connection before any other request kind is processed.
- Two static, server-configured token classes: `ReadOnly` (can
  `GetById`/`FilterEq`/`ScanField`/`Parent`/`Children`/`Neighbors`/
  `DescribeSchema`) and `ReadWrite` (everything `ReadOnly` can, plus
  `UpdateField`).
- Constant-time token comparison (a new, narrow dependency — see
  ADR-0012's own "Considered options").
- Documentation of the transport-encryption gap this proposal does
  *not* close, and what pairing it with an external TLS-terminating
  proxy/tunnel would look like operationally.

**Explicitly out of scope, named directly rather than left implicit**
(per ADR-0012's own Considered options):

- Native transport encryption (TLS) inside this crate's own server code.
  Deferred to a real, separate future decision (see "Revisit triggers"
  in ADR-0012) — this proposal requires an external proxy/tunnel for any
  non-localhost deployment instead.
- Per-user identity, accounts, or any authentication mechanism beyond a
  small, fixed set of server-configured shared-secret tokens. This crate
  has no concept of "users" anywhere else in its data model; a real
  identity system is its own multi-week design.
- Per-field, per-record, or per-domain authorization. Two coarse,
  connection-wide classes are the entire authorization model this
  proposal defines.
- Token rotation, expiry, or revocation. A token is valid for as long as
  the server process runs with it configured; changing it means
  restarting the server with a new one.
- mTLS client-certificate authentication — depends on this crate owning
  TLS directly, which is out of scope above.

## Non-goals

- Not a claim that this design, once implemented, makes a server built
  from this crate safe to expose beyond a trusted network on its own.
  Transport encryption remains a separate, required companion — see
  "Security, privacy, and compatibility" below.
- Not session or transaction semantics across multiple requests —
  authentication state is the only new per-connection state this
  proposal adds; everything ADR-0010's own "no transaction semantics"
  non-goal already excluded remains excluded.
- Not a replacement for `SERVER-001`'s existing protocol/framing/
  concurrency-model decisions. Length-prefixed `bincode` framing,
  thread-per-connection dispatch, and the `ConnectionStore` trait shape
  are all unchanged by this proposal.

## Context and terminology

Every domain adapter (`Dog`, `Order`/`Customer`, `Employee`) already
implements `ConnectionStore`, and `dispatch` already translates one
`Request` into one `Response` with no I/O of its own
(`src/server/mod.rs`). This proposal adds exactly one new concern to
that picture: whether the connection sending a given `Request` has
proven it holds a valid token, and if so, which class.

- **"Authenticated"**: a connection that has successfully sent
  `Request::Authenticate { token }` matching one of the server's
  configured tokens, since it was opened. Authentication state lives
  per-connection (in `handle_connection`'s own local state, the same
  place framing buffers already live today) — never in the shared
  store, matching ADR-0010's own "no new coordination beyond what the
  wrapped store already provides" decision driver.
- **`TokenClass`**: `ReadOnly` or `ReadWrite` — the class attached to
  whichever configured token a connection authenticated with. Read
  operations succeed for either class; `UpdateField` requires
  `ReadWrite`.
- **Constant-time comparison**: comparing two byte strings (the
  presented token against each configured token) in a way that takes the
  same amount of time regardless of where or whether they first differ —
  a plain `==` comparison on `&str`/`&[u8]` can leak how many leading
  bytes matched via timing, in principle letting a remote attacker guess
  a valid token one byte at a time. See ADR-0012's own "Considered
  options" for why this proposal takes a new, narrow dependency
  (`subtle`) for exactly this comparison rather than hand-rolling it.

## Requirements

- `AUTH-FR-001`: A new `Request::Authenticate { token: String }` /
  `Response` pair. Success returns `Response::Ok`; an unrecognized token
  returns a typed error (`ErrorCode::Unauthenticated` — see FR-004),
  never `Response::Ok` for a wrong token and never a panic for a
  malformed one.
- `AUTH-FR-002`: A connection that has not yet successfully authenticated
  is rejected with `ErrorCode::Unauthenticated` for every request kind
  except `Authenticate` itself — including `DescribeSchema` (see ADR-0012's
  own "Open questions" on why this proposal picks the uniform rule
  rather than carving out an exception for schema metadata).
- `AUTH-FR-003`: An authenticated connection's `TokenClass` gates
  `UpdateField` specifically: `ReadOnly` connections get
  `ErrorCode::Unauthorized` (a new variant, distinct from
  `Unauthenticated` — "who you are" vs. "what you're allowed to do") for
  any `UpdateField` request; every other request kind (`GetById`,
  `FilterEq`, `ScanField`, `Parent`, `Children`, `Neighbors`,
  `DescribeSchema`) succeeds for both classes once authenticated.
- `AUTH-FR-004`: `ErrorCode` gains two new variants,
  `Unauthenticated`/`Unauthorized`, alongside the existing
  `UnknownField`/`Unsupported`/`Malformed` — matching this crate's
  existing "small, named enum, not a bare error code" discipline
  (`src/server/protocol.rs`'s own doc comment on why `ErrorCode` replaced
  the original design's bare `u8`).
- `AUTH-FR-005`: Configured tokens (one `ReadOnly` token, one `ReadWrite`
  token, at minimum — the exact multi-token story, if any, is an
  implementation-time decision, not fixed here) are read from the
  process environment at server startup, never hardcoded, never
  committed to the repository, and never logged or echoed back in any
  `Response`.
- `AUTH-FR-006`: Token comparison against every configured token uses a
  constant-time comparison (`subtle::ConstantTimeEq` or equivalent),
  applied consistently — not just for the token that happens to match.
- `AUTH-FR-007`: A server started with no configured tokens at all
  behaves exactly as `SERVER-001` does today — every connection
  effectively pre-authenticated, `Authenticate` a no-op success. This is
  the explicit backward-compatibility story for `src/bin/dog_server.rs`
  and every existing integration test: opting into auth is something an
  operator does by configuring tokens, not something forced onto every
  existing deployment by this proposal's mere existence.

## Architecture and interfaces

### Considered options

**Transport encryption: native TLS vs. an external proxy/tunnel.**

1. *Native TLS via `rustls`.* Considered — the conventional choice for a
   Rust server that wants to own encryption directly, and pairs cleanly
   with a future mTLS story. Rejected for this proposal: `rustls` pulls
   in a real dependency footprint (a crypto backend, certificate
   parsing, trust-anchor handling) disproportionate to what this
   proposal needs to close the authentication half of the gap, matching
   the same "don't reach for a heavy dependency before it's clearly
   needed" reasoning ADR-0010 used for `tokio`/gRPC. Not rejected
   forever — see ADR-0012's own revisit triggers.
2. *`native-tls` (system TLS bindings — Schannel/SecureTransport/
   OpenSSL, chosen per platform).* Rejected — platform-dependent
   behavior and configuration is a worse fit for a project that has
   otherwise kept its dependency surface uniform across dev/deploy
   environments.
3. *No native TLS in this crate; require an external TLS-terminating
   proxy or tunnel (`stunnel`, `nginx`/`Caddy` TCP-TLS termination, an
   SSH tunnel) in front of the plaintext socket this crate still
   speaks.* **Chosen.** No new dependency at all; real, well-understood
   operational precedent (this is how several wire-protocol databases
   historically layered TLS on before ever owning it natively). The real
   cost is operational, not architectural: an operator who wants
   encryption has one more process to run and configure, and this crate
   cannot verify from inside the server process that such a proxy is
   even in place — a real, named gap (see "Security, privacy, and
   compatibility" below), not a false promise.

**Authentication mechanism: shared-secret token vs. alternatives.**

1. *No auth (status quo).* Rejected — the entire premise this proposal
   exists to address.
2. *Per-user accounts with a real identity store* (usernames, password
   hashing, session management). Considered and rejected as
   disproportionate: this crate has no concept of "users" anywhere else
   in its data model (every domain — `Dog`, `Order`/`Customer`,
   `Employee` — models application data, not server operators), and a
   real identity system is its own multi-week design effort, not a
   bounded extension of the existing protocol.
3. *mTLS client certificates.* Considered — elegant since it ties
   authentication to the transport layer directly. Rejected for this
   proposal specifically because it depends on this crate owning TLS
   natively, which option 3 above (in the transport-encryption
   consideration) explicitly defers.
4. *A shared-secret token, sent once per connection via a new
   `Request::Authenticate`, checked before any other request is
   served.* **Chosen.** Reuses the existing `Request`/`Response` enum
   shape exactly (one new variant each), no new sub-protocol, no new
   framing — the same "small, bounded extension of what already exists"
   shape ADR-0011's `DescribeSchema` used.

**Authorization granularity: fine-grained vs. coarse.**

1. *Per-field or per-record ACLs* (e.g. "this token can `UpdateField` on
   `Order::amount_cents` but not `Order::status`"). Rejected as
   disproportionate scope for this pass — the same reasoning ADR-0010
   used to scope session/transaction semantics out entirely rather than
   half-build them.
2. *Two coarse, static token classes, `ReadOnly`/`ReadWrite`, checked
   once at authenticate time and cached for the connection's lifetime.*
   **Chosen.** Matches the "coarse but real" bar this crate already
   applies elsewhere (`ErrorCode`'s own three original variants, not a
   rich error hierarchy) — a real access-control boundary (can this
   connection write at all) without pretending to solve a general
   authorization model this proposal has no evidence is needed yet.

**Constant-time token comparison: hand-rolled vs. a dependency.**

1. *Hand-rolled constant-time byte compare* (e.g. XOR-accumulate every
   byte, compare the accumulator to zero at the end, avoid any early
   `return`). Considered, then rejected specifically for this piece:
   getting this right requires defeating compiler optimizations that can
   reintroduce early-exit behavior even from code that looks
   constant-time at the source level — a well-documented class of subtle
   bug in exactly this kind of DIY security code, and the failure mode
   (a timing side channel) is silent, not a compile error or a failing
   test.
2. *The `subtle` crate.* **Chosen** — small, purpose-built for exactly
   this comparison, widely used and scrutinized in the Rust cryptography
   ecosystem specifically because hand-rolling it is a known trap. This
   is the one place this proposal departs from its own "avoid new
   dependencies" driver, deliberately: the correctness bar for
   security-critical code is different from the bar for ordinary
   application logic.

### Proposed shape

```rust
// src/server/protocol.rs — additions, not a rewrite of the existing enums

enum Request {
    // ...every existing variant, unchanged...
    Authenticate { token: String },
}

enum ErrorCode {
    // ...every existing variant, unchanged...
    Unauthenticated, // no valid Authenticate call yet on this connection
    Unauthorized,    // authenticated, but this token's class can't do this
}

enum TokenClass {
    ReadOnly,
    ReadWrite,
}
```

```rust
// src/server/mod.rs — handle_connection gains per-connection auth state;
// dispatch itself is unchanged (it has no notion of "this connection",
// only "this request against this store") — the auth check happens in
// handle_connection's own request loop, one layer up from dispatch,
// before dispatch is ever called for anything but Authenticate.

struct AuthConfig {
    // Loaded once at server startup from the process environment —
    // exact env var names are an implementation-time decision.
    read_only_token: Option<String>,
    read_write_token: Option<String>,
}

// handle_connection's loop, sketched:
// let mut authenticated: Option<TokenClass> = None;
// loop {
//     let req = read_message(...)?;
//     match &req {
//         Request::Authenticate { token } => {
//             authenticated = auth_config.check(token); // constant-time
//             write_message(..., &if authenticated.is_some() { Response::Ok } else { err_response(Unauthenticated) })?;
//             continue;
//         }
//         _ => {}
//     }
//     let Some(class) = authenticated else {
//         write_message(..., &err_response(Unauthenticated))?;
//         continue;
//     };
//     if matches!(req, Request::UpdateField { .. }) && class == TokenClass::ReadOnly {
//         write_message(..., &err_response(Unauthorized))?;
//         continue;
//     }
//     let resp = dispatch(store, req);
//     write_message(..., &resp)?;
// }
```

`AuthConfig::check` is where `subtle`'s constant-time comparison is
used, checked against every configured token (not short-circuited on
the first match) so the response timing doesn't itself leak which token,
if any, was closest to matching.

## Data/state and invariants

- Authentication state (`Option<TokenClass>`) is per-connection, held in
  `handle_connection`'s own local state — never written to the shared
  `ConnectionStore`, never visible to any other connection. Matches
  ADR-0010's own "per-connection state is limited to the TCP stream and
  its read/write buffers" invariant, extended by exactly one field.
- `AuthConfig` (the configured tokens themselves) is loaded once at
  server startup and shared read-only across every connection thread —
  no new lock needed, since it's never mutated after startup (`Arc<AuthConfig>`
  alongside the existing `Arc<S: ConnectionStore>`).
- No new persistent state, no new on-disk format — same as `SERVER-001`'s
  own existing invariant; tokens live in the process environment, not on
  disk.

## Errors, failure, recovery, and observability

- An unauthenticated connection attempting any request but `Authenticate`
  gets `Response::Err { code: ErrorCode::Unauthenticated, .. }` — never a
  panic, never a silently-dropped connection, matching `dispatch`'s
  existing "typed error, not a panic" discipline for every other
  malformed/unsupported case.
- A `ReadOnly`-authenticated connection attempting `UpdateField` gets
  `Response::Err { code: ErrorCode::Unauthorized, .. }` — the connection
  stays open and authenticated; only that one request is rejected, the
  same way an unsupported-operation error today doesn't close the
  connection.
- A wrong token on `Authenticate` gets `Response::Err { code:
  ErrorCode::Unauthenticated, .. }`, indistinguishable in shape from
  "never authenticated yet" — deliberately, so a client (or an attacker)
  can't use the response to distinguish "wrong token" from "no token
  sent yet."
- Out of scope, named rather than silently assumed solved: rate-limiting
  failed authentication attempts, locking out a connection after N
  failures, and any audit log of authentication attempts. A real gap for
  a genuinely adversarial network, not addressed by this proposal.
  *The audit log third taken up as `SERVER-AUTH-AUDIT-DESIGN.md` /
  `ADR-0029` (Proposed): a decisions-only record on an `AuditSink` hung
  on `AuthConfig`, off by default. Rate limiting and lockout stay named,
  now with the record they would be tuned from.*

## Security, privacy, and compatibility

- **This design does not, by itself, make a server built from this crate
  safe to expose beyond a trusted network.** It closes the "anyone who
  can open a TCP connection can do anything" gap; it does not close the
  "anyone who can observe the network can read the token and every
  record in transit" gap. Both are required together — see ADR-0012's
  own "Decision" section.
- Tokens are plaintext on the wire under this proposal, exactly as every
  other field value already is — pairing this design with an external
  TLS-terminating proxy or tunnel is not optional for any deployment
  where the network path itself isn't already trusted (e.g. still fine
  for `127.0.0.1`-only use, exactly like `SERVER-001` today).
- Constant-time comparison (`AUTH-FR-006`) closes one specific,
  measurable timing side channel; it does not make this design resistant
  to a genuinely adversarial network attacker in general (no
  rate-limiting, no lockout — see "Errors" above).
- Backward compatible by construction (`AUTH-FR-007`): a server started
  with no configured tokens behaves exactly as today's unauthenticated
  `SERVER-001` does. No existing test, binary, or documented usage
  breaks unless an operator opts in by configuring tokens.

## Acceptance criteria

(For the eventual implementation unit, once this design is accepted —
not attempted by this proposal itself.)

- A real client connecting without ever sending `Authenticate` gets
  `ErrorCode::Unauthenticated` for `GetById` (and every other non-`Authenticate`
  request kind, including `DescribeSchema`) against a server configured
  with tokens.
- A real client sending `Authenticate` with the wrong token gets the same
  `ErrorCode::Unauthenticated`, not a distinguishable error.
- A real client authenticated with a `ReadOnly` token succeeds on every
  read-shaped request kind and gets `ErrorCode::Unauthorized` specifically
  for `UpdateField`.
- A real client authenticated with a `ReadWrite` token succeeds on every
  request kind, including `UpdateField`.
- A server started with no configured tokens behaves identically to
  today's `SERVER-001` for every existing integration test, with no test
  changes required — the backward-compatibility bar `AUTH-FR-007` sets.
- A timing-based measurement (comparing response latency for tokens that
  differ at the first byte vs. tokens that differ only at the last byte)
  shows no statistically distinguishable difference — the real evidence
  `AUTH-FR-006`'s constant-time requirement needs, not just "we used the
  `subtle` crate so it must be fine."

## Verification plan

(Also for the eventual implementation unit.)

- Unit tests: `AuthConfig::check` against configured/unconfigured tokens,
  correct/incorrect tokens, both classes; `ErrorCode`'s two new variants
  round-trip through `bincode` the same way every existing variant does.
- Real end-to-end tests: matching the existing `tests/server_*_integration.rs`
  pattern — a genuine `TcpListener`/`TcpStream` pair, a server configured
  with both token classes, real clients exercising every acceptance
  criterion above.
- A timing measurement for the constant-time comparison claim — the one
  piece of this design that needs empirical evidence, not just a
  read-through, since a timing side channel is exactly the kind of bug
  that looks correct at the source level.

## Traceability

Would implement: the authentication/authorization gap ADR-0010,
`SERVER-001`, and `docs/FUTURE-GROWTH.md` each independently name — once
ADR-0012 is accepted. No spec registered yet; per ADR-0010's own
precedent (`SERVER-001` registered as a separate step after that ADR's
acceptance), a real implementation would extend `SERVER-001` with new
FRs (or register a dedicated spec) as its own follow-up unit, not part
of this design-only pass.

## Open questions

- Should `DescribeSchema` require authentication? This proposal picks
  the uniform rule (yes, same as everything else) for simplicity — a
  case could be made that schema metadata alone isn't sensitive and
  could be exempted, at the cost of a less uniform rule. Owner's call,
  not decided here.
- Exact environment-variable naming and whether to support more than one
  token per class (e.g. multiple read-write tokens for multiple
  operators, each independently revocable) are implementation-time
  decisions, not fixed by this design.
- Whether a future mTLS design (once/if native TLS is ever adopted)
  would replace this token scheme entirely or layer alongside it is
  unaddressed — a real question for that future revisit, not this one.
  *Resolved: layer — `SERVER-MTLS-DESIGN.md` / `ADR-0023`, implemented
  as `SERVER-001` v0.13.0 / FR-023: the certificate decides admission,
  this token scheme still decides class, unchanged.* *Extended by
  `SERVER-MTLS-CLASS-DESIGN.md` / `ADR-0028` (Proposed): the certificate
  may also decide class, when the operator pins that certificate to one
  on `AuthConfig`; a valid token still replaces it.*

## Change history

- 2026-09-01: Initial proposal, in response to the owner selecting
  authentication/encryption as one of three next directions (alongside
  the schema-driven client library, done, and session/transaction
  semantics, still pending its own design).
- 2026-09-01: Accepted as proposed, no changes requested. Implementation
  is a separate, not-yet-started unit — see ADR-0012's own "Decision"
  section.
- 2026-09-01: Implemented as `SERVER-001` v0.6.0 (`SERVER-001-FR-016`) —
  see ADR-0012's own "Acceptance and implementation" section for the full
  account, including the two implementation-time decisions this document
  left open (environment variable names, `AuthConfig::new`/`::from_env`
  split) and why the timing-measurement acceptance criterion is verified
  against `AuthConfig::check` directly rather than over a real TCP round
  trip.
