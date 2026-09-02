# bincode Encoding Stability Design (Proposed)

- Status: **Proposed** (owner's call; the acceptance question is in
  "Acceptance criteria" and the ADR's "Acceptance and implementation")
- Date: 2026-09-02
- Related: `docs/decisions/ADR-0021-bincode-encoding-stability-proposal.md`
  (the decision record this document backs), `ADR-0010` and `SERVER-001`
  (whose named limitation — *"`bincode`'s wire-format stability across
  crate versions is unverified"* — this answers; `PROJECT-STATUS` item 33
  carries it), `STORAGE-014` v0.2.0 / `STORAGE-015` v0.2.0 / `STORAGE-016`
  v0.2.0 (the three companion blobs whose bodies are `bincode`),
  `ADR-0019` (whose revisit trigger asks this unit whether the record
  *layout*, not just the record type, needs recording), `ADR-0005` /
  `ADR-0006` (the durability round that introduced `bincode` and the
  `research`-gated WAL/snapshot/LSM files that also use it),
  `PROJECT-STATUS` item 68 (the owner's scoping text for this, the fourth
  of four queued follow-ups: *"verify and document what `bincode`'s
  default configuration pins today, and decide whether to pin it
  explicitly"*)

## Purpose and scope

Every byte this crate writes through `bincode` — the wire protocol's
frames, the three companion blobs' bodies, and the `research`-gated
WAL/snapshot/LSM files — is produced by `bincode`'s *free functions*
(`bincode::serialize`, `bincode::deserialize`, `bincode::serialize_into`)
with no configuration named anywhere in `src/`. What those functions
pin (integer width, byte order, length prefixes, trailing-byte policy)
is a property of the `bincode` crate's *defaults*, documented in its
readme and source, not of this crate. No test pins a single byte: every
existing test is a round trip through the same build, which passes
under any self-consistent encoding, including a different one.

`ADR-0010` named this as a deliberate limitation when the server layer
was proposed (*"previously only mattered within one process's own
on-disk lifetime, a materially different compatibility bar"*), `SERVER-001`
carries it as an open question, and `PROJECT-STATUS` item 33 has held it
open since. The three blobs raised the bar again: `STORAGE-014`–`016`
exist so that a file written by one build can be read by another, and
their bodies are `bincode`.

This design does the two things the owner's scoping text asks for. It
**verifies and records** what the free functions pin today — from the
vendored `bincode` 1.3.3 source and an executed probe, not from memory —
and it **proposes pinning that configuration explicitly** in one
`pub(crate)` codec module every call site routes through, with golden
byte vectors captured on today's `main` so that any future change to
the crate, its dependencies, or the configuration is a test failure and
not a silent format change.

**In scope for this proposal:**

- The findings: what `bincode` 1.3.3's free functions pin, what its
  stability promise says, how the free functions differ from the
  `Options`-struct defaults, and exactly what each of this crate's 23
  call sites relies on (the "Context and terminology" section).
- A new `pub(crate)` module `src/codec.rs` exposing `encode`,
  `encode_into`, and `decode` over one explicit `bincode::Options`
  value: fixed-width integers, little-endian, no size limit — the free
  functions' configuration, named — plus `reject_trailing_bytes()` on
  the read side (the one behavioral change; offered as the acceptance
  question).
- Every production `bincode::serialize`/`deserialize`/`serialize_into`
  call routed through it: the 11 front-door sites (`src/server/
  framing.rs`, `src/durability/record_blob.rs`, `src/generic/record_blob.
  rs`, `src/generic/edge_blob.rs`) and the 12 `research`-gated sites
  (`src/durability/mod.rs`, `hybrid.rs`, `lsm_store.rs`,
  `snapshot_rebuild.rs`).
- Golden byte tests: primitives and composites in `codec.rs`; every
  `Request` and `Response` variant in `src/server/protocol.rs`; one
  `DOGBLOB\0` body, one `GENBLOB\0` body, one `GENEDGE\0` body — the
  vectors captured on current `main` *before* the routing change and
  checked in as hex literals, so the routing is proven byte-identical.
- The evolution rules the codec makes explicit: what may change in a
  `Serialize` type without changing its bytes, and what may not.
- Documentation: the crate-level and module-level statements that the
  wire format and blob bodies are *this* configuration, and a resolution
  of `ADR-0010`'s limitation, `SERVER-001`'s open question,
  `PROJECT-STATUS` item 33, and `ADR-0019`'s revisit trigger.

**Explicitly out of scope, named directly:**

- Moving to `bincode` 2.x. `Cargo.toml` says `bincode = "1"`; 2.x is a
  different API (`bincode::serde::encode_to_vec`, a `config::` module)
  whose `config::standard()` is *varint*, i.e. a different format, with
  `config::legacy()` documented as the 1.x free-function-compatible
  configuration. Nothing here prevents that migration; the golden
  vectors are exactly what would prove it byte-identical. Named in
  "Open questions".
- A wire-protocol version field or hello handshake. Frames carry no
  version today; this design pins the *encoding* of what is in them, not
  the *shape* (`Request`/`Response` variants), which is `SERVER-001`'s.
  Named as a revisit trigger.
- A record-layout fingerprint in the blob headers. `ADR-0019`'s revisit
  trigger asks whether this unit concludes one is needed; the conclusion
  is below (it is not, for this crate, today) and the trigger stays
  armed.
- Any format version bump. `DOGBLOB\0`, `GENBLOB\0`, `GENEDGE\0` stay at
  `BLOB_VERSION` 2; the frame layout is unchanged; every existing file
  and every existing client decode exactly as before, because the
  proposal writes the same bytes.
- Golden vectors for the `research`-gated files. They are routed
  through the codec (so the pin applies) but not pinned by test: they
  are single-process, regenerated by every benchmark run, and
  `ADR-0006` never promised them cross-build.
- Cross-language interoperability (`PROJECT-STATUS` item 38). A pinned
  `bincode` configuration is a *documented* format, which is the
  precondition for a non-Rust client, not the client.

## Non-goals

- Not a change to any byte on disk or on the wire, on either side of
  the acceptance question. The writer produces identical output; the
  only behavioral change offered is a stricter *reader* on trailing
  bytes.
- Not a dependency change. `bincode = "1"` (resolving to 1.3.3) and
  `serde` stay as they are; the `Options` API used is in 1.3.3's public
  surface and has been since 1.3.0.
- Not a performance change. `bincode`'s `Options` methods are
  zero-sized configuration types resolved at compile time; the free
  functions are themselves thin wrappers over the same `Options` path
  (see "Context"). The blob fingerprint streams through
  `serialize_into` today and would stream through `encode_into`
  tomorrow, the same `Fnv1a64` writer underneath.
- Not a security boundary. `reject_trailing_bytes()` refuses a frame
  with junk after a valid message; it does not authenticate one.
  `MAX_FRAME_BYTES` and the blob fingerprint are the existing guards and
  are unchanged.

## Context and terminology

### What `bincode` 1.3.3's free functions pin

Verified against the vendored `bincode-1.3.3` source
(`src/lib.rs`, `src/config/mod.rs`, `src/config/int.rs`,
`src/config/trailing.rs`):

- `bincode::serialize(v)` is `DefaultOptions::new()
  .with_fixint_encoding().allow_trailing_bytes().serialize(v)`.
- `bincode::deserialize(bytes)` is the same options' `deserialize`.
- `bincode::serialize_into(w, v)` is `DefaultOptions::new()
  .with_fixint_encoding().serialize_into(w, v)` — the trailing policy is
  irrelevant to a writer; bytes are identical to `serialize`.
- `bincode::DefaultOptions::new()` on its own is **not** that
  configuration: it is infinite limit, little-endian, **varint**
  integer encoding, and **reject** trailing bytes. `bincode`'s own docs
  carry a table headed "Options Struct vs bincode functions" to warn
  about exactly this difference, and `bincode::options()` returns the
  struct form.

So the configuration this crate has been relying on, unnamed, is:

| Setting | Free functions (what this crate uses) | `DefaultOptions` / `bincode::options()` |
|---|---|---|
| Integer encoding | **fixint** — every integer at its Rust width | varint |
| Byte order | little-endian | little-endian |
| Size limit | none | none |
| Trailing bytes on read | **allowed** | rejected |

Under fixint little-endian, the format is the one the readme describes:
`u8`–`u64`/`i8`–`i64` at their width LE; `bool` one byte; `char` as UTF-8
bytes; `f32`/`f64` IEEE-754 LE; `String`/`Vec<T>`/`&[T]` as a `u64` LE
length then the elements; `Option<T>` as a `u8` tag (0 = `None`, 1 =
`Some`) then the value; enums as a `u32` LE variant index then the
variant's fields in declaration order; structs and tuples as their
fields in declaration order with no tag, no padding, and no field
names; unit types and unit variants' fields as nothing.

`uuid` 1.25.0's `Serialize` impl (`src/external/serde_support.rs`) calls
`serialize_bytes(self.as_bytes())` for a non-human-readable serializer,
which under fixint is a `u64` LE length of 16 then the 16 bytes — 24
bytes per `Uuid`, not 16. This is what every `RecordId` in every frame
and every `Id` in every blob costs today, and it is part of the format
being pinned; changing it (a `[u8; 16]` newtype, or `serde_bytes`) would
be a format change, not a codec change.

### The stability promise

`bincode` 1.x's readme, verbatim: *"The encoding format is stable across
minor revisions, provided the same configuration is used. This should
ensure that later versions can still read data produced by a previous
versions of the library if no major version change has occured."*

Two consequences for this crate. First, `bincode = "1"` in `Cargo.toml`
is a semver range that excludes 2.x, so `cargo update` cannot move the
crate across the promise's boundary. Second, and the reason this design
proposes code and not just a paragraph: the promise is conditioned on
*"the same configuration"*, and today this crate's configuration is
"whatever the free functions do", which is stable by the same promise
but named nowhere in this repository. A future contributor who reaches
for `bincode::options()` (the struct form, varint) at one call site —
the obvious thing to type — produces a different, incompatible format
that every existing round-trip test passes.

### What the probe showed

A throwaway probe crate (not committed; run against the same 1.3.3 and
`uuid` 1.25.0 this crate resolves) confirmed the table above and
established the behaviors the design depends on:

- `Uuid`: free functions 24 bytes (`10 00 00 00 00 00 00 00` then 16
  bytes); `bincode::options()` 17 bytes (varint length 1 byte).
- `enum E { A, B(u32), C { s: String, x: i64 } }`, value `C { s: "hi",
  x: -1 }`: free functions 22 bytes — `02 00 00 00` (variant), `02 00 00
  00 00 00 00 00` (string length), `68 69`, `ff ×8`; `bincode::options()`
  5 bytes — `02 02 68 69 01` (zigzag varint `-1`).
- Trailing bytes: `bincode::deserialize::<E>(serialize(&E::B(7)) ++ [de
  ad])` is `Ok(B(7))`. With `.reject_trailing_bytes()` it is `Err("Slice
  had bytes remaining after deserialization")`.
- Cross-configuration decode: the 5 varint bytes read as fixint failed
  in this instance (`invalid value: integer 1768423938, expected variant
  index 0 <= i < 3`) — but that is luck, not a guarantee; a varint
  payload whose first four bytes happen to form a valid `u32` variant
  index decodes as garbage.
- `Vec<u8>` of `[1, 2, 3]`: `03 00 00 00 00 00 00 00 01 02 03`.
  `Option<u32>`: `Some(1)` = `01 01 00 00 00`, `None` = `00`. `true` =
  `01`; `'é'` = `c3 a9`; `1.5f64` = `00 00 00 00 00 00 f8 3f`.

### What this crate's call sites rely on

23 production call sites, all free functions, none using `Options`:

| Site | Function | Feature gate | Format it produces |
|---|---|---|---|
| `src/server/framing.rs:61` | `serialize` | `server` | frame payload |
| `src/server/framing.rs:91` | `deserialize` | `server` | frame payload |
| `src/durability/record_blob.rs:284` | `serialize` | default | `DOGBLOB\0` body (fingerprint) |
| `src/durability/record_blob.rs:295` | `serialize` | default | `DOGBLOB\0` body (image) |
| `src/durability/record_blob.rs:340` | `deserialize` | default | `DOGBLOB\0` body |
| `src/generic/record_blob.rs:195` | `serialize_into` | default | `GENBLOB\0` body (fingerprint, streamed into `Fnv1a64`) |
| `src/generic/record_blob.rs:207` | `serialize` | default | `GENBLOB\0` body (image) |
| `src/generic/record_blob.rs:274` | `deserialize` | default | `GENBLOB\0` body |
| `src/generic/edge_blob.rs:107` | `serialize_into` | default | `GENEDGE\0` body (fingerprint, streamed) |
| `src/generic/edge_blob.rs:119` | `serialize` | default | `GENEDGE\0` body (image) |
| `src/generic/edge_blob.rs:182` | `deserialize` | default | `GENEDGE\0` body |
| `src/durability/mod.rs:248` | `serialize` | `research` | WAL entry (`append_wal_entry`) |
| `src/durability/mod.rs:283` | `deserialize` | `research` | WAL entry (`read_wal_entries`) |
| `src/durability/mod.rs:476` | `serialize` | default build, `research` callers | `CanonicalCachedState::write_to` |
| `src/durability/mod.rs:483` | `deserialize` | default build, `research` callers | `CanonicalCachedState::read_from` |
| `src/durability/hybrid.rs:139`, `:190` | `deserialize`, `serialize` | `research` | hybrid snapshot |
| `src/durability/lsm_store.rs:170`, `:228`, `:266`, `:271` | `deserialize` ×2, `serialize` ×2 | `research` | SSTable / memtable images |
| `src/durability/snapshot_rebuild.rs:82`, `:104` | `deserialize`, `serialize` | `research` | rebuild snapshot |

The first 11 are the owner's *"wire protocol and the three blob
formats"* exactly — the every-build surface, and the one that crosses
process and build boundaries by design. The `src/server/protocol.rs`
test module also calls `bincode::serialize`/`deserialize` directly in
its round-trip tests; those are tests and are where the golden vectors
go.

Everything around those bodies is already explicit and hand-written:
the frame's `u32` LE length prefix (`to_le_bytes`/`from_le_bytes`,
checked against `MAX_FRAME_BYTES` before allocation), the blobs' 20- and
28-byte headers (magic, `u32` LE version, `u64` LE FNV-1a 64
fingerprint, `u64` LE tag hash), and the WAL's `u32` LE entry length.
Only the bodies are `bincode`.

Two existing guards bear on the trailing-bytes question. The three
blobs fingerprint the *raw body bytes*, so a body with trailing junk
already fails the fingerprint compare before `bincode` is reached —
`reject_trailing_bytes()` changes nothing for them. Frames have no such
guard: a frame whose payload is a valid message followed by junk is
accepted today, silently, and the junk is discarded.

### Terminology

**Configuration** means a `bincode::Options` value: integer encoding,
byte order, size limit, trailing-byte policy. **Pinning** means naming
that configuration in one place in this crate, routing every call site
through it, and holding it with byte-level tests. **Golden vector** means
a checked-in hex literal of the bytes a specific value encodes to under
the pinned configuration, captured from the *current* code before the
change and asserted after it. **Front-door sites** are the 11 call sites
compiled under default features or `server`; **research sites** are the
12 compiled only under `research` (or reachable only from there).

## Requirements

- `BINENC-FR-001`: A `pub(crate)` module `src/codec.rs` defines the one
  `bincode::Options` value this crate encodes and decodes with:
  `DefaultOptions::new().with_fixint_encoding().with_little_endian()
  .with_no_limit()` on both sides, plus `.reject_trailing_bytes()` on
  the decode side (or `.allow_trailing_bytes()`, per the acceptance
  question — see `BINENC-FR-006`). It exposes `encode<T: Serialize +
  ?Sized>(&T) -> Result<Vec<u8>, bincode::Error>`, `encode_into<W:
  Write, T: Serialize + ?Sized>(W, &T) -> Result<(), bincode::Error>`,
  and `decode<'a, T: Deserialize<'a>>(&'a [u8]) -> Result<T,
  bincode::Error>`. The error type is unchanged so every call site's
  existing `?`/`map_err` compiles as is.
- `BINENC-FR-002`: The bytes `encode`/`encode_into` produce are
  identical to what `bincode::serialize`/`serialize_into` produce today
  for every `T`. Proven by golden vectors captured on the pre-change
  code.
- `BINENC-FR-003`: No production code in `src/` calls
  `bincode::serialize`, `bincode::deserialize`, `bincode::serialize_into`,
  `bincode::deserialize_from`, `bincode::options()`, or
  `bincode::DefaultOptions` outside `src/codec.rs`. All 23 sites in the
  table route through the codec. (Test modules may use the free
  functions to *state* expectations; the golden tests do.)
- `BINENC-FR-004`: Golden byte tests pin: in `codec.rs`, one vector each
  for `u8`, `u16`, `u32`, `u64`, `i64`, `bool`, `char`, `f64`, `String`,
  `Vec<u8>`, `Option<u32>` (both arms), a unit-variant, tuple-variant,
  and struct-variant enum, a tuple, a struct, and `Uuid`; in
  `src/server/protocol.rs` (behind `server`), every `Request` variant
  and every `Response` variant with one representative value; in
  `src/durability/record_blob.rs`, one `DOGBLOB\0` body holding one
  record and one edge; in `src/generic/record_blob.rs`, one `GENBLOB\0`
  body holding one `Order`-shaped record; in `src/generic/edge_blob.rs`,
  one `GENEDGE\0` body holding one `(Uuid, Uuid)` pair. Each test
  asserts `encode(&v) == GOLDEN` and `decode(GOLDEN) == v`.
- `BINENC-FR-005`: The codec module's docs state the configuration, the
  `bincode` 1.x stability promise it relies on and the `Cargo.toml`
  range that keeps it in 1.x, and the evolution rules: adding an enum
  variant at the *end* keeps every existing variant's bytes; reordering
  or inserting variants, adding/removing/reordering struct fields,
  changing a field's type, or changing an integer's width changes the
  bytes and is a format change requiring the owning format's version
  bump (`BLOB_VERSION` for a blob; for the wire protocol, `SERVER-001`'s
  amendment and, absent a version field, a named incompatibility).
- `BINENC-FR-006`: Acceptance question, one of two:
  - **(a) Proposed**: `decode` rejects trailing bytes. Behavioral
    change: a frame whose payload is a valid message plus trailing
    junk becomes `FrameError::Encoding` (today: silently accepted). No
    change for the three blobs (the fingerprint already refuses such a
    body) or the research files (single-writer, exact-length reads).
  - **(b) Alternative**: `decode` allows trailing bytes. Reader behavior
    byte-for-byte as today; the pin is the whole change.
- `BINENC-FR-007`: `ADR-0010`'s limitation, `SERVER-001`'s open
  question, and `PROJECT-STATUS` item 33 are resolved by pointer to the
  ADR and the codec; `ADR-0019`'s revisit trigger records this unit's
  conclusion (no layout fingerprint needed now; the trigger stays).
- `BINENC-FR-008`: No new dependency; no change to `Cargo.toml`'s
  `bincode = "1"`; no format version bump; no public API change (the
  codec is `pub(crate)`; `FrameError::Encoding(bincode::Error)` and
  `DurabilityError::Serde(bincode::Error)` keep their payload type).

## Architecture and interfaces

### Considered options

**Whether to pin at all.**

1. *Document only.* Write the findings into the ADR and the module docs
   and leave the free-function calls. Cheapest, and it does answer the
   first half of the owner's question. Rejected as the whole answer:
   the stability promise is conditioned on "the same configuration",
   and a documented-but-unnamed configuration is one `bincode::options()`
   away from silently becoming a different one, with every test green.
   Documentation without a byte-level test is a promise nobody checks.
2. *Pin explicitly in one `pub(crate)` codec, route every site, and
   hold it with golden vectors.* Proposed. The configuration becomes a
   single named value; the golden vectors turn "same configuration"
   from a convention into a test; the routing makes the free functions
   unreachable from production code by review (`BINENC-FR-003` is a
   `grep`).
3. *Pin per format* — a codec per blob and one for frames, each with
   its own `Options`. Rejected: there is one configuration, and four
   copies of it are four places to drift. The per-format *golden
   vectors* are kept (they pin the format, not the configuration); the
   *configuration* is one.

**Which configuration to pin.**

1. *The free functions' (fixint, LE, no limit).* Proposed — it is what
   every existing file and every existing client was written with, so
   the pin is a no-op on the bytes. A `bincode` 2.x migration would
   name it `config::legacy()`.
2. *`DefaultOptions` / varint.* Smaller output (a `Uuid` at 17 bytes
   rather than 24; small integers at 1 byte), and `bincode` 2.x's
   `standard()`. Rejected for this round: it is a format change to
   every blob (three `BLOB_VERSION` bumps and a heal-on-`open` story
   for each) and to the wire protocol (no version field to negotiate
   it), for a size win nothing has asked for. Named as a revisit
   trigger, with the golden vectors as the tool that would make the
   switch auditable.
3. *Big-endian.* No reason; every hand-written prefix in this crate is
   LE. Rejected.

**Where the size limit lives.**

1. *In the codec (`with_limit(MAX_FRAME_BYTES)`).* Rejected — the limit
   is a *framing* concern with one value for frames and none for blobs;
   `read_message` already enforces it before allocation, which is the
   right place (the codec sees a slice that has already been sized).
2. *`with_no_limit()` in the codec; each format keeps its own bound.*
   Proposed. Frames: `MAX_FRAME_BYTES` on the length prefix. Blobs: the
   file's own length plus the fingerprint. WAL: the entry-length prefix.

**Trailing bytes.**

1. *Reject (`reject_trailing_bytes()`).* Proposed. A decode that
   consumes fewer bytes than it was given is, for every format in this
   crate, a mismatch between what the writer thought it wrote and what
   the reader thinks it read — exactly the cross-build symptom this
   unit exists to surface. For the blobs it is already unreachable (the
   fingerprint runs first); for frames it turns a silent discard into
   `FrameError::Encoding`. The stricter reader is also what `bincode`'s
   own `DefaultOptions` and 2.x's `standard()` do.
2. *Allow (`allow_trailing_bytes()`).* The alternative offered. Zero
   behavioral change; a valid-prefix frame with junk keeps being
   accepted. Chosen if the owner wants this round to be *purely* a pin.

**Golden-vector capture.**

1. *Hand-derive the hex from the format description.* Rejected — the
   point is to pin what the code *does*, not what the docs say it does;
   a hand-derived vector that disagrees with the code would be "fixed"
   to match the code at the first failure.
2. *Capture from the current `main` before the codec exists, by a
   throwaway test that prints the bytes; check the hex literals in; then
   route the sites and confirm the same tests still pass.* Proposed.
   The commit history shows the vectors passing against the free
   functions and against the codec, which is the byte-identity proof
   `BINENC-FR-002` asks for.

**Scope of routing.**

1. *Front-door sites only (11).* Rejected — leaves 12 sites on the free
   functions, so the `grep` in `BINENC-FR-003` is not clean and the
   "one configuration" claim has a `research`-shaped hole.
2. *All 23.* Proposed. Route everything; pin the front-door formats
   with golden vectors; leave the research files unpinned by test
   (regenerated by every run, never promised cross-build).

**A record-layout fingerprint in the blob headers (`ADR-0019`'s
question to this unit).**

1. *Add one — a hash of the field layout — to `GENBLOB\0`/`GENEDGE\0`
   as version 3.* Not proposed. This crate has no layout-description
   source to hash (serde does not expose one; the trait-method
   fingerprint the owner closed as not warranted was the nearest thing),
   and the change it would guard against — a struct's fields changing
   between builds — is, under the evolution rules `BINENC-FR-005`
   states, a format change that bumps `BLOB_VERSION` by rule. The
   version field is the layout fingerprint, maintained by discipline
   rather than computed. `ADR-0019`'s trigger stays armed for the case
   where that discipline proves insufficient.

### Proposed shape

```rust
// src/codec.rs — new, pub(crate), no feature gate (every build has at
// least the three blobs).

//! The one `bincode` configuration this crate encodes and decodes with.
//!
//! Every `bincode` byte this crate writes — wire frames (`SERVER-001`),
//! the `DOGBLOB\0`/`GENBLOB\0`/`GENEDGE\0` bodies (`STORAGE-014`–`016`),
//! and the `research`-gated WAL/snapshot/LSM files — goes through
//! `encode`/`encode_into`, and every byte it reads goes through
//! `decode`. The configuration is `bincode` 1.x's free-function default,
//! named: fixed-width integers, little-endian, no size limit. [... the
//! format description, the stability promise, the `Cargo.toml` range,
//! and the evolution rules from BINENC-FR-005 ...]

use bincode::Options;
use serde::{Deserialize, Serialize};
use std::io::Write;

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_no_limit()
        .reject_trailing_bytes()   // (b): .allow_trailing_bytes()
}

pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, bincode::Error> {
    options().serialize(value)
}

pub(crate) fn encode_into<W: Write, T: Serialize + ?Sized>(
    writer: W,
    value: &T,
) -> Result<(), bincode::Error> {
    options().serialize_into(writer, value)
}

pub(crate) fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, bincode::Error> {
    options().deserialize(bytes)
}

// src/lib.rs
pub(crate) mod codec;

// Every call site, mechanically:
//   bincode::serialize(x)          -> crate::codec::encode(x)
//   bincode::serialize_into(w, x)  -> crate::codec::encode_into(w, x)
//   bincode::deserialize(b)        -> crate::codec::decode(b)
// Error types, `?`s, and `map_err`s unchanged.
```

Golden-vector test shape (one of many):

```rust
// src/server/protocol.rs, #[cfg(test)]
#[test]
fn get_by_id_request_bytes_are_pinned() {
    let req = Request::GetById { id: Uuid::from_bytes([0x11; 16]) };
    const GOLDEN: &[u8] = &[
        0x00, 0x00, 0x00, 0x00,                          // variant 0
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // Uuid: len 16
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    ];
    assert_eq!(crate::codec::encode(&req).unwrap(), GOLDEN);
    assert_eq!(crate::codec::decode::<Request>(GOLDEN).unwrap(), req);
}
```

(The literal above is illustrative; the implementation captures every
vector from the running code, per "Golden-vector capture" option 2, and
the variant indices/shapes are whatever `protocol.rs` declares at
capture time.)

## Data/state and invariants

- One configuration, one place. `options()` is private; the three
  `pub(crate)` functions are the only way production code touches
  `bincode`. `BINENC-FR-003` is checkable by `grep -rn "bincode::" src
  --include=*.rs` returning only `codec.rs`, the two error-type
  positions, and test modules.
- The pin is a no-op on writer bytes. Under either acceptance option,
  every value encodes to the same bytes it does today; `BINENC-FR-002`'s
  golden vectors, captured before the change, are the proof. Existing
  files open; existing clients (there are none outside this repository,
  but the property holds) decode.
- The format is a function of `(configuration, T's serde shape)`. The
  configuration is now pinned; `T`'s shape is under the evolution rules.
  A `Serialize` derive on a type in a pinned format is part of that
  format's definition — the module docs say so, and the golden vectors
  fail if it drifts.
- `bincode::Error` stays the error currency. `FrameError::Encoding`,
  `DurabilityError::Serde`, and the blobs' `unreadable(format!("body
  does not decode: {e}"))` are unchanged in type and text; the only new
  message possible (option (a)) is `bincode`'s own "Slice had bytes
  remaining after deserialization" inside those wrappers.
- Zero-cost. `Options` combinators are ZST configuration composed at
  compile time; `bincode`'s free functions are already
  `DefaultOptions::new().with_fixint_encoding()...` underneath. No
  allocation, branch, or indirection is added.

## Errors, failure, recovery, and observability

- Under option (a), one new failure mode on frames: a payload of `len`
  bytes that decodes as `T` in fewer than `len` bytes is
  `FrameError::Encoding(…)` with `bincode`'s trailing-bytes message. The
  connection handling is whatever `read_message`'s callers do with
  `Encoding` today (a typed error response or a closed connection —
  `SERVER-001-FR-004`'s existing story). Under option (b), no new
  failure mode.
- Blobs: no change to any check, order, or message. Trailing junk is a
  fingerprint mismatch before the body is decoded, as today.
- Research files: no change; every read is an exact-length read (WAL
  entries by prefix, snapshots by whole file) of a body one process
  wrote.
- Golden-vector failures are the new observability: a change to
  `bincode`, `serde`, `uuid`'s serde impl, the configuration, or a
  pinned type's derive fails a named test that says which format moved.
  That is the entire point.

## Security, privacy, and compatibility

- **Format compatibility**: unchanged on the writer side, both options.
  Option (a) narrows what a *reader* accepts to what a conforming
  writer produces. No file, frame, or client in this repository
  produces trailing bytes.
- **API compatibility**: none affected. `codec` is `pub(crate)`. The
  public error enums keep `bincode::Error` as the payload.
- **Dependency compatibility**: `bincode = "1"` unchanged; the `Options`
  trait and `DefaultOptions` combinators used exist in every 1.3.x.
  The golden vectors are what would detect a *patch* release of
  `bincode` breaking its own promise — the scenario the readme rules
  out but this crate would otherwise have no test for.
- **Security**: option (a) closes a minor lenience (junk after a valid
  frame is discarded silently today). It is not a security boundary;
  `MAX_FRAME_BYTES` and the blob fingerprint remain the guards.
- **Privacy**: no new data written anywhere.

## Acceptance criteria

1. `grep -rn "bincode::" src --include=*.rs`, excluding `#[cfg(test)]`
   modules, matches only `src/codec.rs` and the two error-type positions
   (`FrameError::Encoding(bincode::Error)`,
   `DurabilityError::Serde(bincode::Error)`).
2. Every golden vector listed in `BINENC-FR-004` was captured on the
   pre-change code (a commit on the branch shows the vectors passing
   against `bincode::serialize` before `codec.rs` exists) and passes
   against `codec::encode`/`decode` after routing.
3. A `GENBLOB\0` file, a `GENEDGE\0` file, and a `DOGBLOB\0` file written
   by the pre-change build are read by the post-change build — proven by
   the golden body vectors (2) plus the existing header tests, or by a
   checked-in fixture if the implementation prefers.
4. Option (a) only: a frame with a valid `Request` followed by two junk
   bytes is `FrameError::Encoding` from `read_message`, and the same
   frame without the junk decodes. Option (b): the junk frame decodes
   and the test documents that it does.
5. A `GENBLOB\0` body with trailing junk is still a *fingerprint*
   mismatch, not a decode error, under either option.
6. Every existing test in `framing.rs`, `protocol.rs`, the three blob
   modules, `durability/mod.rs`, and the research modules passes with
   no change beyond the call-site rename.
7. The full sweep (`cargo fmt --all -- --check`, `cargo clippy
   --all-targets --all-features -- -D warnings`, `cargo test`, `cargo
   test --all-features`, `cargo doc --no-deps` with zero warnings) is
   green.
8. `ADR-0010`, `SERVER-001`, `PROJECT-STATUS` item 33, and `ADR-0019`'s
   revisit trigger each carry a resolution pointer to `ADR-0021` and the
   spec the implementation registers.

## Verification plan

- Capture commit: a temporary test (or the golden tests themselves,
  written against the free functions) printing the hex for every
  `BINENC-FR-004` value on current `main`; hex literals checked in;
  tests green.
- Routing commit: `codec.rs` added, 23 sites renamed, golden tests
  switched to `codec::*`; tests green with the literals unchanged.
- `cargo test` (default features: the 3 blob modules' vectors + the
  primitives), `cargo test --features server` (the protocol vectors),
  `cargo test --all-features` (research routing compiles and its
  round-trip tests pass).
- No benchmark: no runtime path changes (ZST options, same underlying
  code). If a reviewer wants proof, one `cargo bench --bench workloads
  -- production` before/after is a five-minute check.

## Traceability

| Requirement | Where it lands (implementation unit) |
|---|---|
| `BINENC-FR-001`, `-005` | `src/codec.rs` (module, docs), `src/lib.rs` |
| `BINENC-FR-002`, `-004` | golden tests in `codec.rs`, `server/protocol.rs`, the three blob modules |
| `BINENC-FR-003` | the 23 call-site renames; acceptance criterion 1 |
| `BINENC-FR-006` | `options()`'s trailing policy; acceptance criterion 4 |
| `BINENC-FR-007` | `ADR-0010`, `SERVER-001`, `PROJECT-STATUS` item 33, `ADR-0019` pointers |
| `BINENC-FR-008` | `Cargo.toml` untouched; `BLOB_VERSION`s untouched; `pub(crate)` |
| Spec | one new spec registered at implementation — recommended `STORAGE-018` (the codec is the storage layer's shared encoding; `SERVER-001` amended to cite it) over a new `ENCODING-` category; owner's call, flagged at acceptance |

## Open questions

- **Wire-protocol versioning.** Frames carry no protocol version and
  there is no hello handshake, so a `Request`/`Response` shape change is
  a named incompatibility, not a negotiated one. This design pins the
  encoding under the shape; the shape's evolution is `SERVER-001`'s and
  a future ADR's. Revisit trigger: a second deployed client build.
- **`bincode` 2.x.** When or whether to migrate. The pinned
  configuration corresponds to 2.x's `config::legacy()`; the golden
  vectors are the migration's acceptance test. Not proposed now — no
  driver, and 1.3.3 is stable by its own promise.
- **`Uuid` at 24 bytes.** Every id costs 8 bytes of length prefix under
  this configuration. A `[u8; 16]`-serializing newtype would save a
  third of every id — and is a format change to every blob and every
  frame. Not proposed; recorded so the cost is known.
- **Spec placement.** `STORAGE-018` (recommended) versus a new category.
  Flagged for acceptance.

## Change history

- 2026-09-02: Proposed.
