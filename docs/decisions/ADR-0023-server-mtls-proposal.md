# ADR-0023: Mutual TLS — client certificates as an admission gate, layered under `AuthConfig`

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, option (a): client certificates as
  an admission gate layered under `AuthConfig`, one optional input on
  each end; (b) hold for class-from-certificate and (c) close as not
  warranted declined; no changes requested). Acceptance authorizes the
  design; implementation follows as its own unit — see "Acceptance and
  implementation" below.
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/SERVER-MTLS-DESIGN.md` (the full design document
  this ADR summarizes), `ADR-0014` / `docs/design/SERVER-TLS-DESIGN.md`
  (native TLS; named mTLS out of scope and as its first revisit trigger
  — this answers it), `ADR-0012` / `docs/design/SERVER-AUTH-DESIGN.md`
  (the token scheme; left open whether mTLS would replace or layer —
  this answers *layer*), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.12.0 (FR-019 server TLS, FR-022 client TLS), `rusty_tls` at the
  pinned `9fd2e27` (`TlsAcceptor::new_with_client_auth`,
  `TlsStream::new_with_client_identity`), `PROJECT-STATUS` items 72–74.
- Supersedes/Superseded by: none. Extends `ADR-0014`'s `TlsConfig` by
  one optional input and `SERVER-001` v0.12.0's `ClientTlsConfig` by one
  optional input; changes nothing either already does.

## Context

Since v0.9.0 a server can require every connection to complete a TLS
handshake, and since v0.12.0 the crate's own client can complete one.
The server proves its identity with a certificate; the client proves
nothing at the transport layer and proves its class with a shared-secret
token after the handshake. `ADR-0014` recorded the consequence plainly:
"a sufficiently determined attacker who obtains a valid token can still
authenticate, same as today; this proposal only removes the ability to
*observe* that token on the wire," and named mTLS as its first revisit
trigger — "a real, separate future design, not decided here."

`rusty_tls` already ships both halves of the mechanism:
`TlsAcceptor::new_with_client_auth(cert_chain, key, client_ca_roots)`
fails the handshake for a client that presents no certificate or one
that does not chain to the roots, and
`TlsStream::new_with_client_identity(sock, name, policy, chain, key)`
presents one. What it does not ship is any way to *read* the admitted
certificate afterwards — `TlsServerStream` keeps its
`rustls::ServerConnection` private — and this crate has no X.509 parser.
That fact, verified from the pinned source, shapes the decision below.

The owner selected this revisit as the second of four next directions.
This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0022` took.

## Decision drivers

- Make possession of a token insufficient on its own: an attacker must
  also hold an admitted client certificate's private key.
- Change nothing that exists: `serve`'s signature, `handle_connection`'s
  body, `AuthConfig`, `TokenClass`, the wire, `PROTOCOL_VERSION`, every
  existing `TlsConfig`/`ClientTlsConfig` call site.
- Use `rusty_tls` exactly as shipped, with no upstream change as a
  prerequisite and no new dependency in this crate.
- Answer `SERVER-AUTH-DESIGN.md`'s open question ("replace or layer")
  in a way a later, richer design can build on without undoing.
- Keep the operator story the one `TlsConfig` already has: PEM files,
  environment variables, a startup error rather than a silent downgrade.

## Considered options

1. **Client certificate as admission only, layered under `AuthConfig`**
   — proposed. `TlsConfig` gains an optional set of client CA roots;
   with them, the handshake fails for any client not presenting a
   certificate chaining to one, before any framed byte (including
   `Authenticate`) is read. The token then decides class exactly as
   today, in every combination: tokens only, certificates only (every
   admitted connection `ReadWrite`, `AUTH-FR-007`), both, neither.
   `ClientTlsConfig` gains an optional identity so `SchemaDrivenClient`
   can reach such a server. `handle_connection` does not change: the
   acceptor carries the policy and the lazy handshake already fails a
   rejected client on the first `read_message` (`TLS-FR-003`).
2. **Class derived from the certificate** (one root per class, or a
   subject/SAN convention). The elegant end state; rejected for this
   proposal because it cannot be built on the pinned `rusty_tls` — no
   peer-certificate accessor exists, and "which root signed it" is not
   observable without one — and would then need an X.509 parsing
   decision here. Named as the future revisit; option 1 is its
   strictly compatible base.
3. **Certificate replaces tokens** — an mTLS server ignores
   `AuthConfig`. Rejected: discards the `ReadOnly`/`ReadWrite` split
   for nothing option 1 does not already offer (configure no tokens).
4. **Optional client certificate** (request, admit without). Rejected:
   not exposed by `rusty_tls`, and an admission check that admits
   without checking is not one.
5. **Where the roots live**: a second `TlsConfig` constructor
   (proposed — mTLS is a property of the acceptor, and `rusty_tls`
   models it that way) vs. a fifth `serve` parameter (rejected:
   meaningless without the fourth) vs. on `AuthConfig` (rejected: it is
   not a class, and it would imply the certificate takes part in the
   class decision, which option 1 denies).

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/mod.rs`: `TlsConfig::new_with_client_auth(cert_chain_der,
  private_key_der, client_ca_roots_der)`,
  `TlsConfig::from_pem_files_with_client_ca(chain, key, client_ca)`,
  `TlsConfig::from_env` reading optional `SERVER_TLS_CLIENT_CA_PATH`
  (set without the chain/key variables → `Some(Err(..))`, never `None`),
  `TlsConfig::requires_client_certificate()`. `TlsConfigError` needs no
  new variant (`Tls(InvalidClientCaRoots)`, `Io`, `Pem`); the
  partial-environment error's exact shape is implementation-time.
  `handle_connection`, `serve`, `AuthConfig`, `TokenClass`: unchanged.
- `src/server/client.rs`: `ClientTlsConfig::with_identity(chain_der,
  key_der)`, `with_identity_pem_files(chain, key)`, `has_identity()`;
  `connect_with` chooses `TlsStream::new` or
  `new_with_client_identity` accordingly. `Transport` unchanged.
- `src/bin/dog_server.rs`: module doc names the third variable; no
  logic change (the error path already exists).
- `tests/server_tls_integration.rs`: an mTLS section on the existing
  `rcgen` footing — a throwaway CA and a client leaf signed by it,
  generated per test; round trip, no-certificate rejection, wrong-CA
  rejection, composition with `AuthConfig` (both directions),
  `SchemaDrivenClient` with and without an identity. Preceded by a
  throwaway probe (never committed) confirming `rustls`' client
  verifier accepts an `rcgen` `ClientAuth` leaf and fails a
  no-certificate client cleanly.
- `SERVER-001` v0.13.0, FR-023 (`MTLS-FR-001`–`008`); the v0.9.0 open
  question resolved by pointer; `SERVER-AUTH-DESIGN.md`'s "replace or
  layer" question, `ADR-0012`'s and `ADR-0014`'s mTLS triggers pointed
  here; `SPEC-REGISTRY`, `TRACEABILITY`, `ROADMAP` (`SERVER-MTLS`),
  `PROJECT-STATUS`.
- No `Cargo.toml` dependency change (`rcgen` is already a
  dev-dependency); no wire, framing, codec, or `PROTOCOL_VERSION`
  change.

## Consequences

### Positive

- A stolen token no longer authenticates on its own; the attacker also
  needs an admitted private key — the gap `ADR-0014` named, closed at
  the layer that can close it.
- Nothing existing changes shape: one optional input on each end, and
  the connection loop is untouched. The FR-019 and FR-022 suites are
  the regression test, unmodified.
- The layering answer is future-proof: a class-from-certificate design
  adds a rule on top of admission; it does not reverse this decision.
- No upstream prerequisite, no new dependency, hermetic tests with what
  `Cargo.toml` already has.

### Negative / tradeoffs

- **Still no identity from the certificate.** An admitted client is
  "someone the operator's CA signed," nothing finer; class is still the
  token's. The richer design waits on an upstream `rusty_tls` accessor
  and a parser decision — named, not hidden.
- **No revocation short of a restart** with a new root and re-issued
  client certificates. `rusty_tls` exposes revocation only on the
  client's trust policy today.
- **The operator now runs a CA**, and its key is the highest-value
  secret this design introduces. Issuance and distribution are the
  operator's tooling, as the server certificate's already are.
- **A misconfigured client hangs no worse than today** but fails no
  clearer: a rejected identity surfaces as `Frame(Io(..))` under the
  `Hello`, the same shape as any other handshake failure, because the
  handshake is lazy on both ends. A dedicated error would need an eager
  handshake in `rusty_tls`; not pursued here.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0012` through
  `ADR-0022`: no implementation, no test, no dependency at acceptance.
  The one thing this design asserts from reading rather than running —
  that `rustls`' `WebPkiClientVerifier` accepts an `rcgen`-issued
  `ClientAuth` leaf chained to an `rcgen` CA, and fails a no-certificate
  client without stalling — is checked by a throwaway probe before
  implementation; if it fails, the design is revised, not the test.
- Revisit if: class-from-certificate becomes wanted — first an upstream
  `rusty_tls` peer-certificate accessor, then an X.509 reading decision
  in this crate; this decision's admission gate stays as the base.
  *Taken up as `ADR-0028` / `docs/design/SERVER-MTLS-CLASS-DESIGN.md`,
  proposed in PR #149: the accessor spelled out and verified by a local
  patch probe; the X.509 decision answered by exact-DER pinning on
  `AuthConfig` — no parser. Admission stays as this decision made it.
  Implemented as `SERVER-001` v0.21.0 / FR-031 (PR #166.), once the
  upstream accessor landed as `Rusty-Mill/rusty_mill` PR #148.*
- Revisit if: revocation without a restart becomes a real operational
  need — an upstream `rusty_tls` change to expose a CRL on the server's
  client verifier comes first.
- Revisit if: `rusty_tls` gains an eager-handshake or typed-handshake-
  error surface — a rejected identity could then be `ClientError::Tls`
  rather than `Frame(Io(..))`.
  *Checked in `ADR-0028`'s probe: the pinned `rusty_tls` already has
  `complete_handshake` on both streams, but under TLS 1.3 the server's
  rejection arrives after the client's `Finished`, so an eager client
  handshake returns `Ok` and the error still surfaces on the first read.
  Trigger stays armed; the server side does get a typed reason.*

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — admission
  only, layered under `AuthConfig`, one optional input on each end, no
  upstream prerequisite; **(b)** hold for class-from-certificate — open
  the upstream `rusty_tls` accessor first and design the class rule
  with it, accepting a longer path and a parser decision; **(c)** close
  as not warranted — no deployment today needs client certificates, and
  the token scheme behind TLS is judged sufficient; `ADR-0014`'s trigger
  stays armed. Proposed in PR #133.
- 2026-09-02: accepted as proposed (option (a); (b) and (c) declined).
  The design's throwaway probe (never committed) ran before acceptance
  and confirmed the mechanism: an `rcgen` CA-signed `ClientAuth` leaf
  round-trips through `TlsAcceptor::new_with_client_auth`; a client
  with no identity fails at its first write with
  `AlertReceived(CertificateRequired)` (server: "peer sent no
  certificates"); a leaf from a different CA fails with
  `AlertReceived(DecryptError)` (server: `BadSignature`); an empty root
  set is `InvalidClientCaRoots` at construction — no hangs, every
  rejection on the existing `TLS-FR-003` path. The next unit registers
  `SERVER-001` v0.13.0 / FR-023 and implements per
  `docs/design/SERVER-MTLS-DESIGN.md`. (PR #134.)
- 2026-09-02: implemented as `SERVER-001` v0.13.0 (FR-023) in PR #135
  — `TlsConfig::new_with_client_auth` / `from_pem_files_with_client_ca`
  / `from_env` with `SERVER_TLS_CLIENT_CA_PATH` (a partial configuration
  is `Some(Err(Io(NotFound)))`, the table factored for a hermetic unit
  test) / `requires_client_certificate()`; `ClientTlsConfig::with_identity`
  / `with_identity_pem_files` / `has_identity()` with a key-redacting
  `Debug`; `dog_server` reporting the mode. `handle_connection`, `serve`,
  `AuthConfig`, `TokenClass`, the wire, `PROTOCOL_VERSION`, and
  `Cargo.toml` unchanged, exactly as the Decision said. Four tests in
  `tests/server_tls_integration.rs` on a throwaway `rcgen` CA, one unit
  test; every acceptance criterion 1–10 holds; no deviation. Full sweep
  green (338 lib tests, 337 + 1; TLS suite 12/12).
