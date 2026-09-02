# ADR-0019: Tag the generic companion blobs with the record type they hold — `GENBLOB\0` and `GENEDGE\0` version 2

- Status: **Proposed** — awaiting the owner's acceptance
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/BLOB-SCHEMA-TAG-DESIGN.md` (the full design
  document this ADR summarizes),
  `docs/decisions/ADR-0017-generic-store-file-portability-proposal.md`
  and `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (the `GENBLOB\0`
  record blob, `STORAGE-015` v0.1.0 — its "Open questions" defers
  exactly this), `docs/decisions/ADR-0018-symmetric-edge-list-portability-proposal.md`
  and `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (the `GENEDGE\0`
  edge blob, `STORAGE-016` v0.1.0 — defers the same question and its
  revisit trigger says the two resolve together), `STORAGE-014` v0.2.0
  (the shared 20-byte header this extends, and the `DOGBLOB\0` version
  1 → 2 migration whose shape is reused), ADR-0009 (the generic schema
  library)
- Supersedes/Superseded by: none. Extends ADR-0017 and ADR-0018 — the
  blobs' bodies, fingerprints, and write paths are unchanged; the header
  grows by 8 bytes and the version by one.

## Context

`ADR-0017` gave `GenericMmapStore` a `.records` companion blob and
`ADR-0018` gave `Symmetric` a `.edges` one. Both are generic over the
record type `R`; neither records which `R` it holds. Both decisions named
this and deferred it: `GENERIC-STORE-PORTABILITY-DESIGN` — *"Whether the
blob should record which `R` it holds (a caller-supplied schema tag, or
the slot widths as a weak check) — deferred"*; `ADR-0018` — *"the same
schema-tag question, deferred here to be resolved together"* and, as a
revisit trigger, *"the schema-tag question is resolved for `STORAGE-015`
— the same `BLOB_VERSION` bump applies here, at the same time."* The
owner queued it as follow-up 2 of four ("1, 2, 3, then 4"), and
`PROJECT-STATUS` item 66 names it as the next unit.

The failure it addresses is specific. Handed a `.records` blob written
for `Employee`, `GenericMmapStore::<Order, _, _>::open_portable` passes
magic, version, and fingerprint (the fingerprint covers the body,
whichever `R` encoded it) and fails — if it fails — inside `bincode`,
with a message about the bytes rather than about the mistake. Two record
types whose field sequences share a layout decode silently into each
other. The edge blob has no defence at all: every `Uuid`-keyed domain's
`Vec<(Uuid, Uuid)>` decodes as every other's. The `.mmap` file makes the
same trust assumption, but it is the portable-open path — a directory
copied from elsewhere — where a foreign file arrives, and that path is
the blobs'.

`STORAGE-014` v0.2.0 already showed how a header change lands: bump
`BLOB_VERSION`, refuse the old version on the read-only paths, and let
`open` — which holds the data — treat "not current" as "rewrite." This
ADR proposes the same for both generic blobs at once, as the two specs
agreed. A new on-disk field, a version bump on two formats, and a new
public trait that becomes a bound on the library's constructors are
consequential enough for this project's convention to want a decision
record first. This ADR proposes a design and authorizes no
implementation, the posture ADR-0016, ADR-0017, and ADR-0018 all took.

## Decision drivers

- Resolve the one open question `STORAGE-015` and `STORAGE-016` both
  hold, in a single version bump for both, as `ADR-0018` committed to.
- Refuse a foreign-`R` blob before `bincode` sees it, with an error that
  names the expected type — not a decode error, and never a silent
  mis-decode.
- Keep the shared 20-byte header (`durability::record_blob`) and
  `DOGBLOB\0` untouched: the `ProductionStore` path gains nothing from a
  tag (its magic is its schema) and should not carry one.
- Keep the tag's source stable across compiler versions and refactors —
  the domain names its type; the compiler does not.
- Bound only what touches a blob. `Symmetric::new`, the in-memory stacks,
  `Record`, `Reversed`, and every forwarding trait impl are not persisted
  and gain nothing.
- Reuse the `DOGBLOB\0` version 1 → 2 migration story: version-1 files
  are refused on read-only paths and healed by `open`.

## Considered options

**Source of the tag.**

1. `std::any::type_name::<R>()` — rejected: documented as unstable across
   compiler versions, and it embeds the module path, so moving a type
   refuses its own blobs.
2. A required `const` on `Record` — rejected: nine impls in this crate
   plus the doc example, most in-memory-only, all changed; every external
   `Record` impl broken.
3. **Chosen** — an opt-in `SchemaTag { const SCHEMA_TAG: &'static str; }`
   trait, bounded only on the blob-touching functions. Two impls in this
   crate (`Order`, `Employee`) plus the `Widget` doc example.

**Encoding in the header.**

1. Variable-length string — rejected: the header stops being fixed-width
   and the 20-byte currency read becomes two reads.
2. Fixed-width 32-byte NUL-padded string — considered; readable on disk
   and in errors, at the cost of a 32-character cap that is a new
   create-time failure. Offered as the alternative at acceptance.
3. **Chosen** — `u64` LE FNV-1a 64 of the tag bytes at offset 20, after
   the unchanged 20-byte header: fixed width, the hash already in the
   file, `parse_header`/`encode_image` untouched, a 28-byte currency read.
   The readable name is in the error message from the expecting side.
4. Slot widths as a weak check — rejected: every `Uuid`/`i64` domain
   shares them, and the edge blob has no scan value.

**Bound placement on `GenericMmapStore`.**

1. Every `impl` block, the `STORAGE-015` precedent — rejected: six blocks
   for four functions, and every trait impl would advertise a requirement
   it does not have.
2. The existing single block (helpers plus constructors) — rejected: the
   trait impls call the private helpers and would need the bound too.
3. **Chosen** — split the four blob-touching functions (`create`, `open`,
   `read_portable_records`, `open_portable`) into their own block bounded
   `R: SchemaTag`, the `STORAGE-016` shape on `Symmetric`. Since those are
   the only constructors, every `R` used with `GenericMmapStore`
   implements `SchemaTag` in practice — a named breaking change.

**The edge blob's tag.**

1. A `Marker`-level `const` on `SymmetricRelation` — rejected: touches
   every `SymmetricRelation` impl including the in-memory spike's, for a
   two-relations-per-stack case no durable stack has.
2. **Chosen** — `R::SCHEMA_TAG`, passed as a value into `EdgeBlob` (which
   is generic over `Id`, not `R`). The magic distinguishes records from
   edges; two relations over one `R` share a tag, a semantic ambiguity
   named in the design's open questions and aligned with `ADR-0018`'s
   "second `Symmetric` in one stack" revisit trigger.

**Version-1 files.**

1. Accept as untagged — rejected: the check would be optional forever.
2. **Chosen** — refuse on `read_portable_*`/`open_portable` (version
   mismatch), heal on `open` (not current → rewrite), the `DOGBLOB\0`
   story.

## Decision

Proposed, subject to acceptance:

- A new public, opt-in trait `SchemaTag` in `src/generic/traits.rs`
  with one associated `const SCHEMA_TAG: &'static str`, re-exported
  wherever `Record` is; not a supertrait of `Record`.
- Both generic blobs (`GENBLOB\0`, `GENEDGE\0`) go to `BLOB_VERSION` 2
  and carry, at offset 20, a `u64` LE FNV-1a 64 hash of `SCHEMA_TAG`'s
  bytes. The first 20 bytes remain the `STORAGE-014` v0.2.0 header
  (version field reading 2); the body starts at 28; the body and its
  fingerprint are unchanged. `STORAGE-015` → v0.2.0, `STORAGE-016` →
  v0.2.0, registered by the implementation unit.
- New `pub(crate)` helpers in `generic::record_blob` (`TAGGED_HEADER_LEN
  = 28`, `tag_hash`, `encode_tagged_image`, `parse_tagged_header`),
  built on the unchanged shared helpers, imported by `generic::edge_blob`.
  `durability::record_blob` and `DOGBLOB\0` are not modified.
- Check order on every read: magic → version → tag → fingerprint → body
  decode. A tag mismatch is `DurabilityError::RecordBlobUnreadable {
  path, cause }` with `cause` of the form `schema tag mismatch: this
  store expects \`order_customer::Order\` (0x…), file holds 0x…`. No new
  variant.
- `is_current_at` reads 28 bytes and requires magic, version, tag, and
  fingerprint all to match; `GenericMmapStore::open` and `Symmetric::open`
  therefore rewrite a version-1, foreign-`R`, or stale blob and never a
  current one.
- The `R: SchemaTag` bound lands on exactly the blob-touching functions:
  `GenericMmapStore::create`/`open`/`read_portable_records`/
  `open_portable`, moved into their own `impl` block, and `Symmetric`'s
  `STORAGE-016` block. The private slot helpers, every trait impl,
  `Symmetric::new`, `Reversed`, and the in-memory stacks keep their
  bounds exactly.
- `Order` and `Employee` implement `SchemaTag` (`"order_customer::Order"`,
  `"employee::Employee"`); the `GenericProductionStore` doctest's
  `Widget` gains the impl it needs. No other record type does.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No source file is modified by this ADR itself. The next unit
  bumps both specs, updates the registry, and implements per the design
  document.
- Not decided here: tagging the `.mmap` file itself; distinct tags for
  two `Symmetric` relations over one `R`; the stack-level manifest.

## Consequences

### Positive

- Closes the open question both specs hold, in one bump for both, as
  promised — no third "deferred" note.
- A foreign-`R` blob is refused with an error naming the expected type,
  before `bincode` is reached; a layout-compatible mis-decode becomes
  impossible for tagged blobs.
- The shared header, `DOGBLOB\0`, `ProductionStore`, the `.mmap` file,
  the blob bodies, their fingerprints, and the write path are all
  untouched; the `durability::record_blob` diff is zero.
- The tag is the domain's name, chosen once, stable across compiler
  versions and refactors; the library never guesses it.
- Version-1 directories heal on their first `open` with the data in
  hand, exactly as `DOGBLOB\0` version-1 blobs did.
- Steady-state `open` cost: a 28-byte read instead of 20 and one `u64`
  compare, on a path where the body fingerprint dominates.

### Negative / tradeoffs

- **A named breaking change, twice.** On disk: version-2 readers refuse
  version-1 files on the read-only paths, and version-1 readers refuse
  version-2 files; no build reads the other's body. In the API: every
  `R` used with `GenericMmapStore`'s constructors or `Symmetric`'s
  `create`/`open`/`open_portable` must implement `SchemaTag`. In-crate
  the set is `Order`, `Employee`, and the doc example; out-of-crate
  callers add one impl per type.
- **The tag on disk is a hash, not a name.** A hex dump shows 8 bytes;
  the readable string is only on the expecting side, so a mismatch error
  says what was expected and only the hash of what was found. The
  readable alternative (a 32-byte fixed string) is offered at acceptance.
- **`SCHEMA_TAG` is part of the format.** Renaming it makes every
  existing blob for that type "not current" (healed by `open`) or a tag
  mismatch (read-only paths). Documented on the trait; a deliberate
  property, not an accident.
- **Two relations over one `R` share a tag.** A semantic ambiguity, not
  a decode one; no durable stack in this crate has two, and `ADR-0018`'s
  revisit trigger already names the case.
- **A domain can collide its own tags.** Two record types given the same
  `SCHEMA_TAG` string are indistinguishable to the library — the domain
  owns the namespace, as it owns the paths.
- The `.mmap` file still makes the trust assumption the blobs no longer
  do. Named in the design's open questions; the portable-open path is
  the blobs', so the practical exposure is small.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching ADR-0017 and
  ADR-0018 — it extends an accepted, implemented mechanism by one fixed-
  width field, using the hash and helpers the file already has, so the
  implementation's own test suite is the direct verification, per the
  design document's "Verification plan."
- **Real validation, post-acceptance**: `STORAGE-015` v0.2.0 and
  `STORAGE-016` v0.2.0; unit tests in both blob modules (tag round trip;
  wrong tag → tag error naming the expected string; version-1 image →
  version error; short tagged header; `tag_hash` fixed vector; cross-
  magic still magic error); `mmap_store.rs`/`store.rs` tests for an
  `Employee` blob refused as `Order` on the read-only paths and healed by
  `open`, and `is_current_at` false for wrong-tag and version-1 files;
  every existing test passing with only the two impls and the constants
  changed; the `GenericProductionStore` doctest passing.
- Revisit if: a durable stack gains a second `Symmetric` relation over
  the same `R` — a `Marker`-level tag or a tag argument on
  `Symmetric::create` becomes the small next step.
- Revisit if: unit 4 (bincode encoding stability) concludes that the
  record layout, not just the record type, needs recording — that would
  be a second field or a different hash input, and a version 3.
- Revisit if: a use case needs the tag readable from the file without
  the expecting build — the fixed-width string option, or the manifest.
- Revisit if: the `.mmap` file is ever opened portably without its
  `.records` blob — it would then need the tag too.

## Acceptance and implementation

- (pending) Owner's decision: accept as proposed, prefer the readable
  32-byte tag string, or request other changes.
