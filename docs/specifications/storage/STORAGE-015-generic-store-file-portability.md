# STORAGE-015 — `GenericMmapStore` file portability (companion record blob + `open_portable`)

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-012` (`GenericMmapStore`, the generic library),
  `STORAGE-014` (the `ProductionStore` treatment this ports and whose
  blob machinery it shares), ADR-0017 and
  `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (both Accepted —
  the design this spec turns into requirements)
- Supersedes: none. Extends, does not reverse, `GENERIC-SCHEMA-DESIGN.md`
  §4.2's one-durable-field scope: `GenericMmapStore`'s own `.mmap` format
  is unchanged.

## Purpose and scope

`GenericMmapStore<R, IndexMarker, ScanMarker>`'s on-disk state was, until
this spec, one fixed-width `(R::Id, R::ScanValue)` slot per record: both
`create(records, path)` and `open(records, path)` needed the caller to
already hold the full `Vec<R>`, and every relation layer stacked above
the store (`Reversed` in `OrderProductionStack`) was rebuilt from that
same caller-supplied vector. `STORAGE-014` closed the identical gap for
`ProductionStore`; ADR-0016 named the generic half as deliberately out
of scope, and `PROJECT-STATUS.md`'s item 63 queued it as the owner's
"1 then 2" follow-up (b).

This spec closes it, additively, exactly as the accepted design
proposes: `create` also writes a **companion record blob** next to the
`.mmap` file holding the full record set; a new associated function,
`read_portable_records(path)`, reads that set back in persisted order;
and `open_portable(path)` reopens a store from `path` alone. Two files,
one caller-supplied path, zero changes to the `.mmap` slot format or to
the existing constructors' signatures. The eight requirements below are
the design document's `GPORT-FR-001..008`, renumbered into this spec's
namespace, plus the two implementation decisions recorded in
"Traceability" (the `R::Id` bound, and the `Employee` helper).

## Non-goals

- Not a change to `GenericMmapStore`'s `.mmap` layout (`GMMAPST\0`,
  schema version 2, the `[id][value][COMMITTED]` slot), its slot
  helpers, or its reconciliation loop (`STORAGE-015-FR-007` forbids it).
- Not more than one durable field. The blob persists the fields the
  store never mutates; the one `ScanMarker` field stays in the `.mmap`
  file, which remains the source of truth for it.
- Not persistence of a `Symmetric` layer's edge list. `Reversed` needs
  only the records (`ChildOf::parent_id` is on each child);
  `Symmetric::new(inner, edges)` takes an external list no record
  carries, and the layering ADR-0009 established says that state belongs
  to the layer that indexes it. See "Traceability" for the consequence
  for `Employee`.
- Not a change to `GenericProductionStore<S>` — it is store-agnostic by
  design; portability lands entirely below it (its doctest gains an
  `open_portable` round trip, its code does not change).
- Not a schema-migration story for the blob, and not a record of which
  `R` the blob holds. Both are the `.mmap` file's existing posture:
  detect an incompatible version, trust the caller on the type.

## Context and terminology

- **`.mmap` file**: `GenericMmapStore`'s own file at the caller-supplied
  `path` — the mutable half of the state (one `ScanValue` per record),
  unchanged by this spec.
- **Companion record blob** (or just **the blob**): the new sibling file
  at `<path>.records` (`amount.mmap` → `amount.mmap.records`; the
  `STORAGE-014` `companion_path` convention, reused): a 20-byte header
  (`GENBLOB\0` magic, `u32` little-endian blob version, currently `1`,
  then the record set's `u64` little-endian **fingerprint**) followed by
  the `bincode` encoding of `Vec<R>`, in caller order. The `ScanMarker`
  field rides along because `R` serializes it; on read it is only the
  seed `open`'s reconciliation uses for a record the `.mmap` file has no
  slot for yet.
- **Fingerprint**: FNV-1a 64 (the `STORAGE-014` implementation, now
  shared) over the streamed `bincode` encoding of the record set — the
  bytes the blob body would hold, hashed without being materialized.
  Unlike `STORAGE-014`'s hand-walked fingerprint it *includes* the
  mmap-backed field, because a generic `R` gives the blob no view inside
  a record; the consequence is named under "Data/state and invariants".
- **Current blob**: a blob whose header is readable, carries this
  build's blob version, and records the fingerprint of the record set a
  caller just handed `open`.

## Requirements

- `STORAGE-015-FR-001` (design `GPORT-FR-001`): `create(records, path)`
  persists the full `records` to the companion blob at `<path>.records`
  — a location derived from `path`, never a second caller-supplied path
  — in the same call, encoded before `records` is consumed and written
  after the `.mmap` file is created and flushed. The header is the
  `STORAGE-014` v0.2.0 layout (20 bytes, same field order) under a
  magic of its own, `GENBLOB\0`, so a `Dog` blob and a generic blob can
  never be mistaken for one another.
- `STORAGE-015-FR-002` (design `GPORT-FR-002`):
  `read_portable_records(path) -> Result<Vec<R>, DurabilityError>`
  returns the persisted records in the order they were persisted,
  touching only the blob — never the `.mmap` file, never writing.
- `STORAGE-015-FR-003` (design `GPORT-FR-003`):
  `open_portable(path) -> Result<Self, DurabilityError>` is exactly
  `open(read_portable_records(path)?, path)` — it inherits `open`'s
  identity-keyed reconciliation, header/version checks, and
  append-on-missing path unchanged, with no second code path. Because
  the records came from the blob, `open`'s own currency check finds the
  blob current and writes nothing.
- `STORAGE-015-FR-004` (design `GPORT-FR-004`): `open(records, path)`
  keeps the blob current: the blob's 20-byte header is read and its
  fingerprint compared with the fingerprint of the caller's set; if the
  blob is missing, unreadable, from another blob version, or
  fingerprints differently, it is rewritten after the `.mmap` file opens
  successfully. The ordering is `STORAGE-014` v0.2.0's — header check →
  encode only if stale → open the `.mmap` file → write only if stale —
  so an `.mmap` error never clobbers a valid blob for a different
  dataset, and a pre-`STORAGE-015` directory (`.mmap` file only) is
  healed on its first `open`, no migration step.
- `STORAGE-015-FR-005` (design `GPORT-FR-005`): a missing, non-blob
  (wrong magic or shorter than a header), incompatible-version,
  fingerprint-mismatched (the body hashes differently from what the
  header claims — a spliced or bit-flipped file), or non-decoding blob
  at `read_portable_records`/`open_portable` time is the existing
  `DurabilityError::RecordBlobUnreadable { path, cause }`, with `path`
  naming the companion and `cause` naming which of those it is. Never
  `InvalidMagic`/`SchemaVersionMismatch` (those describe the `.mmap`
  file and stay distinct), never a panic, never a silently-empty store.
- `STORAGE-015-FR-006` (design `GPORT-FR-006`): the one new requirement
  on record types: `R: Serialize + DeserializeOwned` joins
  `GenericMmapStore`'s existing bounds on its inherent impl and its
  `GetById`/`FilterEq`/`ScanField`/`UpdateField`/`Flush` impls.
  `Order`, `OrderStatus`, `Customer`, `Employee`, `Department`, and the
  `production.rs` doctest's `Widget` gain `#[derive(Serialize,
  Deserialize)]`; nothing else about them changes, and every existing
  `create`/`open` call site compiles without argument changes.
- `STORAGE-015-FR-007` (design `GPORT-FR-007`): `src/generic/mmap_store.rs`'s
  slot layout, header, `write_slot_into`/`append_committed_slot`/
  `is_committed`, and the reconciliation loop are unchanged in
  behavior; the only edits to `create`/`open` are the added blob
  encode/check/write, the bound lines, and the two new functions —
  verified by `git diff`'s removed lines being the six bound lines and
  nothing else, and by `tests/mmap_record_identity_keying.rs` and the
  module's own pre-existing tests passing unmodified.
- `STORAGE-015-FR-008` (design `GPORT-FR-008`):
  `open_order_production_stack_portable(path)` in `order_customer.rs`
  (re-exported from `crate::generic` under `research`) builds the full
  `OrderProductionStack` from the path alone, via
  `read_portable_records` + the existing `open_order_production_stack`
  — no duplicated stack-building code — so `Reversed`'s per-customer
  child order follows the blob's persisted order, deterministically.

## Architecture and interfaces

- `src/durability/record_blob.rs` (the `STORAGE-014` module, behavior
  unchanged): `HEADER_LEN`, `Fnv1a64` (now also `impl io::Write`, so a
  serializer can stream into it), `parse_header(bytes, magic,
  expected_version)`, `encode_image(magic, version, fingerprint, body)`,
  `companion_path`, and `EncodedRecordBlob::write` become `pub(crate)`,
  parameterized by magic/version where they were `Dog`-constant. This
  is the shared machinery's second real call site — the project's own
  threshold for sharing over duplicating. `RecordBlob`'s 12 tests pass
  unmodified.
- `src/generic/record_blob.rs` (new, `pub(crate)`, unconditional — not
  `research`-gated, since `GenericMmapStore` uses it): `MAGIC =
  GENBLOB\0`, `BLOB_VERSION = 1`; `GenericRecordBlob<'a, R: Serialize>`
  borrowing `&'a [R]` with `fingerprint()` (streams `bincode` into
  `Fnv1a64`, no allocation), `encode() -> EncodedRecordBlob`, and
  `is_current_at(&Path) -> bool` (one fingerprint pass plus a 20-byte
  read — never the body); free `read<R: DeserializeOwned>(&Path) ->
  Vec<R>` (header check, body hashed and verified against the header,
  then decode) and `blob_path(&Path)` (= `companion_path`). 9 tests.
- `src/generic/mmap_store.rs`: `create` = encode → existing create →
  blob write; `open` = `is_current_at` → (encode only if stale) →
  existing open → write if stale; `read_portable_records(path)` =
  `record_blob::read`; `open_portable(path)` = `open(read…?, path)`.
  Module docs gain a "companion record blob" section. 6 new
  `research`-gated tests.
- `src/generic/order_customer.rs`: `open_order_production_stack_portable`
  and 1 new test; `src/generic/mod.rs` declares `record_blob` and
  re-exports the helper. `src/generic/production.rs`'s doctest reopens
  via `open_portable` and re-checks both updated values.
- No new dependency; `bincode`/`serde` and the existing
  `DurabilityError::RecordBlobUnreadable` variant are reused unchanged.

## Data/state and invariants

- The two files are a unit: `open_portable` needs both at their derived
  relative locations. Copying both (to any directory, under any `.mmap`
  file name) and reopening with the new path works; copying one does
  not, and says so (`STORAGE-015-FR-005`).
- After any successful `create` or `open`, the blob is current for the
  record set that call was given. After `open_portable`, the blob is
  untouched (bytes and mtime).
- A blob's header fingerprint equals the FNV-1a 64 of its body bytes —
  `encode` establishes it, `read` verifies it, and the streamed
  fingerprint of the in-memory records equals both (pinned by a test).
  A change to the encoding or the hash is a format change and a
  `BLOB_VERSION` bump.
- **The fingerprint covers the whole record, mmap-backed field
  included.** A caller that reopens with regenerated records whose scan
  values differ from create-time values sees a blob rewrite the `Dog`
  design would skip. That is a cost, not a correctness issue: the
  `.mmap` file stays the truth for that field on every read, `open`
  seeds slots from `records` only for ids with no slot yet, and
  `open_portable` never writes. No call site in this crate does this.
- The persisted fields are immutable through the store: nothing in
  `crate::generic` mutates a record's id, index value, parent id, or any
  non-`ScanMarker` field after construction. This is the load-bearing
  assumption ADR-0017 names as its revisit trigger.
- Blob/`.mmap` disagreement about which ids exist is not a new failure
  mode: `open`'s reconciliation handles it exactly as it handles any
  caller-supplied/file mismatch today.
- Multi-writer behavior for the blob is last-writer-wins by atomic
  rename, each blob self-consistent — the same scope as the `.mmap`
  file's own multi-process story, per the design's non-goals.

## Errors, failure, recovery, and observability

- `create`/`open`: `DurabilityError::Io` for any blob write failure, on
  top of the `.mmap` file's own errors; `DurabilityError::Serde` if the
  record set can't be serialized (propagated, never `expect`ed).
- `create` failing between the `.mmap` file and the blob leaves a
  `.mmap` file without its companion — the same state `open` already
  heals.
- `read_portable_records`/`open_portable`: `RecordBlobUnreadable` before
  the `.mmap` file is touched; otherwise `open`'s own errors.
- Every blob write goes to `<companion>.rewrite-tmp`, is `fsync`'d, then
  `rename`d into place — `STORAGE-014`'s write path, reused verbatim. A
  stray temp file is not a companion and is consumed by the next write.
- No `unwrap`/`expect` outside `#[cfg(test)]`.

## Security, privacy, and compatibility

- Backward compatible on disk: a `.mmap` file written before this spec
  opens exactly as before via `open`, which then writes the missing
  blob. Only the portable path requires the blob.
- Forward-detecting: a blob with a newer `BLOB_VERSION` is refused with
  a `version` cause, never partially decoded.
- The blob's magic (`GENBLOB\0`) differs from `GenericMmapStore`'s
  (`GMMAPST\0`), `MmapAgeStore`'s (`DOGMMAP\0`), and `RecordBlob`'s
  (`DOGBLOB\0`), so pointing `read` at any of those files fails on the
  header, before `bincode` sees a byte — and vice versa.
- The one API change is the bound tightening (`STORAGE-015-FR-006`);
  `publish = false`, every in-crate record type covered.
- Synthetic data only; no network surface.

## Acceptance criteria

- `create(records, path)` → drop → `open_portable(path)` returns a store
  whose `get`/`filter_eq`/`scan`/`update` results are identical to the
  original's for every record — including the non-durable fields
  (`customer_id`, `status`, `created_at_unix_ms`, `discount_cents`) the
  `.mmap` file alone could never supply, and including a value written
  and `flush`ed before the drop.
- Through `open_order_production_stack_portable`: `parent`/`children`
  (the `Reversed` layer) and the `Status` index come back identical from
  the two files alone.
- Both files copied to a fresh directory (under a different `.mmap`
  file name) → `open_portable` there succeeds with the same answers.
- Only the `.mmap` file present → `open_portable` returns
  `RecordBlobUnreadable` naming the companion path; `open` on the same
  directory still succeeds and writes the blob, after which
  `open_portable` succeeds.
- `open` with an unchanged record set leaves the blob's bytes and mtime
  alone, as does `open_portable`; `open` with a changed set rewrites it.
- A `DOGBLOB\0` (`ProductionStore`) blob at the companion path is a
  `magic` cause, not a decode attempt; a truncated `.mmap` file is still
  `InvalidMagic` from both `open` and `open_portable`, and the failed
  `open` leaves the current blob untouched.
- `tests/mmap_record_identity_keying.rs` and `mmap_store.rs`'s 8
  pre-existing tests pass unmodified; `git diff src/generic/mmap_store.rs`
  removes only the six bound lines. `RecordBlob`'s 12 tests and
  `production.rs`'s 6 portability tests pass unmodified.
- The generic blob's own unit tests cover: round trip; the streamed
  fingerprint equalling the header's; a different set not current;
  missing file; `DOGBLOB\0` magic; wrong version; tampered body
  (fingerprint mismatch); truncated body (decode failure); path
  derivation.
- `create()`'s and `open()`'s added cost is *measured* (ADR-0017 asked
  for it), reported in `RESULTS.md`, and judged against the design's
  own trigger (an `open` delta at 1M nearer `STORAGE-014` v0.1.0's +27%
  than v0.2.0's +0.3–4% would call for the trait-method fingerprint).
  Measured in place at ~4% (see "Open questions"); the fallback measured
  and closed as not warranted.

## Verification plan

- `cargo test` (default features) and `cargo test --all-features`: the
  9 `generic::record_blob` tests, 6 `generic::mmap_store` portability
  tests, 1 `order_customer` test, and the updated `production.rs`
  doctest above, plus every pre-existing test, passing.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo doc --no-deps` (no new warnings): clean.
- A scratch (uncommitted) release-mode timing of `GenericMmapStore::
  create`/`open` before vs. after, plus `open_portable`, at 1K/100K/1M
  `Order` records, median of 7/7/3 samples, two runs each — the numbers
  are in `RESULTS.md`'s `### GenericMmapStore file portability
  (STORAGE-015)` subsection.
- `crash_writer`/`crash_safety_harness`/`multiprocess_writer`/
  `multiprocess_harness` build unchanged (`--all-targets --all-features`
  compiles them); their slot semantics are untouched.

## Traceability

Implements: ADR-0017 / `GENERIC-STORE-PORTABILITY-DESIGN.md`
(`GPORT-FR-001..008` ↔ `STORAGE-015-FR-001..008`, one-to-one). Depends
on: `STORAGE-012` (the type extended), `STORAGE-014` (the blob header,
fingerprint, and write path shared). Feeds: `RESULTS.md`'s portability
subsection and the resolution of the "narrower" open question there and
in `PROJECT-STATUS.md` (`GenericMmapStore`'s one-durable-field gap).

Two places the implementation diverges from the design's sketch, on
purpose:

- **No bound on `R::Id`.** The design's shape added `Serialize +
  DeserializeOwned` to `R::Id` as well. Only `Vec<R>` is ever
  serialized, and `R`'s derive already requires its `Id` field to
  serialize, so the extra bound would constrain nothing and is not
  added. `GPORT-FR-006`'s "every existing `Id` already satisfies it"
  remains true, just unneeded.
- **No `Employee` portable helper.** The design allowed one "to the
  extent the non-goals allow", honest in its name about still taking
  the `CollaboratesWith` edge list. `Employee`'s durable stack is
  research-gated spike material with no caller asking for it; its
  record types gain the derives (so `GenericMmapStore<Employee, ..>`
  keeps compiling and now writes a blob), and the helper waits for the
  `Symmetric`-companion decision the design's open questions defer.

## Open questions

- **`open`'s streamed-serialization fingerprint cost — resolved: measured
  in place at ~4% of `open` at 1M; the design's fallback (a per-type
  trait-method fingerprint) measured at the owner's request and closed as
  not warranted.** The first-round record, kept for the trail: at 1K it
  is +0.1 ms on a
  0.15–0.2 ms call (one extra file open plus the hash); at 100K it is
  +19–33% (about 10–17 ms on a ~52 ms `open`, the cost of
  `bincode`-encoding 100K records into the hasher). At 1M — the cell the
  trigger names — the throwaway example's three-sample medians put the
  delta inside noise (−4% to +2% across three after-runs), but the
  published 20-sample Criterion `generic_production_open` group, run
  twice in the same session against a `git stash`-isolated before-run,
  puts it at +24–27% (about 300 ms on a 1.25 s `open`); `RESULTS.md`
  records both and treats the Criterion figure as the trustworthy one.
  That is nearer `STORAGE-014` v0.1.0's +27% than v0.2.0's +0.3–4%, so
  the design's named fallback — a per-type trait-method fingerprint over
  immutable fields only, no serialization pass — is the next step, an
  owner's call (it adds to the record traits' API, its own decision).
  `generic_production_create` and `generic_production_open` are the two
  published Criterion groups that time these constructors. **Second
  round**: an in-place A/B (the same binary, `is_current_at` with its
  `fingerprint()` call stubbed out) puts `open` at 1M at 1,222 ms with the
  fingerprint and 1,170 ms without — ~52 ms, 4%; the Criterion group
  re-run on unchanged code moved +8.8% against itself, so its +24–27%
  was drift. In isolation the streamed encoding is 79 ms, a hand-walk
  over every `Order` field 72 ms (−7 ms, the same bytes hashed), and
  only hashing fewer fields goes lower (42 ms / 21 ms) — at the cost of
  a reopen with a changed non-hashed field keeping the blob silently
  stale. Offered the three-way choice, the owner closed it: the shipped
  fingerprint stays, `BLOB_VERSION` stays 1, no record-trait method is
  added, this spec stays at 0.1.0 (no requirement changed). Numbers in
  `RESULTS.md`'s `#### Follow-up: the trait-method fingerprint, measured
  and not built`.
- The spurious-rewrite case (regenerated scan values on `open`) is
  named, not measured — no call site exercises it.
- A `Symmetric`-level edge companion: resolved by `STORAGE-016` v0.1.0
  (`<path>.edges`, `GENEDGE\0`, sharing this spec's header/hash/write
  path a third time); `open_employee_production_stack_portable(path)`
  is the `Employee` helper this spec's Traceability said would wait.
  Whether the blob should record `R` is now proposed in
  `BLOB-SCHEMA-TAG-DESIGN.md` / `ADR-0019` and accepted as proposed (this spec's v0.2.0,
  next: `GENBLOB\0` version 2 with a schema-tag header field, and the
  same for `GENEDGE\0`).

## Change history

- 0.1.0 (2026-09-02, later the same day; no version bump — no
  requirement changed): the `Symmetric`-companion open question resolved
  by `STORAGE-016` v0.1.0 (see "Open questions"). No code in this spec's
  scope changed.

- 0.1.0 (2026-09-02, later the same day; no version bump — no
  requirement changed): the `open`-cost open question resolved. The
  design's trait-method-fingerprint fallback was measured before being
  built (~4% of `open` at 1M for the shipped fingerprint, ~7 ms saved by
  a whole-record trait walk) and closed as not warranted by the owner.
  "Acceptance criteria" and "Open questions" updated; no code change.
- 0.1.0 (2026-09-02): Initial accepted draft, alongside the real
  implementation (`src/generic/record_blob.rs`, the `pub(crate)` sharing
  refactor of `src/durability/record_blob.rs`, `src/generic/mmap_store.rs`,
  `src/generic/order_customer.rs`, derives on the in-crate record types,
  the `production.rs` doctest), 16 new tests, and the measured
  `create`/`open`/`open_portable` cost in `RESULTS.md`. Registers the
  design ADR-0017 accepted on 2026-09-02 as requirements; records the
  two deliberate deviations from the design sketch under "Traceability".
