# Server Transport Encryption Design (Proposed)

- Status: **Proposed** — awaiting owner review. Does not authorize any
  implementation; see ADR-0014's own "Decision" section.
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

- Native TLS termination inside this crate's own server process, via the
  `rustls` crate — not an external TLS-terminating proxy/tunnel.
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
  `rustls`'s own default, vetted policy rather than authoring one.
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

## Context and terminology

- **TLS termination point**: inside the server process itself, before
  `framing.rs`'s `read_message`/`write_message` ever see the bytes — as
  opposed to ADR-0012's chosen design (an external proxy terminates TLS
  in a separate process; this crate never sees encrypted bytes at all,
  and can't verify the proxy is even in place).
- **`rustls`'s synchronous API**: `rustls::ServerConnection` plus its
  `rustls::Stream`/`StreamOwned` adapters implement `std::io::Read`/
  `Write` over an inner `TcpStream`, so a TLS-wrapped connection can be
  used exactly like a plain one by any code already written against
  `Read`/`Write` — no async runtime required, compatible with the
  existing thread-per-connection blocking-socket model unchanged.
- **The genuine architectural finding this design depends on**:
  `src/server/framing.rs`'s `write_message<W: Write, T: Serialize>`/
  `read_message<R: Read, T: DeserializeOwned>` are already generic over
  `Read`/`Write`, not hardcoded to `TcpStream` — verified by reading the
  current source, not assumed. This means the framing layer needs no
  change at all; only the concrete stream type `handle_connection`/
  `send_response` hold (currently `TcpStream` directly) needs to become
  "either a plain socket or a TLS-wrapped one."

## Requirements

- `TLS-FR-001`: `serve` accepts an optional `TlsConfig` alongside the
  existing `AuthConfig`. Passing none (or a default) reproduces today's
  plaintext behavior exactly — zero change required for
  `AuthConfig::default()`-style existing callers (`dog_server`, every
  test, every benchmark) unless they opt in.
- `TLS-FR-002`: When configured, every accepted connection performs a TLS
  server handshake (via `rustls::ServerConnection`) before any framed
  message is read. `dispatch`/`ConnectionStore` remain completely
  unaware transport encryption exists — the same "no new coordination
  beyond what's needed" driver ADR-0010 used, extended one layer down.
- `TLS-FR-003`: A connection that fails the TLS handshake (unsupported
  client, corrupt handshake bytes) is dropped cleanly — no panic,
  extending this crate's existing "never panic on malformed/hostile
  input" discipline one layer below the existing frame-level guarantees.
- `TLS-FR-004`: `framing.rs` requires zero changes — already generic over
  `Read`/`Write` (see "Context and terminology" above).
- `TLS-FR-005`: `handle_connection`/`send_response` need a stream
  abstraction — exact shape is an implementation-time decision (see
  "Architecture and interfaces" below), not fixed by this design.
- `TLS-FR-006`: Certificate and private key are loaded once at server
  startup from operator-supplied PEM file paths, never hardcoded, never
  committed to the repository — matching `AUTH-FR-005`'s own "read from
  config at startup, not embedded" precedent, applied to key material.
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
2. *Native TLS via `rustls`, terminated inside this crate's own server
   process.* **Chosen.** ADR-0012 rejected this option for authentication
   specifically because it judged a full native-TLS stack disproportionate
   dependency weight — but that judgment implicitly bundled TLS with an
   async-runtime shift (the conventional way most Rust servers add TLS,
   via `tokio-rustls`). `rustls` itself ships a synchronous,
   `Read`/`Write`-compatible API that composes with this crate's existing
   thread-per-connection blocking-socket model with **zero** changes to
   `framing.rs` — undercutting the original "disproportionate for what
   this needs" framing once the async-runtime assumption is separated
   out. `rustls` is also the de facto standard pure-Rust TLS
   implementation (no OpenSSL/system-library binding to manage per
   platform, avoiding the same "platform-dependent behavior" objection
   ADR-0012 raised against `native-tls`), and closes a gap an external
   proxy structurally cannot: this crate has no way to verify from inside
   its own process that a proxy is actually in front of it.
3. *`native-tls` (system TLS bindings — Schannel/SecureTransport/
   OpenSSL, chosen per platform).* Rejected — same platform-dependent
   reasoning ADR-0012 already gave; unchanged by this proposal.
4. *mTLS (client-certificate authentication) bundled into this same
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
2. *A small enum, e.g. `enum Connection { Plain(TcpStream),
   Tls(rustls::StreamOwned<rustls::ServerConnection, TcpStream>) }`,
   implementing `Read`/`Write` by delegating to whichever variant is
   active.* **Tentatively preferred** — keeps `handle_connection`'s own
   signature concrete (one type, not a generic parameter), the same
   shape `AuthConfig`'s `Option<TokenClass>` already uses (per-connection
   enum/state, not a type-level parameter). Not fixed by this design —
   an implementation-time decision, matching how `SERVER-AUTH-DESIGN.md`
   left `AuthConfig::new`/`AuthConfig::from_env`'s exact split open.

### Proposed shape

```rust
// src/server/mod.rs — additive, not a rewrite of the existing types

struct TlsConfig {
    // Loaded once at server startup from operator-supplied PEM file
    // paths — exact field shape (raw paths vs. a pre-built
    // rustls::ServerConfig) is an implementation-time decision.
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

// A per-connection stream abstraction (sketch — see "Considered
// options" above for the alternative of a generic handle_connection):
// enum Connection {
//     Plain(TcpStream),
//     Tls(rustls::StreamOwned<rustls::ServerConnection, TcpStream>),
// }
// impl Read for Connection { /* delegates to whichever variant */ }
// impl Write for Connection { /* delegates to whichever variant */ }

// handle_connection's own signature changes from `TcpStream` to
// `Connection`; framing.rs's read_message/write_message are called
// exactly as they are today — no change there at all, since both are
// already generic over Read/Write.
```

The TLS handshake itself (`rustls::ServerConnection::new` plus driving
it to completion over the raw `TcpStream`, wrapped as `StreamOwned` once
the handshake finishes) happens once, per accepted connection, before
`handle_connection`'s own request loop starts — the same place
`AuthConfig`'s per-connection state begins its life today.

## Data/state and invariants

- TLS handshake/session state lives inside `rustls::ServerConnection`,
  held for the connection's lifetime — the same per-connection lifetime
  as today's read/write buffers and (since v0.6.0) `Option<TokenClass>`.
  Never shared across connections, never touching the wrapped store.
- The loaded `rustls::ServerConfig` (parsed certificate chain + key) is
  built once at server startup and shared read-only across every
  connection thread via `Arc`, the same pattern `AuthConfig` already
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
  beyond `rustls`'s own safe defaults. This proposal takes `rustls`'s
  default, vetted policy rather than authoring a custom one.

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

- Unit tests: certificate/key loading (valid, malformed, missing file),
  `rustls::ServerConfig` construction.
- Real end-to-end tests: a real client/server pair over an actual TLS
  handshake, using a self-signed certificate generated at test time (no
  committed certificate or key material in the repository — applying
  `AUTH-FR-005`'s own "never committed" bar to key material this time).
- A transcript-level check confirming plaintext record data is genuinely
  absent on the wire once TLS is configured — the direct evidence this
  design's own purpose depends on, not just "we used `rustls` so it must
  be fine."

## Traceability

Would implement: the transport-encryption half of the gap `SERVER-001`/
ADR-0010 have named since v0.1.0, explicitly left open by ADR-0012's own
"Decision" section — once accepted. No spec registered yet; per
`SERVER-AUTH`/`SERVER-TRANSACTION`'s own precedent, a real implementation
would extend `SERVER-001` with new FRs as its own follow-up unit, only
after this design is explicitly accepted.

## Open questions

- Exact stream-abstraction shape (`enum Connection` vs. a generic
  `handle_connection<S: Read + Write>`) is an implementation-time
  decision, not fixed here — see "Considered options" above.
- Whether to ship a convenience for generating a local self-signed
  certificate (e.g. a `dog_server` startup flag), or leave certificate
  generation entirely to the operator's own tooling (`openssl`, `mkcert`)
  — an implementation-time UX call, not a design decision.
- Whether mTLS is worth designing as a real follow-up now that this
  crate would own TLS natively — a real, separate future decision, not
  decided here. Matches `SERVER-AUTH-DESIGN.md`'s own open question on
  this exact point.
- Certificate rotation without a server restart (e.g. a SIGHUP-triggered
  reload) is unaddressed — the "restart with new config" story is the
  same one `AuthConfig`'s own token-rotation non-goal already accepted.

## Change history

- 2026-09-01: Initial proposal, in response to the owner selecting
  transport encryption as one of two next directions (alongside a
  transaction throughput/latency benchmark, done — see `SERVER-001`
  v0.8.0).
