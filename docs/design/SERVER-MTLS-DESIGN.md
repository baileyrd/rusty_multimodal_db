# Server Mutual TLS Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, option (a): admission gate layered
  under `AuthConfig`; (b) hold for class-from-certificate and (c) close
  as not warranted declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit — see
  ADR-0023's own "Acceptance and implementation" section.
- Date: 2026-09-02
- Related: `docs/design/SERVER-TLS-DESIGN.md` / `docs/decisions/ADR-0014-server-transport-encryption-proposal.md`
  (native TLS, Accepted and implemented as `SERVER-001` v0.9.0 / FR-019;
  named mTLS as out of scope and as its own first revisit trigger),
  `docs/design/SERVER-AUTH-DESIGN.md` / `docs/decisions/ADR-0012-server-authentication-proposal.md`
  (the token scheme; left open "whether a future mTLS design would
  replace this token scheme entirely or layer alongside it" — this
  document answers that question), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.12.0 (FR-019 server-side TLS, FR-022 `SchemaDrivenClient` TLS — the
  two ends this design extends), `rusty_tls` (`Rusty-Mill/rusty_mill`,
  `crates/rusty_tls`, pinned at `9fd2e27`), whose
  `TlsAcceptor::new_with_client_auth` and
  `TlsStream::new_with_client_identity` are the mechanism this design
  adopts rather than builds.

## Purpose and scope

ADR-0014 closed the transport-encryption half of ADR-0010's original
"no auth, no encryption" gap and named, as its first revisit trigger,
"mTLS becomes a real requirement now that this crate owns TLS natively —
a real, separate future design, not decided here." It also recorded the
one thing TLS alone does not fix: "a sufficiently determined attacker who
obtains a valid token can still authenticate, same as today; this
proposal only removes the ability to *observe* that token on the wire."
The owner selected this revisit as the next design round. This document
is that design.

The decision this document actually has to make is not *whether* the
server can verify a client certificate — `rusty_tls` already does that,
in one constructor — but **what a verified client certificate means to
this server**. `SERVER-AUTH-DESIGN.md` left exactly that open: does a
certificate replace `AuthConfig`'s tokens, or layer alongside them?

**In scope for this proposal:**

- An opt-in **client-certificate requirement** on `TlsConfig`: when
  configured with a set of client CA roots, every connection must
  present a certificate chaining to one of them or the TLS handshake
  fails — before any framed byte, `Authenticate` included, is ever read.
  Mechanism: `rusty_tls::TlsAcceptor::new_with_client_auth`.
- The **answer to the layering question**: the certificate decides
  *admission* (who may hold a connection at all); `AuthConfig`'s token
  decides *class* (`ReadOnly`/`ReadWrite`) exactly as today. The two
  compose unchanged — see "Architecture and interfaces" for why the
  alternatives lose.
- The **client half** in `SchemaDrivenClient`: `ClientTlsConfig` gains
  an optional client identity (certificate chain + private key), so the
  crate's own client can reach an mTLS server. Mechanism:
  `rusty_tls::TlsStream::new_with_client_identity`.
- The same **operator plumbing** `TlsConfig` already has: PEM files,
  one more environment variable, `from_env` treating a partial
  configuration as an error rather than a silent downgrade.

**Explicitly out of scope, named directly rather than left implicit:**

- Deriving a `TokenClass` (or any identity) *from* the client
  certificate — its subject, a SAN, or which root signed it. Two hard
  reasons, not a preference: `rusty_tls::TlsServerStream` exposes no
  peer-certificate accessor (its `rustls::ServerConnection` is private),
  and reading a subject or SAN out of DER means an X.509 parser — a new
  dependency or a hand-rolled ASN.1 decoder far beyond `src/server/pem.rs`'s
  scale. Both are real, bounded future steps (an upstream `rusty_tls`
  accessor; a parser decision); neither is this proposal. See "Open
  questions".
- Client-certificate revocation. `rusty_tls` exposes revocation only on
  the *client's* trust policy (`TrustPolicy::PinnedAnchorsWithRevocation`),
  not on the server's client verifier. A compromised client key is
  handled the way a leaked token is handled today: restart the server
  with a new client CA root (and re-issue every client certificate under
  it). Named, not solved.
- Certificate issuance and distribution automation. The operator runs
  their own CA (`openssl`, `step-ca`, `mkcert`) and hands each client its
  certificate and key out of band — the same "operator's own tooling"
  story `SERVER-TLS-DESIGN.md` accepted for the server certificate.
- Rotation without a restart — unchanged from `TlsConfig`'s and
  `AuthConfig`'s own accepted non-goal.
- An "optional client certificate" mode (request one, admit without).
  `rustls` can express it; `rusty_tls` does not expose it; and an
  admission check that admits without checking is not an admission
  check.

## Non-goals

- Not a replacement for `AuthConfig`. A server may run tokens alone (as
  today), client certificates alone (every admitted connection is
  `ReadWrite`, exactly `AUTH-FR-007`'s no-tokens posture behind a
  certificate gate), or both.
- Not a wire, framing, or protocol-version change. `PROTOCOL_VERSION`
  stays 2; `framing.rs` and the codec are untouched; every golden vector
  stays true. The entire feature lives below the first frame.
- Not a change to `handle_connection`'s logic. The acceptor carries the
  policy; `accept` and the lazy handshake already fail a rejected client
  on the first `read_message` (`TLS-FR-003`). This is a real finding of
  this design, verified by reading `src/server/mod.rs`: the server-side
  change is one constructor on `TlsConfig`, nothing in the connection
  loop.
- Not a new dependency. `rcgen` (already a dev-dependency for
  `tests/server_tls_integration.rs`) can issue a CA and a leaf signed by
  it (`CertificateParams::signed_by`, `IsCa::Ca`,
  `ExtendedKeyUsagePurpose::ClientAuth`), so the test suite stays
  hermetic with what is already in `Cargo.toml`.

## Context and terminology

- **Admission vs. class.** Today a connection passes two gates:
  the TLS handshake (when `TlsConfig` is set) decides whether framed
  traffic happens at all, and the `Authenticate` intercept decides what
  class the connection holds. This design adds a condition to the first
  gate and leaves the second alone. "Admission" below always means the
  first gate.
- **Client CA roots.** DER-encoded CA certificates; a presented client
  certificate must chain to one of them. This is the operator's *own*
  CA, never the OS trust store — a public CA's client certificates say
  nothing about who this operator wants admitted.
- **Client identity (client side).** A leaf certificate chain plus its
  private key, presented during the handshake. `ClientTlsConfig` carries
  it; `SchemaDrivenClient` never inspects it.
- **`rusty_tls`'s mTLS surface, verified from the pinned source.**
  Server: `TlsAcceptor::new_with_client_auth(cert_chain_der,
  private_key_der, client_ca_roots_der)` builds a `WebPkiClientVerifier`
  over the roots and fails the handshake for a client that presents no
  certificate or one that does not chain. Client:
  `TlsStream::new_with_client_identity(sock, server_name, policy,
  client_cert_chain_der, client_key_der)`. Both take DER, like their
  non-mTLS siblings, so `src/server/pem.rs` covers the PEM step exactly
  as it does for the server certificate. Neither exposes the peer's
  certificate afterwards (see "out of scope").
- **Lazy handshakes on both ends.** `TlsAcceptor::accept` and
  `TlsStream::new` perform no I/O; the handshake runs under the first
  read or write. Server-side that is `handle_connection`'s first
  `read_message`, which already returns on error (`TLS-FR-003`);
  client-side it is the `Hello`, which already surfaces as
  `ClientError::Frame(FrameError::Io(..))` (FR-022). Every mTLS failure
  therefore lands on a path that already exists and is already tested.

## Requirements

- `MTLS-FR-001`: `TlsConfig` accepts an optional set of client CA roots.
  When present, every connection must complete a handshake presenting a
  client certificate that chains to one of those roots; a connection
  that presents none, or one that does not chain, is dropped before any
  framed message is read — no `Authenticate` token is ever received from
  an unadmitted client. When absent, `TlsConfig` behaves exactly as at
  v0.9.0 (FR-019).
- `MTLS-FR-002`: Admission and class are independent and layered. With
  `AuthConfig` configured, an admitted connection still starts
  unauthenticated and must present a token (`AUTH-FR-001`/`002`); with
  no tokens configured, an admitted connection starts `ReadWrite`
  (`AUTH-FR-007`). The certificate never changes a connection's class,
  and a token never bypasses the certificate requirement.
- `MTLS-FR-003`: `ClientTlsConfig` accepts an optional client identity
  (DER certificate chain, leaf first, plus DER private key);
  `SchemaDrivenClient::connect_with` presents it during the handshake.
  A client with no identity, or an identity the server's roots reject,
  fails under the `Hello` as `ClientError::Frame(FrameError::Io(..))`
  — the FR-022 failure shape, unchanged; an identity `rusty_tls`
  rejects outright (key does not match certificate, bad DER) is
  `ClientError::Tls` before any I/O.
- `MTLS-FR-004`: Operator plumbing mirrors `TLS-FR-006`. Server: a PEM
  file that may hold several `CERTIFICATE` blocks (the root set), a
  constructor taking it beside the existing chain/key paths, and one new
  environment variable read by `TlsConfig::from_env`. A client-CA
  variable set while the chain/key variables are unset is a startup
  error (`Some(Err(..))`), never a silent plaintext or no-mTLS server.
  Client: PEM-file constructors for the identity, decoded by the same
  `pem` module.
- `MTLS-FR-005`: The server never inspects the admitted certificate's
  contents. No subject, SAN, or issuer is read, logged, or exposed to
  `dispatch`/`ConnectionStore`. Identity remains the token's job.
- `MTLS-FR-006`: No wire, protocol-version, framing, codec, or
  `ConnectionStore` change; no new dependency. `PROTOCOL_VERSION` stays
  2. `handle_connection`'s body is unchanged.
- `MTLS-FR-007`: Backward compatible by construction. A `TlsConfig`
  built without client CA roots, and a `ClientTlsConfig` without an
  identity, behave exactly as at v0.12.0; every existing test, bench,
  and binary passes unchanged.
- `MTLS-FR-008`: Rejection never panics and never answers. A rejected
  client sees a failed handshake (a TLS alert then EOF), not a
  `Response::Err`, because the connection never reaches the framing
  layer — extending `TLS-FR-003` to the new rejection cause, on the
  path that already implements it.

## Architecture and interfaces

### Considered options

**What a verified client certificate means.**

1. **Admission only, layered under `AuthConfig` (proposed).** The
   certificate gates the handshake; the token gates the class. Both
   existing mechanisms are unchanged and compose in every combination
   (tokens only, certificates only, both, neither). Uses `rusty_tls`
   exactly as shipped; `handle_connection` does not change. Answers
   `SERVER-AUTH-DESIGN.md`'s open question with *layer*, and closes
   ADR-0014's "a stolen token still authenticates" consequence: a token
   alone no longer reaches the `Authenticate` intercept.
2. **Class derived from the certificate** — one root per class, or a
   subject/SAN convention, and `AuthConfig` becomes optional. The
   elegant end state `SERVER-AUTH-DESIGN.md` called "ties authentication
   to the transport layer directly." Rejected *for this proposal*
   because it cannot be built on the pinned `rusty_tls`: no
   peer-certificate accessor exists on `TlsServerStream`, and "which root
   signed it" is not observable either without one. It would need an
   upstream `rusty_tls` change first, then an X.509 parsing decision in
   this crate. Named as the future revisit, with option 1 as its
   strictly compatible base: a later class-from-certificate design adds
   a rule on top of admission, it does not undo anything here.
3. **Certificate replaces tokens** — an mTLS server ignores
   `AuthConfig`; every admitted connection is `ReadWrite`. Rejected:
   throws away the `ReadOnly`/`ReadWrite` split with nothing to replace
   it, for no gain over option 1 (which already allows "certificates
   only" by simply configuring no tokens).
4. **Optional client certificate** (request, admit without). Rejected —
   see "out of scope": not exposed by `rusty_tls`, and not an admission
   control.

**Where the roots live.**

1. **On `TlsConfig`, as a second constructor (proposed).** mTLS is a
   property of the server's TLS acceptor, and `rusty_tls` models it that
   way (`new` vs. `new_with_client_auth` on the same `TlsAcceptor`).
   `serve`'s signature is unchanged; `handle_connection` is unchanged.
2. **A separate `MtlsConfig` parameter on `serve`.** Rejected: a fifth
   `serve` parameter that is meaningless without the fourth, and a
   second place to get the cert/key/roots trio inconsistent.
3. **On `AuthConfig`.** Rejected: it is not an authentication class and
   `AuthConfig` has no access to the acceptor; it would also imply the
   certificate participates in the class decision, which option 1 above
   deliberately denies.

### Proposed shape

Server (`src/server/mod.rs`; every existing item unchanged):

```rust
impl TlsConfig {
    /// `TlsConfig::new` plus the DER-encoded CA certificates a client
    /// certificate must chain to (`MTLS-FR-001`); at least one.
    pub fn new_with_client_auth(
        cert_chain_der: Vec<Vec<u8>>,
        private_key_der: Vec<u8>,
        client_ca_roots_der: Vec<Vec<u8>>,
    ) -> Result<Self, TlsConfigError>;   // rusty_tls::TlsAcceptor::new_with_client_auth

    /// `from_pem_files` plus a PEM file of one or more CA certificates.
    pub fn from_pem_files_with_client_ca(
        cert_chain_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
        client_ca_path: impl AsRef<Path>,
    ) -> Result<Self, TlsConfigError>;

    /// `SERVER_TLS_CERT_CHAIN_PATH` + `SERVER_TLS_PRIVATE_KEY_PATH`, as
    /// today, plus optional `SERVER_TLS_CLIENT_CA_PATH`. Chain/key unset
    /// and client-CA set → `Some(Err(TlsConfigError::…))`, not `None`.
    pub fn from_env() -> Option<Result<Self, TlsConfigError>>;

    /// Whether this configuration requires a client certificate.
    pub fn requires_client_certificate(&self) -> bool;
}
```

`TlsConfigError` needs no new variant: an empty or unparseable root set
is `Tls(rusty_tls::Error::InvalidClientCaRoots(..))`, a bad file is
`Io`/`Pem`, and the partial-environment case reuses `Io` with a
`NotFound`-kind error naming the missing variable (or a small new
variant if the implementer judges that clearer — an implementation-time
call, not fixed here). `handle_connection` has no mTLS branch: the
acceptor it is handed already carries the policy.

Client (`src/server/client.rs`; every existing item unchanged):

```rust
impl ClientTlsConfig {
    /// Present this DER certificate chain (leaf first) and DER private
    /// key during the handshake (`MTLS-FR-003`).
    pub fn with_identity(self, cert_chain_der: Vec<Vec<u8>>, key_der: Vec<u8>) -> Self;

    /// The same from PEM files, decoded by the `pem` module.
    pub fn with_identity_pem_files(
        self,
        cert_chain_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> Result<Self, ClientTlsConfigError>;   // Io | Pem, mirroring TlsConfigError

    pub fn has_identity(&self) -> bool;
}
```

`connect_with` picks `TlsStream::new` or `TlsStream::new_with_client_identity`
by `has_identity()`; nothing else in the client changes, and the private
`Transport` enum is untouched (`TlsStream<TcpStream>` either way).

`dog_server` (`src/bin/dog_server.rs`): no code change beyond what
`TlsConfig::from_env` already returns — it logs the misconfiguration
error it already handles. Its module doc gains the third variable.

## Data/state and invariants

- No new per-connection state on the server: admission is decided
  inside the handshake and never consulted again. `authenticated:
  Option<TokenClass>` keeps exactly its v0.6.0 meaning.
- Invariant: on an mTLS-configured server, no `Request` — `Hello` and
  `Authenticate` included — is ever decoded from a connection that did
  not present an admitted certificate. Follows from `TLS-FR-002`'s
  ordering (handshake completes under the first `read_message`) plus
  `rustls` refusing to complete a handshake the client verifier rejects.
- Invariant: `TlsConfig` without roots ≡ v0.9.0 `TlsConfig`;
  `ClientTlsConfig` without identity ≡ v0.12.0 `ClientTlsConfig`.
- The CA root set is loaded once at startup and shared across
  connection threads inside the `Arc`-backed acceptor, the same
  lifecycle `TlsConfig` and `AuthConfig` already have.

## Errors, failure, recovery, and observability

- Startup: an unreadable or non-PEM root file, an empty root set, or a
  root `rustls` cannot add all fail `TlsConfig` construction with a
  typed `TlsConfigError` — the server never starts "with mTLS silently
  off." A partial environment (`MTLS-FR-004`) is the same kind of error.
- Per connection, server side: a client without a certificate, with one
  that does not chain, or with an expired one fails the handshake; the
  server's first `read_message` errors and `handle_connection` returns
  (`TLS-FR-003`). No response is written; nothing is logged (the server
  logs nothing per connection today; an audit log stays the open gap
  `SERVER-AUTH-DESIGN.md` already names).
- Per connection, client side: a rejected identity is
  `ClientError::Frame(FrameError::Io(..))` under the `Hello`; an
  identity `rusty_tls` rejects at construction (key/certificate mismatch,
  malformed DER) is `ClientError::Tls` before any I/O. Neither writes a
  frame, so no token leaves the client.
- Recovery from a compromised client key: restart with a new root and
  re-issued client certificates — the only revocation this design offers
  (see "out of scope").

## Security, privacy, and compatibility

- Closes the consequence ADR-0014 recorded: with client certificates
  required, possession of a token is no longer sufficient to
  authenticate — an attacker also needs an admitted certificate's
  private key. This is the strongest client-identity posture this
  server can take without a class-from-certificate design, and it is
  strictly additive to today's.
- The operator's client CA is the trust anchor, never the OS store.
  Compromise of that CA's key admits anyone; it is the highest-value
  secret this design introduces and is the operator's to protect, as
  the server's private key already is.
- The client's `TrustPolicy` (how it verifies the *server*) and the
  server's client verifier (how it verifies the *client*) are
  independent. `TrustPolicy::DangerNoVerification` on the client still
  means what it did at FR-022 and does not weaken the server's check.
- Backward compatible by construction (`MTLS-FR-007`): every existing
  `TlsConfig`/`ClientTlsConfig`/`serve`/`connect_with` call site is
  unchanged. Not a wire change (`MTLS-FR-006`).
- Still open, unchanged: revocation, audit logging, rate limiting.

## Acceptance criteria

1. `TlsConfig::new_with_client_auth` / `from_pem_files_with_client_ca`
   exist; `from_env` honors `SERVER_TLS_CLIENT_CA_PATH` and errors on a
   partial configuration (`MTLS-FR-001`, `MTLS-FR-004`).
2. Over a real socket, a client presenting a certificate signed by the
   configured CA completes a request/response round trip identically to
   the FR-019 suite (`MTLS-FR-001`).
3. Over a real socket, a client presenting no certificate, and one
   presenting a certificate signed by a *different* CA, each fail the
   handshake; the server writes no `Response` and does not panic; the
   client sees an error, not a hang (`MTLS-FR-001`, `MTLS-FR-008`).
4. Composed with `AuthConfig`: an admitted connection without a token
   is `Unauthenticated`; with a read token it is refused a write; with a
   write token it writes — exactly the FR-016 matrix behind the
   certificate gate (`MTLS-FR-002`).
5. Without `AuthConfig`: an admitted connection writes immediately
   (`MTLS-FR-002`, `AUTH-FR-007`).
6. `ClientTlsConfig::with_identity` / `with_identity_pem_files` exist;
   `SchemaDrivenClient::connect_with` reaches an mTLS server with them
   and fails under the `Hello` without them (`MTLS-FR-003`).
7. The server never reads the client certificate: no new accessor,
   field, or log line carries any of its contents (`MTLS-FR-005`).
8. `handle_connection`, `framing.rs`, `protocol.rs`, the codec, and
   `PROTOCOL_VERSION` are unchanged; `git diff --stat` shows no
   `Cargo.toml` dependency change (`MTLS-FR-006`).
9. Every pre-existing test, bench, and binary passes unchanged
   (`MTLS-FR-007`).
10. `cargo doc` clean: `TlsConfig`'s, `ClientTlsConfig`'s, and
    `handle_connection`'s docs describe admission vs. class in those
    terms.

## Verification plan

- `tests/server_tls_integration.rs` gains an mTLS section on the same
  `rcgen` footing: a helper issuing a throwaway CA
  (`IsCa::Ca(BasicConstraints::Unconstrained)`) and a client leaf signed
  by it (`CertificateParams::signed_by`, with
  `ExtendedKeyUsagePurpose::ClientAuth`), generated fresh per test and
  never committed. Tests for criteria 2–6 over real sockets, including
  the second-CA rejection and the no-certificate rejection, both
  asserting the client got an error and the server sent nothing.
- The partial-environment rule (criterion 1) is tested through the
  path-taking constructor with a missing file plus a unit test of the
  variable-combination logic factored so it does not read the real
  process environment — the same constraint `AuthConfig::from_env`'s
  docs already impose on tests.
- Before implementation, a throwaway probe (never committed, per
  precedent) confirms two things this design asserts from reading
  source rather than running it: that `rustls`' `WebPkiClientVerifier`
  accepts an `rcgen` leaf carrying `ClientAuth` EKU chained to an
  `rcgen` CA, and that a no-certificate client fails the handshake
  rather than stalling. If either is false the design, not the test, is
  revised.
- The FR-019 and FR-022 suites run unmodified as the regression test
  for `MTLS-FR-007`.

## Traceability

- At implementation: `SERVER-001` v0.13.0, `SERVER-001-FR-023`
  (`MTLS-FR-001`–`008`); `SERVER-001`'s v0.9.0 open question ("still not
  mTLS") resolved by pointer; `SERVER-AUTH-DESIGN.md`'s "replace or
  layer" open question and ADR-0012's/ADR-0014's mTLS revisit triggers
  pointed here; `SPEC-REGISTRY`, `TRACEABILITY`, `PROJECT-STATUS`.
- Roadmap: `SERVER-MTLS-DESIGN` (this document), then `SERVER-MTLS`
  (the implementation unit) — the `SERVER-TLS-DESIGN` → `SERVER-TLS`
  precedent.

## Open questions

- **Class from certificate** (option 2 above): the natural next step
  once `rusty_tls::TlsServerStream` exposes the peer certificate. Two
  decisions wait on it — an upstream `rusty_tls` accessor (a small PR,
  likely the implementer's own) and how this crate reads a subject or
  SAN without adopting an X.509 parser it does not otherwise need.
  Neither is decided here; option 1 is designed so that step is purely
  additive.
- **Revocation** without a restart (a CRL on the server's client
  verifier). `rustls` supports it; `rusty_tls` exposes it only
  client-side today. An upstream question first.
- Whether `dog_server` should refuse to start when `SERVER_TLS_CLIENT_CA_PATH`
  is set without tokens *and* the operator plausibly meant both — no: a
  certificates-only server is a legitimate configuration (`MTLS-FR-002`),
  and guessing intent is not this crate's job. Recorded so it is not
  re-asked.
- Exact error shape for the partial-environment case (reuse `Io` vs. a
  new `TlsConfigError` variant) — implementation-time.

## Change history

- 2026-09-02: Initial proposal, in response to the owner selecting the
  mTLS revisit (ADR-0014's first trigger) as the second of four next
  directions, after the legacy Evidence backfill and before the
  transaction-session design and the reconnect-without-hello fallback.
  (PR #133.)
- 2026-09-02: Accepted as proposed; the verification plan's throwaway
  probe ran first and confirmed every assertion this document made from
  reading source (see ADR-0023's acceptance log). No content change.
