# STORAGE-018 — bincode encoding stability (one `pub(crate)` codec, golden vectors, trailing bytes rejected)

- Version: 0.1.1
- Status: Accepted
- Owners: baileyrd
- Depends on: ADR-0021 and `docs/design/BINCODE-ENCODING-STABILITY-DESIGN.md`
  (both Accepted — the design this spec turns into requirements),
  `STORAGE-008`/`STORAGE-009` (the durability variants whose
  WAL/snapshot/SST files the codec now encodes),
  `STORAGE-014`/`STORAGE-015`/`STORAGE-016`
  (the three companion blobs whose bodies it encodes), `SERVER-001`
  (the wire protocol whose frames it encodes; amended to cite this
  spec)
- Supersedes: none. Changes no on-disk or on-wire byte.

## Purpose and scope

Every `serde`-encoded byte this crate writes — the `DOGBLOB\0`,
`GENBLOB\0` and `GENEDGE\0` blob bodies, the research durability
variants' snapshot/WAL/SST files, and every `Request`/`Response` frame
— was produced by `bincode`'s free functions (`bincode::serialize`,
`serialize_into`, `deserialize`), 23 production call sites in all.
Those functions apply a configuration nobody in this crate had written
down (fixint integers, little-endian, no size limit, trailing bytes
allowed), and `bincode` 1.x's stability promise holds only *"provided
the same configuration is used"*. `ADR-0010` and `SERVER-001` both
recorded the wire format's cross-version stability as unverified;
`PROJECT-STATUS` item 33 carried the same open item.

This spec adds one `pub(crate)` module, **`src/codec.rs`**, that
states the configuration explicitly as a `bincode::Options` value and
exposes `encode`/`encode_into`/`decode` over it; routes all 23 sites
through it; pins the resulting bytes with golden vectors captured on
the pre-change code; and — the one behavioral change, accepted as
option (a) — rejects a payload with bytes left over after the value.
The eight requirements below are the design document's
`BINENC-FR-001..008`, kept under their design names since the spec
adds none.

## Non-goals

- Not a format change. No `BLOB_VERSION`, slot-file version, or
  protocol shape changes; the bytes written are identical to before.
- Not a `bincode` 2.x migration. The pinned configuration corresponds
  to 2.x's `config::legacy()`; the golden vectors are that migration's
  acceptance test if it ever happens.
- Not wire-protocol versioning. Frames still carry no protocol version
  and there is no hello handshake — a `Request`/`Response` shape change
  remains a named incompatibility, `SERVER-001`'s own concern.
- Not a smaller `Uuid` encoding. Every id still costs 24 bytes (an
  8-byte length prefix plus 16); recorded, not changed.
- Not a layout fingerprint on the blobs (`ADR-0019`'s revisit trigger).
  The evolution rules the codec documents make a layout change a
  version bump by discipline; the trigger stays armed.
- Not a public API. The codec is `pub(crate)`; `FrameError::Encoding`
  and `DurabilityError::Serde` keep `bincode::Error` as their payload.

## Context and terminology

- **Free functions**: `bincode::serialize`/`serialize_into`/
  `deserialize`, which in 1.3.3 are `DefaultOptions::new()
  .with_fixint_encoding().allow_trailing_bytes()` — fixint,
  little-endian, no limit, trailing bytes ignored.
- **The codec**: `crate::codec`, whose private `options()` is
  `DefaultOptions::new().with_fixint_encoding().with_little_endian()
  .with_no_limit().reject_trailing_bytes()`, and whose three functions
  are the only way production code touches `bincode`.
- **Golden vector**: a byte literal checked in beside the value that
  must encode to it, captured on the code as it stood *before* the
  codec existed (commit `4ae86bd`), so a post-routing pass proves the
  codec reproduces the free functions' bytes rather than its own.
- **Trailing bytes**: bytes left in a payload after `T` is fully
  decoded. Under the free functions they were ignored; under the codec
  they are a decode error (`bincode`'s "Slice had bytes remaining after
  deserialization").
- **Format**: fixint integers at their natural width, little-endian;
  `bool` one byte; `f64` eight bytes of IEEE 754 bits; `char` its UTF-8
  bytes; sequences and strings a `u64` count then elements; `Option` a
  one-byte discriminant then payload; structs/tuples their fields in
  order; enums a `u32` variant index then fields; `Uuid` a
  length-prefixed 16-byte string (24 bytes).

## Requirements

- `BINENC-FR-001`: `src/codec.rs`, `pub(crate)`, defines the one
  `bincode::Options` this crate encodes and decodes with (see
  "Context") and exposes `encode<T: Serialize + ?Sized>(&T) ->
  Result<Vec<u8>, bincode::Error>`, `encode_into<W: Write, T:
  Serialize + ?Sized>(W, &T) -> Result<(), bincode::Error>`, and
  `decode<'a, T: Deserialize<'a>>(&'a [u8]) -> Result<T,
  bincode::Error>`. The error type is unchanged, so every call site's
  `?`/`map_err` compiles as is.
- `BINENC-FR-002`: the bytes `encode`/`encode_into` produce are
  identical to what the free functions produced, for every `T` —
  proven by the golden vectors of `BINENC-FR-004`, captured on the
  pre-change code and passing unchanged after routing.
- `BINENC-FR-003`: no production code in `src/` calls
  `bincode::serialize`, `deserialize`, `serialize_into`,
  `deserialize_from`, `options()`, or `DefaultOptions` outside
  `src/codec.rs`; all 23 sites route through the codec. Test modules
  may use the free functions to *state* an expectation (the codec's
  own equivalence test does).
- `BINENC-FR-004`: golden tests pin, in `codec.rs`, one vector each for
  `u8`, `u16`, `u32`, `u64`, `i64`, `bool`, `char`, `f64`, `String`,
  `Vec<u8>`, `Option<u32>` (both arms), a unit-, tuple- and
  struct-variant enum, a tuple, a struct, and `Uuid`; in
  `src/server/protocol.rs` (behind `server`), every `Request` and
  `Response` variant with one representative value; in
  `src/durability/record_blob.rs`, one `DOGBLOB\0` body holding one
  record and one edge; in `src/generic/record_blob.rs`, one `GENBLOB\0`
  body holding one `Order`; in `src/generic/edge_blob.rs`, one
  `GENEDGE\0` body holding one `(Uuid, Uuid)` pair. Each asserts
  `encode(&v) == GOLDEN` and that `GOLDEN` decodes back to `v` (by
  `PartialEq` where the type has it, by re-encoding where it does not
  — `Request`/`Response`).
- `BINENC-FR-005`: the codec's module docs state the configuration,
  the format, the `bincode` 1.x stability promise and the `Cargo.toml`
  range (`bincode = "1"`) that keeps it in 1.x, and the evolution rules
  — appending an enum variant is compatible; reordering or inserting
  variants, adding/removing/reordering struct fields, changing a
  field's type or an integer's width is a format change taking the
  owning format's version bump (`BLOB_VERSION`; the slot-file header
  version; a `SERVER-001` amendment for the wire protocol).
- `BINENC-FR-006`: **option (a), accepted** — `decode` rejects trailing
  bytes. A frame whose payload is a valid message plus trailing junk
  is `FrameError::Encoding` from `read_message` (previously silently
  accepted). The two generic blobs are unaffected (their byte
  fingerprint refuses such a body first); the research files are
  single-writer, exact-length reads. See "Traceability" for the
  `DOGBLOB\0` case the design did not anticipate.
- `BINENC-FR-007`: `ADR-0010`'s limitation, `SERVER-001`'s open
  question, and `PROJECT-STATUS` item 33 are resolved by pointer to
  `ADR-0021`, this spec, and the codec; `ADR-0019`'s revisit trigger
  records this unit's conclusion (no layout fingerprint now; trigger
  stays).
- `BINENC-FR-008`: no new dependency; `Cargo.toml`'s `bincode = "1"`
  unchanged; no format version bump; no public API change.

## Architecture and interfaces

- `src/codec.rs` (new, `pub(crate)`): module docs per `BINENC-FR-005`;
  private `fn options() -> impl Options`; `encode`, `encode_into`,
  `decode`. Tests: the primitive/enum/struct/`Uuid` golden vectors
  (7 tests), the trailing-bytes-rejected + free-function-equivalence
  test, and an `encode_into == encode` test.
- `src/lib.rs`: `pub(crate) mod codec;`.
- `src/test_support.rs`: `#[cfg(test)]` helpers `hex_literal` (bytes as
  a paste-ready `0x..` literal body), `assert_golden` (encode ==
  golden; golden decodes to a value that re-encodes to golden) and
  `assert_golden_eq` (plus `decode(golden) == value`), both `#[track_
  caller]` and printing the actual bytes as a literal on drift.
- The 23 routed sites, unchanged in behaviour and error mapping:
  `src/server/framing.rs` (`write_message`, `read_message`);
  `src/generic/record_blob.rs` and `src/generic/edge_blob.rs`
  (fingerprint via `encode_into` into `Fnv1a64`, `encode` for the
  image, `decode` on read); `src/durability/record_blob.rs` (`encode`,
  `read`, and the test-only `write_legacy_v1`);
  `src/durability/mod.rs` (WAL append/replay, `CanonicalOnlySnapshot`
  round trip); `src/durability/snapshot_rebuild.rs`,
  `src/durability/hybrid.rs`, `src/durability/lsm_store.rs` (research
  snapshot/SST/memtable files).
- New tests beyond the vectors: `framing.rs`
  `a_frame_with_bytes_after_the_message_is_an_encoding_error`
  (criterion 4); `generic/record_blob.rs`
  `a_body_with_trailing_junk_is_still_a_fingerprint_mismatch`
  (criterion 5); `durability/record_blob.rs`
  `a_body_with_trailing_junk_is_unreadable_with_a_decode_cause` (the
  finding under "Traceability").
- No new dependency (`BINENC-FR-008`).

## Data/state and invariants

- One configuration, one place. `BINENC-FR-003` is checkable by
  `grep -rn "bincode::" src --include=*.rs`.
- The bytes written are the bytes written before: every golden vector
  was captured on `4ae86bd` (free functions) and passes on the routed
  code with no literal changed.
- Decode is exact-length: a payload is the value and nothing else.
  Every format in this crate already frames its payload by an explicit
  length (the frame prefix, the blob file's length after its header,
  the WAL entry's length prefix, the whole SST/snapshot file), so the
  slice handed to `decode` is exactly one value in every correct case.
- `options()` is a zero-sized value built per call; no runtime path
  changes (same `bincode` code underneath). No benchmark.

## Errors, failure, recovery, and observability

- `encode`/`encode_into` fail only as `bincode::serialize` did (not
  expected for this crate's shapes; not assumed infallible).
- `decode` fails on a short, malformed, or over-long payload. Each
  caller's mapping is unchanged: `FrameError::Encoding`,
  `DurabilityError::Serde`, or a `RecordBlobUnreadable` with a
  "body does not decode: …" cause.
- No `unwrap`/`expect` outside `#[cfg(test)]`.

## Security, privacy, and compatibility

- Every pre-change file reads unchanged: the blob bodies' golden
  vectors, written to disk under each blob's real header and read back
  through each blob's real `read`, are the proof (criterion 3).
- The one narrowing: a frame or a `DOGBLOB\0` body with junk after the
  value is now refused. A conforming writer never produces either; a
  reader that previously accepted one was accepting a malformed input.
- No public API change. `codec` is `pub(crate)`; the two error enums'
  payload type is unchanged.
- Synthetic data only; no new network surface.

## Acceptance criteria

Numbered as in the design document.

1. `grep -rn "bincode::" src --include=*.rs`, outside `#[cfg(test)]`
   modules, matches only `src/codec.rs` and the two error-type
   positions (`FrameError::Encoding(bincode::Error)`,
   `DurabilityError::Serde(bincode::Error)`). ✔ (the routing commit's
   `grep`: those three files only; `codec.rs`'s remaining free-function
   uses are inside its `#[cfg(test)]` equivalence test).
2. Every `BINENC-FR-004` vector was captured on the pre-change code
   (commit `4ae86bd`, tests green against the free functions, no
   `codec` functions yet) and passes against `codec::encode`/`decode`
   after routing (`049e4f6`) with no literal changed. ✔
3. A `GENBLOB\0`, a `GENEDGE\0`, and a `DOGBLOB\0` file written by the
   pre-change build are read by the post-change build — proven by each
   body vector written under its real header and read through the real
   `read`. ✔
4. A frame with a valid `Request` followed by two junk bytes is
   `FrameError::Encoding` from `read_message`; the same frame without
   the junk decodes. ✔
5. A `GENBLOB\0` body with trailing junk is still a *fingerprint*
   mismatch, not a decode error. ✔
6. Every existing test in `framing.rs`, `protocol.rs`, the three blob
   modules, `durability/mod.rs`, and the research modules passes with
   no change beyond the call-site rename. ✔
7. The full sweep is green. ✔ (see "Verification plan")
8. `ADR-0010`, `SERVER-001`, `PROJECT-STATUS` item 33, and `ADR-0019`'s
   revisit trigger each carry a resolution pointer to `ADR-0021` and
   this spec. ✔

## Verification plan

- `cargo test --all-features`: 333 lib tests (316 before this unit:
  +12 in the capture commit — 7 `codec`, 2 `protocol`, 3 blob bodies —
  and +5 in the routing commit: 2 `codec`, 1 `framing`, 1 `GENBLOB\0`
  junk, 1 `DOGBLOB\0` junk). `cargo test` (default features): 136 lib
  + 2 integration (the `Order`-shaped `GENBLOB\0` vector and junk
  tests are `research`-gated with the type they use; the `Request`/
  `Response` vectors are `server`-gated).
- `cargo fmt --all -- --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo doc --all-features --no-deps`
  (zero warnings): clean.
- No benchmark: no runtime path changes.

## Traceability

Implements: ADR-0021 / `BINCODE-ENCODING-STABILITY-DESIGN.md`
(`BINENC-FR-001..008`, kept under their design names). Resolves:
`ADR-0010`'s limitation, `SERVER-001`'s open question (amended to
v0.9.1), `PROJECT-STATUS` item 33. Answers `ADR-0019`'s revisit
trigger without firing it.

Where the implementation settles or corrects something the design left
to it:

- **`DOGBLOB\0` is not "no change".** The design's `BINENC-FR-006`
  said the three blobs were unaffected by option (a) because "the
  fingerprint already refuses such a body". That is true of the two
  generic blobs, which hash the body *bytes* before decoding. The
  `Dog` blob (`src/durability/record_blob.rs`) decodes the body
  *first* and fingerprints the decoded records, so a body with junk
  appended used to decode fine, fingerprint fine, and be accepted.
  Under option (a) it is now `RecordBlobUnreadable` with a "body does
  not decode" cause — the same class of refusal a truncated body gets.
  Pinned by `a_body_with_trailing_junk_is_unreadable_with_a_decode_cause`.
  A conforming writer never produces such a file, so no real file is
  affected; recorded because the design's claim was wrong for one of
  the three.
- **`Request`/`Response` decode is stated by re-encoding.** Neither
  type derives `PartialEq`, so `assert_golden` proves `decode(GOLDEN)`
  by checking it re-encodes to `GOLDEN`; the other vectors use
  `assert_golden_eq`. Deriving `PartialEq` on the protocol types just
  for a test was not worth the public-surface change.
- **The vectors were hand-derived, then confirmed.** The design
  offered capture-from-output; every literal was written from the
  format description and passed on first run against the free
  functions, which is a stronger statement about the format's
  legibility than a captured dump would have been.
- **`options()` stays a function.** `bincode`'s option builders are
  not `const fn`, so the configuration is rebuilt (zero-sized) per
  call rather than held in a `const`.

## Open questions

- **Wire-protocol versioning.** Frames carry no protocol version;
  a `Request`/`Response` shape change is a named incompatibility.
  Revisit trigger: a second deployed client build (`ADR-0021`).
- **`bincode` 2.x.** `config::legacy()` against the golden vectors is
  the migration's acceptance test; `standard()` is a format change to
  everything. No driver now.
- **`Uuid` at 24 bytes.** A `[u8; 16]` newtype saves a third of every
  id and is a format change to every blob and every frame. Recorded,
  not proposed.
- **`DOGBLOB\0` fingerprint-before-decode — closed as not warranted
  (v0.1.1, owner's call).** Reordering `RecordBlob::read` to hash bytes
  first, as the generic blobs do, would make the junk case a
  fingerprint mismatch there too and let a corrupt body fail before
  `bincode` sees it. Examined for a driver and found none: the `Dog`
  blob's fingerprint is over the *decoded* fields and deliberately
  skips `age`, so that `ProductionStore::open`'s `is_current_at`
  (a 20-byte header read against the in-memory fingerprint) does not
  rewrite the blob after every `update_age` — the ages file, not the
  blob, is authoritative for ages. A byte hash includes the ages, so
  the reorder is one of (i) a `BLOB_VERSION` 2 → 3 with a second
  header field (byte hash checked before decode *plus* the existing
  content fingerprint, header 20 → 28 bytes) or (ii) a v3 that zeroes
  ages in the blob body and hashes bytes like `GENBLOB\0`; either is a
  design-doc/ADR round for a blob only this crate's tests and benches
  write. What it would buy: a "fingerprint mismatch" cause instead of
  a "body does not decode" cause for a junk-tailed or corrupt body.
  What already holds without it: the codec refuses trailing bytes
  (criterion 6), a corrupt body fails inside `bincode` bounded by the
  file's own length with no panic path, and a conforming writer never
  produces either file. Re-arms only if a second writer of `DOGBLOB\0`
  appears or the blob's read path becomes a measured cost.

## Change history

- 0.1.1 (2026-09-02): Docs-only. The `DOGBLOB\0` fingerprint-before-
  decode open question examined and closed as not warranted by the
  owner — the reorder is a `BLOB_VERSION` 2 → 3 format change (the
  `Dog` blob's fingerprint is over decoded, age-free content so `open`
  does not rewrite after `update_age`; a byte hash would not be) and
  buys only a different error cause for files no conforming writer
  produces. No code change, no test change; re-arm trigger recorded.
- 0.1.0 (2026-09-02): Initial accepted draft, alongside the real
  implementation (`src/codec.rs`, `src/lib.rs`, the
  `src/test_support.rs` golden helpers, 23 call-site renames across
  `src/server/framing.rs`, the three blob modules, `src/durability/
  {mod,snapshot_rebuild,hybrid,lsm_store}.rs`) and 17 new tests across
  two commits (capture `4ae86bd`, routing `049e4f6`). Registers the
  design ADR-0021 accepted on 2026-09-02 as requirements; records the
  `DOGBLOB\0` finding and three smaller implementation calls under
  "Traceability".
