# ADR-0021: `bincode` encoding stability — name the configuration, route every call site through one codec, pin the bytes with golden vectors

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner approved the design as proposed, option 2 with `reject_trailing_bytes()`, over the `allow_trailing_bytes()` variant and document-only; spec placement `STORAGE-018` approved over a new category; no changes requested)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md` (the full
  design document this ADR summarizes),
  `docs/decisions/ADR-0010-server-query-layer-proposal.md` (whose named
  limitation — *"`bincode`'s wire-format stability across crate versions
  is unverified for this new use"* — this answers),
  `docs/specifications/server/SERVER-001-query-layer.md` (which carries
  the same point as an open question), `PROJECT-STATUS` item 33 (the
  open-question entry) and item 68 (the owner's scoping text: *"verify
  and document what `bincode`'s default configuration pins today, and
  decide whether to pin it explicitly"*), `STORAGE-014` v0.2.0 /
  `STORAGE-015` v0.2.0 / `STORAGE-016` v0.2.0 (the three companion blobs
  whose bodies are `bincode`), `ADR-0019` (whose revisit trigger asks
  this unit whether the record *layout* needs recording), `ADR-0005` /
  `ADR-0006` (the durability round that introduced `bincode`)
- Supersedes/Superseded by: none. Resolves a limitation `ADR-0010`
  named; changes no format `ADR-0005`, `ADR-0006`, `ADR-0010`,
  `ADR-0013`, `ADR-0017`, `ADR-0018`, or `ADR-0019` produced.

## Context

Every `bincode` byte this crate writes — wire frames, the
`DOGBLOB\0`/`GENBLOB\0`/`GENEDGE\0` bodies, and the `research`-gated
WAL/snapshot/LSM files — comes from `bincode`'s *free functions*
(`serialize`, `deserialize`, `serialize_into`), with no configuration
named anywhere in `src/`. `ADR-0010` said so when the server layer was
proposed: the crate's `bincode` use *"previously only mattered within
one process's own on-disk lifetime, a materially different
compatibility bar"* than a wire protocol between builds. `SERVER-001`
carries it as an open question; `PROJECT-STATUS` item 33 has held it
open since. The three companion blobs (`STORAGE-014`–`016`) then raised
the bar a second time: they exist precisely so a file written by one
build is read by another, and their bodies are `bincode`.

The design document verified, from the vendored `bincode` 1.3.3 source
and an executed probe, what the free functions pin: `DefaultOptions::
new().with_fixint_encoding().allow_trailing_bytes()` — fixed-width
integers at their Rust width, little-endian, no size limit, trailing
bytes accepted on read. That is **not** `bincode::DefaultOptions::new()`
or `bincode::options()`, which are varint and reject trailing bytes;
`bincode`'s own docs carry a table to warn about the difference. A
`Uuid` costs 24 bytes (a `u64` length prefix of 16, then 16 bytes) in
every frame and every blob; the probe showed the same enum value at 22
bytes under the free functions and 5 under `bincode::options()`.

`bincode` 1.x's readme promises: *"The encoding format is stable across
minor revisions, provided the same configuration is used."*
`Cargo.toml`'s `bincode = "1"` keeps `cargo update` on the promise's
side of the 2.x boundary. But the promise is conditioned on *the same
configuration*, and this crate's configuration is "whatever the free
functions do" — stable by that promise and named nowhere. No test pins
a byte: every existing test is a round trip through one build, which
passes under any self-consistent encoding, including a different one.
A contributor who types `bincode::options()` at one call site produces
an incompatible format with every test green.

There are 23 production call sites: 11 front-door (`server/framing.rs`,
`durability/record_blob.rs`, `generic/record_blob.rs`,
`generic/edge_blob.rs` — the owner's *"wire protocol and the three blob
formats"* exactly) and 12 `research`-gated or research-only-reached
(`durability/mod.rs`, `hybrid.rs`, `lsm_store.rs`,
`snapshot_rebuild.rs`). Everything around the bodies — frame length
prefix, blob headers, WAL entry lengths — is already explicit,
hand-written LE.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0020` all took, and the one
`PROJECT-STATUS` item 68 asks for.

## Decision drivers

- Answer the owner's two-part question directly: what is pinned today
  (verified, not recalled), and whether to pin it explicitly (yes, with
  a reason).
- Turn `bincode`'s "same configuration" condition from a convention
  into a test. A documented format with no byte-level test is a promise
  nobody checks.
- Change no byte on disk or on the wire. `DOGBLOB\0`/`GENBLOB\0`/
  `GENEDGE\0` are at `BLOB_VERSION` 2; frames carry no version field to
  negotiate a change through.
- Zero cost: `bincode`'s `Options` combinators are compile-time ZSTs,
  and the free functions are thin wrappers over the same path.
- Make the wrong thing hard: production code should have exactly one
  way to reach `bincode`, so a divergent configuration is a review
  finding (a `grep`), not a latent incompatibility.
- No new dependency; no `bincode` 2.x migration in this round.

## Considered options

1. **Document only** — write the findings into an ADR and module docs;
   leave the 23 free-function calls. Cheapest, and it answers the first
   half of the question. Rejected as the whole answer: the promise is
   conditioned on "the same configuration", and a documented-but-unnamed
   configuration is one `bincode::options()` away from silently becoming
   a different one, with every round-trip test passing.
2. **One `pub(crate)` codec, every site routed, golden vectors** —
   proposed. `src/codec.rs` with a private `options()` =
   `DefaultOptions::new().with_fixint_encoding().with_little_endian()
   .with_no_limit().reject_trailing_bytes()` and three functions
   (`encode`, `encode_into`, `decode`) returning `bincode::Error` so
   every call site's `?`/`map_err` compiles unchanged. All 23 sites
   renamed mechanically. Golden byte vectors — primitives and
   composites, every `Request`/`Response` variant, one body each of the
   three blobs — captured on the *pre-change* code and checked in as
   hex, so the routing is proven byte-identical by the commit history.
   Writer bytes unchanged; the one behavioral change (a stricter reader
   on trailing bytes) is the acceptance question.
3. **Pin per format, switch to varint, or bump formats** — a codec per
   blob and one for frames (four places to drift for one configuration);
   or `bincode::options()`/varint (a `Uuid` at 17 bytes — and a format
   change to three blobs and the wire protocol for a size win nothing
   asked for); or a `BLOB_VERSION` 3 carrying a record-layout
   fingerprint (`ADR-0019`'s question — but serde exposes no layout
   description to hash, and the change it would guard against is
   already a version bump by the evolution rules). All rejected; varint
   is named as a revisit trigger with the golden vectors as the tool
   that would make the switch auditable.

## Decision

Accepted: option 2. Concretely, at implementation:

- `src/codec.rs` (`pub(crate)`, no feature gate): private `options()`;
  `encode<T: Serialize + ?Sized>(&T) -> Result<Vec<u8>, bincode::Error>`,
  `encode_into<W: Write, T>(W, &T) -> Result<(), bincode::Error>`,
  `decode<'a, T: Deserialize<'a>>(&'a [u8]) -> Result<T, bincode::Error>`.
  Module docs state the configuration, the format it yields, the 1.x
  stability promise and the `Cargo.toml` range that keeps it in force,
  and the evolution rules (appending an enum variant keeps existing
  bytes; reordering/inserting variants, changing struct fields or
  integer widths is a format change requiring the owning format's
  version bump).
- All 23 call sites routed: `bincode::serialize` → `codec::encode`,
  `serialize_into` → `codec::encode_into`, `deserialize` →
  `codec::decode`. After the change, `bincode::` appears in production
  `src/` only in `codec.rs` and the two error-type positions
  (`FrameError::Encoding`, `DurabilityError::Serde`).
- Golden tests, captured on pre-change `main` in a commit that precedes
  `codec.rs`: `u8`–`u64`, `i64`, `bool`, `char`, `f64`, `String`,
  `Vec<u8>`, `Option<u32>` both arms, unit/tuple/struct enum variants, a
  tuple, a struct, `Uuid` (in `codec.rs`); every `Request` and
  `Response` variant (`protocol.rs`, behind `server`); one `DOGBLOB\0`,
  one `GENBLOB\0`, one `GENEDGE\0` body (their modules). Each asserts
  `encode(&v) == GOLDEN` and `decode(GOLDEN) == v`.
- Trailing bytes: `reject_trailing_bytes()` (frames with junk after a
  valid message become `FrameError::Encoding` instead of a silent
  discard; the blobs' fingerprint already refuses such a body; research
  reads are exact-length). `allow_trailing_bytes()` was offered as the
  alternative for a purely no-behavior-change round and declined.
- Size limit stays a framing concern (`MAX_FRAME_BYTES` on the length
  prefix, before allocation); the codec is `with_no_limit()`.
- No format change: `BLOB_VERSION` 2 for all three blobs, frame layout
  unchanged, `bincode = "1"` unchanged, no public API change.
- Pointers: `ADR-0010`'s limitation, `SERVER-001`'s open question,
  `PROJECT-STATUS` item 33, and `ADR-0019`'s revisit trigger resolved by
  pointer to this ADR and the spec. `ADR-0019`'s conclusion: no layout
  fingerprint needed now; the trigger stays armed.
- A new spec carrying `BINENC-FR-001` to `-008`, registered at
  implementation as `STORAGE-018` (the codec is the storage layer's
  shared encoding; `SERVER-001` amended to cite it) — approved over a
  new category at acceptance.

## Consequences

### Positive

- The configuration is a named value in one file, not a property of
  another crate's defaults; "same configuration" is a test.
- Every front-door format has byte vectors; a change to `bincode`,
  `serde`, `uuid`'s serde impl, the configuration, or a pinned type's
  derive fails a named test that says which format moved.
- Writer bytes unchanged under either option: every existing file opens,
  every existing decode works.
- `ADR-0010`'s limitation, `SERVER-001`'s open question, and
  `PROJECT-STATUS` item 33 close with a pointer rather than a hedge.
- A `bincode` 2.x migration, if ever wanted, has its acceptance test
  ready (`config::legacy()` must reproduce the vectors).
- A documented, pinned format is the precondition for a non-Rust client
  (`PROJECT-STATUS` item 38), though not the client. *The client
  followed: `ADR-0043` / `SERVER-002` v0.1.0 — the pinned codec written
  down byte-for-byte, the golden vectors exported as an enforced fixture,
  a Python client implemented from the document; item 38 closed.*

### Negative / tradeoffs

- Option (a) is a behavioral change on frames: a valid-prefix payload
  with trailing junk is refused. No writer in this repository produces
  one; a foreign client that did would now see `FrameError::Encoding`.
- Golden vectors are maintenance: a deliberate format change (a new
  `Request` variant, a blob version bump) updates the literals, which
  is the point but is also a step contributors must know about. The
  module docs and the evolution rules are where that is written.
- The `Uuid` 24-byte cost is now pinned and documented rather than
  incidental; saving it is a format change and stays not proposed.
- The research files are routed but not pinned by test; the "one
  configuration" claim covers them, the byte-level guarantee does not.
- The wire protocol still has no version field or handshake; this pins
  the encoding *under* the `Request`/`Response` shape, not the shape.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching
  `ADR-0017`–`ADR-0020`; written from direct investigation of the
  vendored `bincode` 1.3.3 source (`lib.rs`, `config/mod.rs`,
  `config/int.rs`, `config/trailing.rs`), `uuid` 1.25.0's serde impl,
  an executed probe crate (not committed), and every one of the 23 call
  sites and their feature gates.
- **Real validation, post-acceptance**: the spec at v0.1.0; the design's
  eight acceptance criteria as tests and checks (the `grep` for
  `bincode::` outside `codec.rs`; every golden vector passing before
  and after routing; pre-change blob bodies decoding post-change; the
  trailing-junk frame test for whichever option is accepted; a
  trailing-junk blob body still a fingerprint mismatch; every existing
  test unchanged beyond the rename; the full sweep green; the four
  resolution pointers in place). No benchmark: no runtime path changes.
- Revisit if: a second deployed client build exists — a wire-protocol
  version field or hello handshake becomes `SERVER-001`'s next
  amendment and its own ADR. *Taken ahead of the trigger: `ADR-0022` /
  `SERVER-001` v0.10.0 (FR-020).*
- Revisit if: `bincode` 2.x is wanted — `config::legacy()` against the
  golden vectors is the migration's acceptance test; `standard()`
  (varint) is a format change to every blob and the protocol.
- Revisit if: id size matters on the wire or on disk — a `[u8; 16]`
  newtype for `Uuid` is a format change with a measured saving of a
  third of every id, and needs the version bumps that go with it.
- Revisit if: a struct's field layout changes between builds without
  its format's version bump — the discipline `BINENC-FR-005` relies on
  has failed and `ADR-0019`'s layout-fingerprint trigger fires.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed, with
  `reject_trailing_bytes()` (recommended); **(b)** accept with
  `allow_trailing_bytes()` — the pin with zero reader change; **(c)**
  document only — record the findings, route nothing, pin nothing.
  Also flagged: spec placement, `STORAGE-018` (recommended) versus a
  new category.
- 2026-09-02: accepted as proposed (option (a): option 2 with
  `reject_trailing_bytes()`; (b) and (c) declined; `STORAGE-018`
  approved as the spec). The next unit registers `STORAGE-018` v0.1.0
  and implements per
  `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md`.
- 2026-09-02: implemented as `STORAGE-018` v0.1.0 in PR #120 —
  `src/codec.rs` (`encode`/`encode_into`/`decode` over the one explicit
  `Options`, docs per `BINENC-FR-005`), all 23 call sites routed, golden
  vectors captured on the pre-change code in a separate first commit
  and passing unchanged after routing, the trailing-junk frame test,
  the four resolution pointers (`ADR-0010`, `SERVER-001` v0.9.1,
  `PROJECT-STATUS` item 33, `ADR-0019`). One correction to this ADR's
  claim that option (a) is "no change for the three blobs": the `Dog`
  blob decodes its body *before* fingerprinting (its fingerprint is
  over the decoded records, not the bytes), so a junk-padded
  `DOGBLOB\0` body — silently accepted before — is now a decode-cause
  `RecordBlobUnreadable`. True as stated for the two generic blobs.
  No conforming writer produces such a file; recorded in the spec's
  "Traceability".
- 2026-09-02: the one follow-up that finding opened — reorder
  `RecordBlob::read` to fingerprint before decoding, as the generic
  blobs do — examined and closed as not warranted by the owner
  (`STORAGE-018` v0.1.1, docs-only, PR #122). It is a
  `BLOB_VERSION` 2 → 3 format change, not a reorder: the `Dog` blob's
  fingerprint is over decoded, age-free content so `ProductionStore::
  open` does not rewrite the blob after `update_age`, and a byte hash
  would include the ages. Either a second header field or an
  age-zeroed body would restore that, at the cost of a design round
  whose payoff is a "fingerprint mismatch" cause in place of a
  "body does not decode" cause. Re-arm trigger in the spec.
