# Server Protocol Version Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, option (a): optional first-frame
  `Hello`, `min` negotiation, `PROTOCOL_VERSION = 2`, rules written, no
  gating state yet; (b) hello-required and (c) rule-only declined; no
  changes requested)
- Date: 2026-09-02
- Related: `docs/decisions/ADR-0022-server-protocol-version-proposal.md`
  (the decision record this document backs),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.9.1 (whose
  last open question — *"frames carry no protocol version and there is
  no hello handshake, so a `Request`/`Response` change is a named
  incompatibility, not a negotiated one — revisit trigger: a second
  deployed client build"* — this answers ahead of the trigger),
  `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md` / `ADR-0021` /
  `STORAGE-018` (which pinned the *encoding* under the wire shape and
  named the shape's evolution as their first revisit trigger),
  `docs/design/SERVER-QUERY-LAYER-DESIGN.md` / `ADR-0010` (whose
  "Security, privacy, and compatibility" section said *"no explicit
  version negotiation exists in this proposal — a client and server
  built from different crate versions have no compat guarantee"*),
  `docs/design/SERVER-AUTH-DESIGN.md` / `ADR-0012` (the
  `Request::Authenticate` precedent for a request `handle_connection`
  answers itself, before any store access), `PROJECT-STATUS` item 33
  (resolved for the encoding, open for the shape) and item 69

## Purpose and scope

The wire protocol (`SERVER-001-FR-001`/`FR-002`) is a `u32` little-endian
length prefix followed by one `crate::codec`-encoded `Request` or
`Response`. Since `STORAGE-018`, every byte *inside* a frame is pinned:
the configuration is named once, and every variant of both enums has a
golden vector. What is not pinned, or even named, is the **shape** —
which variants exist, in which order, with which fields. A `Request` on
the wire is its `u32` declaration index followed by its fields, so the
shape *is* the version, and nothing on the wire says which shape a peer
was built with.

Today that costs nothing, because every client and server come from the
same build: `src/bin/dog_server.rs`, the four integration test suites,
`benches/server.rs`, and `server::client::SchemaDrivenClient` all
compile from one tree. `SERVER-001` has carried the gap as an open
question since v0.9.1, with a named trigger — *a second deployed client
build* — and no driver. This design answers it before the trigger fires,
because the answer is cheapest while there is still exactly one build in
the world: whatever handshake exists when the second build appears is
the one that build will have to speak.

What actually breaks today, concretely, when two builds differ:

- **Old server, new client.** A request variant the server does not
  know is a `bincode` decode error on the variant index;
  `handle_connection` treats every read error alike and returns, closing
  the connection with no response frame. The client sees an EOF —
  `FrameError::Io`, indistinguishable from a network fault.
- **New server, old client.** If the new build only *appended* variants
  (`STORAGE-018`'s evolution rule), every request the old client can
  send still decodes and dispatches. But the new server may reply with a
  `Response`, `ErrorCode`, `ScanValue`, or `ValueKind` variant the old
  client does not have — a decode error client-side, reported as
  `ClientError::Frame(Encoding)`. Nothing today stops a new server from
  doing that, because the server has no way to know what the client
  understands.
- **Any other change** — reordering, inserting, or removing a variant;
  changing a field — breaks both directions at the variant index or the
  field bytes, and is exactly what `STORAGE-018` calls a format change
  "under the owning format's version". For the wire, that owning
  version does not exist.

This design adds the missing version: a **protocol version number**, a
**one-round-trip hello** that lets each side learn the other's, and the
**compatibility rules** that make "append a variant" a *negotiated*
evolution rather than a named incompatibility.

**In scope for this proposal:**

- A `pub const PROTOCOL_VERSION: u32` in `src/server/protocol.rs`,
  starting at **2** — version 1 is retroactively the shape as of
  `SERVER-001` v0.9.1 (no hello), so a peer that never says hello is a
  version-1 peer.
- Two appended variants, `Request::Hello { protocol_version: u32 }`
  (index 10) and `Response::Hello { protocol_version: u32 }` (index 10):
  a client may send `Hello` as its first frame; the server answers with
  the **negotiated** version, `min(client, PROTOCOL_VERSION)`. `Hello`
  is answered by `handle_connection` itself, before authentication,
  exactly as `Authenticate` is — it touches no store and reveals nothing
  a `DescribeSchema` wouldn't.
- The compatibility rules, written into `protocol.rs`'s module docs and
  `SERVER-001`: (i) the shape only ever grows by appending variants or
  adding new request kinds — never reorder, insert, remove, or change a
  field; (ii) every variant records the protocol version that introduced
  it; (iii) a server sends a variant introduced at version *N* only on a
  connection whose negotiated version is at least *N*, substituting the
  nearest older shape otherwise; (iv) a client sends a request variant
  introduced at *N* only to a server that negotiated at least *N*.
- `SchemaDrivenClient::connect` sends `Hello` before `DescribeSchema`
  and exposes the negotiated version as `server_protocol_version()`.
- Golden vectors for both new variants; every existing vector unchanged
  (the append rule, demonstrated).
- `SERVER-001` v0.10.0 with a new FR-020 and the v0.9.1 open question
  resolved by pointer; `STORAGE-018`'s first revisit trigger and
  `ADR-0010`/`ADR-0021`'s compatibility notes pointed here.

**Explicitly out of scope, named directly:**

- A capability or feature list in the hello. Under the append-only rule
  the version integer *is* the capability set — monotonic, total, one
  number. A feature bitmap would let two builds at the same version
  differ, which is the situation the number exists to make impossible.
- Any change to the frame layout (`FR-002`). The length prefix, the
  `MAX_FRAME_BYTES` check, and the codec are untouched; the version
  lives in the payload as an ordinary appended variant. A per-frame
  version byte was considered and is rejected below.
- Making an old (version-1, pre-hello) *server* answer a `Hello`. It
  cannot: it drops the connection on the unknown index, as it does for
  any unknown request. There is no deployed version-1 server to protect
  (the trigger has not fired); the behavior is named as a limitation and
  the client reports it as the `FrameError::Io` it is, with a doc note.
- Version-gated *behavior* at v0.10.0. Rule (iii) above is a rule with
  no instance yet: every variant through index 9 is version 1, `Hello`
  is version 2 and only ever answers `Hello`. The first change that adds
  a version-3 response shape is the first change that has to branch on
  the negotiated version, and the rule says where.
- Per-request or mid-connection renegotiation. One hello per connection,
  as the first frame; the negotiated version holds for the connection's
  life.
- `bincode` 2.x, a smaller `Uuid` encoding, and cross-language clients
  (`PROJECT-STATUS` item 38). Each stays where `STORAGE-018` left it. A
  protocol version is the precondition for the first two — they are
  version-bumping shape changes — not the change.

## Non-goals

- Not a security mechanism. `Hello` is answered unauthenticated, as
  `Authenticate` is; it exposes one integer that `DescribeSchema` would
  expose by inference. Authentication (`FR-016`) and TLS (`FR-019`) are
  unchanged and gate everything they gated before.
- Not a change for any existing caller. A connection that never sends
  `Hello` is served exactly as at v0.9.1: `benches/server.rs`, the
  integration suites, and any hand-written client keep working
  unchanged, negotiated at version 1 by default.
- Not a performance change on the request path. One extra round trip at
  `SchemaDrivenClient::connect` (before the `DescribeSchema` it already
  pays); nothing per request; nothing in `dispatch`.
- Not a new dependency, not a `Cargo.toml` change.

## Context and terminology

### The wire today, and where the version has to go

A frame is `len: u32 LE` then `len` bytes of payload. The payload for a
`Request` is `index: u32 LE` then the variant's fields, fixint, LE
(`STORAGE-018`). `handle_connection` (`src/server/mod.rs`) reads a
frame, decodes a `Request`, intercepts `Authenticate`, gates on the
connection's `Option<TokenClass>`, and otherwise calls
`dispatch(store, req) -> Response`. Any read or decode error ends the
connection silently (`Err(_) => return`). There is no per-connection
state beyond the auth class, no first-frame requirement, and no reply
for an unparseable frame.

`SchemaDrivenClient::connect` (`src/server/client.rs`) sends
`DescribeSchema` as its first frame and keeps the `DomainSchema`. Under
an auth-configured server that first frame is refused
(`Unauthenticated`), which is a separate, pre-existing gap in the client
library — noted, not addressed here.

There are exactly three places a version could live:

1. **In the frame header** — a byte of the length prefix. `MAX_FRAME_BYTES`
   is 16 MiB = `0x0100_0000`, so the top byte of every length prefix is
   `0x00` except for a frame of exactly 16 MiB. The top byte could carry
   a version at zero wire cost. Rejected: it versions *every frame* for
   a property that is *per connection*; it caps at 255; it moves a
   shape concern into the transport layer (`FR-002`) and changes the
   framing tests and the 16 MiB edge; and an old server reads it as
   `FrameTooLarge` — no better a failure than the unknown-index drop.
2. **In an existing message** — a `protocol_version` field on
   `DomainSchema`. Rejected: adding a struct field changes
   `Response::Schema`'s bytes for every existing client (a format
   change by `STORAGE-018`'s rules, the thing this design exists to
   avoid), and `DescribeSchema` is refused before authentication, so a
   client could not learn the version before choosing how to
   authenticate.
3. **In a new message** — an appended `Request::Hello` / `Response::Hello`
   pair. Chosen. Appending is the one shape change `STORAGE-018` already
   guarantees is compatible with every existing decoder; the exchange
   is one round trip, once; and it slots into `handle_connection`
   exactly where `Authenticate` already sits.

### Why "negotiated = min", and why the server answers with a number

The client says what it was built with; the server answers with the
highest version *both* understand. Under the append-only rule that is
simply `min(client, server)`: a version-*N* build understands every
variant introduced at or below *N*, no more, no less. The client then
knows what it may send (rule iv); the server knows what it may reply
(rule iii). Neither side needs the other's *exact* version, only the
minimum, so that is what goes on the wire back.

A client newer than the server is the case the number is *for*: the
client adapts (or refuses, its choice) with a clear signal instead of a
closed socket. A client older than the server is served at its own
version. A client that says nothing is served at version 1.

### Terminology

- **Protocol version**: the `u32` that names a wire shape — the set of
  `Request`/`Response` variants (and the enums they carry) a build
  knows. Version 1 is the v0.9.1 shape; `PROTOCOL_VERSION` is the
  running build's.
- **Negotiated version**: `min(client's Hello, PROTOCOL_VERSION)` for a
  connection that sent `Hello`; 1 otherwise. Fixed for the connection.
- **Introduced at**: the protocol version in which a variant first
  existed. Recorded per variant in `protocol.rs`.
- **Append-only**: the evolution rule from `STORAGE-018`, now the wire
  shape's own — new variants go at the end, and nothing existing moves.

## Requirements

- `PROTO-FR-001`: `src/server/protocol.rs` defines
  `pub const PROTOCOL_VERSION: u32 = 2`. Version 1 is defined, in the
  module docs, as the shape at `SERVER-001` v0.9.1: `Request` indices 0–9
  (`GetById` … `Transaction`), `Response` indices 0–9 (`Record` …
  `TransactionFailed`), and every `ScanValue`, `ValueKind`, `ErrorCode`,
  `ParentLookup`, and struct as they stand. Version 2 adds `Hello` to
  both enums and nothing else.
- `PROTO-FR-002`: `Request::Hello { protocol_version: u32 }` is appended
  as index 10; `Response::Hello { protocol_version: u32 }` is appended
  as index 10. Both carry a golden vector (`0x0a, 0x00, 0x00, 0x00`
  then the `u32` LE). Every golden vector that exists at v0.9.1 is
  unchanged by this unit.
- `PROTO-FR-003`: `handle_connection` answers `Request::Hello` itself,
  before the authentication gate and without calling `dispatch`, on
  every connection — unauthenticated, `ReadOnly`, `ReadWrite`, plain or
  TLS — with `Response::Hello { protocol_version: min(client,
  PROTOCOL_VERSION) }`. A `Hello` with `protocol_version == 0` is
  answered `Response::Err { code: Malformed, .. }` and changes nothing.
  `dispatch` treats `Request::Hello` as it treats `Authenticate`:
  `ErrorCode::Unsupported`, since `handle_connection` never lets it
  through.
- `PROTO-FR-004`: A connection that never sends `Hello` is served at
  negotiated version 1, byte-for-byte as at v0.9.1. Existing integration
  suites and `benches/server.rs` pass unchanged; that is the test.
- `PROTO-FR-005`: The compatibility rules are written in `protocol.rs`'s
  module docs and in `SERVER-001`: (i) append-only — no variant or field
  of any wire type is reordered, inserted, removed, retyped, or resized
  once shipped; a change that needs one of those is a new variant,
  or a new major protocol (out of scope, unnamed); (ii) every variant
  records the version that introduced it, in a per-version table in the
  module docs, and `PROTOCOL_VERSION` is bumped by exactly one in the
  same change; (iii) the server sends a variant introduced at *N* only
  on a connection negotiated at ≥ *N*, otherwise the nearest older
  shape (a new `ErrorCode` → `Unsupported`; a new `Response` kind →
  `Response::Err { Unsupported }`; a new `ValueKind`/`ScanValue` in a
  schema → that field omitted from `DomainSchema` for that connection);
  (iv) a client sends a request introduced at *N* only after
  negotiating ≥ *N*, else it reports the gap locally without a round
  trip — the same posture `SchemaDrivenClient` already takes for
  capability checks.
- `PROTO-FR-006`: `SchemaDrivenClient::connect` sends `Request::Hello {
  protocol_version: PROTOCOL_VERSION }` before `Request::DescribeSchema`,
  keeps the negotiated version, and exposes it as
  `pub fn server_protocol_version(&self) -> u32`. A `Response` other than
  `Hello` to that frame is `ClientError::UnexpectedResponse("Hello")`.
  A pre-hello server closes the connection and the client reports
  `ClientError::Frame(FrameError::Io(..))` — named in the method's docs
  as the one way that failure presents.
- `PROTO-FR-007`: `SERVER-001` goes to v0.10.0 with FR-020 (this
  design's `PROTO-FR-001` to `-006`); its v0.9.1 open question is
  resolved by pointer; `STORAGE-018`'s "second deployed client build"
  trigger, `ADR-0010`'s compatibility note, and `ADR-0021`'s revisit
  trigger point here. `SPEC-REGISTRY` and `PROJECT-STATUS` item 33
  updated.
- `PROTO-FR-008`: No new dependency; no `Cargo.toml` change; no change
  to `src/server/framing.rs`, `MAX_FRAME_BYTES`, or `crate::codec`; no
  change to any existing `Request`/`Response` variant's bytes; no
  change outside `src/server/{protocol,mod,client}.rs` and their tests
  (plus the integration test that exercises the handshake and docs).

## Architecture and interfaces

### Considered options

**Where the version lives.**

1. *A per-frame version byte in the length prefix's top byte.* Zero
   wire cost (the byte is `0x00` today for every frame under 16 MiB).
   Rejected: it versions the transport for a property of the shape,
   caps at 255, changes `FR-002` and the framing tests, costs the exact
   16 MiB edge (`MAX_FRAME_BYTES` would have to become 16 MiB − 1), and
   an old server's failure mode (`FrameTooLarge`, connection dropped) is
   no better than the unknown-index drop it replaces.
2. *A field on `DomainSchema`.* Rejected: a struct-field change is a
   format change to `Response::Schema` for every existing client, and
   `DescribeSchema` is refused before authentication.
3. *An appended `Hello` request/response pair* — **proposed**. The one
   evolution `STORAGE-018` already guarantees compatible; one round
   trip, once per connection; answered where `Authenticate` already is.

**What the hello carries.**

4. *A version integer only* — **proposed**. Under append-only, the
   version *is* the capability set; `min` is the negotiation.
5. *A feature bitmap or list.* Rejected: it lets two builds at one
   version differ, which reintroduces the ambiguity the version removes,
   and there is no feature today that is optional within a build.
6. *Two integers, `min_supported` and `current`.* Rejected for now:
   append-only means every build supports every version from 1 to its
   own, so `min_supported` is always 1 until a major break is ever
   designed, and that design would add it then.

**Whether the hello is required.**

7. *Optional, first frame if sent; a missing hello means version 1* —
   **proposed**. Keeps every existing client (`benches/server.rs`, the
   suites, the binary's users) working unchanged; version 1 is exactly
   what they speak.
8. *Required first frame; anything else first is `Malformed`.* Rejected
   as the default: it breaks every existing client for no compatibility
   gain (the server already knows a silent client is version 1). Offered
   as the strict alternative in "Open questions".
9. *Accepted at any time, re-negotiating on each.* Rejected: a
   connection whose shape can change mid-stream is harder to reason
   about than one that cannot, and nothing needs it. A `Hello` after
   the first frame is answered `Malformed`; the negotiated version does
   not change.

**Whether to build the version-gating branch now.**

10. *Record the negotiated version per connection and branch on it in
    `handle_connection`/`dispatch` now.* Rejected: at version 2 there is
    no variant to gate (every response shape through index 9 is version
    1; `Hello` only answers `Hello`), so the branch would be dead code
    guarded by a rule, and `dispatch`'s public signature would change
    for nothing. The rule is written (`PROTO-FR-005` iii) and names where
    the branch goes when version 3 needs it.
11. *Write the rule; add the state when the first gated variant does* —
    **proposed**.

**Whether to do this at all now.**

12. *Wait for the trigger.* Rejected by the owner's choice of this unit:
    the hello a second build has to speak is cheapest to fix while
    there is one build. Nothing here is speculative beyond one integer
    and one round trip.
13. *Rule only — document append-only, no wire change.* Offered as an
    alternative in "Open questions". It fixes the *new server, old
    client* direction by discipline (never send a new response shape
    except in reply to a new request kind), but leaves the server
    unable to know what its peer understands, so a new `ErrorCode`
    could never be returned to any request — and leaves *old server,
    new client* as a silent drop.

### Proposed shape

`src/server/protocol.rs`:

```rust
/// The wire shape this build speaks — see the module docs' per-version
/// table. Bumped by exactly one in any change that appends a variant.
pub const PROTOCOL_VERSION: u32 = 2;

pub enum Request {
    // … indices 0–9 unchanged (protocol 1) …
    /// Protocol 2. Optional first frame: the client's `PROTOCOL_VERSION`.
    /// Answered by `handle_connection` before authentication with
    /// `Response::Hello` carrying `min(client, server)`.
    Hello { protocol_version: u32 },
}

pub enum Response {
    // … indices 0–9 unchanged (protocol 1) …
    /// Protocol 2. The negotiated version for this connection.
    Hello { protocol_version: u32 },
}
```

Module docs gain a **"Protocol versions"** section:

| Version | Introduced | Shape |
|---|---|---|
| 1 | `SERVER-001` v0.1.0 – v0.9.1 | `Request` 0–9, `Response` 0–9, all carried enums and structs as of v0.9.1 |
| 2 | `SERVER-001` v0.10.0 (this design) | + `Request::Hello`, `Response::Hello` |

and the four compatibility rules of `PROTO-FR-005`, next to the
`STORAGE-018` evolution rules they specialize.

`src/server/mod.rs`, `handle_connection`, before the `Authenticate`
intercept:

```rust
let mut first_frame = true;
loop {
    let req: Request = match framing::read_message(&mut reader) { … };
    if let Request::Hello { protocol_version } = &req {
        let resp = if !first_frame || *protocol_version == 0 {
            err_response(ErrorCode::Malformed)
        } else {
            Response::Hello {
                protocol_version: (*protocol_version).min(PROTOCOL_VERSION),
            }
        };
        first_frame = false;
        if !send_response(&mut writer, &resp) { return; }
        continue;
    }
    first_frame = false;
    // … Authenticate intercept, auth gate, dispatch — unchanged …
}
```

`dispatch`: `Request::Hello { .. } => err_response(ErrorCode::Unsupported)`,
alongside `Authenticate`, with the same comment.

`src/server/client.rs`, `SchemaDrivenClient`:

```rust
pub struct SchemaDrivenClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    schema: DomainSchema,
    server_protocol_version: u32,
}

// in connect(), before DescribeSchema:
framing::write_message(&mut writer, &Request::Hello { protocol_version: PROTOCOL_VERSION })?;
writer.flush().map_err(FrameError::from)?;
let server_protocol_version = match framing::read_message(&mut reader)? {
    Response::Hello { protocol_version } => protocol_version,
    _ => return Err(ClientError::UnexpectedResponse("Hello")),
};

pub fn server_protocol_version(&self) -> u32 { self.server_protocol_version }
```

No change to `framing.rs`, `dog.rs`, `order.rs`, `employee.rs`,
`pem.rs`, `src/bin/dog_server.rs`, or `benches/server.rs`.

## Data/state and invariants

- `PROTOCOL_VERSION` is a compile-time constant; there is one per build.
- Per connection: whether the first frame has been read (`bool`), which
  the `Hello` intercept consumes. No stored negotiated version at
  v0.10.0 (option 11); when a gated variant exists, the negotiated
  version joins `authenticated` as the second piece of per-connection
  state, set by the `Hello` intercept and defaulting to 1.
- Invariant: for every shipped protocol version *N* ≥ 1, a version-*N*
  decoder decodes every frame a version-*M* ≤ *N* encoder produces
  (append-only). The golden vectors are the check: a change that breaks
  an existing vector is a rule (i) violation, not a version bump.
- Invariant: `Response::Hello.protocol_version ≤ PROTOCOL_VERSION` of
  the server and `≤` the client's own `Hello`. `min` makes both true.
- Invariant: a `Hello` is answered at most once per connection, and only
  as the first frame.

## Errors, failure, recovery, and observability

- Client newer than server (both ≥ 2): `Response::Hello` carries the
  server's version; the client library exposes it and sends nothing the
  server would not decode. No error unless the caller needs a newer
  request kind, in which case it is the caller's typed decision.
- Client older than server: served at its own version; nothing to
  observe.
- Client ≥ 2, server pre-hello (version 1 without the `Hello` variant):
  the server drops the connection on the unknown index; the client sees
  EOF, `ClientError::Frame(FrameError::Io(..))`. Named in
  `SchemaDrivenClient::connect`'s docs. Not distinguishable from a
  network fault on the wire, and not worth a reconnect-without-hello
  heuristic while no such server is deployed. *Since `SERVER-001`
  v0.16.0 / FR-026, at the owner's call: `SchemaDrivenClient` reconnects
  once without a `Hello` on exactly that EOF and speaks version 1;
  `ConnectOptions::require_hello()` restores this row's behavior.*
- `Hello` with version 0, or after the first frame:
  `Response::Err { code: Malformed, .. }`; connection stays open; the
  negotiated version is unchanged (1 in the second case, since no valid
  first-frame `Hello` was seen).
- Nothing new is logged; the server logs nothing today.

## Security, privacy, and compatibility

- `Hello` is answered before authentication, as `Authenticate` is. It
  discloses one integer — the server's protocol version — to an
  unauthenticated peer. Over TLS (`FR-019`) that integer is encrypted
  like every frame; in plaintext it is as visible as the frames were.
  Any peer could already infer the version by probing request indices
  until the connection dropped, so nothing new is disclosed.
- No change to the auth gate's order of checks; the `Hello` intercept
  precedes it and touches neither `auth` nor `store`.
- Backward compatibility: total for clients (a silent client is version
  1). Forward compatibility: a pre-hello server against a hello-sending
  client is the one incompatibility, named above.
- Wire bytes: every existing frame unchanged; two new payloads of five
  bytes each (`0x0a 0x00 0x00 0x00` then `u32` LE).

## Acceptance criteria

1. `PROTOCOL_VERSION == 2`, `pub`, documented with the per-version
   table (`PROTO-FR-001`, `-005`).
2. Golden vectors: `Request::Hello { protocol_version: 2 }` →
   `[0x0a,0,0,0, 0x02,0,0,0]`; `Response::Hello { protocol_version: 2 }`
   → the same bytes; every v0.9.1 golden line unchanged (`PROTO-FR-002`).
3. Against an auth-configured server, unauthenticated: `Hello` →
   `Response::Hello`; then `DescribeSchema` → `Unauthenticated` (the
   gate is intact) (`PROTO-FR-003`).
4. `Hello { 5 }` to a server at 2 → `Response::Hello { 2 }`;
   `Hello { 1 }` → `Response::Hello { 1 }`; `Hello { 0 }` → `Err {
   Malformed }`; a second `Hello` → `Err { Malformed }` (`PROTO-FR-003`).
5. `dispatch(store, Request::Hello { .. })` → `Err { Unsupported }`
   (`PROTO-FR-003`).
6. Every existing integration suite and `benches/server.rs` pass with
   no change (`PROTO-FR-004`).
7. `SchemaDrivenClient::connect` against every domain reports
   `server_protocol_version() == 2`, and its subsequent requests work
   as before (`PROTO-FR-006`).
8. A hand-framed request with index 11 (unknown to a version-2 server)
   closes the connection — the documented pre-hello-server behavior,
   pinned from the other side (`PROTO-FR-006`'s named failure).
9. `SERVER-001` v0.10.0, FR-020, open question resolved; pointers in
   `STORAGE-018`, `ADR-0010`, `ADR-0021`, `SPEC-REGISTRY`,
   `PROJECT-STATUS` (`PROTO-FR-007`).
10. `git diff --stat` touches `src/server/{protocol,mod,client}.rs`, one
    integration test, and docs only; `Cargo.toml`/`Cargo.lock` unchanged
    (`PROTO-FR-008`).

## Verification plan

- Unit tests in `protocol.rs` (criteria 1–2), `mod.rs` (5, plus the
  intercept against `FixtureStore` via a loopback `serve` — 3–4), and
  `client.rs`/an integration test `tests/server_protocol_version.rs`
  (7–8) under `--features server`.
- The full sweep: `cargo fmt --all -- --check`; `cargo clippy
  --all-targets --all-features -- -D warnings`; `cargo test
  --all-features`; `cargo test`; `cargo doc --all-features --no-deps`
  with zero warnings.
- No benchmark: nothing on the per-request path changes;
  `benches/server.rs` is unchanged and negotiates at 1 by default.

## Traceability

- Answers `SERVER-001` v0.9.1's open question and `ADR-0010`'s
  "no explicit version negotiation" note; discharges `STORAGE-018`'s and
  `ADR-0021`'s "second deployed client build" revisit trigger ahead of
  the trigger.
- Reuses `ADR-0012`'s precedent for a request `handle_connection`
  answers before authentication.
- Specializes `STORAGE-018`'s evolution rules (append-only is compatible)
  into the wire shape's own four rules.
- Implementation: `SERVER-001` v0.10.0, FR-020, `PROTO-FR-001` to `-008`.

## Open questions

- **Acceptance question — resolved 2026-09-02: (a) accepted.** The
  three options as offered, first recommended:
  **(a)** Accept as proposed: optional first-frame `Hello`, `min`
  negotiation, `PROTOCOL_VERSION = 2`, rules written, no gating state
  until a gated variant exists. **(b)** Accept with a *required* hello:
  a first frame that is not `Hello` is `Malformed` — strict, and it
  breaks every existing client including `benches/server.rs`, for no
  compatibility gain today. **(c)** Rule only: write the append-only
  rules into `protocol.rs` and `SERVER-001`, add no `Hello`; a new
  response shape may then only ever answer a new request kind (so no
  new `ErrorCode` can ever be returned to an old request), and an old
  server still drops a new client silently.
- Whether `SchemaDrivenClient` should gain `authenticate(token)` so it
  can be used against an auth-configured server at all. Pre-existing,
  independent of this design; noted for a separate unit. *Resolved:
  `SERVER-001` v0.11.0 / FR-021 — `connect_authenticated(addr, token)`
  (the token has to reach the constructor, since `AUTH-FR-002` gates
  `DescribeSchema` itself) plus `authenticate(&mut self, token)`.*
- Whether a pre-hello server should ever be given a reconnect-without-
  hello fallback in the client library. Not while none is deployed;
  re-arm if one is. *Resolved the other way, by the owner: `SERVER-001`
  v0.16.0 / FR-026 ships it default-on, bounded to one silent reconnect
  on an EOF-class error under the `Hello`, with `require_hello()` as the
  opt-out — over the recommendation to close it.*

## Change history

- 0.1.0 (2026-09-02): Proposed (PR #124).
- 2026-09-02: Accepted as proposed (option (a); (b) and (c) declined).
  No changes to the design text beyond status. The next unit registers
  `SERVER-001` v0.10.0 / FR-020 and implements per the verification
  plan. (PR #125.)
- 2026-09-02: Implemented as `SERVER-001` v0.10.0 / FR-020 (PR #126),
  per the verification plan: acceptance criteria 1–9 hold as written;
  criterion 10 (`Cargo.toml` unchanged) holds except for the one
  `[[test]]` registration `tests/server_protocol_version.rs` needs to
  compile under default features — no dependency change. `ADR-0022`'s
  acceptance log carries the same note.
