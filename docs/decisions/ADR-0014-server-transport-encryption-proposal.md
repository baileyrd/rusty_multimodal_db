# ADR-0014: Add native transport encryption (TLS) to the server/query layer

- Status: **Proposed** — awaiting owner review.
- Date: 2026-09-01
- Deciders: baileyrd
- Related: `docs/design/SERVER-TLS-DESIGN.md` (the full design document
  this ADR summarizes), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (named "no authentication, authorization, or transport encryption" as a
  real, unresolved gap at acceptance time), `docs/decisions/ADR-0012-server-authentication-proposal.md`
  (closed the authentication half of that gap; named this proposal's own
  revisit trigger directly — see ADR-0012's "Validation and revisit
  triggers"), `docs/specifications/server/SERVER-001-query-layer.md` (the
  spec this proposal would extend, not modify, until accepted)
- Supersedes/Superseded by: none. Extends (does not supersede)
  ADR-0012's own "transport encryption remains a separate, still-open
  gap" consequence — this ADR is the revisit ADR-0012 itself named as a
  trigger.

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
   **native TLS via `rustls`, terminated inside this crate's own server
   process** (**chosen** — `rustls` ships a synchronous, `Read`/`Write`-
   compatible API that composes with the existing thread-per-connection
   model with zero changes to `framing.rs`, undercutting ADR-0012's
   original objection once the implicit async-runtime assumption is
   separated out; also closes a gap an external proxy structurally
   cannot, since this crate can never verify from inside its own process
   that a proxy is actually in front of it).
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
  `AuthConfig`; a per-connection TLS handshake via `rustls::ServerConnection`
  completed before any framed `Request`/`Response` traffic (including
  `Authenticate`) is ever read or written; `src/server/framing.rs`
  requires zero changes, since `read_message`/`write_message` are
  already generic over `Read`/`Write`.
- **Client identity is unchanged by this proposal.** TLS gives the
  server a real certificate to authenticate itself with, and encrypts
  every byte on the wire (including `AuthConfig`'s tokens); it does not
  add client-certificate (mTLS) authentication. `AuthConfig`'s existing
  shared-secret token scheme remains the entire client-identity story,
  now traveling encrypted rather than plaintext.
- A server with no `TlsConfig` configured behaves exactly as today's
  `SERVER-001` — the same backward-compatibility bar `AUTH-FR-007` set
  for authentication, applied here to encryption.
- One new dependency: `rustls` (plus whatever crypto-provider crate it
  requires — e.g. `aws-lc-rs` or `ring`, an implementation-time choice
  between `rustls`'s supported backends, not fixed by this ADR). A real,
  meaningfully larger dependency addition than `subtle` was for
  authentication — named plainly, not minimized, in "Consequences" below.
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

### Negative / tradeoffs

- **A real, meaningfully larger dependency addition than any prior
  server-layer round has taken.** `rustls` plus a crypto-provider crate
  is a bigger footprint than `subtle` (a small, single-purpose
  comparison utility) — this project's own minimal-dependency posture
  means this tradeoff should be weighed by the owner explicitly, not
  waved through because encryption sounds obviously necessary.
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
  connection setup.
- The exact stream-abstraction shape (`enum Connection` vs. a generic
  `handle_connection<S: Read + Write>`) is left as an implementation-time
  decision — a real, if bounded, design choice still ahead of whoever
  implements this.

## Validation and revisit triggers

- **This proposal is design-only, matching ADR-0012's/ADR-0013's own
  precedent** — no implementation, no test suite, no dependency actually
  added yet. No standalone scratch-crate compile probe was built for this
  one either, matching ADR-0012's own reasoning: the proposed additions
  (one new optional config struct, a per-connection stream wrapper) are
  incremental extensions of `SERVER-001`'s existing, already-compiling
  shapes, not a genuinely new type-system structure. Flagged here
  explicitly as a deliberate scope choice, not an oversight.
- Revisit if: mTLS becomes a real requirement now that this crate owns
  TLS natively — a real, separate future design, not decided here (see
  `SERVER-TLS-DESIGN.md`'s own "Open questions").
- Revisit if: certificate rotation without a server restart becomes a
  real operational need — this design's "restart the process with new
  cert/key files" story would need replacing.
- Revisit if: the owner judges the `rustls`/crypto-provider dependency
  weight disproportionate on review — the external-proxy path remains
  fully available as the alternative, and this ADR would be rejected (or
  revised to scope the dependency differently) rather than force through
  regardless.
