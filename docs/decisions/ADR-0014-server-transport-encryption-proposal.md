# ADR-0014: Add native transport encryption (TLS) to the server/query layer

- Status: **Accepted and implemented** (promoted from Proposed on
  2026-09-01 — the owner approved the design as revised; no further
  changes requested. Implemented the same day — see "Acceptance and
  implementation" below.)
- Date: 2026-09-01
- Deciders: baileyrd
- Related: `docs/design/SERVER-TLS-DESIGN.md` (the full design document
  this ADR summarizes), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (named "no authentication, authorization, or transport encryption" as a
  real, unresolved gap at acceptance time), `docs/decisions/ADR-0012-server-authentication-proposal.md`
  (closed the authentication half of that gap; named this proposal's own
  revisit trigger directly — see ADR-0012's "Validation and revisit
  triggers"), `docs/specifications/server/SERVER-001-query-layer.md` (the
  spec this proposal extends, `SERVER-001` v0.9.0)
- Supersedes/Superseded by: none. Extends (does not supersede)
  ADR-0012's own "transport encryption remains a separate, still-open
  gap" consequence — this ADR is the revisit ADR-0012 itself named as a
  trigger.

## Acceptance and implementation

`SERVER-001` v0.9.0 (`docs/specifications/server/SERVER-001-query-layer.md`,
`SERVER-001-FR-019`) records the real implementation: `TlsConfig`
(`src/server/mod.rs`), a small hand-written PEM/base64 decoder
(`src/server/pem.rs`, new). Implements the accepted design essentially
as revised — `serve` gains a fourth parameter, `tls: Option<TlsConfig>`;
`handle_connection` performs a TLS server handshake (via
`rusty_tls::TlsAcceptor::accept`) before any framed `Request`/`Response`
traffic, including `Authenticate`, is ever read or written;
`dispatch`/`ConnectionStore` remain completely unaware transport
encryption exists; `src/server/framing.rs` required zero changes, as the
design predicted (already generic over `Read`/`Write`).

**One real implementation-time finding beyond the design's own sketch**:
`handle_connection`'s existing plaintext path splits a connection into
independent read/write halves via `TcpStream::try_clone` (two real
OS-level socket handles) — but `rusty_tls::TlsServerStream`'s
`rustls::ServerConnection` state can't be split that way; a read and a
write both need to reach through the *same* connection object. Resolved
with a new `ReadHalf`/`WriteHalf` enum pair: the `Plain` variant keeps
the existing `try_clone` split completely unchanged (zero behavior
change, verified by the full existing plaintext test suite passing
unmodified); the `Tls` variant shares one
`Rc<RefCell<TlsServerStream<TcpStream>>>` between both halves — `Rc`/
`RefCell`, not `Arc`/`Mutex`, since each connection is served by exactly
one OS thread (thread-per-connection, unchanged), so a single-threaded
runtime borrow check is sufficient and the `Rc` itself never crosses a
thread boundary (constructed entirely inside the already-spawned
connection thread). This is a real, if bounded, design choice the
original design document left as an "implementation-time decision" for
exactly this reason.

`TlsConfig::new` takes DER-encoded certificate chain + private key
directly, matching `rusty_tls::TlsAcceptor::new`'s own shape.
`TlsConfig::from_pem_files`/`TlsConfig::from_env` (the latter mirroring
`AuthConfig::from_env`'s own pattern, reading
`SERVER_TLS_CERT_CHAIN_PATH`/`SERVER_TLS_PRIVATE_KEY_PATH`) decode the
common PEM-file case via `src/server/pem.rs` — a small, hand-written
decoder, not a new dependency, exactly as the design's own "Ecosystem
check" proposed: standard base64 decoding is a fully-specified,
deterministic transform with no invisible-to-testing correctness
property (unlike `AuthConfig::check`'s constant-time comparison, which
*is* a dependency, `subtle`, for exactly that reason). `src/bin/dog_server.rs`
is the one caller using `TlsConfig::from_env`; every test and benchmark
passes `None` directly, the same pattern `AuthConfig::default()`
established for v0.6.0.

`tests/server_tls_integration.rs` (new, `required-features = ["server"]`)
covers every functional acceptance criterion `SERVER-TLS-DESIGN.md`
names over a real socket, using `rusty_tls::TlsStream` (this ecosystem
crate's own client-side half, not a hand-rolled test client) and
`rcgen`-generated throwaway self-signed certificates (a new dev-only
dependency, matching `rusty_tls`'s own identical precedent for its own
identical purpose). The transcript-level acceptance criterion — proving
`Authenticate`'s token is genuinely absent from the wire, not just "we
used `rusty_tls` so it must be fine" — is covered directly: a
byte-recording `TcpStream` wrapper captures every byte a real TLS client
actually sends, and the test asserts the plaintext token string is not
present anywhere in that capture.

One new dependency landed exactly as the revised design specified:
`rusty_tls` (`Rusty-Mill/rusty_mill`, `crates/rusty_tls`, pinned to
commit `9fd2e27c9cfd5d1b21a5e58b55e368258a0a2779`) — this crate's first
git dependency, `dep:`-gated behind the `server` feature. `rustls` and
its crypto provider (`ring`, `rusty_tls`'s own committed choice) remain
in the dependency graph transitively; `rusty_multimodal_db`'s own source
never names `rustls` directly, preserving the seam the design's
"Ecosystem check" argued for.

No existing source file outside `src/server/mod.rs`, `src/bin/dog_server.rs`,
every `serve` call site (a one-argument addition each, `None`), and
`Cargo.toml`'s `[dependencies]`/`[dev-dependencies]`/`[[test]]` entries
was modified — verified by diff, satisfying this ADR's own "additive,
not a rewrite" decision driver, the same bar `SERVER-AUTH`'s/
`SERVER-TRANSACTION`'s own implementation rounds held themselves to.

## Context

The owner picked three next directions after `SERVER-QUERY-LAYER` v0.4.0
landed: a schema-driven client library (shipped directly, `SERVER-001`
v0.5.0, bounded/additive, no ADR needed), authentication/authorization
(ADR-0012, design-first then accepted then implemented, `SERVER-001`
v0.6.0), and session/transaction semantics (ADR-0013, same treatment,
`SERVER-001` v0.7.0). With all three done, the owner was asked "what's
next" and picked two of the offered options: a transaction throughput/
latency benchmark (bounded/additive, shipped directly as `SERVER-001`
v0.8.0, no ADR needed — see `RESULTS.md`'s `## Server / query layer`
section) and this proposal.

ADR-0012 closed the access-control half of ADR-0010's original "no auth,
no encryption" gap but was explicit that it did not close the encryption
half: "tokens and every record value are still plaintext on the wire; a
server exposed beyond `127.0.0.1`/a trusted network still requires an
external TLS-terminating proxy or tunnel." ADR-0012's own "Validation and
revisit triggers" named exactly this proposal as the condition for
revisiting: "transport encryption becomes needed natively (not via an
external proxy) ... at which point `rustls` gets its own real evaluation,
not deferred again by default." This ADR is that evaluation.

`SERVER-001`, ADR-0010, and ADR-0012 each independently still name
transport encryption as the single remaining piece of the most-repeated
open gap in this project's own documentation.

**Revised during review**: the owner asked whether this owner's own
`rusty_*`/`Rusty-Mill` ecosystem already had a hand-rolled or wrapped
solution for the dependency this ADR was about to recommend, before it
was accepted. It does — `Rusty-Mill/rusty_mill`'s `crates/rusty_tls`, a
crate that exists specifically so no consumer in this ecosystem depends
on `rustls` directly, with a server-side `TlsAcceptor`/`TlsServerStream`
API that is close to a direct fit for this proposal's own needs. This
ADR and its design document are revised accordingly, in place, as part
of the same review pass — not superseded, not a second proposal. See
`docs/design/SERVER-TLS-DESIGN.md`'s own "Ecosystem check" section for
the full finding.

## Decision drivers

- **Close the last remaining half of the most-repeated named gap**,
  honestly — this proposal must not overstate what it closes (still not
  mTLS; client identity is unchanged) or understate the real dependency
  cost involved.
- **Re-evaluate the dependency-weight objection ADR-0012 raised, on its
  actual merits, not by default deferral.** ADR-0012's own trigger says
  explicitly: "not deferred again by default." `rustls`'s synchronous API
  changes the calculus that led to the original rejection — this driver
  requires actually checking that, not reflexively repeating the earlier
  conclusion.
- **Prefer this owner's own ecosystem over a fresh third-party
  dependency when an equivalent already exists there.** Not a driver
  this ADR started with — added mid-review once the owner asked the
  question directly (see "Context" above) — but a real one going
  forward for any dependency this project takes on: `rusty_tls` is
  already tested, fuzzed, and maintained by the same owner specifically
  to prevent every consumer in their ecosystem from separately
  evaluating and depending on `rustls` (or a competing TLS crate) on its
  own.
- **Preserve the "additive, not a rewrite" bar** every prior server-layer
  round has held itself to (ADR-0010's original implementation, `SERVER-AUTH`,
  `SERVER-TRANSACTION`). A design that would force changes throughout
  `dispatch`/`ConnectionStore`/every domain adapter is a much larger
  proposal than transport encryption itself needs to be.
- **Backward compatibility by construction**, matching `AuthConfig`'s own
  precedent — a server with no `TlsConfig` configured must behave exactly
  as today, so this proposal costs nothing for every existing
  test/benchmark/binary that doesn't opt in.

## Considered options

See `docs/design/SERVER-TLS-DESIGN.md`'s own "Architecture and
interfaces" section for the full reasoning. Summarized:

1. **Transport encryption mechanism**: continue requiring an external
   TLS-terminating proxy/tunnel only (ADR-0012's own choice — rejected as
   the *sole* answer this time, but named as remaining available, not
   removed) vs. `native-tls`/system TLS bindings (rejected — same
   platform-dependent-behavior reasoning ADR-0012 already gave) vs.
   depending on `rustls` directly (this ADR's own original choice —
   rejected on revision, see below) vs. **native TLS via `rusty_tls`
   (`Rusty-Mill/rusty_mill`, this owner's own ecosystem-wide wrapper
   around `rustls`), terminated inside this crate's own server process**
   (**chosen** — its sync API composes with the existing
   thread-per-connection model with zero changes to `framing.rs`,
   undercutting ADR-0012's original objection once the implicit
   async-runtime assumption is separated out; also closes a gap an
   external proxy structurally cannot, since this crate can never verify
   from inside its own process that a proxy is actually in front of it;
   and, on top of that, `rusty_tls` exists specifically so no consumer in
   this owner's ecosystem depends on `rustls` — or picks its own
   crypto-provider crate, cipher policy, or trust-anchor handling —
   independently, so depending on `rustls` directly would have quietly
   reintroduced the exact fragmentation `rusty_tls` exists to prevent,
   for no benefit: `rusty_tls`'s server-side `TlsAcceptor`/
   `TlsServerStream` already matches this proposal's own needs almost
   exactly, tested and fuzzed already).
2. **Stream abstraction for the existing concrete-`TcpStream` call
   sites** (`handle_connection`/`send_response`): a generic
   `handle_connection<S: Read + Write>` (considered — cleanest typing,
   but a real signature change through the call chain) vs. **a small
   enum wrapping either a plain or TLS-wrapped stream, implementing
   `Read`/`Write` by delegation** (**tentatively preferred** — keeps
   `handle_connection`'s signature concrete, the same per-connection-
   enum shape `AuthConfig`'s `Option<TokenClass>` already uses; left as
   an implementation-time decision, not fixed by this ADR).
3. **mTLS bundled into this same proposal**: considered and rejected as
   this proposal's own scope — a real, larger design (client certificate
   issuance/distribution/revocation) that `SERVER-AUTH-DESIGN.md`'s own
   Non-goals explicitly deferred "pending a native-TLS decision." This
   proposal is that decision, without also deciding mTLS in the same
   pass.

## Decision

- `docs/design/SERVER-TLS-DESIGN.md` records the full proposed design:
  an opt-in `TlsConfig` (certificate chain + private key, PEM,
  operator-supplied file paths) accepted alongside the existing
  `AuthConfig`; a per-connection TLS handshake via
  `rusty_tls::TlsAcceptor::accept` completed before any framed
  `Request`/`Response` traffic (including `Authenticate`) is ever read
  or written; `src/server/framing.rs` requires zero changes, since
  `read_message`/`write_message` are already generic over `Read`/`Write`
  and `rusty_tls::TlsServerStream<S>` is already exactly the
  `Read`/`Write`-implementing wrapper this needs.
- **Client identity is unchanged by this proposal.** TLS gives the
  server a real certificate to authenticate itself with, and encrypts
  every byte on the wire (including `AuthConfig`'s tokens); it does not
  add client-certificate (mTLS) authentication. `AuthConfig`'s existing
  shared-secret token scheme remains the entire client-identity story,
  now traveling encrypted rather than plaintext.
- A server with no `TlsConfig` configured behaves exactly as today's
  `SERVER-001` — the same backward-compatibility bar `AUTH-FR-007` set
  for authentication, applied here to encryption.
- One new dependency: a pinned git dependency on `rusty_tls`
  (`Rusty-Mill/rusty_mill`, `crates/rusty_tls` — this owner's own
  ecosystem-wide TLS wrapper, matching the pinning convention `rusty_tls`
  itself already uses for its own sibling dependencies). `rustls` and its
  crypto provider (`ring`, already `rusty_tls`'s own committed choice —
  its own `Cargo.toml` explains why over `aws-lc-rs`) remain in the
  dependency graph transitively, but this crate's own `Cargo.toml` never
  names `rustls` directly — preserving the "no consumer rolls its own
  TLS" seam `rusty_tls` exists to establish across this owner's
  ecosystem. Still a real, meaningfully larger dependency addition than
  `subtle` was for authentication in terms of transitive graph weight —
  named plainly, not minimized, in "Consequences" below — but a
  materially smaller *decision* than evaluating and adopting `rustls`
  fresh, since the crypto-provider choice, cipher policy, and
  trust-anchor handling are already made and already tested.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No existing source file is modified by this ADR itself. Per
  `SERVER-AUTH`/`SERVER-TRANSACTION`'s own precedent, a real
  implementation would extend `SERVER-001` with new FRs as its own
  follow-up unit, only after this design is explicitly accepted.

## Consequences

### Positive

- Closes the last remaining half of the single most-repeated named
  security gap across this project's own documentation (`SERVER-001`,
  ADR-0010, ADR-0012 each name it independently).
- Genuinely additive at the architecture level: `framing.rs` needs zero
  changes (a verified finding, not an assumption — its functions are
  already generic over `Read`/`Write`), and `dispatch`/`ConnectionStore`/
  every domain adapter remain completely unaware transport encryption
  exists, the same "no new coordination beyond what's needed" shape
  every prior server-layer round has held to.
- Removes the one gap an external proxy structurally cannot close: this
  crate can now enforce encryption itself, rather than depending on an
  operator correctly running and configuring a separate process it has
  no way to verify.
- Does not remove the external-proxy option — an operator who prefers
  that model can still use it; this proposal only stops requiring it as
  the sole path to encryption.
- Depends on an already-tested, already-fuzzed wrapper (`rusty_tls`)
  rather than evaluating and integrating `rustls` from scratch — this
  crate inherits `rusty_tls`'s own hermetic rejection-path test suite
  (wrong hostname, expired cert, untrusted root, a real OS-trust-anchor
  corpus test) instead of needing to build equivalent coverage itself,
  and inherits working TLS 1.3 session-ticket resumption for free
  (`rusty_tls::server`'s own `finish_config` already sets a real ticketer
  — a detail this ADR would otherwise have had to get right itself).

### Negative / tradeoffs

- **A real, meaningfully larger dependency addition than any prior
  server-layer round has taken**, even sourced through `rusty_tls`
  rather than raw `rustls`. `rustls`, its crypto provider, and
  `rusty_tls` itself are a bigger transitive footprint than `subtle` (a
  small, single-purpose comparison utility) — this project's own
  minimal-dependency posture means this tradeoff should be weighed by
  the owner explicitly, not waved through because encryption sounds
  obviously necessary, or because the dependency happens to be a sibling
  repo rather than a stranger's crate.
- **A new kind of dependency for this project**: every existing
  `Cargo.toml` dependency in `rusty_multimodal_db` resolves from
  crates.io; `rusty_tls` would be this crate's first git dependency, on
  a sibling repository this owner also maintains. That is a real
  precedent shift (see `rusty_tls`'s own `docs/versioning.md` for the
  pinning discipline this pattern requires — a `rev`, never a branch,
  and awareness that a monorepo path or repository URL can move), not
  just a footnote — flagged for the owner's own judgment on whether a
  cross-repo git dependency is a pattern this project should adopt at
  all, independent of whether `rusty_tls` itself is the right crate.
- **Still not mTLS.** Client identity remains exactly `AuthConfig`'s
  shared-secret token scheme — a real, named scope limit, not an
  oversight. A sufficiently determined attacker who obtains a valid
  token can still authenticate, same as today; this proposal only
  removes the ability to *observe* that token (or any record data) on
  the wire.
- **Certificate management is now this crate's own operational surface**
  for the first time — generating, distributing, and renewing
  certificates becomes the operator's job in a way it wasn't when TLS
  was entirely delegated to an external proxy (which typically already
  has its own certificate-management tooling, e.g. via `nginx`/`Caddy`'s
  own Let's Encrypt integration). This proposal does not include
  automation for that; a real, accepted scope limit — see
  `SERVER-TLS-DESIGN.md`'s own Non-goals.
- **A self-signed certificate (the expected common case) requires
  explicit client-side trust configuration** — not a plug-and-play
  upgrade for an existing plaintext client without also updating its own
  connection setup. *Taken for this crate's own client at `SERVER-001`
  v0.12.0 / FR-022 (the PR after #129): `SchemaDrivenClient::connect_with`
  + `ClientTlsConfig::new(server_name, TrustPolicy)`; no change to this
  decision.*
- The exact call-site shape wrapping `rusty_tls::TlsServerStream`
  (`enum Connection` vs. a generic `handle_connection<S: Read + Write>`)
  is left as an implementation-time decision — a real, if bounded,
  design choice still ahead of whoever implements this. The TLS-stream
  type itself is no longer open, since `rusty_tls::TlsServerStream<S>`
  already provides it.

## Validation and revisit triggers

- **This ADR was design-only at proposal time, matching ADR-0012's/
  ADR-0013's own precedent** — no implementation, no test suite, no
  dependency actually added, at the point it was accepted. No
  standalone scratch-crate compile probe was built for this one either,
  matching ADR-0012's own reasoning: the proposed additions (one new
  optional config struct, a per-connection stream wrapper) are
  incremental extensions of `SERVER-001`'s existing, already-compiling
  shapes, not a genuinely new type-system structure. Flagged here
  explicitly as a deliberate scope choice, not an oversight. A real
  implementation followed acceptance as its own unit — see "Acceptance
  and implementation" above once it lands.
- Revisit if: mTLS becomes a real requirement now that this crate owns
  TLS natively — a real, separate future design, not decided here (see
  `SERVER-TLS-DESIGN.md`'s own "Open questions"). Narrower than it was
  before this revision: the mechanism (`rusty_tls::TlsAcceptor::new_with_client_auth`)
  already exists, so that future revisit would mostly be a client-cert
  distribution/revocation policy design, not an implementation from
  scratch.
- Revisit if: certificate rotation without a server restart becomes a
  real operational need — this design's "restart the process with new
  cert/key files" story would need replacing.
- Revisit if: the owner judges the `rusty_tls`/`rustls`/crypto-provider
  dependency weight disproportionate on review, **or** judges a git
  dependency on a sibling repository the wrong pattern for this project
  regardless of which crate it names — the external-proxy path remains
  fully available as the alternative, and this ADR would be rejected (or
  revised to scope the dependency differently, e.g. back to a direct
  `rustls` dependency if the sibling-repo pattern itself is rejected)
  rather than force through regardless.
