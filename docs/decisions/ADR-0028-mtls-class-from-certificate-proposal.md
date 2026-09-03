# ADR-0028: Class from certificate — exact-DER pinning on `AuthConfig`, an eager handshake, and the upstream `rusty_tls` accessor first

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): exact-DER pinning on
  `AuthConfig`, eager handshake, certificate class as the starting
  state with a valid token replacing it, the upstream accessor first;
  (b) SPKI pinning and (c) close as not warranted declined; no changes
  requested). Acceptance authorizes the design; implementation begins
  with the upstream `rusty_tls` PR and proceeds in this crate only once
  that lands — see "Acceptance and implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-MTLS-CLASS-DESIGN.md` (the full design
  this ADR summarizes), `ADR-0023` / `docs/design/SERVER-MTLS-DESIGN.md`
  (mTLS as admission, `SERVER-001` v0.13.0 / FR-023; its first revisit
  trigger — *class-from-certificate becomes wanted — first an upstream
  `rusty_tls` peer-certificate accessor, then an X.509 reading decision
  in this crate* — is what this answers), `ADR-0012` /
  `docs/design/SERVER-AUTH-DESIGN.md` (`AuthConfig`, `TokenClass`,
  `Authenticate`), `ADR-0014` (`TlsConfig`, the lazy handshake),
  `Cargo.toml` (`rusty_tls` pinned at `9fd2e27`),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.16.0.
- Supersedes/Superseded by: none. Extends `ADR-0023`'s layering — the
  certificate still admits; it may now also class — and `ADR-0012`'s
  `AuthConfig` by one kind of credential. Reverses nothing.

## Context

`ADR-0023` made a client certificate an admission gate and left class
to the token, because the pinned `rusty_tls` exposed no peer
certificate server-side and because reading an identity out of one
would need an X.509 decision. It named both as the revisit. The owner
picked it.

Three facts, all established by reading and running rather than
recalling. The pinned `rusty_tls` already has the accessor on the
*client* stream (`TlsStream::peer_certificate_der`) and
`complete_handshake` on both streams; the server stream lacks only
the mirror — an eight-line patch, spelled out in the design and run
against this crate through a local `[patch]` (discarded): the server
sees exactly the client's leaf DER after an eager handshake. An eager
handshake also hands the server a typed reason for a rejection
(`NoCertificatesPresented`), where today it sees a read error. And the
identity question has an answer that needs no parser at all: pin the
certificates themselves, by byte equality — what an operator with a
handful of client certificates would do anyway.

One constraint of this session: the upstream repository could not be
attached (refused by the session's permission policy), so the
accessor is designed and verified here but not landed. The
implementation unit begins with that PR.

The owner selected this as the third of four directions. This ADR
proposes a design and authorizes no implementation — the posture
`ADR-0016` through `ADR-0027` took.

## Decision drivers

- Let an operator give a client certificate a class, so a
  certificates-only deployment is a real posture rather than
  "everyone admitted is `ReadWrite`."
- Read nothing out of a certificate that needs a parser; add no
  dependency; change no wire.
- Keep `ADR-0023`'s layering intact: admission stays the certificate
  CA check, class stays `AuthConfig`'s decision — now with one more
  input.
- Make the upstream prerequisite exact and verified, so the upstream
  PR is a copy, not a design.

## Considered options

1. **Exact-DER pinning on `AuthConfig`, eager handshake, certificate
   class as the connection's starting state with a valid token
   replacing it; upstream accessor first** — proposed.
2. **SubjectPublicKeyInfo pinning.** Survives re-issuance under the
   same key; needs the first X.509 code in this crate (a DER walk or a
   dependency). Offered as option (b).
3. **Subject/SAN patterns.** A parser and a pattern language;
   rejected.
4. **Class from the issuing CA.** The verifier does not report which
   root matched; rejected.
5. **The map on `TlsConfig`.** Two objects deciding authorization;
   rejected.
6. **Lazy handshake, read the certificate after the first frame.** The
   first frame is already gated by then; rejected.
7. **Certificate class immutable, `Authenticate` refused.** Removes a
   behavior for no gain; rejected.
8. **Close** — admission-only stands. Offered as option (c).

## Decision

Proposed: option 1. Concretely, at implementation:

- Upstream first: `TlsServerStream::peer_certificate_der` in
  `rusty_tls` (the design's patch, verbatim); then `Cargo.toml`'s
  `rusty_tls` rev moves to that commit. No new dependency.
- `src/server/mod.rs`: `AuthConfig::{with_certificate_class,
  with_certificate_class_pem_file, class_for_certificate}`,
  `is_configured()` true with classes only, `Debug` printing counts;
  `handle_connection` calls `complete_handshake()` after `accept` and
  seeds `authenticated` from the classed leaf (`CLS-FR-002`–`004`).
- `src/bin/dog_server.rs`: `SERVER_AUTH_READ_ONLY_CLIENT_CERTS` /
  `SERVER_AUTH_READ_WRITE_CLIENT_CERTS` (`:`-separated PEM files);
  refuses to start with classes but no `SERVER_TLS_CLIENT_CA_PATH`.
- `SERVER-001`'s next minor / FR (`CLS-FR-001`–`008`); `ADR-0023`'s
  first trigger and `SERVER-MTLS-DESIGN.md`'s open question resolved
  by pointer, its third trigger restated with the probe's finding;
  `SERVER-AUTH-DESIGN.md`'s layering answer extended; `SPEC-REGISTRY`,
  `TRACEABILITY`, `ROADMAP` (`SERVER-MTLS-CLASS`), `PROJECT-STATUS`.
- Tests per the design's verification plan on the existing
  throwaway-CA helpers; every existing suite unchanged.
- No wire, `PROTOCOL_VERSION`, `TokenClass`, `TlsConfig`,
  `ConnectionStore`, or store change.

## Consequences

### Positive

- A certificate can carry authorization, not only admission, with the
  operator naming exactly which certificates mean what.
- No parser, no dependency, no wire change; a server without classes
  is byte-for-byte v0.16.0.
- The server holds a typed reason for every rejected handshake — the
  input the audit-log design will want.
- The upstream change is verified before it is asked for.

### Negative / tradeoffs

- **Rotation is manual.** A re-issued certificate has new bytes and
  loses its class until the map is updated — failure closed, and the
  PEM-file list makes the update one file, but it is a chore SPKI
  pinning would remove at the cost of X.509 code.
- **An upstream dependency on the critical path.** Nothing here ships
  until `rusty_tls` merges the accessor and the pin moves; this
  session could not open that PR.
- **`is_configured()` changes meaning** for an operator who has
  certificates but no tokens and adds a class: admitted-but-unclassed
  connections go from `ReadWrite` to unauthenticated. The safe
  direction, and named.
- One more linear scan and one more `Option` per TLS connection —
  negligible, but a new branch in the connection setup path.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0027`, with the upstream mechanism run: the accessor patch and
  the eager handshake were exercised against this crate through a
  local `[patch]` and two throwaway tests (both discarded), recorded in
  the design.
- Revisit if: rotation becomes a real operational burden — SPKI
  pinning (option 2), the X.509 reading decision `ADR-0023` deferred.
- Revisit if: a third class appears (`ADR-0012`'s own trigger) — the
  map's value type is `TokenClass` and follows it.
- Revisit if: `rusty_tls` exposes the matched root or a parsed
  subject — class-from-CA or subject patterns become cheap.
- Revisit if: the typed handshake error is wanted on the client — the
  probe shows an eager client handshake does not surface it under
  TLS 1.3; a different mechanism (reading the alert on the first read
  and mapping it) is a separate, client-side decision.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — exact-DER
  pinning on `AuthConfig`, eager handshake, certificate class as the
  starting state with a valid token replacing it, the upstream
  accessor landed first; **(b)** accept with SPKI pinning — the same,
  matching on the certificate's `SubjectPublicKeyInfo` so re-issuance
  under one key keeps its class, at the cost of the first X.509 DER
  code in this crate; **(c)** close as not warranted — admission-only
  stands, `ADR-0023`'s trigger stays armed. Whichever is chosen, the
  upstream `rusty_tls` PR is the first step and needs either the
  repository attached to a session or the owner applying the design's
  patch. Proposed in PR #149.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implementation is gated on `CLS-FR-001`: the upstream
  `TlsServerStream::peer_certificate_der` accessor (the design's patch,
  verbatim) must land in `rusty_tls` and the pin move to it before any
  code here changes. This session cannot open that PR (the repository
  attach was refused by the session's permission policy); the crate-side
  unit is queued behind `ADR-0026`, `ADR-0027`, and `ADR-0029` and
  starts when the owner lands or authorizes the upstream change. (PR #153.)
- 2026-09-03: `CLS-FR-001` satisfied — a separately-spawned session opened
  `Rusty-Mill/rusty_mill` PR #148 with the accessor exactly as designed;
  the owner merged it directly. Implemented in this crate as
  `SERVER-001` v0.21.0 (FR-031) per `docs/design/SERVER-MTLS-CLASS-DESIGN.md`:
  `Cargo.toml`'s `rusty_tls` rev moved to the merge commit (no new
  dependency); `AuthConfig`'s exact-DER certificate-class map;
  `handle_connection`'s already-eager handshake (from `ADR-0029`) now
  reads `peer_certificate_der()` and seeds the connection's starting
  class from it, a later valid `Authenticate` still replacing it;
  `SERVER_AUTH_READ_ONLY_CLIENT_CERTS`/`SERVER_AUTH_READ_WRITE_CLIENT_CERTS`
  in `dog_server`, refused without `SERVER_TLS_CLIENT_CA_PATH`. Every
  acceptance criterion 1–7 holds; no deviation. (PR #166.)
