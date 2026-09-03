# Server mTLS Class-from-Certificate Design (Proposed)

- Status: **Proposed** (not yet accepted; no implementation authorized).
  One decision, `ADR-0028`: an admitted client certificate may *decide
  the connection's class* — by exact match of its DER bytes against
  certificates the operator configured per `TokenClass` — after an
  eager server-side handshake, on top of an upstream `rusty_tls`
  accessor whose patch is spelled out and verified below. Acceptance
  authorizes the design; implementation follows as its own unit, and
  begins upstream — see `ADR-0028`'s "Acceptance and implementation"
  section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-MTLS-DESIGN.md` / `ADR-0023` (mutual TLS
  as an admission gate, `SERVER-001` v0.13.0 / FR-023; its "Class from
  certificate" open question and first revisit trigger — *first an
  upstream `rusty_tls` peer-certificate accessor, then an X.509 reading
  decision in this crate* — are what this document answers),
  `docs/design/SERVER-AUTH-DESIGN.md` / `ADR-0012` (`AuthConfig`,
  `TokenClass`, the `Authenticate` gate this composes with),
  `docs/design/SERVER-TLS-DESIGN.md` / `ADR-0014` (`TlsConfig`, the
  lazy handshake this makes eager), `Cargo.toml` (the `rusty_tls` pin,
  `9fd2e27`), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0.

## Purpose and scope

`SERVER-001` v0.13.0 (FR-023, `ADR-0023`) made a client certificate an
*admission* gate: with `TlsConfig::new_with_client_auth`, a connection
must present a certificate chaining to the operator's CA or the
handshake fails; an admitted connection then starts exactly where
`AuthConfig` says — unauthenticated if tokens are configured,
`ReadWrite` if not — and nothing reads the certificate. The design
chose that on purpose: the pinned `rusty_tls` exposed no peer
certificate on the server side, and "which root signed it" is not
available either. It named the next step as its open question — class
from the certificate — and split it into two decisions: an upstream
accessor, then how this crate reads an identity "without adopting an
X.509 parser it does not otherwise need."

The owner picked that step as the third of four directions. This
document decides both halves:

**In scope:**

- The upstream prerequisite, exactly: `rusty_tls::TlsServerStream::peer_certificate_der()`,
  a mirror of the accessor the client stream already has. The patch is
  given verbatim and was run against this crate (see "Evidence").
- An eager server-side handshake in `handle_connection`, so the
  certificate is known before the first frame is read.
- How this crate maps a certificate to a class: **exact match of the
  presented end-entity certificate's DER bytes** against certificates
  the operator configured per class — no X.509 parsing, no new
  dependency. Configuration on `AuthConfig` (class is its vocabulary),
  from DER, from PEM files, and from two environment variables in
  `dog_server`.
- Precedence between a certificate-derived class and a token.

**Out of scope (see "Non-goals")**: reading a subject, SAN, or public
key out of the certificate; class from the issuing CA; revocation; a
typed handshake error on the client.

## Non-goals

- **X.509 parsing.** A subject/SAN/SPKI match would need a parser this
  crate does not have and `AGENTS.md` forbids adding without cause;
  `rusty_tls`'s own hand-rolled X.509 module is behind a permanently
  non-default feature and not a public API. Exact-DER pinning needs
  none of it. `ADR-0028` offers SPKI pinning as option (b) so the
  trade is visible.
- **Class from the issuing CA** (one CA per class). `rustls`'s verifier
  reports admission, not which root matched; finding it means walking
  the chain — parsing again.
- **Revocation without a restart** — `ADR-0023`'s second trigger,
  untouched.
- **A typed handshake error on the client** — `ADR-0023`'s third
  trigger. The probe below shows an eager *client* handshake does not
  deliver it (TLS 1.3 sends the server's rejection after the client's
  `Finished`), so the trigger stays armed; this design does give the
  *server* a typed reason, which the audit-log round can use.
- **Any wire change.** No `Request`/`Response`/`ErrorCode` variant, no
  `PROTOCOL_VERSION` change, no change to `TokenClass`.

## Context and terminology

- **Admission**: the CA check `rustls` performs during the handshake
  when the acceptor was built `with_client_auth`. Unchanged here; still
  the gate.
- **Class**: `TokenClass::{ReadOnly, ReadWrite}`, the per-connection
  authorization state `handle_connection` keeps as
  `authenticated: Option<TokenClass>` and `Authenticate` sets.
- **Leaf**: the end-entity certificate the client presented, as DER
  bytes — the first element of what `rustls` calls `peer_certificates()`.
- **Certificate class map**: the operator's list of `(leaf DER, class)`
  pairs; a presented leaf whose bytes equal a configured one gets that
  class.
- **Eager handshake**: `TlsServerStream::complete_handshake()`, which
  drives the handshake to completion before any application byte;
  today the server never calls it and the handshake runs lazily under
  the first `read`.

### What the current code does, read from `main` `8edaf7e`

`handle_connection` wraps the accepted `TcpStream` with
`tls.acceptor.accept(stream)` (a configuration step — no I/O) and
immediately starts the frame loop; the handshake happens inside the
first `framing::read_message`, and a rejected client shows up as that
read's `Err`, ending the connection. `authenticated` is initialized
from `auth.is_configured()` alone. `AuthConfig` holds two optional
tokens, compares in constant time via `subtle`, and reads
`SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`. The
pinned `rusty_tls` (`9fd2e27`) has `TlsStream::peer_certificate_der`
(client side) and `complete_handshake` on both streams, but no
server-side certificate accessor.

### Evidence: a throwaway probe against a patched `rusty_tls`

The upstream repository could not be attached from this session (the
attach was refused by the session's permission policy), so the
accessor was verified locally instead: the pinned checkout was copied,
the patch below applied, this crate pointed at the copy with a
temporary `[patch]` section, and a throwaway test run under
`--features server` — all discarded before commit, nothing in the
tree changed.

```rust
// crates/rusty_tls/src/server.rs — inside `impl<S: Read + Write> TlsServerStream<S>`
/// The DER-encoded end-entity certificate the client presented during
/// the handshake, if it has completed (see
/// [`TlsServerStream::complete_handshake`]) and the client sent one —
/// an acceptor built with [`TlsAcceptor::new_with_client_auth`]
/// requires one, so `None` past the handshake only happens on an
/// acceptor that did not ask. Raw bytes, not a parsed certificate,
/// for the same reason as [`TlsStream::peer_certificate_der`](crate::TlsStream::peer_certificate_der).
pub fn peer_certificate_der(&self) -> Option<&[u8]> {
    self.conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.as_ref())
}
```

Two probe tests, both passing:

1. With a throwaway `rcgen` CA and a `ClientAuth` leaf it signed, an
   acceptor built `new_with_client_auth` returns `None` from
   `peer_certificate_der()` before `complete_handshake()` and, after
   it, **exactly the client's leaf DER** — byte-equal to what the
   client was given.
2. With a client that presents no certificate, the server's
   `complete_handshake()` fails with a typed
   `Error::Io(InvalidData, NoCertificatesPresented)` — and the
   client's own `complete_handshake()` returns `Ok`, because the
   rejection arrives after the client's `Finished`. The client learns
   of it on its first read, as today.

## Requirements

- `CLS-FR-001` — **Upstream accessor.** `rusty_tls::TlsServerStream::peer_certificate_der(&self) -> Option<&[u8]>`
  as above, landed upstream and the `rusty_tls` pin in `Cargo.toml`
  moved to the commit that carries it — a rev bump on an existing
  dependency, not a new one. Implementation begins with that PR and
  does not proceed until it is merged.
- `CLS-FR-002` — **Eager handshake.** On a TLS connection,
  `handle_connection` calls `complete_handshake()` right after
  `accept` and before the frame loop. A failure ends the connection
  exactly as a lazy failure does today (`TLS-FR-003`: no `Response`,
  no panic), so no client-visible behavior changes; the difference is
  that the server holds a typed `rusty_tls::Error` for the reason.
- `CLS-FR-003` — **The map.** `AuthConfig::with_certificate_class(leaf_der: Vec<u8>, class: TokenClass) -> Self`
  (builder, repeatable) and `AuthConfig::with_certificate_class_pem_file(path, class) -> Result<Self, TlsConfigError>`
  (every `CERTIFICATE` block in the file, via the existing `pem`
  module). A presented leaf is classed by **byte equality** with a
  configured one. `AuthConfig::is_configured()` is true when any token
  *or* any certificate class is set — so a certificates-only
  deployment starts unauthenticated, as `AUTH-FR-007` requires of a
  configured server.
- `CLS-FR-004` — **Precedence.** After admission, if the leaf matches
  a configured certificate, the connection *starts* at that class
  (`authenticated = Some(class)`), with no `Authenticate` needed. A
  later `Authenticate` with a valid token replaces it (the client
  asked for the token's class); an invalid token is `Unauthenticated`
  and leaves the class as it was — `Authenticate`'s existing semantics,
  unchanged. An admitted leaf with no configured class leaves the
  connection where `AuthConfig` puts every connection today.
- `CLS-FR-005` — **Environment.** `dog_server` reads
  `SERVER_AUTH_READ_ONLY_CLIENT_CERTS` and `SERVER_AUTH_READ_WRITE_CLIENT_CERTS`,
  each a `:`-separated list of PEM files; an unreadable or non-PEM
  file is a startup error naming the variable. Either variable set
  while `SERVER_TLS_CLIENT_CA_PATH` is not is a startup error too — a
  class map on a server that never asks for a certificate is inert,
  and inert security configuration is a mistake to refuse, not honor
  (the `MTLS-FR-004` posture). The library itself does not refuse it
  (`AuthConfig` cannot see `TlsConfig`); it documents that the map
  matches nothing without client auth.
- `CLS-FR-006` — **Secrecy.** Configured certificates are public
  material, but `AuthConfig`'s `Debug` prints only how many are
  configured per class, never bytes; nothing logs a presented leaf.
- `CLS-FR-007` — **Nothing else changes.** No wire, `PROTOCOL_VERSION`,
  `TokenClass`, `TlsConfig`, `ConnectionStore`, or store change; the
  one `Cargo.toml` change is the rev bump. Every existing auth, TLS,
  and mTLS test passes unchanged — plain TLS and token-only servers
  never see a certificate class and take no new branch beyond one
  `Option` check per connection.
- `CLS-FR-008` — `SERVER-001`'s next minor / FR at implementation;
  `ADR-0023`'s first trigger and `SERVER-MTLS-DESIGN.md`'s open
  question resolved by pointer; its third trigger (typed client error)
  restated with the probe's finding; `SERVER-AUTH-DESIGN.md`'s
  layering answer extended ("the certificate may also decide class,
  when the operator says which").

## Considered options

**What identifies a certificate.**

1. **Exact DER bytes of the leaf (proposed).** Zero parsing, zero
   dependencies, and the strongest possible match — the operator pins
   the certificates themselves. Cost: re-issuing a client certificate
   (rotation, expiry) changes its bytes, so the map must be updated —
   which is what the PEM-file list is for.
2. **SubjectPublicKeyInfo pinning.** Survives re-issuance under the
   same key. Needs a DER walk to the SPKI — a small hand-rolled parser
   or a dependency; the "X.509 reading decision" `ADR-0023` deferred.
   Offered as option (b); not proposed, because it buys rotation
   convenience at the price of the first X.509 code in this crate.
3. **Subject / SAN patterns.** Needs a real parser and a pattern
   language; rejected.
4. **Class from the issuing CA.** Not reported by the verifier;
   rejected.

**Where the map lives.**

1. **`AuthConfig` (proposed).** Class is `AuthConfig`'s vocabulary;
   `handle_connection` already consults it for the starting state and
   for `Authenticate`. `TlsConfig` stays admission-only, as `ADR-0023`
   left it.
2. **`TlsConfig`.** Rejected: it would have to know `TokenClass`, and
   two objects would decide authorization.

**Handshake timing.**

1. **Eager, before the frame loop (proposed).** The class must be
   known before the first frame's gate; also the only way the server
   ever holds a typed reason for a rejected handshake.
2. **Lazy, read the certificate after the first frame.** Rejected: the
   first frame has already been gated by then, and a
   certificate-classed connection's first request would be refused.

**Precedence.**

1. **Certificate class as starting state, a valid token replaces it
   (proposed).** Composes with every existing posture; a client that
   presents both gets what a token-presenting client gets today.
2. **Certificate class immutable; `Authenticate` refused.** Rejected:
   takes away a behavior for no gain, and makes a mixed fleet awkward.
3. **Both required (certificate class AND token).** That is today's
   layering already — admission and token — with no class from the
   certificate; it is the status quo, not an option.

## Proposed shape

```rust
// src/server/mod.rs
pub struct AuthConfig {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
    certificate_classes: Vec<(Vec<u8>, TokenClass)>,   // leaf DER → class
}
impl AuthConfig {
    pub fn with_certificate_class(mut self, leaf_der: Vec<u8>, class: TokenClass) -> Self;
    pub fn with_certificate_class_pem_file(self, path: &Path, class: TokenClass) -> Result<Self, TlsConfigError>;
    pub fn is_configured(&self) -> bool;                 // tokens or certificates
    pub(crate) fn class_for_certificate(&self, leaf_der: &[u8]) -> Option<TokenClass>; // byte equality
}
// handle_connection, TLS arm:
let mut tls_stream = tls.acceptor.accept(stream)?;
if tls_stream.complete_handshake().is_err() { return; }        // CLS-FR-002
let certificate_class = tls_stream.peer_certificate_der().and_then(|der| auth.class_for_certificate(der));
// ... then:
let mut authenticated = match (certificate_class, auth.is_configured()) {
    (Some(class), _) => Some(class),                               // CLS-FR-004
    (None, false) => Some(TokenClass::ReadWrite),
    (None, true) => None,
};

// src/bin/dog_server.rs: SERVER_AUTH_READ_ONLY_CLIENT_CERTS / SERVER_AUTH_READ_WRITE_CLIENT_CERTS
```

`Cargo.toml`: the `rusty_tls` `rev` moves to the upstream commit; the
justification comment gains one sentence naming the accessor.

## Data/state and invariants

- Admission is decided by `rustls` during the handshake and is never
  weakened: a leaf that is not in the map is admitted exactly as today
  and classed exactly as today.
- The map is read-only after construction; matching is a linear scan
  over a handful of certificates, once per connection, by `==` on
  byte slices (no constant-time requirement: certificates are public).
- `authenticated` remains the only authorization state; the
  certificate only sets its initial value.
- `TlsConfig` still carries no policy beyond "require a certificate
  chaining to these roots."

## Errors, failure, recovery, and observability

- Handshake failure: connection ends, no `Response` (unchanged). The
  server now holds the `rusty_tls::Error`; this design does nothing
  with it beyond ending the connection — the audit-log round decides
  whether to record it.
- PEM file errors at construction: `TlsConfigError::{Io, Pem}`, the
  existing shapes; `dog_server` refuses to start and names the
  variable.
- A map with no client auth: inert in the library (documented),
  refused at startup by `dog_server`.
- Not observable except by what a connection is allowed to do.

## Security, privacy, and compatibility

- Strictly additive: a server with no certificate classes behaves
  byte-for-byte as v0.16.0 on every path; the FR-016/FR-019/FR-023
  suites are the regression test.
- A certificates-only deployment (no tokens, a class per certificate)
  becomes possible without the `AUTH-FR-007` fallback to `ReadWrite`
  for everyone: `is_configured()` sees the classes, so an admitted
  certificate *not* in the map starts unauthenticated. Named, because
  it is the one behavior change an existing certificates-only operator
  who adds a class would see — and it is the safe direction.
- The operator's client CA key remains the highest-value secret; the
  class map adds no secret.
- Exact-DER pinning means a re-issued certificate loses its class
  until the map is updated — a failure closed, not open.

## Acceptance criteria

1. Upstream: the accessor merged in `rusty_tls` with the patch above
   (its own tests there); `Cargo.toml` pinned to that commit; `cargo
   tree` shows no new dependency.
2. Eager handshake: a client without a certificate, and one from a
   foreign CA, are rejected before any frame is read — the existing
   mTLS rejection test passes unchanged — and the server never panics.
3. A leaf configured `ReadOnly` reads and is refused writes with
   `Unauthorized` without any `Authenticate`; one configured
   `ReadWrite` writes; an admitted leaf not in the map behaves as
   today (unauthenticated with tokens, `ReadWrite` without).
4. Precedence: a `ReadOnly`-classed connection that `Authenticate`s
   with the `ReadWrite` token writes afterwards; with a wrong token it
   is `Unauthenticated` and still reads.
5. A certificates-only server (classes, no tokens): the classed leaf
   is served at its class; an unclassed admitted leaf is
   `Unauthenticated` on every request.
6. PEM path: a two-block file classes both leaves; `Io`/`Pem` errors
   surface; `dog_server` refuses classes without a client CA.
7. `Debug` shows counts only. No wire, `PROTOCOL_VERSION`, `TokenClass`,
   `TlsConfig`, or `ConnectionStore` change; every existing test,
   bench, and binary unchanged.

## Verification plan

- `tests/server_tls_integration.rs`: four to five tests appended on
  the existing throwaway-CA helpers (criteria 2–6), including the
  two-connection precedence case.
- `src/server/mod.rs`: unit tests for `class_for_certificate`,
  `is_configured` with classes only, and the `Debug` shape.
- `src/bin/dog_server.rs`: the startup refusal, factored like
  `TlsConfig::from_env_values` so a unit test drives it.

## Traceability

- → `SERVER-001` next minor / FR (`CLS-FR-001`–`008`), `ADR-0028`;
  resolves `ADR-0023`'s first trigger and `SERVER-MTLS-DESIGN.md`'s
  "Class from certificate" open question; extends
  `SERVER-AUTH-DESIGN.md`'s layering answer.
- Roadmap: `SERVER-MTLS-CLASS-DESIGN` (this document), then
  `SERVER-MTLS-CLASS` as the implementation unit if accepted — its
  first step the upstream PR.

## Open questions

- **Who lands the upstream patch.** This session could not attach the
  `rusty_mill` repository; the patch is verbatim above and verified.
  The implementation unit needs either that permission or the owner
  applying it.
- Whether `is_configured()` growing to include certificate classes
  should be its own line in `SERVER-AUTH-DESIGN.md` (`AUTH-FR-007`) —
  proposed yes, at implementation.
- Whether the eager handshake should also apply when no client auth
  is configured. Proposed yes — one code path, and the typed reason is
  useful either way; the cost is nil (the handshake happens on the
  first read otherwise).

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting
  class-from-certificate as the third of four next directions
  ("1, 2, 3, 4"). The upstream accessor verified by a local patch probe
  (discarded) because the upstream repository could not be attached
  from this session. (PR #149.)
