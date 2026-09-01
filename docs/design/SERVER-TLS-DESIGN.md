# Server Transport Encryption Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-01 — the owner
  approved the design as revised, no further changes requested).
  Acceptance authorizes the design; implementation follows as its own
  unit — see ADR-0014's own "Acceptance and implementation" section
  once it lands.
- Date: 2026-09-01
- Related: `docs/specifications/server/SERVER-001-query-layer.md` (the
  spec this design would extend, once accepted), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (named "no authentication, authorization, or transport encryption" as a
  real, unresolved gap at acceptance time), `docs/design/SERVER-AUTH-DESIGN.md`/`docs/decisions/ADR-0012-server-authentication-proposal.md`
  (closed the authentication half of that gap; explicitly left the
  encryption half open and named its own revisit trigger for this
  proposal — see ADR-0012's "Validation and revisit triggers")

## Purpose and scope

`SERVER-001`, ADR-0010, and ADR-0012 each independently name the same
still-open gap: "anyone who can observe the network can read the token
and every record in transit." ADR-0012 closed the access-control half of
ADR-0010's original "no auth, no encryption" gap (`AuthConfig`,
`Request::Authenticate`) but explicitly did not close this half, and
named its own revisit trigger for it directly: "transport encryption
becomes needed natively (not via an external proxy) ... at which point
`rustls` gets its own real evaluation, not deferred again by default."
This document is that evaluation.

**In scope for this proposal:**

- Native TLS termination inside this crate's own server process, via
  **`rusty_tls`** (`Rusty-Mill/rusty_mill`, `crates/rusty_tls` — this
  owner's own ecosystem-wide TLS wrapper, itself built on `rustls`), not
  a direct `rustls` dependency and not an external TLS-terminating
  proxy/tunnel. See "Ecosystem check" below — this was revised from an
  initial draft that proposed depending on `rustls` directly, after the
  owner asked whether an existing hand-rolled/wrapped solution already
  covered it.
- An opt-in `TlsConfig` (certificate chain + private key, PEM,
  operator-supplied file paths), mirroring `AuthConfig`'s own opt-in
  shape: a server started with no `TlsConfig` behaves exactly as today
  (plaintext) — the same backward-compatibility bar `AUTH-FR-007` set for
  authentication.
- A stream abstraction so `handle_connection`/`send_response` (currently
  concrete over `std::net::TcpStream`) work uniformly whether or not TLS
  is active.
- Documentation of exactly what this does and doesn't close: encrypts
  wire traffic and lets a client verify the server's identity via a real
  certificate; does **not** add client-certificate (mTLS) authentication
  — client identity remains `AuthConfig`'s existing shared-secret token
  scheme, now traveling encrypted rather than plaintext.

**Explicitly out of scope, named directly rather than left implicit:**

- mTLS / client-certificate authentication. `SERVER-AUTH-DESIGN.md`'s own
  Non-goals deferred it "pending a native-TLS decision" — this proposal
  *is* that decision, but does not also decide mTLS in the same pass. A
  real, separate future design (see "Open questions").
- Certificate provisioning/rotation automation (ACME/Let's Encrypt
  integration, SIGHUP-triggered reload). An operator supplies a cert/key
  file path; replacing them means restarting the server with new files —
  the same "restart the process with new config" story `AuthConfig`'s own
  token-rotation non-goal already accepted.
- A custom TLS version/cipher-suite policy. This proposal takes
  `rusty_tls`'s own default, vetted policy rather than authoring one.
- Removing or deprecating the external-proxy option. An operator who
  prefers a proxy/tunnel in front of a plaintext server can still run
  one — nothing in this design forces native TLS to be used. This
  proposal only stops requiring it as the *sole* option.

## Non-goals

- Not a claim that this design, once implemented, closes every remaining
  security gap on its own. No rate-limiting of failed authentication
  attempts, no audit log — both already named as open gaps by
  `SERVER-AUTH-DESIGN.md` and unchanged by this proposal.
- Not a rewrite of the wire protocol or framing. `src/server/framing.rs`'s
  `read_message`/`write_message` are already generic over `Read`/`Write`
  and need **zero** changes for this proposal — a real, verified
  architectural finding this design surfaces (see "Context and
  terminology" below), not an assumption.
- Not mTLS (see "out of scope" above).
- Not session/transaction semantics — an unrelated axis, already
  delivered separately (`SERVER-001` v0.7.0, ADR-0013).

## Ecosystem check: `rusty_tls` already exists

Before finalizing this proposal, the owner asked whether this ecosystem
(`Rusty-Mill/rusty_mill` plus this owner's other `rusty_*` repos) already
had a hand-rolled or wrapped solution for the dependency being proposed
here, rather than reaching for `rustls` fresh. It does, and it is a
direct, close-to-drop-in fit:

- **`rusty_tls`** (`Rusty-Mill/rusty_mill`, `crates/rusty_tls`) exists
  specifically so "no consumer (`rusty_request`, `rusty_rdp`, and
  eventually `rusty_tail`) ever rolls its own TLS again" — its own README
  states the rule plainly: "consumers import `rusty_tls`, never
  `rustls`." That is exactly the seam this proposal was about to
  reinvent by depending on `rustls` directly.
- Its server-side surface already matches this design's own shape almost
  exactly: `TlsAcceptor::new(cert_chain_der, private_key_der)` builds a
  reusable, `Arc`-backed config once (mirroring `TLS-FR-006`'s own "load
  once at startup, share across connections" requirement almost
  verbatim); `TlsAcceptor::accept<S: Read + Write>(sock: S) ->
  Result<TlsServerStream<S>, Error>` wraps an already-accepted socket,
  and `TlsServerStream<S>` itself implements `Read`/`Write` by
  delegating to `rustls::Stream` internally — the exact "small stream
  wrapper implementing `Read`/`Write` by delegation" this document's own
  original "Considered options" named as the tentatively preferred,
  implementation-time-decided shape for `TLS-FR-005`. It is generic over
  any `S: Read + Write`, so it slots directly onto `TcpStream` with no
  change needed to accommodate it.
- It ships tested, fuzzed rejection-path coverage (wrong hostname,
  expired cert, untrusted root; a real corpus test against actual OS
  trust anchors) that this proposal would otherwise have had to build
  from scratch to meet its own "Verification plan" below.
- `TlsAcceptor::new_with_client_auth` already implements mTLS server-side
  — not something this proposal is adopting now (see "Explicitly out of
  scope" above; a client-certificate *distribution and revocation*
  policy is still a real, separate future decision), but it means a
  future mTLS revisit would be a policy design, not a from-scratch
  implementation, changing "Open questions"'s own cost estimate for that
  revisit trigger.
- Dependency-weight consequence: this changes what "one new dependency"
  in ADR-0014's own "Decision"/"Consequences" sections means. Rather than
  `rusty_multimodal_db`'s `Cargo.toml` naming `rustls` (plus a
  crypto-provider crate) directly, it would name a single pinned git
  dependency on `rusty_tls` — matching the pinning convention `rusty_tls`
  itself already uses for its own sibling dependencies (`rustils`'
  `docs/versioning.md` §3: pin a `rev`, never track a branch). `rustls`
  and its crypto provider (`ring`, already `rusty_tls`'s own committed
  choice — see its `Cargo.toml`'s own justification comment) remain in
  the dependency graph transitively, but this crate's own source never
  names `rustls` — preserving the "no consumer rolls its own TLS" seam
  the wider ecosystem has already standardized on, and keeping this
  crate's own `Cargo.toml` justification comments pointed at a sibling
  the owner already maintains rather than a fresh third-party crate.
- One real gap, not papered over: `rusty_tls::TlsAcceptor::new` takes
  DER-encoded bytes (`Vec<Vec<u8>>` cert chain, `Vec<u8>` key), not PEM
  file paths — `rusty_tls` deliberately keeps its own public seam narrow
  (DER in, DER out) and does not re-expose a PEM parser. `TLS-FR-006`'s
  "operator-supplied PEM file paths" still needs a small PEM→DER step
  somewhere (stripping `-----BEGIN/END-----` lines and base64-decoding
  the body — a handful of lines, no new dependency; `base64` decoding
  alone doesn't need a crate for this scale of use, and if it's ever
  wanted as reusable capability rather than a one-off, it belongs
  upstream in `rusty_tls` itself as a convenience constructor, not
  duplicated here). See "Open questions" below.
- One structural detail worth naming for whoever implements this:
  `rusty_tls` currently lives inside the `Rusty-Mill/rusty_mill`
  monorepo as `crates/rusty_tls`, while its own `Cargo.toml` still
  points at `https://github.com/baileyrd/rusty_tls` for its own
  `repository` field and at separate `baileyrd/rustils`/`baileyrd/rusty_tokio`
  repos for its sibling git dependencies. The exact git-dependency
  incantation `rusty_multimodal_db`'s own `Cargo.toml` would need (a
  monorepo path dependency vs. a still-current standalone mirror) is an
  implementation-time detail to verify against whichever URL is current
  at implementation time, not fixed by this design.

## Context and terminology

- **TLS termination point**: inside the server process itself, before
  `framing.rs`'s `read_message`/`write_message` ever see the bytes — as
  opposed to ADR-0012's chosen design (an external proxy terminates TLS
  in a separate process; this crate never sees encrypted bytes at all,
  and can't verify the proxy is even in place).
- **`rusty_tls`'s synchronous API**: `TlsAcceptor`/`TlsServerStream`
  (themselves built on `rustls::ServerConnection` and its
  `rustls::Stream` adapter) implement `std::io::Read`/`Write` over an
  inner, generic `S: Read + Write`, so a TLS-wrapped connection can be
  used exactly like a plain one by any code already written against
  `Read`/`Write` — no async runtime required, compatible with the
  existing thread-per-connection blocking-socket model unchanged. (The
  `rusty-tokio` feature exists for an async counterpart, but this
  proposal has no use for it — `rusty_multimodal_db` has no async
  runtime anywhere, and none of `rusty_tls`'s async-specific
  version-pinning hazards, documented in its own `docs/versioning.md`,
  apply here.)
- **The genuine architectural finding this design depends on**:
  `src/server/framing.rs`'s `write_message<W: Write, T: Serialize>`/
  `read_message<R: Read, T: DeserializeOwned>` are already generic over
  `Read`/`Write`, not hardcoded to `TcpStream` — verified by reading the
  current source, not assumed. This means the framing layer needs no
  change at all; only the concrete stream type `handle_connection`/
  `send_response` hold (currently `TcpStream` directly) needs to become
  "either a plain socket or a TLS-wrapped one" — and `rusty_tls`'s own
  `TlsServerStream<S>` is already exactly that wrapper, generic over the
  same `S: Read + Write` bound.

## Requirements

- `TLS-FR-001`: `serve` accepts an optional `TlsConfig` alongside the
  existing `AuthConfig`. Passing none (or a default) reproduces today's
  plaintext behavior exactly — zero change required for
  `AuthConfig::default()`-style existing callers (`dog_server`, every
  test, every benchmark) unless they opt in.
- `TLS-FR-002`: When configured, every accepted connection performs a TLS
  server handshake (via `rusty_tls::TlsAcceptor::accept`) before any
  framed message is read. `dispatch`/`ConnectionStore` remain completely
  unaware transport encryption exists — the same "no new coordination
  beyond what's needed" driver ADR-0010 used, extended one layer down.
- `TLS-FR-003`: A connection that fails the TLS handshake (unsupported
  client, corrupt handshake bytes) is dropped cleanly — no panic,
  extending this crate's existing "never panic on malformed/hostile
  input" discipline one layer below the existing frame-level guarantees.
- `TLS-FR-004`: `framing.rs` requires zero changes — already generic over
  `Read`/`Write` (see "Context and terminology" above).
- `TLS-FR-005`: `handle_connection`/`send_response` need a stream
  abstraction — `rusty_tls::TlsServerStream<S>` (generic over
  `S: Read + Write`) already provides it directly; the remaining
  decision is only the enum-vs-generic shape wrapping *that* type at the
  `handle_connection`/`serve` call sites (see "Architecture and
  interfaces" below), not the TLS-stream abstraction itself.
- `TLS-FR-006`: Certificate and private key are loaded once at server
  startup from operator-supplied PEM file paths, never hardcoded, never
  committed to the repository — matching `AUTH-FR-005`'s own "read from
  config at startup, not embedded" precedent, applied to key material.
  `rusty_tls::TlsAcceptor::new` itself takes DER bytes, not PEM paths, so
  this requirement includes a small PEM→DER decode step (see "Ecosystem
  check" above) — not a new dependency, a few lines of code.
- `TLS-FR-007`: A server with both `TlsConfig` and `AuthConfig`
  configured composes both unchanged — `Authenticate`'s token now
  travels over the encrypted channel rather than plaintext, closing
  exactly the gap ADR-0012 named, with no ordering hazard (the TLS
  handshake completes fully before any framed `Request`, including
  `Authenticate`, is ever read).
- `TLS-FR-008`: Backward compatible by construction: no `TlsConfig`
  configured means every existing test/benchmark/binary keeps working
  with zero changes — the same bar `AUTH-FR-007` set for authentication.

## Architecture and interfaces

### Considered options

**Transport encryption mechanism.**

1. *Continue requiring an external TLS-terminating proxy/tunnel only*
   (status quo, ADR-0012's own choice). Rejected as the sole answer for
   this proposal specifically — the owner picked "transport encryption"
   as a next direction to close, not to re-affirm the existing gap. Named
   explicitly as remaining *available*, not removed: an operator who
   prefers a proxy can still run one instead of configuring `TlsConfig`.
2. *Native TLS, terminated inside this crate's own server process.*
   **Chosen — via `rusty_tls`, not a direct `rustls` dependency (revised
   after the "Ecosystem check" above).** ADR-0012 rejected native TLS for
   authentication specifically because it judged a full native-TLS stack
   disproportionate dependency weight — but that judgment implicitly
   bundled TLS with an async-runtime shift (the conventional way most
   Rust servers add TLS, via `tokio-rustls`). `rusty_tls`'s sync API
   composes with this crate's existing thread-per-connection
   blocking-socket model with **zero** changes to `framing.rs`,
   undercutting the original "disproportionate for what this needs"
   framing once the async-runtime assumption is separated out. Depending
   on `rusty_tls` rather than `rustls` directly is a further refinement
   on top of that reversal, not a separate decision: this owner's own
   ecosystem already maintains a tested, fuzzed wrapper purpose-built so
   no consumer depends on `rustls` (or picks its own crypto-provider
   crate, cipher policy, etc.) independently — reaching past it for
   `rustls` directly would have quietly reintroduced the exact
   fragmentation `rusty_tls` exists to prevent, and this crate would gain
   none of its rejection-path test coverage for free. It closes a gap an
   external proxy structurally cannot either way: this crate has no way
   to verify from inside its own process that a proxy is actually in
   front of it.
3. *Depend on `rustls` directly, bypassing `rusty_tls`.* This was this
   document's own original choice, before the owner asked whether an
   ecosystem alternative already existed. Rejected on revision — see
   "Ecosystem check" above for the full reasoning; superseded by option 2.
4. *`native-tls` (system TLS bindings — Schannel/SecureTransport/
   OpenSSL, chosen per platform).* Rejected — same platform-dependent
   reasoning ADR-0012 already gave; unchanged by this proposal.
5. *mTLS (client-certificate authentication) bundled into this same
   proposal.* Rejected as this proposal's own scope — real, larger design
   (certificate issuance/distribution to clients, revocation), matching
   how `SERVER-AUTH-DESIGN.md`'s own Non-goals deferred it "pending a
   native-TLS decision." This proposal *is* that decision, without also
   deciding mTLS in the same pass — see "Open questions."

**Stream abstraction for `handle_connection`/`send_response`.**

1. *Make `handle_connection`/`send_response` generic over
   `Stream: Read + Write`.* Cleanest from a types perspective, but
   changes both functions' signatures and how `serve`'s `accept()` loop
   spawns per-connection threads (today `TcpStream` is a concrete,
   directly-`Send`-able type handed straight to `thread::spawn`; a
   generic would need the TLS-wrapped type constructed once per
   connection, still `Send`, before the thread is spawned — mechanically
   fine, but a real signature change through the call chain).
2. *A small enum, `enum Connection { Plain(TcpStream),
   Tls(rusty_tls::TlsServerStream<TcpStream>) }`, implementing
   `Read`/`Write` by delegating to whichever variant is active.*
   **Tentatively preferred** — keeps `handle_connection`'s own signature
   concrete (one type, not a generic parameter), the same shape
   `AuthConfig`'s `Option<TokenClass>` already uses (per-connection
   enum/state, not a type-level parameter). The TLS half of this enum is
   no longer this design's own invention — `rusty_tls::TlsServerStream<S>`
   already exists and already implements `Read`/`Write` for any
   `S: Read + Write`; the only remaining decision this design leaves open
   is the enum-vs-generic choice at the call site, not the wrapper type
   itself. Not fixed by this design — an implementation-time decision,
   matching how `SERVER-AUTH-DESIGN.md` left
   `AuthConfig::new`/`AuthConfig::from_env`'s exact split open.

### Proposed shape

```rust
// src/server/mod.rs — additive, not a rewrite of the existing types

struct TlsConfig {
    // Loaded once at server startup from operator-supplied PEM file
    // paths — exact field shape (raw paths vs. pre-parsed DER) is an
    // implementation-time decision; either way a small PEM->DER decode
    // step sits between this struct and rusty_tls::TlsAcceptor::new,
    // which takes DER bytes directly (see "Ecosystem check" above).
    cert_chain_path: PathBuf,
    private_key_path: PathBuf,
}

// serve gains one more optional parameter, alongside AuthConfig:
// pub fn serve<S: ConnectionStore + 'static>(
//     listener: TcpListener,
//     store: Arc<S>,
//     auth: AuthConfig,
//     tls: Option<TlsConfig>,
// ) { ... }
// Internally, a configured TlsConfig builds one rusty_tls::TlsAcceptor
// once (via TlsAcceptor::new(cert_chain_der, private_key_der)), the same
// "build once, Arc-share across connections" lifecycle AuthConfig's own
// tokens already use.

// A per-connection stream abstraction (sketch — see "Considered
// options" above for the alternative of a generic handle_connection).
// The Tls variant is rusty_tls's own type, not a hand-rolled wrapper:
// enum Connection {
//     Plain(TcpStream),
//     Tls(rusty_tls::TlsServerStream<TcpStream>),
// }
// impl Read for Connection { /* delegates to whichever variant */ }
// impl Write for Connection { /* delegates to whichever variant */ }

// handle_connection's own signature changes from `TcpStream` to
// `Connection`; framing.rs's read_message/write_message are called
// exactly as they are today — no change there at all, since both are
// already generic over Read/Write.
```

The TLS handshake itself (`rusty_tls::TlsAcceptor::accept(sock)`, which
performs no I/O itself — the handshake runs lazily, driven by the first
`Read`/`Write` call, exactly like the framing layer's own first call —
or `TlsServerStream::complete_handshake()` to force it eagerly) happens
once, per accepted connection, before `handle_connection`'s own request
loop starts — the same place `AuthConfig`'s per-connection state begins
its life today.

## Data/state and invariants

- TLS handshake/session state lives inside `rusty_tls::TlsServerStream`
  (itself wrapping `rustls::ServerConnection`), held for the connection's
  lifetime — the same per-connection lifetime as today's read/write
  buffers and (since v0.6.0) `Option<TokenClass>`. Never shared across
  connections, never touching the wrapped store.
- The loaded `rusty_tls::TlsAcceptor` (built once from the parsed
  certificate chain + key, internally `Arc`-backed already — see
  `rusty_tls::server`'s own doc comment on `TlsAcceptor::accept` "only
  clones an `Arc`") is built once at server startup and shared read-only
  across every connection thread, the same pattern `AuthConfig` already
  uses for its own configured tokens — no new lock.
- No new persistent state, no new on-disk format — cert/key files are
  read once at startup, never embedded in this crate's own on-disk store
  format.

## Errors, failure, recovery, and observability

- A failed TLS handshake drops the connection cleanly, no panic — before
  any `Request`/`Response` traffic, so `dispatch`/the store are never
  involved.
- Malformed or missing cert/key files at startup fail the server's own
  construction with a typed `Result`, matching this crate's existing "no
  `unwrap()`/`expect()` outside tests" constraint — fails fast at
  startup, not per-connection.
- Out of scope, named directly rather than silently assumed solved: TLS
  session resumption/ticket policy, OCSP stapling, cipher-suite pinning
  beyond `rusty_tls`'s (and, transitively, `rustls`'s) own safe defaults.
  This proposal takes `rusty_tls`'s default, vetted policy rather than
  authoring a custom one — `rusty_tls::server`'s own `finish_config`
  already sets a real session-ticket producer for TLS 1.3 resumption, so
  this proposal gets working session resumption without deciding
  anything about it itself.

## Security, privacy, and compatibility

- **Closes the "anyone who can observe the network can read the token
  and every record in transit" gap** — the one gap `ADR-0012`'s own
  "Decision" section explicitly left open. Combined with `AuthConfig`,
  this closes both halves of the original "no auth, no encryption" gap
  ADR-0010 named at acceptance.
- **Still not mTLS.** The server authenticates itself to the client (a
  real certificate); client identity remains exactly `AuthConfig`'s
  existing shared-secret token scheme, now traveling encrypted rather
  than plaintext.
- A self-signed certificate (the expected common case for a project with
  no public DNS name or CA-issued certificate) means a client must be
  configured to trust it explicitly (pinning or a custom trust root) —
  this proposal does not include tooling to generate one. The operator's
  responsibility, named directly rather than silently assumed away.
- Backward compatible by construction (`TLS-FR-008`): unless `TlsConfig`
  is configured, nothing about today's `SERVER-001` behavior changes.

## Acceptance criteria

(For the eventual implementation unit, once this design is accepted —
not attempted by this proposal itself.)

- A real client connecting to a `TlsConfig`-configured server over a
  plain (non-TLS) socket fails the handshake / never receives a valid
  response — proving TLS is genuinely enforced, not merely offered.
- A real client, configured to trust the server's certificate, completes
  a full request/response round trip (e.g. `GetById`) over TLS
  identically to today's plaintext behavior — proving `dispatch`/
  `ConnectionStore` genuinely don't need to know transport encryption
  exists.
- `Request::Authenticate`'s token, captured on the wire for a
  `TlsConfig`-configured connection, is not present in plaintext — the
  actual evidence this design's whole purpose depends on, mirroring
  `AUTH-FR-006`'s own "prove it, don't just assert it" bar.
- A server started with no `TlsConfig` behaves identically to today's
  `SERVER-001` for every existing integration test, zero test changes
  required.

## Verification plan

(Also for the eventual implementation unit.)

- Unit tests: the new PEM→DER decode step (valid, malformed, missing
  file) and `rusty_tls::TlsAcceptor::new` construction from the decoded
  bytes. `rusty_tls` itself already carries its own hermetic
  rejection-test suite (wrong hostname, expired cert, untrusted root, a
  real OS-trust-anchor corpus test) — this crate's own tests don't need
  to re-prove TLS correctness, only that it's wired in correctly.
- Real end-to-end tests: a real client/server pair over an actual TLS
  handshake, using a self-signed certificate generated at test time (no
  committed certificate or key material in the repository — applying
  `AUTH-FR-005`'s own "never committed" bar to key material this time).
  `rusty_tls`'s own dev-dependency on `rcgen` for exactly this purpose
  (throwaway certificates for its own rejection-path suite) is a real
  precedent for how to generate one without adding a new dependency to
  `rusty_multimodal_db` itself — a plain `openssl`/`mkcert` invocation in
  the test setup, or a small dev-only helper, are both cheaper options
  than taking `rcgen` on directly for one crate's own test suite.
- A transcript-level check confirming plaintext record data is genuinely
  absent on the wire once TLS is configured — the direct evidence this
  design's own purpose depends on, not just "we used `rusty_tls` so it
  must be fine."

## Traceability

Would implement: the transport-encryption half of the gap `SERVER-001`/
ADR-0010 have named since v0.1.0, explicitly left open by ADR-0012's own
"Decision" section — once accepted. No spec registered yet; per
`SERVER-AUTH`/`SERVER-TRANSACTION`'s own precedent, a real implementation
would extend `SERVER-001` with new FRs as its own follow-up unit, only
after this design is explicitly accepted.

## Open questions

- Exact call-site shape wrapping `rusty_tls::TlsServerStream`
  (`enum Connection` vs. a generic `handle_connection<S: Read + Write>`)
  is an implementation-time decision, not fixed here — see "Considered
  options" above. The TLS-stream type itself is no longer an open
  question (see "Ecosystem check" above).
- Where the PEM→DER decode step for `TLS-FR-006` lives: a small private
  helper inside `rusty_multimodal_db` itself, or contributed upstream to
  `rusty_tls` as a `TlsAcceptor::from_pem_files`-style convenience
  constructor so every consumer benefits. Leaning toward the latter as
  the better long-term home (matching the "no consumer rolls its own
  TLS" seam this whole proposal is built on — a hand-rolled PEM parser
  duplicated in this crate would be a small instance of exactly what
  `rusty_tls` exists to prevent), but not decided here — an
  implementation-time call, possibly the implementer's own upstream PR.
- The exact git-dependency coordinates for `rusty_tls` (a `Rusty-Mill/rusty_mill`
  monorepo path vs. a still-current standalone `baileyrd/rusty_tls`
  mirror, and which commit to pin) — verify against whichever is current
  at implementation time; see "Ecosystem check" above.
- Whether to ship a convenience for generating a local self-signed
  certificate (e.g. a `dog_server` startup flag), or leave certificate
  generation entirely to the operator's own tooling (`openssl`, `mkcert`)
  — an implementation-time UX call, not a design decision.
- Whether mTLS is worth designing as a real follow-up now that this
  crate would own TLS natively — a real, separate future decision, not
  decided here. Matches `SERVER-AUTH-DESIGN.md`'s own open question on
  this exact point, though the mechanism (`rusty_tls::TlsAcceptor::new_with_client_auth`)
  already exists, unlike when that open question was first written —
  see "Ecosystem check" above.
- Certificate rotation without a server restart (e.g. a SIGHUP-triggered
  reload) is unaddressed — the "restart with new config" story is the
  same one `AuthConfig`'s own token-rotation non-goal already accepted.

## Change history

- 2026-09-01: Initial proposal, in response to the owner selecting
  transport encryption as one of two next directions (alongside a
  transaction throughput/latency benchmark, done — see `SERVER-001`
  v0.8.0). Originally proposed depending on `rustls` directly.
- 2026-09-01: Revised after the owner asked whether this owner's own
  `rusty_*`/`Rusty-Mill` ecosystem already had a hand-rolled or wrapped
  solution for the dependency being proposed. It does —
  `Rusty-Mill/rusty_mill`'s `crates/rusty_tls` — and this document's
  "In scope"/"Considered options"/`TLS-FR-005`/`TLS-FR-006`/"Proposed
  shape"/"Data and state"/"Verification plan"/"Open questions" sections
  are revised to depend on it instead of `rustls` directly. See
  "Ecosystem check" above for the full finding. Still Proposed, not yet
  accepted — this revision is part of the same review pass, not a
  separate proposal.
- 2026-09-01: Accepted as revised, no further changes requested. ADR-0014
  and this document are now Accepted. Implementation follows as its own
  unit — see ADR-0014's own "Acceptance and implementation" section once
  it lands.
