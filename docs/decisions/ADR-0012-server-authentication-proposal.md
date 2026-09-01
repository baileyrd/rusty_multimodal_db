# ADR-0012: Add authentication and coarse read/write authorization to the server/query layer

- Status: **Accepted and implemented** (promoted from Proposed on 2026-09-01 — the owner approved the design as proposed; no changes requested. Implemented the same day — see "Acceptance and implementation" below.)
- Date: 2026-09-01 (proposed and accepted same day)
- Deciders: baileyrd
- Related: `docs/design/SERVER-AUTH-DESIGN.md` (the full design document this
  ADR summarizes), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (names exactly this gap as one of its own "Validation and revisit
  triggers"), `docs/specifications/server/SERVER-001-query-layer.md` (the
  spec this proposal would extend, not modify, until accepted),
  `docs/FUTURE-GROWTH.md` ("Path to a server / query layer" — names
  authentication/authorization as "genuinely new," not an incremental
  extension)
- Supersedes/Superseded by: none. Extends (does not supersede) ADR-0010's
  "no authentication, authorization, or transport encryption" consequence
  — this ADR is the revisit ADR-0010 itself named as a trigger.

## Acceptance and implementation

`SERVER-001` v0.6.0 (`docs/specifications/server/SERVER-001-query-layer.md`, `SERVER-001-FR-016`) records the real implementation: `AuthConfig`/`TokenClass` (`src/server/mod.rs`), `Request::Authenticate`/`ErrorCode::{Unauthenticated,Unauthorized}` (`src/server/protocol.rs`). Implements the accepted design essentially as proposed — one new `Request` variant, two new `ErrorCode` variants, per-connection authentication state checked in `handle_connection`'s own request loop before `dispatch` is ever reached for anything but `Authenticate`, constant-time comparison via `subtle`. No completion beyond the design's own sketch was needed this time (unlike ADR-0010's implementation round) — the design's `Proposed shape` compiled essentially unchanged.

`AuthConfig::new`/`AuthConfig::from_env` split the "already have tokens" and "read tokens from the process environment" cases the design left as an implementation-time decision; `SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN` are the two environment variable names chosen (`src/bin/dog_server.rs` is the one caller that uses `from_env`; every test and benchmark uses `AuthConfig::new`/`::default()` directly, since `cargo test` runs many tests in parallel within one process and real environment variables would race). `AuthConfig::default()` (no tokens configured) required zero changes to any pre-existing call site's behavior — verified directly: `tests/server_{dog,order,employee}_integration.rs`, `tests/server_schema_driven_client.rs`, and `benches/server.rs` all pass unmodified in substance (only the `serve` call itself gained one new, unconditionally-default argument) — the exact backward-compatibility bar `AUTH-FR-007` set.

`tests/server_auth_integration.rs` (new, `required-features = ["server"]`) covers every functional acceptance criterion `SERVER-AUTH-DESIGN.md` names over a real socket. The timing-measurement acceptance criterion (`AUTH-FR-006`) is covered by a unit test measuring `AuthConfig::check` directly (`server::tests::token_comparison_time_does_not_depend_on_where_the_mismatch_is`) rather than over a real TCP round trip — a deliberate choice, not a scope-narrowing: network jitter (microseconds to milliseconds) would completely swamp the signal this specific claim is about (a difference on the order of one byte comparison in a same-length byte string), so measuring at the network layer would produce a test that always passes regardless of whether the underlying comparison is actually constant-time — no real evidence at all. Measuring `AuthConfig::check` directly is the more meaningful test of the actual claim.

One new dependency landed exactly as the design specified: `subtle = "2"` (`Cargo.toml`), used only by `AuthConfig::check`.

No existing source file outside `src/server/{mod,protocol}.rs`, every `serve` call site (a one-argument addition each), `src/bin/dog_server.rs`, `tests/server_auth_integration.rs` (new), and `Cargo.toml`'s `[dependencies]`/`[[test]]` entries was modified — verified by diff, satisfying this ADR's own "additive, not a rewrite" decision driver, the same bar ADR-0010's own implementation round held itself to.

## Context

The owner picked three next directions at once after `SERVER-QUERY-LAYER`
v0.4.0 landed: a schema-driven client library, this proposal
(authentication/encryption), and session/transaction semantics. The
client library was bounded/additive and shipped directly as
`SERVER-001` v0.5.0, no new ADR needed (it completed ADR-0011's
already-accepted decision rather than deciding something new). This
proposal and the transaction-semantics one are different: both are
named "genuinely new," not incremental, in `docs/FUTURE-GROWTH.md`'s own
"Path to a server / query layer" section, and both change this crate's
security or consistency posture in a way a client could come to depend
on — the same "hard to reverse once a client depends on it" reasoning
ADR-0010 itself used to justify a design-only-first pass before
`SERVER-QUERY-LAYER` was implemented. This ADR follows that same
treatment: **it authorizes a design, not implementation code**, matching
this project's `adr-cadence.md` Regime 1 discipline for a consequential
decision during active major development.

ADR-0010's own "Validation and revisit triggers" section named this
directly: "the project decides to pursue authentication/encryption, at
which point this ADR's 'no auth' consequence should be superseded rather
than silently outdated." This ADR is that supersession — not of ADR-0010
as a whole (its protocol/framing/concurrency-model decisions are
unchanged and out of scope here), only of its "no auth" consequence.

`SERVER-001`, ADR-0010, and `docs/FUTURE-GROWTH.md` each independently
name "no authentication, no authorization, no transport encryption" as
the single reason a server built from this crate cannot leave a trusted,
localhost/development network. This is the most-repeated open gap in
this project's own documentation.

## Decision drivers

- **Close the most-repeated named gap**, but honestly — this proposal
  must not create a false sense of security. Authentication without
  transport encryption is not "safe to expose beyond localhost"; a
  sniffed token on an unencrypted network defeats the whole scheme. This
  proposal has to be explicit about which half of the gap it closes and
  which it doesn't.
- **Minimal new dependency footprint**, the same discipline that chose a
  hand-rolled binary protocol over gRPC/JSON-HTTP and thread-per-connection
  over `tokio` in ADR-0010. A full native-TLS stack (`rustls` plus its
  transitive crypto/cert dependencies) is a materially larger dependency
  bite than anything this crate has taken on for the server layer so far
  — this proposal weighs that explicitly rather than reaching for it by
  default.
- **Coarse-grained, not a general ACL/RBAC system.** Matches this
  project's own "no abstraction before a real need" discipline and
  ADR-0010's own precedent of scoping session/transaction semantics out
  entirely rather than half-building them.
- **Correctness-critical code gets extra scrutiny, not the usual
  dependency default.** A hand-rolled constant-time comparison is a
  well-known place where DIY security code silently regresses (compiler
  optimizations can reintroduce early-exit timing behavior); this
  proposal treats that specific piece differently from the rest of its
  "avoid new dependencies" posture.

## Considered options

See `docs/design/SERVER-AUTH-DESIGN.md`'s own "Architecture and
interfaces" section for the full reasoning. Summarized:

1. **Transport encryption**: native TLS via `rustls` (rejected for this
   proposal — a real, meaningful new dependency footprint, deferred, not
   ruled out forever) vs. `native-tls`/system TLS bindings (rejected —
   platform-dependent behavior) vs. **no native TLS in this crate;
   require an external TLS-terminating proxy or tunnel in front of the
   plaintext socket** (**chosen** for this proposal — no new dependency,
   real precedent in how wire-protocol databases have historically
   layered TLS on before owning it natively).
2. **Authentication mechanism**: no auth (rejected — the entire premise)
   vs. per-user accounts with a real identity store (rejected —
   disproportionate; this crate has no concept of "users" anywhere else
   in its data model) vs. mTLS client certificates (out of scope until/
   unless native TLS is revisited, since it depends on owning TLS
   directly) vs. **a shared-secret token sent once per connection via a
   new `Request::Authenticate`, checked before any other request is
   served** (**chosen**).
3. **Authorization granularity**: per-field/per-record ACLs (rejected —
   disproportionate scope, same reasoning ADR-0010 used to scope out
   transactions) vs. **two coarse, static token classes,
   `ReadOnly`/`ReadWrite`, checked once at authenticate time**
   (**chosen**).
4. **Constant-time token comparison**: hand-rolled compare, no new
   dependency (considered, then rejected specifically for this
   correctness-critical piece) vs. **the `subtle` crate, small and
   purpose-built for exactly this** (**chosen** — the one new dependency
   this proposal adds, and the one place this proposal departs from its
   own "avoid new dependencies" driver above, deliberately).

## Decision

- `docs/design/SERVER-AUTH-DESIGN.md` records the full accepted design:
  `Request::Authenticate { token: String }`; per-connection authenticated
  state (unauthenticated connections get a typed rejection —
  `ErrorCode::Unauthenticated` — for every other request kind, including
  `DescribeSchema`); two token classes (`ReadOnly`/`ReadWrite`) configured
  server-side via environment variables at process startup, never
  embedded in source or committed to the repository; constant-time
  comparison via the `subtle` crate.
- **Transport encryption is explicitly not part of this proposal's own
  server code.** Documented as a required companion (an external
  TLS-terminating proxy or tunnel) for any non-localhost deployment. This
  proposal alone does not authorize deploying beyond a trusted network —
  it closes the "no access control" half of the gap, not the "no
  encryption" half. Both are still required together before ADR-0010's
  "do not expose beyond localhost/trusted-network" conclusion is lifted.
- One new dependency: `subtle` (small, purpose-built for constant-time
  comparison, not a general crypto/TLS library).
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No existing source file is modified by this ADR itself. Per
  ADR-0010's own precedent (`SERVER-001` registered and implemented as a
  separate step after that ADR's acceptance), a real implementation would
  extend `SERVER-001` with new FRs (or register a dedicated spec) as its
  own follow-up unit, only after this design is explicitly accepted.

## Consequences

### Positive

- Closes the single most-repeated named security gap across this
  project's own documentation (`SERVER-001`, ADR-0010,
  `docs/FUTURE-GROWTH.md` all name it independently) with a small,
  bounded design.
- Reuses the existing wire protocol's own `Request`/`Response`/
  `ErrorCode` shapes rather than inventing a parallel auth sub-protocol —
  `Authenticate` is one more `Request` variant, `Unauthenticated` is one
  more `ErrorCode` variant, nothing structurally new.
- Keeps the "no native-TLS dependency" line in the same place the
  charter's minimal-dependency posture already drew it for `tokio`/gRPC
  in ADR-0010, deferring a much heavier dependency addition until it's
  actually needed rather than reaching for it because auth is
  security-adjacent.

### Negative / tradeoffs

- **Auth alone, without transport encryption, does not make this safe to
  expose beyond a trusted network** — a real, named limitation of this
  specific proposal, not a hidden one. A sniffed token on an unencrypted
  network defeats the whole scheme; this design depends on the operator
  correctly pairing it with a TLS-terminating proxy, which this crate
  cannot enforce or verify from inside the process.
- Two static token classes is coarse — no per-user identity, no
  per-field/per-record authorization, no audit log of who did what. A
  real, accepted scope limit, not an oversight — see `SERVER-AUTH-DESIGN.md`'s
  own Non-goals.
- Token rotation/revocation has no story in this design — a token is
  valid until the server process restarts with a different one
  configured; no expiry, no revocation list.
- One new dependency (`subtle`) where this project has otherwise avoided
  new dependencies aggressively for the server layer specifically —
  flagged explicitly for the owner's own judgment on whether the
  correctness benefit justifies it over a hand-rolled comparison.

## Validation and revisit triggers

- **This proposal is design-only, matching ADR-0010's own precedent** —
  no implementation, no test suite yet. Unlike ADR-0009's/ADR-0010's own
  proposals, no standalone scratch-crate compile probe was built for this
  one: the proposed additions (one new `Request` variant, one new
  `ErrorCode` variant, a per-connection auth-state check, a constant-time
  compare) are incremental extensions of `SERVER-001`'s existing,
  already-compiling `protocol.rs`/`mod.rs` shapes, not a genuinely new
  type-system structure the way `crate::generic`'s trait system or the
  original `ConnectionStore`/dispatch design were — the risk a probe
  would be de-risking is judged low enough not to warrant one. Flagged
  here explicitly as a deliberate scope choice, not an oversight.
- Revisit if: transport encryption becomes needed natively (not via an
  external proxy) — e.g. a deployment target with no ability to run a
  separate TLS-terminating process — at which point `rustls` gets its own
  real evaluation, not deferred again by default.
- Revisit if: authorization needs to grow finer-grained than two static
  classes (e.g. per-domain or per-field scoping) — a real, larger design,
  not a small extension of this one.
- Revisit if: token rotation/revocation becomes a real operational need —
  this design's "restart the process with a new token" story would need
  replacing.
