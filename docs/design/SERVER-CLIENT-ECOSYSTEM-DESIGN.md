# Server Client Ecosystem: a Protocol Anyone Can Implement (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved the design as proposed, `ADR-0043` option (a): the
  `client` Cargo feature, `SERVER-002` plus the enforced conformance
  fixture, and the Python reference client with offline and live CI
  verification; (b), (c), and (d) declined). Acceptance authorizes the
  design; implementation follows as its own unit — see `ADR-0043`'s
  "Acceptance and implementation" section.
- Date: 2026-09-05
- Related: `ADR-0010`/`docs/design/SERVER-QUERY-LAYER-DESIGN.md` (named
  "not a network protocol usable by non-Rust clients" as a Non-goal and
  "a non-Rust or cross-language client becomes a real requirement" as
  the revisit trigger this round answers), `ADR-0011` (schema
  discovery — resolved field *naming* for a foreign client, explicitly
  not serialization), `ADR-0021`/`STORAGE-018`/`docs/design/
  BINCODE-ENCODING-STABILITY-DESIGN.md` (the pinned, documented codec
  that both call "the precondition for a non-Rust client, not the
  client"), `ADR-0022`/`SERVER-001-FR-020` (the version table and the
  `Hello` handshake a foreign client must speak), `ADR-0014`/
  `SERVER-001-FR-019`/`FR-022` (TLS via `rusty_tls` — standard TLS, so a
  foreign client's own TLS stack connects), `PROJECT-STATUS` item 38
  (the standing "non-Rust support remains unaddressed" note).
- Supersedes: none. Additive — no byte on the wire or on disk changes
  under any option below.

## Purpose and scope

The owner's ordered pick "3" after `ADR-0040`: the **client ecosystem**
— "drivers for other languages, a CLI, general tooling"
(`docs/FUTURE-GROWTH.md`), a gap `ADR-0010` named on day one, `ADR-0011`
and `ADR-0021` each said they were *not* closing, and `PROJECT-STATUS`
item 38 has carried unchanged ever since.

This document does what every round in this line has done first: reads
the real merged shape and asks what "a client ecosystem" would actually
require of it. The answer splits into three independent questions, and
the finding that reframes the round is that **the protocol is already
trivially implementable in any language — it is just not *specified*
anywhere a non-Rust programmer can read it.** Every byte is pinned
(`STORAGE-018`'s codec, 41 golden vectors), every rule is written
down — as Rust doc comments and a `#[cfg(test)]` module. The gap is not
a second encoding or a gateway; it is a language-neutral specification,
a conformance fixture a foreign implementation can be checked against
without a Rust toolchain, and one reference implementation that proves
the specification is sufficient.

Three questions, each answerable on its own:

1. **Can a *Rust* consumer take the client without the server?** Today,
   no — see "Context."
2. **Can a *non-Rust* program implement the protocol from what is
   written down?** Today, only by reading Rust source.
3. **Should a reference non-Rust client exist, and how is it kept
   honest?**

## Non-goals

- **Not a second wire encoding.** No JSON payload negotiated at
  `Hello`, no text mode. `bincode` with fixint/little-endian is among
  the simplest binary encodings there are (see "Context") — the
  difficulty a foreign implementer faces is finding the rules, not
  following them. A second encoding would be a second protocol to
  version forever, for a readability benefit no consumer has asked for.
- **Not an HTTP/JSON gateway or gRPC.** `ADR-0010` rejected both for
  v1 and named "a non-Rust client becomes a real requirement" as the
  trigger to *reconsider* them. Reconsidered here, in "Considered
  options," and rejected again with reasons that hold regardless of the
  option chosen — the trigger has fired and the answer is no.
- **Not publishing the crate.** `Cargo.toml` says `publish = false`; a
  Rust consumer depends on it by git. Whether that changes is the
  owner's call and not this round's — this round only makes the client
  *separable* from the server (option (a)), which is a precondition for
  publishing a client crate, not the act.
- **Not a full-coverage foreign client.** The reference client covers
  the connection handshake and the read path (`Hello`, `Authenticate`,
  `DescribeSchema`, `GetById`, `FilterEq`, `ScanField`, `Neighbors`,
  `NeighborsByRelation`, `ListRelationKinds`, `Query`, `Aggregate`) plus
  `UpdateField` and `Transaction`. Sessions (`Begin`/`BeginWith`/
  `Commit`/`Rollback`) are specified but not implemented in the
  reference client — they are the one stateful part of the protocol
  and add a `Session` object with drop semantics the reference
  implementation does not need to demonstrate that the *encoding* is
  implementable. Named, not hidden.
- **Not a CLI.** `docs/FUTURE-GROWTH.md` lists one under "client
  ecosystem"; it is a separate, Rust-side deliverable with its own
  design questions (argument shape, output format) and no dependency on
  anything here. Named for a later round.
- **Not changing `SchemaDrivenClient`'s API**, `Request`/`Response`,
  `PROTOCOL_VERSION`, or any byte anywhere.

## Context and terminology

Everything below was read from `main` at `89d7c81` (the merge of
`SERVER-001` v0.34.0), not assumed.

### The wire, as it actually is

- **Framing** (`src/server/framing.rs`): a 4-byte little-endian `u32`
  length, then that many payload bytes; frames above `MAX_FRAME_BYTES`
  (16 MiB) are refused before allocation. One request frame, one
  response frame, strictly alternating, per connection.
- **Payload encoding** (`src/codec.rs`, `STORAGE-018`): `bincode` 1.x,
  **fixint**, **little-endian**, trailing bytes rejected. The complete
  rule set fits in one list, and is already written in that module's
  doc comment: integers at natural width LE; `bool` one byte; `f64`
  eight IEEE-754 bytes LE; `String`/`Vec<T>` a `u64` count then the
  elements; `Option<T>` one byte (0/1) then the payload; structs and
  tuples as their fields in declaration order with no names or count;
  enums as a `u32` variant index then the variant's fields; `Uuid` as a
  length-prefixed byte string — `u64` 16, then 16 bytes big-endian (24
  bytes per id). That is the entire encoding. There are no varints, no
  tags, no schemas, no alignment.
- **Shape** (`src/server/protocol.rs`): `Request` has 19 variants
  (indices 0–18, `GetById` … `ListRelationKinds`), `Response` 15 (0–14,
  `Record` … `RelationKinds`), `ScanValue` 6 (`U32`/`I64`/`Bool`/`Str`/
  `F64`/`StrList`), `ValueKind` 5, `ErrorCode` 11, `CompareOp` 6,
  `AggregateFn` 5, `Selection` 2, `ParentLookup` 3, plus eight structs
  (`TransactionOp`, `Predicate`, `AggregateSpec`, `AggregateGroup`,
  `FieldCapabilities`, `FieldDescriptor`, `RelationCapabilities`,
  `DomainSchema`). `FieldRef` is `u16`; `RecordId` is `Uuid`.
- **Version table** (`protocol.rs`'s module doc, `ADR-0022`): eleven
  rows, 1 through 11, each naming the variants and flag bits it
  introduced; four compatibility rules. A client's optional first frame
  is `Hello { protocol_version }`; the server answers `min(client,
  server)`; a client that sends no `Hello` is served at version 1 — and
  therefore never sees `aliases` (`FR-044`), sessions, `Query`, or
  anything after `SERVER-001` v0.9.1.
- **Handshake order** on an authenticated server (`FR-016`/`FR-021`):
  `Hello` (optional, first), then `Authenticate { token }`, then
  anything. `DescribeSchema` itself is behind the gate.
- **Transport security** (`FR-019`/`FR-022`/`FR-023`): `rusty_tls`
  wraps `rustls` — standard TLS with SNI, optional client certificates
  for mTLS. Any language's standard TLS library connects; there is
  nothing proprietary in the transport.
- **Pinned bytes**: 41 `assert_golden`/`assert_golden_eq` call sites in
  `protocol.rs`'s `#[cfg(test)]` module pin every `Request` and
  `Response` variant's exact bytes (`BINENC-FR-004`), plus every
  `ErrorCode` through `Err`. They are Rust `const` slices in a test
  module — authoritative, and invisible to anyone not reading Rust.

### Who the consumers are

- **`rusty_remind_me`** (`baileyrd/rusty_remind_me@29602f1`, the clone
  `ADR-0042` read): six Rust crates (`remind_me_api`, `_cli`, `_core`,
  `_hub`, `_mcp`, `_remote`) and two Python helper scripts
  (`scripts/configure_mcp.py`, `scripts/regenerate_schema.py`). **No
  non-Rust consumer exists in the motivating project.** Stated plainly,
  because it is the strongest argument for option (d).
- **A Rust consumer today** must depend on `rusty_multimodal_db` by git
  (`publish = false`) with `features = ["server"]` — the only feature
  that compiles `src/server/`, where `client.rs` lives alongside
  `serve`, `dispatch`, `ServeOptions`, the journal, the audit and access
  logs, and every domain adapter. `client.rs` imports `super::{pem,
  TlsConfigError}` (the *server's* TLS-config error type, reused for
  `ClientTlsConfig::with_identity`'s PEM loading) and `super::sql` (a
  private module). There is no `client` feature. Taking the client means
  taking the whole server — and `rusty_tls`, the crate's one git
  dependency, which is fine (the client needs it for TLS) but is bundled
  with everything else.

### Why "trivially implementable" is a fact, not a hope

A `GetById` for `Uuid::from_u128(1)` is 32 bytes:
`1c 00 00 00` (frame length 28) · `00 00 00 00` (variant 0) ·
`10 00 00 00 00 00 00 00` (16) · sixteen id bytes. A `Hello { 11 }` is
`08 00 00 00 · 0a 00 00 00 · 0b 00 00 00`. A Python implementation of
the entire codec for these shapes is `struct.pack`/`struct.unpack` with
`<I`, `<Q`, `<q`, `<H`, `<d`, `<?` and a recursive descent over the
type layout — no library, no code generation. The 41 golden vectors
are the test suite such an implementation needs, already written; they
just need to be readable outside `cargo test`.

## Requirements

Requirement IDs are `ECO-FR-nnn`. `ECO-FR-001`–`003` are option (a)'s
first third and stand alone; `004`–`006` are the specification third
(also option (b) in full); `007`–`009` are the reference client.

- `ECO-FR-001` — **A `client` Cargo feature.** `client = ["dep:rusty_
  tls"]`; `server = ["client"]`. `client` compiles exactly `src/server/
  {framing,protocol,client,sql,pem}.rs` and the TLS-config *error* type
  those need; `server` adds everything else. A Rust consumer that wants
  only to talk to a server depends on `features = ["client"]` and gets
  no `serve`, no domain adapters, no journal.
- `ECO-FR-002` — **The `client`-visible surface moves out from under
  `serve`'s own types**: `TlsConfigError` and `pem` become
  `client`-gated (they are file-loading helpers, not server logic —
  `ServeOptions`/`TlsConfig` keep using them under `server`). No public
  path changes: `rusty_multimodal_db::server::client::SchemaDrivenClient`
  stays where it is, `rusty_multimodal_db::server::TlsConfigError`
  stays where it is. The `server` module itself exists under `client`,
  with its server-only items `#[cfg(feature = "server")]`-gated inside
  it — the pattern `research` already uses for `order`/`employee`.
- `ECO-FR-003` — **`cargo build --features client` compiles and `cargo
  test --features client` runs the client-side tests** (`protocol.rs`'s
  golden vectors, `framing.rs`, `sql.rs`, `client.rs`'s unit tests)
  with no server compiled. `cargo build` with no features is unchanged.
  CI gains one build step for the `client`-only feature set.
- `ECO-FR-004` — **A language-neutral wire specification**, registered
  as **`SERVER-002` v0.1.0** (`docs/specifications/server/SERVER-002-
  wire-format.md`). Byte-level, written for a reader with no Rust: the
  framing; every primitive encoding; every enum's variant table (name,
  index, fields in order) and every struct's field table (name, type,
  order), transcribed from `protocol.rs` at a stated `PROTOCOL_VERSION`;
  the version table and the four compatibility rules restated in
  protocol terms; the connection lifecycle (`Hello` first or never,
  `Authenticate` before anything on an authenticated server, one
  request → one response, the two server-closes-without-reply cases —
  an unknown request index and a frame over 16 MiB); every `ErrorCode`'s
  meaning; TLS as "standard TLS, SNI required, optional client
  certificate." It *derives from* `STORAGE-018` and `SERVER-001` and
  says so — it adds no rule of its own; a conflict between it and the
  golden vectors is a bug in the specification.
- `ECO-FR-005` — **A machine-readable conformance fixture**, `tests/
  fixtures/wire-vectors.txt`: one line per golden vector, `<name>\t<hex
  bytes>`, plain text, no JSON (no new dependency — `serde_json` is not
  a direct dependency of this crate and would not become one). Produced
  and *checked* by the existing golden-vector tests: the `assert_golden`
  helpers additionally look each vector up in the checked-in file and
  fail if it is absent or differs, so the file can never drift from the
  Rust pins. A stale file is a test failure, not a warning.
  Regeneration is one documented command (a test behind an environment
  variable that rewrites the file, the same shape `cargo insta`-style
  workflows use, with no dependency).
- `ECO-FR-006` — **`SERVER-002` names the fixture as its test.** A
  foreign implementation is conformant at a version when it encodes
  every request vector and decodes every response vector in the fixture
  byte-for-byte. The fixture's vectors are tagged with the protocol
  version that introduced each shape (the table already has this), so a
  client targeting version *N* knows which vectors apply.
- `ECO-FR-007` — **One reference client, Python 3, standard library
  only**: `clients/python/rusty_multimodal_db/` (a package: `codec.py`,
  `protocol.py`, `client.py`), no PyPI dependencies, `ssl` for TLS,
  `socket` for TCP, `struct`/`uuid` for encoding. It mirrors
  `SchemaDrivenClient`'s posture — `Hello` then optional `Authenticate`
  then `DescribeSchema` at connect; fields by name; capability checks
  client-side before sending; `server_protocol_version` exposed; every
  version-gated request refused locally below its version — and covers
  the shapes the Non-goals list. It is *reference*, not *product*: no
  packaging to PyPI, no async, no connection pooling.
- `ECO-FR-008` — **Two-layer verification, both in CI.** (i) Offline:
  `clients/python/tests/test_vectors.py` encodes/decodes every line of
  `tests/fixtures/wire-vectors.txt` — no server, no Rust, runs anywhere
  with `python3`. (ii) Live: a Rust integration test, `tests/server_
  python_client.rs` (`required-features = ["server"]`), starts a real
  `Entity` server on a loopback port, spawns `python3` running a small
  driver script against it, and asserts the driver's output: a
  `GetById` with four fields including the `StrList`, a `FilterEq` on
  `label`, a `Query`, an `Aggregate`, an `UpdateField`, and — the rule-3
  proof from the other side — the same `GetById` through a driver that
  says `Hello { 10 }` returning three fields. **The live test fails
  loudly if `python3` is not on `PATH`** (this repository's own posture:
  a test that skips is not a test; `ubuntu-latest` ships `python3`).
  The Python test file also runs as its own CI step so its failure is
  attributed to the client, not to Rust.
- `ECO-FR-009` — **The reference client is version-pinned, not
  version-chasing.** It declares the `PROTOCOL_VERSION` it was written
  against and speaks it in `Hello`; a newer server serves it at that
  version by rule 3, exactly as it would any other older client. When
  `protocol.rs` appends a variant, the fixture gains vectors (the Rust
  test forces it), `SERVER-002` gains a row, and the Python client is
  *updated in a later change or not* — its own tests still pass against
  the vectors of the version it declares. The offline test filters the
  fixture by the client's declared version.

## Considered options

**Fork 1 — What "non-Rust interoperability" means for this protocol.**

- **(a) (proposed)** A language-neutral specification of the *existing*
  wire (`SERVER-002`), a checked-in conformance fixture derived from
  the existing golden vectors, and one stdlib-only reference client
  validated against both the fixture and a live server. Zero wire
  change, zero new runtime dependency, zero new protocol to version.
- (b) A second payload encoding (JSON) selected by a flag in `Hello` or
  by a magic byte. Rejected: `Hello` is a `u32` and its frame is
  already pinned; a magic byte before the length prefix changes every
  frame; either way a second encoding doubles the golden-vector
  surface, needs `serde_json` as a real dependency, and buys
  human-readability nobody has asked for. `bincode`'s difficulty is
  discoverability, which (a) fixes directly.
- (c) An HTTP/JSON gateway binary (`src/bin/gateway.rs`) translating to
  the binary protocol, or gRPC. `ADR-0010`'s own revisit trigger says to
  reconsider these when a non-Rust client becomes a real requirement.
  Reconsidered and rejected: a gateway is a second server with its own
  auth, TLS, versioning, and failure modes, plus `hyper`/`axum`/
  `serde_json` (or `tonic`/`prost` and a `.proto` that duplicates
  `protocol.rs`); it makes non-Rust clients speak a *different*
  protocol than the Rust one, so the two drift. Every argument
  `ADR-0010` gave against them at v1 still holds, and the one argument
  for — "a non-Rust client can't speak bincode" — is false for this
  encoding (see "Context").
- (d) Close as not warranted: no non-Rust consumer exists;
  `rusty_remind_me` is Rust. Real, and the owner may pick it. What it
  leaves standing: the protocol remains a Rust API with a byte format
  as an implementation detail, `PROJECT-STATUS` item 38 stays open, and
  the golden vectors stay Rust-only.

**Fork 2 — Whether a reference client should exist, and in what.**

- **(a) (proposed)** Yes, one, Python 3 stdlib-only. Python because it
  is on every CI runner and developer machine this project has used,
  needs no build step, and `struct` maps one-to-one onto the fixint
  rules. Stdlib-only so the reference has no dependency story of its
  own and stays a *specification test*, not a product.
- (b) Specification and fixture only — no reference client (this is the
  whole of option (b) in the ADR). Cheaper by roughly half; the risk it
  accepts is that the specification is never proven sufficient by
  anyone but its author. The fixture mitigates but does not close that
  — a fixture proves bytes, not that the *prose* is complete (handshake
  order, close-without-reply cases, version gating).
- (c) TypeScript/Node. Rejected for the reference: needs a runtime the
  CI image does not guarantee and a build step; nothing in the
  consumer landscape favors it over Python. Named as an obvious second
  client if a browser/Node consumer ever appears.
- (d) A second *Rust* crate (`rusty_multimodal_db_client`) split out of
  this repository. Rejected for this round: it is publishing by another
  name (Non-goals), and `ECO-FR-001` gets a Rust consumer the same
  separation inside this crate with no new repository.

**Fork 3 — How the fixture stays honest.**

- **(a) (proposed)** The existing golden-vector tests *read* the
  checked-in file and fail on any difference; regeneration is an
  explicit, environment-gated rewrite. Drift is impossible without a
  red test.
- (b) A `build.rs` or a separate binary that generates the file on
  demand. Rejected: nothing enforces that it was run; the file could
  be stale on `main`.
- (c) No fixture — `SERVER-002` transcribes the hex by hand. Rejected:
  41 vectors of hand-copied hex is exactly the drift the golden vectors
  exist to prevent.

## Proposed shape

### `Cargo.toml` (`ECO-FR-001`)

```toml
[features]
research = []
# The client half of `src/server/` alone — framing, protocol, the
# schema-driven client, its SQL front end, PEM loading, and `rusty_tls`
# for the client side of TLS. A consumer that only talks to a server
# depends on this and compiles no `serve`. ADR-0043, ECO-FR-001.
client = ["dep:rusty_tls"]
# The network server/query layer — everything `client` has, plus
# `serve`/`dispatch`, `ServeOptions`, the journal, the audit and access
# logs, and every domain adapter.
server = ["client"]
```

```rust
// src/lib.rs
#[cfg(feature = "client")]
pub mod server;

// src/server/mod.rs — module list
pub mod client;                                   // client
pub mod framing;                                  // client
mod pem;                                          // client
pub mod protocol;                                 // client
mod sql;                                          // client
#[cfg(feature = "server")] pub mod access;
#[cfg(feature = "server")] pub mod audit;
#[cfg(feature = "server")] pub mod dog;
#[cfg(all(feature = "server", feature = "research"))] pub mod employee;
#[cfg(feature = "server")] pub mod entity;
#[cfg(feature = "server")] pub mod journal;
#[cfg(all(feature = "server", feature = "research"))] pub mod order;
#[cfg(feature = "server")] pub mod reminder;
// `TlsConfigError` stays `pub` at `server::TlsConfigError`, ungated;
// `TlsConfig`, `ServeOptions`, `serve`, `dispatch`, `ConnectionStore`
// and the rest of mod.rs's server body gain `#[cfg(feature = "server")]`.
```

Every `[[bin]]`/`[[test]]`/`[[bench]]` that says `required-features =
["server"]` is unchanged — `server` implies `client`. Two new `[[test]]`
targets: `server_client_only` (`required-features = ["client"]`, proves
the client-only build by connecting to nothing and exercising the codec
and `sql`), and `server_python_client` (`required-features =
["server"]`).

### `SERVER-002` (`ECO-FR-004`), table of contents

1. Scope and relationship to `SERVER-001`/`STORAGE-018` (derivative,
   not authoritative — the golden vectors are).
2. Transport: TCP; TLS (standard; SNI; optional client certificate);
   `TCP_NODELAY` recommended (the Nagle finding, `SERVER-001-FR-006`).
3. Framing: `u32` LE length, 16 MiB cap, one-in-one-out.
4. Primitive encodings (the `codec.rs` list, verbatim in prose, with
   one worked example each).
5. Types: every enum (variant, index, fields) and struct (field, type)
   at `PROTOCOL_VERSION` 11, with the version that introduced each.
6. Connection lifecycle: `Hello`; version negotiation; silent = 1;
   `Authenticate`; the two close-without-reply cases; per-connection
   session state (specified, with the rule-4 note that a client below 3
   never sends them).
7. Semantics per request (one paragraph each, pointing at the
   `SERVER-001` FR that owns it).
8. Compatibility rules 1–4 restated for an implementer: what an older
   client sees (rule 3, including the `FR-044` content strip), what a
   newer client must not send (rule 4).
9. Conformance: the fixture file, its format, and the statement in
   `ECO-FR-006`.
10. Change history, versioned with `SERVER-001` (a `SERVER-001` minor
    that changes the wire bumps `SERVER-002`'s minor in the same
    change; `SERVER-002` never moves alone).

### The fixture (`ECO-FR-005`)

```text
# tests/fixtures/wire-vectors.txt — generated; see SERVER-002 §9.
# name<TAB>introduced-at-version<TAB>hex
Request/GetById	1	000000001000000000000000000000000000000000000000000000000000000001
Request/Hello	2	0a0000000b000000
Response/Record(StrList)	11	0000000010...
```

```rust
// src/server/protocol.rs tests — the existing helpers, extended
fn assert_golden<T: Serialize>(name: &str, value: &T, expected: &[u8]) {
    let encoded = crate::codec::encode(value).unwrap();
    assert_eq!(encoded, expected, "{name}");
    fixture::check(name, expected);          // new: fail if the file disagrees
}
```

`fixture::check` looks the name up in the checked-in file and compares
hex; when `RMDB_REGENERATE_VECTORS=1` is set it instead records the
vector, and a final test writes the collected set back to the file. The
file is committed; a change to any vector without regenerating it is a
red `cargo test`.

### The reference client (`ECO-FR-007`), surface

```python
from rusty_multimodal_db import Client, ScanValue

c = Client.connect("127.0.0.1:7878", token=None, tls=None)   # Hello(11) → DescribeSchema
c.server_protocol_version        # 11
c.schema                         # DomainSchema: fields[], relations
c.get(uuid.UUID(int=1))          # [("label", "Ada Lovelace"), ..., ("aliases", ["Ada", ...])] or None
c.filter_eq("label", "ada")      # [UUID, ...]          (client-side capability check first)
c.scan("mention_count")          # [3, 5, ...]
c.neighbors(id, relation=None)   # [UUID, ...]
c.query("SELECT label FROM entity WHERE kind = 'person'")   # rows, via the same
                                 # client-side SQL → Request::Query compile as the Rust client
c.update(id, "mention_count", 4) # True/False
c.transaction([(id, "mention_count", 4), ...])
```

The Python `query` reuses the *grammar* `src/server/sql.rs` accepts
(`SELECT` list, `FROM`, `AND`-only `WHERE`, `GROUP BY`, `LIMIT`) —
`SERVER-002` §7 restates that grammar because the server never sees SQL
text, only the compiled `Request::Query`/`Aggregate`; a foreign client
with no SQL front end simply builds those requests directly.

## Data/state and invariants

- No on-disk change, no wire change, no `PROTOCOL_VERSION` change under
  any option. `SERVER-002` describes; it does not decide.
- Invariant (option (a)/(b)): the fixture file and the Rust golden
  vectors are byte-identical — enforced by `cargo test`, not by
  convention.
- Invariant (option (a)): `cargo build --features client` compiles no
  `serve`, `dispatch`, `ConnectionStore`, or domain adapter — pinned by
  the `server_client_only` test target existing and by CI building the
  feature set alone.
- Invariant: the reference client never sends a request introduced at a
  version above the one it negotiated (rule 4), and never needs to
  handle a variant above it (rule 3) — the same two-sided guarantee the
  Rust client relies on.

## Errors, failure, recovery, and observability

- A foreign client that sends malformed bytes gets exactly what the
  Rust client would: `Response::Err { Malformed, .. }` for a decodable
  frame with a bad payload, or the connection closed with no reply for
  an undecodable request index or an oversized frame
  (`SERVER-001-FR-004`, `PROTO-FR-008`). `SERVER-002` §6 states both.
- The fixture test's failure mode is a diff of hex lines, named by
  vector — the same readability the golden vectors already have.
- The live Python test's failure mode is the driver's stderr surfaced
  in the Rust test's panic message; a missing `python3` is a distinct,
  named panic, not a skip.

## Security, privacy, and compatibility

- Nothing about authentication, authorization, TLS, rate limiting, or
  audit changes. A foreign client is subject to every gate the Rust
  client is — the gates are the server's (`FR-016`–`FR-033`), and
  `SchemaDrivenClient`'s client-side checks were never a trust boundary
  (`client.rs`'s own doc says so).
- `SERVER-002` documents the handshake order a foreign client must
  follow to avoid sending a token in plaintext: on a TLS server, TLS
  first, then `Hello`, then `Authenticate` — the same order `connect_
  authenticated` enforces in Rust (`FR-022`).
- The `client` feature (`ECO-FR-001`) narrows what a consumer compiles;
  it widens nothing. `server` builds are byte-identical to today.
- Compatibility rules 1–4 are unchanged and restated, not re-decided.

## Acceptance criteria

1. (option (a)) `cargo build --features client` and `cargo test
   --features client` succeed with no server module compiled; `cargo
   build --features server` is unchanged; every existing test/bench/bin
   target builds as before; CI runs the `client`-only build.
2. (options (a)/(b)) `docs/specifications/server/SERVER-002-wire-
   format.md` exists at v0.1.0, registered in `SPEC-REGISTRY`, and
   contains every table `ECO-FR-004` lists at `PROTOCOL_VERSION` 11; a
   reader can encode `Hello`, `Authenticate`, `DescribeSchema`, and
   `GetById` from it alone (checked by a reviewer doing exactly that
   against the fixture, recorded in the implementation log).
3. (options (a)/(b)) `tests/fixtures/wire-vectors.txt` exists, has one
   line per golden vector (41 at v0.34.0), and `cargo test` fails if
   any line is edited.
4. (option (a)) `clients/python/tests/test_vectors.py` passes offline
   against the fixture with `python3 -m unittest`, no Rust required.
5. (option (a)) `tests/server_python_client.rs` passes: the Python
   driver, against a real `Entity` server, reads four fields including
   the `StrList` at version 11 and three at `Hello { 10 }`, resolves an
   alias via `FilterEq`, runs a `Query` and an `Aggregate`, applies an
   `UpdateField` the Rust client then observes; and the test panics
   with a named message when `python3` is absent.
6. (option (a)) The Python client refuses `begin`-shaped calls and every
   request above its declared version locally, with no frame sent.
7. (all options) No byte of any existing golden vector, blob, or file
   changes; `PROTOCOL_VERSION` stays 11; `SERVER-001` stays v0.34.0 (a
   `SERVER-001` *patch* entry records the new spec's existence and the
   feature split, since neither changes a requirement of `SERVER-001`).

## Verification plan

- `cargo test --features client` (new CI step), `cargo test
  --all-features` (existing), `cargo doc --all-features --no-deps` at
  the 64-warning baseline.
- `python3 -m unittest discover clients/python/tests` as its own CI
  step, before the Rust steps that need it.
- The live test in `tests/server_python_client.rs` under `cargo test
  --all-features`.
- A manual, recorded check for criterion 2: encode the four handshake
  frames by hand from `SERVER-002` alone and compare to the fixture.

## Traceability

- → a new spec, `SERVER-002` v0.1.0 (options (a)/(b)); a `SERVER-001`
  patch entry (no FR — nothing in `SERVER-001`'s requirements changes);
  `ADR-0043`. Closes `ADR-0010`'s non-Rust Non-goal and revisit trigger,
  `ADR-0011`'s and `ADR-0021`'s "not the client" caveats, and
  `PROJECT-STATUS` item 38.
- Sourced from `docs/FUTURE-GROWTH.md`'s "Client ecosystem — drivers
  for other languages, a CLI, general tooling" — the first round in
  this session to come from that document rather than from the
  `rusty_remind_me` line; the CLI third of it is named for later.

## Open questions

- Whether to publish the crate (or a split client crate) to crates.io —
  `publish = false` today; the owner's call, and the feature split is
  its precondition, not its trigger.
- Whether a second reference client (TypeScript) is ever wanted — only
  if a browser/Node consumer appears; the spec and fixture make it a
  weekend, not a round.
- Whether `SERVER-002` should carry a formal grammar for the SQL subset
  or only restate it — the server never sees SQL, so it is a client
  convenience; restating is proposed, a grammar is not.
- Whether the CLI (`docs/FUTURE-GROWTH.md`'s other named item) should
  be built on the Rust client under the new `client` feature — natural,
  and a separate round.

## Change history

- 2026-09-05: Initial proposal, the owner's ordered pick "3" (client
  ecosystem / non-Rust interoperability) — the first round sourced from
  `docs/FUTURE-GROWTH.md` in this session. Three independent questions
  (client separable from server; protocol specified for a non-Rust
  reader; a reference client kept honest by a fixture), one proposed
  answer to each, `ADR-0010`'s gRPC/JSON revisit trigger reconsidered
  and declined with reasons.
- 2026-09-05: **Accepted** as proposed, `ADR-0043` option (a) — all
  three parts authorized as one implementation unit; (b), (c), and (d)
  declined. No changes to the design text. (PR #192.)
