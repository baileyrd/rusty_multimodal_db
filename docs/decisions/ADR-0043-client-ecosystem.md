# ADR-0043: Client ecosystem — a specified, conformance-tested wire protocol and a separable client

- Status: **Proposed** — awaiting the owner's decision; options (a)–(d)
  below.
- Date: 2026-09-05
- Deciders: baileyrd
- Related: `docs/design/SERVER-CLIENT-ECOSYSTEM-DESIGN.md` (the full
  design this ADR summarizes), `ADR-0010` (named non-Rust clients as a
  Non-goal and "a non-Rust or cross-language client becomes a real
  requirement — reconsider gRPC/JSON-HTTP" as the revisit trigger this
  ADR answers), `ADR-0011` (schema discovery: solved field naming, not
  serialization), `ADR-0021`/`STORAGE-018` (the pinned codec both call
  "the precondition for a non-Rust client, not the client"), `ADR-0022`
  (the version table and rules a foreign client must speak), `ADR-0014`
  (TLS via `rusty_tls` — standard TLS, nothing proprietary in the
  transport), `PROJECT-STATUS` item 38.
- Supersedes: none. Additive; no wire or on-disk change under any
  option.

## Context

`docs/FUTURE-GROWTH.md` lists "client ecosystem — drivers for other
languages, a CLI, general tooling" and notes the project "is Rust-only
and embedded today." `ADR-0010` chose hand-rolled length-prefixed
`bincode` framing over JSON-over-HTTP and gRPC on the explicit premise
that "the only planned consumer is a Rust client speaking the same
protocol," and set a revisit trigger for the day a non-Rust client
became a real requirement. Every server round since has carried the
caveat forward without picking it up.

Reading the merged shape (`main` at `89d7c81`) reframes the question.
The wire is already one of the simplest binary formats there is: a
4-byte little-endian length, then `bincode` with fixint integers,
little-endian byte order, `u64` sequence counts, `u32` enum indices,
positional struct fields, and `Uuid` as a 16-byte length-prefixed byte
string. Every rule is written down (`src/codec.rs`'s doc comment,
`STORAGE-018`), every `Request`/`Response` variant's bytes are pinned
by 41 golden vectors, and the version table plus four compatibility
rules (`ADR-0022`) say exactly what an older or newer client sees. A
`GetById` is 32 bytes; a Python implementation is `struct.pack` and a
recursive descent. **The obstacle to a non-Rust client is not the
encoding — it is that the encoding is specified only in Rust doc
comments and a `#[cfg(test)]` module.**

Two adjacent facts the same reading surfaced: the *Rust* client is not
separable from the server either (one `server` feature compiles
`client.rs` together with `serve`, every domain adapter, the journal,
and the logs; `client.rs` imports the server's `TlsConfigError` and a
private `sql` module; the crate is `publish = false`); and the
motivating consumer, `rusty_remind_me`, is six Rust crates and two
Python helper scripts — **no non-Rust consumer exists today.**

## Decision

Adopt the design document's proposal, option (a): make the protocol a
*protocol* rather than a Rust API, without changing a byte of it.

1. **A `client` Cargo feature** (`ECO-FR-001`–`003`): `client = ["dep:
   rusty_tls"]`, `server = ["client"]`. `client` compiles `framing`,
   `protocol`, `client`, `sql`, `pem`, and `TlsConfigError`; `server`
   adds the rest. No public path changes. CI builds and tests the
   `client`-only feature set.
2. **`SERVER-002` v0.1.0, a language-neutral wire specification**
   (`ECO-FR-004`), and **a checked-in conformance fixture**, `tests/
   fixtures/wire-vectors.txt` — one `name`/`version`/`hex` line per
   golden vector, plain text, no new dependency — that the existing
   golden-vector tests *read and enforce*, so it cannot drift from the
   Rust pins without a red `cargo test` (`ECO-FR-005`/`006`).
   `SERVER-002` derives from `STORAGE-018` and `SERVER-001`; the vectors
   stay authoritative.
3. **One reference client, Python 3 standard library only**
   (`ECO-FR-007`–`009`): `clients/python/rusty_multimodal_db/`, mirroring
   `SchemaDrivenClient`'s posture (`Hello` → optional `Authenticate` →
   `DescribeSchema`; fields by name; client-side capability and version
   checks), covering the handshake, every read request, `UpdateField`,
   and `Transaction` — sessions specified but not implemented. Verified
   offline against the fixture (its own CI step) and live by a Rust
   integration test that starts a real `Entity` server and drives the
   Python client against it, including the rule-3 proof from the other
   side (`Hello { 10 }` sees three fields, `Hello { 11 }` four). The
   live test fails loudly when `python3` is absent; it never skips.

`ADR-0010`'s revisit trigger is answered: gRPC and JSON-over-HTTP were
reconsidered and are declined — a gateway is a second server with its
own auth, TLS, and versioning that makes foreign clients speak a
*different* protocol than the Rust one, and the premise that a non-Rust
client cannot speak this encoding is false.

## Consequences

- Positive: the protocol becomes implementable from a document, and a
  conformance fixture makes "implementable" testable without a Rust
  toolchain. `PROJECT-STATUS` item 38 closes; `ADR-0010`'s, `ADR-0011`'s,
  and `ADR-0021`'s caveats close with pointers.
- Positive: a Rust consumer can take the client without compiling the
  server — the precondition for ever publishing a client crate, without
  deciding to.
- Positive: zero new runtime dependency, zero wire change, zero new
  protocol to version. The fixture is plain text; `serde_json` does not
  become a dependency.
- Named, not hidden: **no consumer is asking for this.** The value is
  structural — a protocol with one implementation is an API; with a
  specification and a second implementation it is a protocol. The owner
  may reasonably judge that not worth a round now (option (d)).
- Named, not hidden: `python3` becomes a **test-time** requirement for
  `cargo test --all-features`. Not a build or runtime dependency, and
  present on every CI runner and developer machine this project has
  used — but a contributor without it sees one integration test fail by
  name rather than skip. This repository's own posture ("a failing test
  is never a flake, a skipped test is not a test") is why fail-loud is
  proposed over an environment-variable opt-in; the alternative is one
  line if the owner prefers it.
- Named, not hidden: the reference client is version-pinned. When the
  wire grows, the fixture and `SERVER-002` must grow in the same change
  (the Rust test forces the fixture; `SERVER-002` is a documentation
  obligation like `SERVER-001`'s table row); the Python client is
  updated when someone wants the new shape, and stays correct at the
  version it declares meanwhile.
- Named, not hidden: the `client` feature is a refactor of `src/server/
  mod.rs`'s gating (`#[cfg(feature = "server")]` on the server body, the
  `research` pattern applied once more). Mechanical, but the largest
  code diff of the three parts.
- No change to `Dog`/`Order`/`Employee`/`Reminder`/`Entity`, to any
  `Request`/`Response` byte, to `PROTOCOL_VERSION`, or to `SERVER-001`'s
  requirements (a patch entry records the new spec and the feature
  split).

## Considered options

The design document's own "Considered options" covers three forks.
**What interoperability means here** — (a) **(proposed)** specify the
existing wire, fixture it, prove it with one client; (b) a second JSON
encoding negotiated at `Hello` [rejected — a second protocol to version
forever, `serde_json` as a real dependency, for readability nobody
asked for]; (c) an HTTP/JSON gateway or gRPC, `ADR-0010`'s literal
revisit suggestion [reconsidered and rejected — a second server, a
different protocol for foreign clients, and a false premise]; (d) close
as not warranted [real — no non-Rust consumer exists]. **Whether a
reference client exists** — (a) **(proposed)** Python 3 stdlib-only;
(b) specification and fixture only [cheaper by half; leaves the prose
unproven]; (c) TypeScript [no runtime guarantee on CI, no consumer
favoring it]; (d) a split Rust client crate [publishing by another name;
the feature split gives the same separation in-repo]. **How the fixture
stays honest** — (a) **(proposed)** the golden-vector tests read and
enforce it; (b) a generator nothing enforces was run [rejected]; (c)
hand-transcribed hex in the spec [rejected — the drift the vectors
exist to prevent].

## Acceptance and implementation

- Options offered at proposal: **(a) accept as proposed** — all three
  parts: the `client` feature, `SERVER-002` + the enforced fixture, and
  the Python reference client with offline and live CI verification;
  **(b) accept the specification and fixture only** — `SERVER-002` and
  `tests/fixtures/wire-vectors.txt` enforced by the golden-vector
  tests, no feature split, no reference client (the protocol becomes
  documented and testable; whether it is *implementable from the
  document* stays unproven); **(c) accept (b) plus the `client` feature,
  no reference client** — everything Rust-side, nothing Python; **(d)
  close as not warranted** — no non-Rust consumer exists; item 38 stays
  open with this ADR as the record of why.
- Sizing, for the owner: (b) about half a day; (c) about a day; (a)
  about two days, the Python client and its two-layer verification
  being the other half. All as `SERVER-002` v0.1.0 plus a `SERVER-001`
  patch entry, no `SERVER-001` FR.
