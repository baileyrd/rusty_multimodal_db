# Companion Blob Schema Tag Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, the hashed 8-byte tag over the
  readable 32-byte string, no changes requested). Nothing in this
  document is implemented yet; see
  `docs/decisions/ADR-0019-blob-schema-tag-proposal.md` for the decision
  record this document backs.
- Date: 2026-09-02
- Related: `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` and
  `docs/decisions/ADR-0017-generic-store-file-portability-proposal.md`
  (the `GENBLOB\0` record blob, `STORAGE-015` v0.1.0 — its "Open
  questions" defers exactly this), `docs/design/SYMMETRIC-EDGE-
  PORTABILITY-DESIGN.md` and `ADR-0018` (the `GENEDGE\0` edge blob,
  `STORAGE-016` v0.1.0 — defers the same question and agrees to resolve
  it together with `STORAGE-015`), `STORAGE-014` v0.2.0 (the shared
  20-byte header this design extends, and the `DOGBLOB\0` version 1 → 2
  migration whose shape is reused), `ADR-0009` (the generic schema
  library — the blobs are generic over `R`, which is why they can hold
  the wrong `R`)

## Purpose and scope

`STORAGE-015` and `STORAGE-016` gave the generic library two companion
blobs: `<path>.records` (`GENBLOB\0`, the `R` records a `GenericMmapStore`
was built from) and `<path>.edges` (`GENEDGE\0`, the `(R::Id, R::Id)`
pairs a `Symmetric` layer was built from). Both are generic over `R`, and
neither records which `R` it holds. Both specs name this in their open
questions and defer it to be resolved together; `ADR-0018`'s revisit
triggers say *"the same `BLOB_VERSION` bump applies here, at the same
time."* The owner queued it as follow-up 2 of four ("1, 2, 3, then 4").

The gap is concrete. `GenericMmapStore::<Order, _, _>::open_portable(path)`
handed a `.records` blob written by `GenericMmapStore::<Employee, _, _>`
does not fail at the header — magic and version match, the fingerprint
matches (it covers the body, whichever `R` encoded it), and the failure,
if any, comes from `bincode` refusing to decode `Employee` bytes as
`Order`. `bincode` is not a schema check: two record types whose field
sequences happen to share a layout decode silently into each other, and a
prefix-compatible layout can decode and then hit a "trailing bytes" or
"unexpected end" error that names the wrong problem. The edge blob is
worse — every `Uuid`-keyed domain's edge blob decodes as every other's,
because `Vec<(Uuid, Uuid)>` is `Vec<(Uuid, Uuid)>`.

This proposal adds an 8-byte schema tag to both blob headers, sourced from
a new opt-in trait the domain implements once per record type, and bumps
both blobs to version 2 so that a version-1 file is refused (or, on
`open`, rewritten) rather than misread. It reuses the `DOGBLOB\0` version
1 → 2 migration shape `STORAGE-014` v0.2.0 already established.

**In scope for this proposal:**

- A new public trait, `SchemaTag`, in `src/generic/traits.rs`, with one
  associated `const SCHEMA_TAG: &'static str` — a stable, domain-chosen
  name for the record type. Opt-in; not a supertrait of `Record`.
- An 8-byte field appended to the shared 20-byte header of both generic
  blobs — the FNV-1a 64 hash of `SCHEMA_TAG` — so the tagged header is 28
  bytes, and the first 20 are byte-for-byte the `STORAGE-014` layout.
- `GENBLOB\0` and `GENEDGE\0` `BLOB_VERSION` 1 → 2, i.e. `STORAGE-015`
  v0.2.0 and `STORAGE-016` v0.2.0, registered by the implementation unit.
- The `R: SchemaTag` bound on `GenericMmapStore`'s `create`/`open`/
  `read_portable_records`/`open_portable` and on `Symmetric`'s
  `create`/`open`/`read_portable_edges`/`open_portable` — the functions
  that touch a blob — and `SchemaTag` impls for the two record types this
  crate persists through them, `Order` and `Employee`.
- Tag verification on every read path, ordered after magic and version
  and before the fingerprint and body decode, as a typed
  `RecordBlobUnreadable` whose cause names the expected tag string.
- `is_current_at` requiring the tag to match, so `open` over a stale,
  version-1, or foreign-`R` blob rewrites it — the heal-on-`open` story
  `STORAGE-015-FR-003` and `SYMPORT-FR-004` already promise.

**Explicitly out of scope, named directly:**

- `DOGBLOB\0` (`RecordBlob`, `STORAGE-014`). It holds exactly one type,
  `DogRecord`, and its magic *is* its schema tag. `BLOB_VERSION` stays 2.
- The `.mmap` file (`GenericMmapStore`'s hardened slot file, `STORAGE-
  013`). It records slot widths, not `R`; it makes the same trust
  assumption today and is not touched. Named in "Open questions".
- `Record`, `Symmetric::new`, `Reversed`, the in-memory stacks
  (`build_employee_generic_store`, `dog_impl.rs`'s spike stack), and
  every `Neighbors`/`GetById`/`FilterEq`/`ScanField`/`UpdateField`/
  `Flush` impl — none touches a blob, none gains a bound.
- Distinguishing two `Symmetric` relations over the same `R` (two
  `Marker`s). The edge blob's tag is `R`'s tag; see "Considered options"
  for why not a `Marker`-level tag, and "Open questions".
- A human-readable tag in the file. The tag is hashed; the readable
  string appears in the error message from the expecting side only. See
  "Considered options" for the fixed-width-string alternative and the
  acceptance question that offers it.
- A stack-level manifest. Still open, still `ADR-0018`'s.

## Non-goals

- Not a security boundary. A tag is a schema check against honest
  mistakes (the wrong file at the right name, a directory copied from a
  different domain's stack), not against a crafted file. `STORAGE-014`'s
  threat model throughout.
- Not a record-layout fingerprint. The tag says *which* `R` the domain
  meant; it does not detect that `R`'s fields changed between builds.
  That is unit 4's territory (bincode encoding stability), and the
  `STORAGE-015` v0.2.0 trait-method fingerprint the owner closed as not
  warranted (`PROJECT-STATUS`, `ADR-0017` acceptance history) is not
  reopened by this design.
- Not a change to what either blob's body holds, how it is hashed, or
  how it is written. Body bytes, the body fingerprint, and the
  temp-then-rename write are unchanged; the header grows by 8 bytes.
- Not a change to `open`'s cost class. The steady-state check grows from
  a 20-byte read to a 28-byte read and one more `u64` compare.

## Context and terminology

`STORAGE-014` v0.2.0 fixed the shared header every blob in this crate
uses (`src/durability/record_blob.rs`, `pub(crate)`):

| Offset | Width | Field |
|---|---|---|
| 0 | 8 | magic (`DOGBLOB\0`, `GENBLOB\0`, `GENEDGE\0`) |
| 8 | 4 | `u32` LE blob version |
| 12 | 8 | `u64` LE FNV-1a 64 fingerprint of the body |
| 20 | — | body (`bincode`) |

`parse_header(bytes, magic, expected_version) -> Result<u64, String>`
checks magic then version and returns the claimed fingerprint;
`encode_image(magic, version, fingerprint, body)` builds the image;
`Fnv1a64` streams the hash. `HEADER_LEN = 20`. Both generic blobs read a
file as: `std::fs::read` → `parse_header` → body = `bytes[HEADER_LEN..]`
→ FNV compare → `bincode::deserialize`. Both `is_current_at` read exactly
`HEADER_LEN` bytes and compare `parse_header(...) == Ok(expected_fp)`.

**Schema tag** in this document means a fixed-width value in the header
that identifies which `R` the body was encoded from, chosen by the domain
that owns `R`, and checked by the reader before the body is decoded.
**Tagged header** means the 20-byte shared header plus the 8-byte tag,
28 bytes, at `TAGGED_HEADER_LEN`.

The `DOGBLOB\0` version 1 → 2 bump (`STORAGE-014` v0.2.0) is the
precedent for how a header change lands: version-1 files fail
`parse_header` with a version mismatch on the read-only paths, and
`ProductionStore::open` treats "not current" as "rewrite," so a stack
opened once with its records in hand heals its blob to version 2. This
design lands the same way for both generic blobs.

Two facts about the generic library shape the design:

- `GenericRecordBlob<'a, R>` is generic over `R` and can name
  `R::SCHEMA_TAG` directly. `EdgeBlob<'a, Id>` is generic over `Id` only —
  it never knew `R` — so the tag must reach it as a value (`&'static str`)
  from the `Symmetric` block that does know `R`.
- `GenericMmapStore`'s only public constructors are `create`, `open`, and
  `open_portable`, and all three touch the blob. A bound on those is, in
  effect, a bound on using `GenericMmapStore` at all. `Symmetric::new`
  is the opposite case: it touches no file, and the in-memory stacks
  depend on it staying bound-free.

## Requirements

- `SCHTAG-FR-001`: A new public trait `SchemaTag { const SCHEMA_TAG:
  &'static str; }` in `src/generic/traits.rs`, re-exported wherever
  `Record` is. It is not a supertrait of `Record`; a type implements it
  only if it is persisted through a generic companion blob.
- `SCHTAG-FR-002`: `GENBLOB\0` and `GENEDGE\0` images carry, at offset
  20, a `u64` LE FNV-1a 64 hash of the UTF-8 bytes of `SCHEMA_TAG`. The
  first 20 bytes are exactly the `STORAGE-014` v0.2.0 header with the
  version field reading 2. The body starts at offset 28. The body and its
  fingerprint are unchanged from version 1.
- `SCHTAG-FR-003`: Every read path — `read_portable_records`,
  `open_portable`, `read_portable_edges`, and the `open` variants'
  currency check — verifies the tag against the expecting `R`'s
  `SCHEMA_TAG` hash after magic and version and before the fingerprint
  compare and the body decode. A mismatch never reaches `bincode`.
- `SCHTAG-FR-004`: A tag mismatch is `DurabilityError::RecordBlobUnreadable
  { path, cause }` with `cause` naming the expected tag string and both
  hashes, in the form `schema tag mismatch: this store expects
  \`<SCHEMA_TAG>\` (<expected:#018x>), file holds <found:#018x>`. No new
  variant. A version-1 file is a version mismatch (the existing message),
  not a tag mismatch — version is checked first.
- `SCHTAG-FR-005`: `is_current_at` reads exactly `TAGGED_HEADER_LEN`
  bytes and returns `true` only when magic, version, tag, and fingerprint
  all match. Consequently `GenericMmapStore::open` and `Symmetric::open`
  rewrite a version-1 blob, a foreign-`R` blob, and a stale blob alike,
  and never rewrite a current one — `STORAGE-015-FR-003` and
  `SYMPORT-FR-004` hold unchanged.
- `SCHTAG-FR-006`: The `R: SchemaTag` bound is added to exactly the
  functions that touch a blob: `GenericMmapStore::create`/`open`/
  `read_portable_records`/`open_portable` (moved into their own `impl`
  block for this purpose) and `Symmetric`'s `STORAGE-016` block. The
  private slot helpers, every trait impl on `GenericMmapStore`,
  `Symmetric::new`, and every forwarding impl keep their bounds exactly.
- `SCHTAG-FR-007`: `Order` (`src/generic/order_customer.rs`) and
  `Employee` (`src/generic_spike/employee_impl.rs`) implement `SchemaTag`
  with the tags `"order_customer::Order"` and `"employee::Employee"`. The
  `GenericProductionStore` doc example's `Widget` gains the impl its
  doctest needs. No other record type in the crate implements it.
- `SCHTAG-FR-008`: Two blobs written from the same `R` at different
  paths, or by `GenericMmapStore` and `Symmetric` over the same `R`,
  carry the same tag; magic, not tag, distinguishes a record blob from an
  edge blob. A `GENBLOB\0` file read as an edge blob (or vice versa) is
  still a magic error, exactly as in version 1.
- `SCHTAG-FR-009`: The tag hash is computed by the same `Fnv1a64` the
  body fingerprint uses, over the tag's bytes only, with no length
  prefix or terminator. The same `SCHEMA_TAG` always hashes to the same
  `u64`, on every platform, in every build.

## Architecture and interfaces

### Considered options

**Where the tag string comes from.**

1. *`std::any::type_name::<R>()`.* Rejected — its docs say the output is
   not stable across compiler versions and may change form; it also
   embeds the crate and module path, so a refactor that moves `Order`
   would refuse every existing blob. A schema tag has to be a name the
   domain chooses and keeps.
2. *A required `const` on `Record`.* Rejected — `Record` has nine impls
   in this crate plus the doc example, most of them in-memory-only types
   (`Rule`, `SelectionGroup`, `Source`, `Customer`, `DogRecord`, the
   `store.rs` test `Node`) that will never be near a blob; every one
   would gain a const it never uses, and every external `Record` impl
   would break.
3. *An opt-in trait, `SchemaTag`, bounded only where a blob is read or
   written.* Chosen — two impls in this crate (`Order`, `Employee`), each
   next to the type it names; a type with no blob has no tag.

**How the tag is stored in the header.**

1. *A variable-length string (`u16` length + bytes).* Rejected — the
   header stops being fixed-width, so `is_current_at`'s exact-size read
   becomes two reads, and the body offset becomes data-dependent.
2. *A fixed-width, NUL-padded 32-byte string.* Considered and offered as
   the alternative at acceptance. Readable in a hex dump and in a
   mismatch error (both sides could be printed), at the cost of a 32-
   character cap that is a new create-time failure mode (a tag too long
   is an error the domain hits at its first `create`) and a 40-byte
   header. Not chosen because the readable name is already available on
   the expecting side, which is the side reporting the error.
3. *A `u64` FNV-1a 64 hash of the string, appended after the fingerprint.*
   Chosen — fixed width, the same hash the file already uses, the first
   20 bytes untouched so `parse_header` and `encode_image` stay exactly
   as they are, and a 28-byte currency read. The readable tag string is
   in the error message from the expecting side; the file's own tag is
   reported as the hash.
4. *The slot widths (`R::Id::BYTE_WIDTH`, `R::ScanValue::BYTE_WIDTH`) as
   a weak check.* Rejected — every `Uuid`-keyed, `i64`-scanned domain
   shares them, which is most of this crate, and the edge blob has no
   scan value at all.

**Where the tagged-header helpers live.**

1. *Extend `durability::record_blob`'s `parse_header`/`encode_image` with
   an optional tag.* Rejected — `DOGBLOB\0` has no tag and is out of
   scope; an `Option` parameter on the shared helpers would put a
   generic-library concept into the `ProductionStore` path for nothing.
2. *New `pub(crate)` helpers in `generic::record_blob`, built on the
   unchanged shared ones, imported by `generic::edge_blob`.* Chosen. The
   shared 20-byte header is parsed by the existing `parse_header`; the
   tag is the 8 bytes after it.

**Where the `SchemaTag` bound goes on `GenericMmapStore`.**

1. *On every `impl` block, as `STORAGE-015` did with `Serialize +
   DeserializeOwned`.* Rejected — six blocks would change for a bound
   only four functions use, and every trait impl would advertise a
   requirement it does not have.
2. *On the single existing block at `mmap_store.rs:475`, which holds both
   the private slot helpers and the constructors.* Rejected — the trait
   impls call the private helpers, so they would need the bound too, which
   is option 1 by another route.
3. *Split the constructors and portability functions into their own
   block, bounded `R: SchemaTag`; leave the helpers and trait impls where
   and as they are.* Chosen — the `STORAGE-016` shape on `Symmetric` (the
   new bound in the block that needs it, and there only), applied to
   `GenericMmapStore`. Since every `GenericMmapStore` is built through
   one of those functions, the practical effect is that every `R` used
   with `GenericMmapStore` implements `SchemaTag` — a named breaking
   change for out-of-crate record types, which is the point.

**What the edge blob's tag is.**

1. *A `Marker`-level `const` on `SymmetricRelation<Marker>`.* Rejected —
   it would touch every `SymmetricRelation` impl (including the in-memory
   `DogRecord` spike's), and no durable stack in this crate has two
   `Symmetric` relations; `ADR-0018`'s revisit trigger already names that
   case.
2. *`R::SCHEMA_TAG`, passed as a value into `EdgeBlob`.* Chosen — one
   tag per record type, used by both of its blobs; the magic tells the
   two blobs apart. Two relations over one `R` would share a tag: a
   semantic ambiguity (which relation?), not a decode one (the `Id` type
   is the same either way), named in "Open questions".

**How version-1 files are treated.**

1. *Accept version 1 as "untagged" and skip the check.* Rejected — the
   check would be optional forever, and a foreign-`R` version-1 blob is
   exactly the file this design exists to refuse.
2. *Refuse on the read-only paths, heal on `open`.* Chosen — the
   `DOGBLOB\0` 1 → 2 story: `read_portable_*`/`open_portable` return a
   version mismatch; `open`, which has the records or edges in hand,
   sees "not current" and rewrites. A pre-feature directory is one
   `open` away from being tagged.

### Proposed shape

```rust
// src/generic/traits.rs — new, opt-in, next to `Record`.

/// A stable, domain-chosen name for a record type, written into the
/// header of every generic companion blob that holds `Self` and checked
/// before the blob's body is decoded. Not a supertrait of `Record`: only
/// types that are persisted through `GenericMmapStore` or `Symmetric`'s
/// blobs implement it. Choose a name that survives refactors — it is
/// part of the on-disk format, not a `type_name`.
pub trait SchemaTag {
    const SCHEMA_TAG: &'static str;
}

// src/generic/order_customer.rs
impl SchemaTag for Order {
    const SCHEMA_TAG: &'static str = "order_customer::Order";
}

// src/generic_spike/employee_impl.rs
impl SchemaTag for Employee {
    const SCHEMA_TAG: &'static str = "employee::Employee";
}

// src/generic/record_blob.rs — BLOB_VERSION 1 -> 2, plus pub(crate)
// helpers the edge blob imports. The shared 20-byte header is parsed by
// the unchanged `durability::record_blob::parse_header`.

pub(crate) const TAGGED_HEADER_LEN: usize = HEADER_LEN + 8; // 28

pub(crate) fn tag_hash(tag: &str) -> u64;           // Fnv1a64 over tag bytes

pub(crate) fn encode_tagged_image(
    magic: &[u8; 8], version: u32, fingerprint: u64, tag: &str, body: &[u8],
) -> Vec<u8>;                                        // header(20) + tag(8) + body

/// magic -> version -> tag; returns the claimed body fingerprint.
pub(crate) fn parse_tagged_header(
    bytes: &[u8], magic: &[u8; 8], expected_version: u32, expected_tag: &str,
) -> Result<u64, String>;

impl<'a, R: Serialize + SchemaTag> GenericRecordBlob<'a, R> {
    pub(crate) fn encode(&self) -> Result<EncodedRecordBlob, DurabilityError>;
    pub(crate) fn is_current_at(&self, path: &Path) -> bool; // 28-byte read
}
pub(crate) fn read<R: DeserializeOwned + SchemaTag>(path: &Path)
    -> Result<Vec<R>, DurabilityError>;

// src/generic/edge_blob.rs — BLOB_VERSION 1 -> 2; the tag arrives as a
// value because EdgeBlob is generic over Id, not R.

impl<'a, Id: Serialize> EdgeBlob<'a, Id> {
    pub(crate) fn new(edges: &'a [(Id, Id)], tag: &'static str) -> Self;
    // encode / is_current_at as before, tagged
}
pub(crate) fn read<Id: DeserializeOwned>(path: &Path, tag: &'static str)
    -> Result<Vec<(Id, Id)>, DurabilityError>;

// src/generic/mmap_store.rs — the four blob-touching functions move into
// a block that adds `R: SchemaTag`; the helper block and every trait impl
// keep their bounds exactly.
impl<R, IndexMarker, ScanMarker> GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker>
        + Clone + Serialize + DeserializeOwned + SchemaTag,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    pub fn create(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError>;
    pub fn open(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError>;
    pub fn read_portable_records(path: &Path) -> Result<Vec<R>, DurabilityError>;
    pub fn open_portable(path: &Path) -> Result<Self, DurabilityError>;
}

// src/generic/store.rs — the STORAGE-016 block gains `R: SchemaTag` and
// passes `R::SCHEMA_TAG` to EdgeBlob::new / edge_blob::read.
impl<S, R, Marker> Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker> + SchemaTag,
    R::Id: Serialize + DeserializeOwned,
{ /* create / open / read_portable_edges / open_portable, unchanged signatures */ }
```

Tagged image layout, both magics:

| Offset | Width | Field |
|---|---|---|
| 0 | 8 | magic (`GENBLOB\0` or `GENEDGE\0`) |
| 8 | 4 | `u32` LE blob version = **2** |
| 12 | 8 | `u64` LE FNV-1a 64 fingerprint of the body |
| 20 | 8 | `u64` LE FNV-1a 64 of `SCHEMA_TAG` bytes |
| 28 | — | body (`bincode`), unchanged from version 1 |

## Data/state and invariants

- The tag is a pure function of `SCHEMA_TAG`; it does not depend on the
  body, the path, `Marker`, or the build. Same tag string, same 8 bytes.
- The body fingerprint covers the body only, as in version 1. The tag is
  not covered by the fingerprint and does not cover it; both are checked
  independently, and a file with a valid body and a wrong tag is refused
  before the body is hashed.
- A blob is *current* for `(R, records-or-edges)` iff magic, version,
  tag, and fingerprint all match the expecting side. `open` writes iff
  not current. Same records, same `R`, same bytes — `create` twice
  produces identical images, so `open` after `create` never writes.
- The first 20 bytes of a version-2 image are what `parse_header` reads;
  a version-1 reader (an older build) handed a version-2 file fails with
  its own version mismatch at offset 8, before it could misread the tag
  as body.
- The tag does not encode `Marker` or `Id`. Two `Symmetric` relations
  over one `R` share a tag; two record types sharing a `SCHEMA_TAG`
  string share a tag (a domain error the library cannot detect — the
  domain owns the namespace, as it owns the paths).
- `SCHEMA_TAG` is part of the on-disk format. Renaming it is a
  domain-level format change: every existing blob for that `R` becomes
  "not current" (healed by `open`) or a tag mismatch (on the read-only
  paths). This is by design and is documented on the trait.

## Errors, failure, recovery, and observability

- Check order on every read: magic → version → tag → fingerprint → body
  decode. Each failure is `RecordBlobUnreadable { path, cause }`, `path`
  naming the blob, `cause` naming the check. The four existing causes
  (`magic number mismatch or file too short for a header — not a record
  blob`, `blob version mismatch: file has 1, this build expects 2`,
  `fingerprint mismatch: …`, `body does not decode: …`) are unchanged;
  one is added: `schema tag mismatch: this store expects
  \`order_customer::Order\` (0x…), file holds 0x…`. A file of 20–27 bytes
  with a valid magic and version is `file too short for a tagged header`.
- `open` (both types) never surfaces a tag mismatch: "not current" is
  "rewrite," and the caller's records or edges are the truth. A stack
  opened with the right `R` at a path holding a foreign-`R` blob
  silently replaces it — the same last-writer-wins the blobs already
  have for stale content, and the right answer, since the `.mmap`/`Symmetric`
  layer above was built from the caller's data, not the blob's.
- `open_portable` (both types) surfaces the tag mismatch as its error;
  nothing is built, nothing is written. Recovery is the existing
  non-portable `open` with the data in hand.
- No new error variant, no new log line, no counter. The failure is
  typed and the cause string is specific; that is the observability
  `STORAGE-014`–`016` chose and this design keeps.

## Security, privacy, and compatibility

- **Format compatibility, named breaking**: version-2 readers refuse
  version-1 files (version mismatch) on the read-only paths and heal
  them on `open`; version-1 readers refuse version-2 files (version
  mismatch). No build reads the other's body. There are no version-1
  generic blobs outside this repository's own test fixtures and
  benchmark scratch directories, all of which are regenerated.
- **API compatibility, named breaking**: any out-of-crate `R` used with
  `GenericMmapStore::create`/`open`/`open_portable` or with `Symmetric::
  create`/`open`/`open_portable` must implement `SchemaTag`. In-crate,
  `Order`, `Employee`, and the `Widget` doc example are the whole set.
  `Symmetric::new`, `Reversed`, the in-memory stacks, `Record`, and every
  trait impl are source-compatible.
- **Privacy**: the file gains an 8-byte hash of a short type name. The
  name itself is not written.
- **Security**: none new; a hash is not an integrity or authenticity
  check, and `STORAGE-014`'s threat model (honest mistakes, not
  adversaries) still applies.

## Acceptance criteria

1. A `.records` blob written by `GenericMmapStore::<Employee, _, _>::create`
   read by `GenericMmapStore::<Order, _, _>::read_portable_records` is
   `RecordBlobUnreadable` whose cause begins `schema tag mismatch: this
   store expects \`order_customer::Order\``, and `bincode` is never
   invoked (verified by the cause string, not by a decode error).
2. The same file handed to `GenericMmapStore::<Order, _, _>::open(orders,
   path)` is rewritten as a current `Order` blob; the returned store and
   a subsequent `open_portable` are correct.
3. A version-1 `GENBLOB\0` fixture (the version-2 image with bytes 8..12
   patched to 1 and the tag bytes removed) is a version mismatch on
   `read_portable_records` and is healed by `open`.
4. The `GENEDGE\0` equivalents of 1–3 through `Symmetric::read_portable_
   edges`/`open` over an in-memory inner store, with `Employee` and a
   second `SchemaTag`-implementing test record type sharing the same
   `Id` type (so the tag, not the `Id` encoding, is what refuses it).
5. `is_current_at` is `false` for a wrong-tag file and a version-1 file,
   and `true` for a freshly written one; `open` after `create` writes
   nothing (mtime unchanged — the existing test's assertion).
6. A `GENBLOB\0` file read as an edge blob and a `GENEDGE\0` file read as a
   record blob are still magic errors.
7. Every existing test in `record_blob.rs`, `edge_blob.rs`, `mmap_store.
   rs`, `store.rs`, `employee_impl.rs`, and `tests/mmap_record_identity_
   keying.rs` passes with no change beyond the two `SchemaTag` impls,
   the version constant, and the header-length constant in the tests
   that patch bytes by offset.
8. The `GenericProductionStore` doctest compiles and passes with a
   `SchemaTag` impl for `Widget` added to the example.

## Verification plan

- Unit tests in `generic::record_blob`: tag round trip; wrong tag →
  tag error; version-1 image → version error; short tagged header →
  short error; `tag_hash` is FNV-1a 64 of the bytes (one fixed vector,
  e.g. the empty string hashes to the FNV offset basis
  `0xcbf29ce484222325`); cross-magic still magic error.
- Unit tests in `generic::edge_blob`: the same set through `EdgeBlob`
  with a tag value.
- `mmap_store.rs` and `store.rs` tests for acceptance criteria 1–5 with
  the real constructors.
- The full sweep (`cargo fmt --all -- --check`, `cargo clippy
  --all-targets --all-features -- -D warnings`, `cargo test`, `cargo test
  --all-features`, `cargo doc --no-deps`) green.
- No cost measurement planned: the change is one 8-byte field and one
  `u64` compare on a path `STORAGE-015` measured at ~4% of `open`,
  dominated by the body fingerprint, which is unchanged.

## Traceability

| Requirement | Where it lands (implementation unit) |
|---|---|
| `SCHTAG-FR-001` | `src/generic/traits.rs`, `generic` re-exports |
| `SCHTAG-FR-002`, `-009` | `src/generic/record_blob.rs` helpers, both `BLOB_VERSION`s |
| `SCHTAG-FR-003`, `-004` | `record_blob::read`, `edge_blob::read`, `parse_tagged_header` |
| `SCHTAG-FR-005` | both `is_current_at`; `GenericMmapStore::open`, `Symmetric::open` |
| `SCHTAG-FR-006` | `mmap_store.rs` block split; `store.rs` `STORAGE-016` block |
| `SCHTAG-FR-007` | `order_customer.rs`, `employee_impl.rs`, `production.rs` doctest |
| `SCHTAG-FR-008` | magic unchanged; cross-magic tests |
| Spec | `STORAGE-015` v0.2.0, `STORAGE-016` v0.2.0, `SPEC-REGISTRY` |

## Open questions

- Whether the `.mmap` file itself should carry the tag too. It has its
  own header (`STORAGE-013`) and makes the same trust assumption; the
  blob tag catches the portable-open case, which is the one where a file
  arrives from elsewhere. Deferred — a `.mmap` without its `.records`
  cannot be opened portably anyway.
- Whether two `Symmetric` relations over one `R` need distinct tags. Not
  until a durable stack has two; then a `Marker`-level tag (option 1
  above, rejected for now) or a tag argument on `Symmetric::create` is
  the small change. Aligned with `ADR-0018`'s "second `Symmetric` in one
  stack" revisit trigger.
- The stack-level manifest, still `ADR-0018`'s open question; a manifest
  would be the natural place for readable tag strings if they are ever
  wanted on disk.

## Change history

- 2026-09-02: Proposed.
- 2026-09-02: Accepted as proposed by the owner, no changes requested.
