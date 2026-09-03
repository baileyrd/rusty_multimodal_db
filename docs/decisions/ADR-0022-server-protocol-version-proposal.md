# ADR-0022: Wire protocol version — a `PROTOCOL_VERSION` constant, an optional first-frame `Hello`, and append-only compatibility rules

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, option (a): optional first-frame
  `Hello`, `min` negotiation, `PROTOCOL_VERSION = 2`, rules written, no
  gating state yet; (b) hello-required and (c) rule-only declined; no
  changes requested)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md` (the full
  design document this ADR summarizes),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.9.1 (whose
  last open question — *"frames carry no protocol version and there is
  no hello handshake … revisit trigger: a second deployed client
  build"* — this answers ahead of the trigger), `ADR-0021` /
  `STORAGE-018` (which pinned the encoding under the shape and named
  the shape's evolution as their first revisit trigger), `ADR-0010`
  (whose design said *"no explicit version negotiation exists in this
  proposal"*), `ADR-0012` (the `Request::Authenticate` precedent for a
  request `handle_connection` answers itself), `PROJECT-STATUS` items
  33 and 69
- Supersedes/Superseded by: none. Extends `ADR-0010`'s protocol by two
  appended variants; changes no existing variant's bytes; changes no
  format `ADR-0005`–`ADR-0021` produced.

## Context

The wire protocol is a `u32` length prefix and one `crate::codec`-encoded
`Request` or `Response`. `STORAGE-018` pinned every byte inside a frame
and gave every variant a golden vector. Nothing names the *shape*: a
`Request` is its `u32` declaration index then its fields, so the set of
variants is the version, and no frame says which set the peer has.

Every client and server today come from one build, so this costs
nothing yet. When two builds differ: a request the server does not know
is a decode error and `handle_connection` closes the connection with no
reply — the client sees an EOF it cannot tell from a network fault; and
a new server has no way to know which `Response`/`ErrorCode`/
`ScanValue`/`ValueKind` variants an old client can decode, so nothing
stops it from sending one the client cannot. `SERVER-001` has carried
this as an open question with a trigger (a second deployed client
build) and no driver. The owner chose to design it ahead of the
trigger: the handshake a second build must speak is cheapest to settle
while there is one build.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0021` took.

## Decision drivers

- Turn "append a variant" from a named incompatibility into a
  negotiated one, so the next `Request`/`Response` change has a version
  to live under, the way a blob change lives under `BLOB_VERSION`.
- Change no existing byte: every v0.9.1 golden vector stays true, every
  existing client keeps working with no change.
- Give both directions of skew a defined outcome: a client learns the
  server's version before sending anything else; a server learns what
  its peer can decode.
- Put the version where `STORAGE-018` already guarantees compatibility
  — an appended variant — not in the transport (`FR-002`) or in an
  existing struct.
- One integer, one round trip, once per connection. No feature lists,
  no per-frame overhead, no new dependency.
- Write the rules now; build the gating branch when a gated variant
  first exists, not before.

## Considered options

1. **A per-frame version byte in the length prefix's top byte** — zero
   wire cost (that byte is `0x00` for every frame under 16 MiB).
   Rejected: versions the transport for a per-connection property, caps
   at 255, changes `FR-002` and its tests and the exact-16 MiB edge, and
   an old server's failure (`FrameTooLarge`, dropped) is no better than
   today's.
2. **A `protocol_version` field on `DomainSchema`** — rejected: a
   struct-field change is a format change to `Response::Schema` for
   every existing client, and `DescribeSchema` is refused before
   authentication.
3. **An appended `Request::Hello` / `Response::Hello` pair, optional,
   first frame if sent, `min` negotiation** — proposed. `pub const
   PROTOCOL_VERSION: u32 = 2` (version 1 is retroactively the v0.9.1
   shape, what a silent client speaks). The server answers `Hello`
   before authentication, as it answers `Authenticate`, with
   `min(client, PROTOCOL_VERSION)`; version 0 or a non-first `Hello` is
   `Malformed`. Four rules written into `protocol.rs` and `SERVER-001`:
   append-only; every variant records its introducing version;
   a server sends a variant introduced at *N* only on a connection
   negotiated at ≥ *N* (nearest older shape otherwise); a client sends
   a request introduced at *N* only after negotiating ≥ *N*.
   `SchemaDrivenClient::connect` says `Hello` before `DescribeSchema`
   and exposes `server_protocol_version()`.
4. **Same, with the hello required** — a first frame that is not `Hello`
   is `Malformed`. Offered as the strict alternative; rejected as the
   default because it breaks every existing client (`benches/server.rs`
   included) for no gain today.
5. **Rule only, no wire change** — write the append-only rules, add no
   `Hello`. Offered as the minimal alternative; rejected as the
   proposal because the server still cannot know its peer's version, so
   a new response shape could only ever answer a new request kind (no
   new `ErrorCode` for an old request, ever), and an old server still
   drops a new client silently.
6. **Feature bitmap in the hello; `min_supported` + `current`; hello
   accepted at any time; gating state built now** — each rejected in the
   design document: under append-only the version integer *is* the
   capability set, `min_supported` is always 1 until a major break is
   designed, mid-connection renegotiation buys nothing, and at version 2
   there is no variant to gate so the branch would be dead code behind
   a public-signature change to `dispatch`.

## Decision

Accepted: option 3. Concretely, at implementation:

- `src/server/protocol.rs`: `pub const PROTOCOL_VERSION: u32 = 2`;
  `Request::Hello { protocol_version: u32 }` at index 10;
  `Response::Hello { protocol_version: u32 }` at index 10; a
  "Protocol versions" table (1: v0.1.0–v0.9.1 shape; 2: + `Hello`) and
  the four compatibility rules in the module docs; golden vectors for
  both (`0x0a 0 0 0` then `u32` LE); every existing vector unchanged.
- `src/server/mod.rs`: `handle_connection` intercepts `Hello` before the
  `Authenticate` intercept and the auth gate, on plain and TLS
  connections alike — first frame with version ≥ 1 → `Response::Hello {
  min(client, PROTOCOL_VERSION) }`; version 0 or not-first → `Err {
  Malformed }`, connection stays open. `dispatch` maps `Hello` to
  `Unsupported`, as it maps `Authenticate`. No negotiated-version state
  stored until a gated variant needs it (the rule names where).
- `src/server/client.rs`: `connect` sends `Hello` first, keeps the
  negotiated version, exposes `server_protocol_version()`; a non-`Hello`
  reply is `UnexpectedResponse("Hello")`; a pre-hello server presents
  as `Frame(Io)` (EOF), named in the docs.
- `SERVER-001` v0.10.0, FR-020 (`PROTO-FR-001`–`008`); the v0.9.1 open
  question resolved by pointer; `STORAGE-018`'s trigger, `ADR-0010`'s
  note, and `ADR-0021`'s trigger pointed here; `SPEC-REGISTRY` and
  `PROJECT-STATUS` item 33 updated.
- No `Cargo.toml` change; `framing.rs`, `MAX_FRAME_BYTES`, and the codec
  untouched; `benches/server.rs` and every integration suite unchanged
  and passing (they are version 1's regression test).

## Consequences

### Positive

- The wire shape has a version and a rule set, like every blob has
  `BLOB_VERSION` and `STORAGE-018`'s rules. The next variant appended is
  version 3, not an incident.
- A new client against an old (≥ 2) server learns the server's version
  before its first real request and can adapt with a typed value
  instead of an EOF.
- An old client against a new server is served at its own version, and
  the server has the information rule (iii) needs to keep it that way.
- Zero cost for anyone who does not opt in: no frame changes, no
  per-request work, no existing test or bench touched.
- `bincode` 2.x, a 16-byte `Uuid`, and any other shape change now have
  a version to be introduced under.

### Negative / tradeoffs

- One more round trip at `SchemaDrivenClient::connect` (before the
  `DescribeSchema` it already pays). Nothing per request.
- A pre-hello server (every server built before v0.10.0) drops a
  hello-sending client with no reply. There is no such deployed server
  to protect; it is named, not mitigated. *Mitigated in the client at
  v0.16.0 / FR-026, at the owner's call (see the revisit trigger
  below): `SchemaDrivenClient` reconnects once without a `Hello` and
  speaks version 1.*
- Rule (iii) is a discipline with no enforcing code at version 2: the
  first version-3 response shape must add the per-connection state and
  the branch. The rule says so; the golden vectors and the "introduced
  at" table are what a reviewer checks.
- `handle_connection` gains one `bool`; `Request`/`Response` each gain
  one variant that `dispatch` must name (as `Unsupported`), exactly as
  `Authenticate` already is.
- A version integer without a feature list means every change, however
  small, is a whole-protocol bump. That is the intended shape: simple,
  total, one number.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching
  `ADR-0017`–`ADR-0021`; written from `src/server/{framing,protocol,
  mod,client}.rs` as they stand at `main` `835d7a9` (frame layout,
  variant indices 0–9 on both enums, the `Authenticate` intercept's
  position, `connect`'s first frame), `STORAGE-018`'s evolution rules,
  and `SERVER-001` v0.9.1's open question. No `src/` change.
- **Real validation, post-acceptance**: the design's ten acceptance
  criteria as tests — the two golden vectors with every v0.9.1 vector
  unchanged; `Hello` answered unauthenticated on an auth-configured
  server with the gate intact behind it; `min` for a newer client, own
  version for an older one, `Malformed` for 0 and for a second `Hello`;
  `dispatch` → `Unsupported`; every existing suite and bench unchanged;
  `server_protocol_version() == 2` through the client library on every
  domain; an unknown index closes the connection; the pointer set; a
  diff confined to `src/server/{protocol,mod,client}.rs`, one
  integration test, and docs. Full sweep green. No benchmark.
- Revisit if: the first version-3 variant is proposed — that change
  adds the per-connection negotiated version and rule (iii)'s branch,
  bumps `PROTOCOL_VERSION` to 3, and extends the table; its ADR cites
  this one. *Taken: `ADR-0024` (transaction sessions), implemented as
  `SERVER-001` v0.14.0 / FR-024 in PR #139 — exactly that
  change, as predicted; no change to this decision.*
- Revisit if: a change cannot be expressed by appending — a major
  protocol break. That is a new design (a `min_supported` in the hello,
  or a new magic), not a bump; nothing here anticipates it beyond
  leaving the hello's field a `u32`.
- Revisit if: a pre-hello server is found deployed against a
  hello-sending client — a reconnect-without-hello fallback in the
  client library becomes worth its heuristic. *Taken ahead of the
  trigger, at the owner's explicit call and against this ADR's own
  "not while none is deployed": `SERVER-001` v0.16.0 / FR-026 in the PR
  after #142 — default-on, bounded to one silent reconnect on an
  EOF-class error under the `Hello`, with `ConnectOptions::require_hello()`
  as the opt-out. No change to this decision's wire shape.*
- Revisit if: `SchemaDrivenClient` is used against an auth-configured
  server — it has no `authenticate` today (pre-existing, independent of
  this design); a separate unit. *Taken: `SERVER-001` v0.11.0 / FR-021
  (`connect_authenticated`, `authenticate`) in PR #128; no change to
  this decision.*

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — optional
  first-frame `Hello`, `min` negotiation, `PROTOCOL_VERSION = 2`, rules
  written, no gating state yet (recommended); **(b)** accept with the
  hello *required* — a first frame that is not `Hello` is `Malformed`;
  **(c)** rule only — document append-only, add no `Hello`.
  Proposed in PR #124.
- 2026-09-02: accepted as proposed (option (a); (b) and (c) declined).
  The next unit registers `SERVER-001` v0.10.0 / FR-020 and implements
  per `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md`. (PR #125.)
- 2026-09-02: implemented as `SERVER-001` v0.10.0 (FR-020) in PR #126 —
  `PROTOCOL_VERSION = 2`, `Request::Hello`/`Response::Hello` at index
  10 with golden vectors, the first-frame intercept before
  authentication (`min`, version 0 / non-first → `Malformed`),
  `dispatch` → `Unsupported`, the rules and version table in
  `protocol.rs`'s module docs, `SchemaDrivenClient` saying hello first
  and exposing `server_protocol_version()`, and
  `tests/server_protocol_version.rs`. Every v0.9.1 golden vector, every
  existing suite, and `benches/server.rs` unchanged. One deviation from
  the Decision's "No `Cargo.toml` change", recorded: the new suite
  needs a `[[test]]` `required-features = ["server", "research"]`
  registration, as every other server suite has — one line, no
  dependency. Full sweep green (337 lib tests, 333 + 4).
