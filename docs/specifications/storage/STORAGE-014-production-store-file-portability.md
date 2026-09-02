# STORAGE-014 — `ProductionStore` file portability (companion record blob + `open_portable`)

- Version: 0.2.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-009` (`MmapAgeStore`), `STORAGE-011`
  (`ProductionStore`), ADR-0016 and
  `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` (both Accepted —
  the design this spec turns into requirements)
- Supersedes: none. Extends, does not reverse, ADR-0006's ages-only
  scope-down: `MmapAgeStore`'s own file format is unchanged.

## Purpose and scope

`ProductionStore`'s on-disk state was, until this spec, only the `age`
column (`MmapAgeStore`'s file): `ProductionStore::open(records, edges,
path)` needed the caller to already hold the full `Vec<DogRecord>`/edge
list in memory. Since `MMAP-AGE-STORE-IDENTITY-FIX` that has been this
project's own standing open question — *"the file is still not portable/
self-contained the way SQLite's/DuckDB's are"* — made concrete by
`RESULTS.md`'s `## External database comparison`.

This spec closes it, additively, exactly as the accepted design
proposes: `create` also writes a **companion record blob** next to the
ages file holding the immutable half of the state (`id`, `breed`, every
`littermate_of` edge); a new constructor, `open_portable(path)`, reopens
a store from `path` alone. Two files, one caller-supplied path, zero
changes to `MmapAgeStore` or to the existing constructors' signatures.
The five requirements below are the design document's
`PORTABILITY-FR-001..005`, renumbered into this spec's namespace, plus
the one implementation decision the design left open (how `open`
detects a changed record set — see `STORAGE-014-FR-001`'s second
paragraph and "Open questions").

## Non-goals

- Not a change to `MmapAgeStore`'s file format, public API, or its 11
  existing tests (`STORAGE-014-FR-003` forbids it).
- Not portability for `crate::generic`/`GenericMmapStore` (the identical
  gap, `GENERIC-SCHEMA-DESIGN.md` §4.2) or for any Tier 1/2 research
  variant — a separate, later decision per ADR-0016.
- Not a single self-describing file. The design chose two files that
  travel together over a string-heap rewrite of the ages file; "portable"
  here means "copy the two files, reopen with one path."
- Not mutation of `breed`/`id`/edges after `create`. The blob's
  immutability is load-bearing (ADR-0016's first revisit trigger).
- Not a schema-migration story for the blob. Its header is versioned so
  an incompatible blob is *detected*, not migrated — same detection-only
  posture `MmapAgeStore`'s own `SchemaVersionMismatch` takes.

## Context and terminology

- **Ages file**: `MmapAgeStore`'s own file at the caller-supplied `path`
  — the mutable half of the state (`age` per record), unchanged by this
  spec.
- **Companion record blob** (or just **the blob**): the new sibling file
  at `companion_path(path)` = `path` with `.records` appended
  (`ages.mmap` → `ages.mmap.records`): a 20-byte header (`DOGBLOB\0`
  magic, `u32` little-endian blob version, currently `2`, then the
  record set's `u64` little-endian **fingerprint**) followed by the
  `bincode` encoding of `{ records: Vec<DogRecord>, edges: Vec<(Uuid,
  Uuid)> }`, in caller order. `DogRecord`'s `age` field rides along
  because the type already serializes it; on read it is only the seed
  `MmapAgeStore::open`'s reconciliation uses for a record the ages file
  doesn't yet hold — the ages file stays the source of truth for every
  live age. (Version 1, `0.1.0` of this spec, had a 12-byte header with
  no fingerprint.)
- **Fingerprint**: FNV-1a 64 (implemented inline, no dependency) over
  the record set's immutable content only — record count; each record's
  `id` bytes, `breed` length, and `breed` bytes, in order; edge count;
  each edge's two `id`s, in order. Ages are excluded on purpose: they
  belong to the ages file. Order-sensitive, as the blob is.
- **Current blob**: a blob whose header is readable, carries this
  build's blob version, and records the fingerprint of the record set a
  caller just handed `open`. (In `0.1.0`: a blob whose bytes equalled,
  exactly, the encoding of that record set — a stricter test that also
  counted seed ages, at the cost `STORAGE-014-FR-001` describes.)

## Requirements

- `STORAGE-014-FR-001` (design `PORTABILITY-FR-001`): `create(records,
  edges, path)` persists `id`, `breed`, and every edge to the companion
  blob at `companion_path(path)` — a location derived from `path`, never
  a second caller-supplied path — in the same call, after the ages file
  is created.

  `open(records, edges, path)` keeps the blob in step with the caller's
  record set: the blob's header (20 bytes) is read and its fingerprint
  compared with the fingerprint of the caller's set; if the blob is
  missing, unreadable, from another blob version, or fingerprints
  differently, it is (re)written after `MmapAgeStore::open` succeeds.
  The steady-state check (the same dataset on every `open`) serializes
  nothing and reads nothing beyond the header; the record set is only
  encoded when a rewrite is actually needed. The rewrite runs after the
  ages file opens so an ages-file error never clobbers a valid blob for
  a different dataset. A pre-`STORAGE-014` directory (ages file only)
  and a version-1 blob (spec `0.1.0`, no fingerprint) are thereby both
  upgraded on their first `open`, no migration step.

  Two record sets that differ only in seed ages share a fingerprint, so
  `open` leaves the blob alone for them — correct, since the ages file
  (which `MmapAgeStore::open` has just reconciled against the same
  caller set) is the source of truth for every age, and `open_portable`
  reads the blob's ages only as seeds for records the ages file lacks,
  of which there are then none.
- `STORAGE-014-FR-002` (design `PORTABILITY-FR-002`):
  `ProductionStore::open_portable(path) -> Result<Self, DurabilityError>`
  reconstructs a fully working `ProductionStore` from `path` alone — no
  `records`/`edges` argument. Every `DogStore`/`ConcurrentStore`/
  `TransactionalStore` method behaves identically to a store built via
  `create`/`open` with the same logical dataset, `breed` and
  `same_breed`/`neighbors` included. `open_portable` never writes the
  blob (it *is* the record set, so it is current by construction).
- `STORAGE-014-FR-003` (design `PORTABILITY-FR-003`): `MmapAgeStore`'s
  on-disk layout and public API, `src/durability/mmap_store.rs` in its
  entirety, and `create`/`open`'s existing signatures are unchanged —
  verified by an empty `git diff` on that file and by every existing
  `create`/`open` call site (README, `dog_server`, every integration
  test, `benches/server.rs`) compiling unmodified.
- `STORAGE-014-FR-004` (design `PORTABILITY-FR-004`): every blob write
  goes to a sibling temp path (`<companion>.rewrite-tmp`), is `fsync`'d,
  then `rename`d into place — `MmapAgeStore::write_via_rename`'s own
  established mechanism. The companion path never holds a partial blob:
  a crash before the rename leaves the prior generation (or nothing), a
  crash after leaves the new, complete file. A stale temp file from an
  interrupted write is overwritten and consumed by the next write.
- `STORAGE-014-FR-005` (design `PORTABILITY-FR-005`): a missing, non-blob
  (wrong magic or shorter than a header), incompatible-version,
  non-decoding, or fingerprint-mismatched (the body decodes but is not
  the record set the header claims — a spliced or bit-flipped file) blob
  at `open_portable` time is one new, distinctly-named variant,
  `DurabilityError::RecordBlobUnreadable { path, cause }`, with `cause`
  naming which of those it is. Never `InvalidMagic`/
  `SchemaVersionMismatch` (those describe the ages file — a caller who
  copied only the `.mmap` file must be told the *companion* is the
  problem), never a panic.

## Architecture and interfaces

- `src/durability/record_blob.rs` (new, `pub(crate)`, unconditional —
  not `research`-gated, since `ProductionStore` uses it):
  `companion_path(&Path) -> PathBuf`; `RecordBlob { records, edges }`
  (`Serialize`/`Deserialize`/`PartialEq`) with `fingerprint() -> u64`,
  `encode() -> EncodedRecordBlob`, `read(&Path) -> Result<Self>` (header
  check, body decode, then fingerprint verification), and
  `is_current_at(&Path) -> bool` (one `fingerprint` pass plus a
  header-only read — never the body); `EncodedRecordBlob` (the
  header+body image) with `write(&Path)` (the temp-then-rename
  install). `encode` is split from `write` so `ProductionStore`
  serializes *before* moving `records`/`edges` into `MmapAgeStore` by
  value — no clone of the record set to persist it. A private
  `Fnv1a64` (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`)
  and `parse_header` are the only additions the fingerprint needed.
- `src/durability/mod.rs`: declares `record_blob`; adds
  `DurabilityError::RecordBlobUnreadable`.
- `src/production.rs`: `create` = encode → `MmapAgeStore::create` →
  blob write; `open` = `is_current_at` → (encode only if stale) →
  `MmapAgeStore::open` → write if stale; `open_portable(path)` =
  `RecordBlob::read` → `MmapAgeStore::open`. Module docs gain a "File
  portability" section.
- No new dependency; `bincode`/`serde`/`DogRecord`'s existing derives are
  reused unchanged, as ADR-0016 required.

## Data/state and invariants

- The two files are a unit: `open_portable` needs both at their derived
  relative locations. Copying both (to any directory, under any ages
  file name) and reopening with the new ages path works; copying one
  does not, and says so (`STORAGE-014-FR-005`).
- After any successful `create` or `open`, the blob is current for the
  record set that call was given. After `open_portable`, the blob is
  untouched.
- A blob's header fingerprint equals `fingerprint()` of its decoded
  body — `write` establishes it, `read` verifies it. The fingerprint is
  a fixed function (FNV-1a 64 over a fixed byte layout), pinned by a
  unit test, so the same record set fingerprints identically on every
  build; a change to the hashed fields or the hash is a format change
  and a `BLOB_VERSION` bump, never a silent rewrite of every deployed
  blob on its next `open`.
- The fingerprint is a change detector against an honest caller (did
  `open` get a different dataset than the blob holds?), not a defense
  against an adversary crafting a 64-bit collision — the same trust
  posture as the ages file, which carries no checksum at all.
- The ages file is the source of truth for every age; the blob's ages
  are seeds only, exactly as a caller's `records` argument to `open` is.

## Errors, failure, recovery, and observability

- `create`/`open`: `DurabilityError::Io` for any blob write failure, on
  top of `MmapAgeStore`'s own errors; `DurabilityError::Serde` if the
  record set can't be serialized (unreachable for any `DogRecord` this
  crate builds, but propagated rather than `expect`ed).
- `create` failing between the ages file and the blob leaves an ages
  file without its companion — the same state `open` already heals.
- `open_portable`: `RecordBlobUnreadable` before the ages file is
  touched; otherwise `MmapAgeStore::open`'s own errors.
- No `unwrap`/`expect` outside `#[cfg(test)]` — the header's version
  and fingerprint bytes are read by bounds-checked slicing after the
  length check, the same pattern `read_wal_entries` uses.

## Security, privacy, and compatibility

- Backward compatible on disk: an ages file written before this spec
  opens exactly as before via `open`, which then writes the missing
  blob. Only `open_portable` requires the blob.
- Blob version 1 → 2 (spec `0.1.0` → `0.2.0`): `open` upgrades a
  version-1 blob in place on first use (it counts as stale); the one
  visible consequence is that `open_portable` on a directory whose blob
  was written by a `0.1.0` build and never since opened via `open`
  returns `RecordBlobUnreadable` with a `version` cause (`file has 1,
  this build expects 2`) until an `open` has run. Detection-only, per
  the non-goals: no migration reader for version 1.
- Forward-detecting: a blob with a newer `BLOB_VERSION` is refused with
  a `version` cause, never partially decoded.
- The blob's magic (`DOGBLOB\0`) differs from `MmapAgeStore`'s
  (`DOGMMAP\0`) and `GenericMmapStore`'s, so pointing `RecordBlob::read`
  at either of those files fails on the header, before `bincode` sees a
  byte.
- Synthetic data only; no network surface.

## Acceptance criteria

- `create(records, edges, path)` → drop → `open_portable(path)` returns
  a store whose `get`/`scan_ages`/`same_breed`/`neighbors` answers are
  identical to the original's for every record — including `breed`, and
  including an `age` written and `flush`ed before the drop.
- Both files copied to a fresh directory (under a different ages file
  name) → `open_portable` there succeeds with the same answers.
- Only the ages file present → `open_portable` returns
  `RecordBlobUnreadable` naming the companion path; `open` on the same
  directory still succeeds and writes the blob, after which
  `open_portable` succeeds.
- `open` with an unchanged record set — including one whose seed ages
  differ — leaves the blob's bytes and mtime alone; `open` with a
  changed set (a record and an edge added) rewrites it, and a
  subsequent `open_portable` sees the new record's `breed` and edge.
- A version-1 blob (written by the `0.1.0` layout) → `open_portable`
  returns `RecordBlobUnreadable`; `open` rewrites it in the version-2
  layout, after which `open_portable` succeeds.
- `MmapAgeStore`'s 11 existing tests pass unchanged;
  `git diff src/durability/mmap_store.rs` is empty; no existing
  `create`/`open` call site changes.
- The record blob's own unit tests cover: path derivation; round trip
  including `breed`/edges; missing file; wrong magic (an ages file's own
  header); shorter-than-header; version mismatch; a version-1 blob;
  truncated body; fingerprint mismatch (header bytes flipped) — each as
  `RecordBlobUnreadable` with the expected `cause` substring; the
  temp-then-rename replacement consuming a stale temp file;
  `is_current_at` detecting a removed record or edge and ignoring a
  changed age; and the fingerprint's pinned value, order sensitivity,
  and sensitivity to `id`/`breed`/edge changes but not to age.
- `create()`'s and `open()`'s added cost is *measured* (ADR-0016 asked
  for it), reported in `RESULTS.md`, and named in "Open questions" below
  — not assumed negligible. As of `0.2.0`, `open`'s steady-state cost
  is re-measured against the `0.1.0` figure.

## Verification plan

- `cargo test` (default features) and `cargo test --all-features`: the
  12 `record_blob` tests and 6 `production` portability tests above,
  plus every pre-existing test, passing.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo doc --no-deps`: clean.
- `git diff --stat src/durability/mmap_store.rs`: empty.
- A scratch (uncommitted) release-mode timing of `MmapAgeStore::create`/
  `open` (the exact pre-spec `ProductionStore` path) vs.
  `ProductionStore::create`/`open`/`open_portable` at 1K/100K/1M records
  from `bench_support::build_dataset`, median of 7/7/3 samples — the
  numbers are in `RESULTS.md`'s `### ProductionStore file portability
  (STORAGE-014)` subsection.

## Traceability

Implements: ADR-0016 / `PRODUCTION-STORE-PORTABILITY-DESIGN.md`
(`PORTABILITY-FR-001..005` ↔ `STORAGE-014-FR-001..005`, one-to-one).
Depends on: `STORAGE-009` (`MmapAgeStore`, the ages file and the
`write_via_rename` pattern reused), `STORAGE-011` (`ProductionStore`,
the type extended). Feeds: `RESULTS.md`'s portability subsection and
the resolution of the `## Open questions` entry `RESULTS.md`/
`PROJECT-STATUS.md` carried since `MMAP-AGE-STORE-IDENTITY-FIX`.

## Open questions

- **`open`'s comparison is not free — measured, not the design's
  "expected zero."** *(`0.1.0`; resolved in `0.2.0`, kept as the
  record.)* The design assumed `open` could piggyback on the rewrite
  decision `MmapAgeStore::open` already makes internally. That
  decision (`open`'s `missing` flag) is private to `mmap_store.rs`, and
  `STORAGE-014-FR-003` forbids exposing it, so `ProductionStore::open`
  detects a changed record set independently: encode the caller's set,
  read the blob, compare bytes. Measured (release, this container):
  `open` went from 0.60 → 0.76 ms at 1K, 111 → 144 ms at 100K, 2.12 →
  2.69 s at 1M (+27%); `create` from 6.1 → 7.1 ms, 90 → 151 ms, 1.57 →
  2.80 s (+78%, dominated by writing a blob 5.6× the ages file's size —
  ~118 B/record with edges vs. 21 B/record of ages). Neither
  constructor is inside any benchmark's `b.iter()` (every call site is
  setup), so no published number moves, and `open_portable` itself
  (2.22 s at 1M) is *cheaper* than `open` since it skips the comparison.
  If `open`'s cost ever matters, the cheap next step is a content hash
  of the body in the blob header so `open` reads 20 bytes instead of the
  whole file — and, beyond that, a fingerprint computed from `records`
  without serializing them, so the unchanged case does no O(N) encode
  either. Both are format changes (a `BLOB_VERSION` bump), deferred
  until a real caller needs them, per ADR-0016's own revisit trigger.

  **`0.2.0` resolution**: both steps at once, as blob version 2 — the
  fingerprint is computed from `records` without serializing them and
  stored in the header, so the unchanged case is one hash pass and a
  20-byte read. Re-measured in the same way (release, same container
  — which ran ~30% slower this session than at `0.1.0`, so only the
  same-run deltas are comparable, not the absolute figures): `open`
  `MmapAgeStore` vs. `ProductionStore` 0.61 → 0.68 ms at 1K (+11%),
  119.4 → 123.9 ms at 100K (+4%), 2,760 → 2,877 ms at 1M (+4%); a
  second run gave +31% / +11% / +0.3%. The 1K figure is dominated by
  the fixed cost of one extra file open and ~0.1 ms of noise; at 1M
  the +27% is gone. `create` is unchanged by design (it must still
  serialize and write the whole blob: +65–78% at 1M in both runs).
  `open_portable` still decodes the full blob, so it lands above the
  fingerprinted `open` now (3.2–3.3 s vs. 2.8–2.9 s at 1M) — the price
  of needing no `records`, unchanged from `0.1.0` in absolute terms.
  Remaining, and now deliberately unaddressed: the fingerprint is
  order-sensitive, so the same records supplied to `open` in a
  different order rewrite the blob once; `MmapAgeStore::open`'s own
  reconciliation is order-insensitive since `MMAP-AGE-STORE-IDENTITY-
  FIX`. Canonicalizing (sort before hashing) would cost an O(N log N)
  sort on every `open` to save one rewrite in a case no caller in this
  crate exercises — not taken.
- The companion naming convention is finalized as `<path>.records`
  (appending, not replacing, the extension) — chosen so it never
  collides with or depends on whatever extension a caller gave the ages
  file (`benches/server.rs` uses `dogs.mmap`, the doctest `dogs.mmap`,
  `fresh_backing_path` `ages.mmap`).
- `GenericMmapStore` portability remains out of scope (ADR-0016).

## Change history

- 0.2.0 (2026-09-02): Blob version 2 — a 64-bit FNV-1a content
  fingerprint (immutable content only, ages excluded) in the header,
  making `open`'s "is the blob current?" check a hash pass plus a
  20-byte read instead of a full serialize-and-compare; `read` verifies
  the decoded body against it. Version-1 blobs are upgraded in place by
  `open`. Re-measured: `open`'s 1M overhead +27% → +0.3–4%. Resolves
  the first open question; adds the order-sensitivity note. 4 new
  tests (3 `record_blob`, 1 `production`).
- 0.1.0 (2026-09-02): Initial accepted draft, alongside the real
  implementation (`src/durability/record_blob.rs`, `src/durability/
  mod.rs`, `src/production.rs`), 14 new tests, and the measured
  `create`/`open` cost in `RESULTS.md`. Registers the design ADR-0016
  accepted on 2026-09-01 as requirements; records the one place the
  implementation diverged from the design's expectation (the
  read-and-compare on `open`) honestly rather than silently.
