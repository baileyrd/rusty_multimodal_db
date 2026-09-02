# STORAGE-016 — `Symmetric` edge-list portability (companion edge blob + `create`/`open`/`open_portable`)

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-012` (`Symmetric`, the generic library),
  `STORAGE-014` v0.2.0 (the header, fingerprint, and write path shared a
  third time), `STORAGE-015` (the `GenericMmapStore` companion this sits
  beside and whose portable helpers the `Employee` helper composes),
  ADR-0018 and `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (both
  Accepted — the design this spec turns into requirements)
- Supersedes: none. Extends, does not reverse, `STORAGE-015`: its
  `.records` blob, `GENBLOB\0` magic, and blob version 1 are unchanged.

## Purpose and scope

`Symmetric<S, R, Marker>` (`src/generic/store.rs`) is the one relation
layer in `crate::generic` whose state is not derivable from records:
`Symmetric::new(inner, edges: &[(R::Id, R::Id)])` takes an external edge
list, builds an adjacency map from it, and holds it in memory only.
`STORAGE-015` made `GenericMmapStore` reopenable from its path alone and
`OrderProductionStack` fully portable (its `Reversed` layer derives
everything from the records), but stopped at `Symmetric` on purpose:
`ADR-0009`'s layering rule says state belongs to the layer that indexes
it, so the edge list did not go into `GenericMmapStore`'s blob, and
`EmployeeProductionStack` — `Reversed` over `Symmetric` over
`GenericMmapStore` — got no portable helper.

This spec closes that gap, additively, exactly as the accepted design
proposes: `Symmetric` gets its own **companion edge blob** at a
caller-supplied path, and three new associated functions —
`create(inner, edges, edges_path)`, `open(inner, edges, edges_path)`,
and `open_portable(inner, edges_path)` — plus the read step
`read_portable_edges(edges_path)` that `open_portable` is built on. The
`Employee` durable helpers switch from `new` to `create`/`open`, and a
new `open_employee_production_stack_portable(path)` rebuilds the whole
stack from `<path>`, `<path>.records`, and `<path>.edges`. The eight
requirements below are the design document's `SYMPORT-FR-001..008`,
renumbered into this spec's namespace.

## Non-goals

- Not a change to `GenericMmapStore`, its `.mmap` file, or its
  `.records` blob. The edge blob is a third file.
- Not a change to `Reversed`, which needs nothing persisted.
- Not persistence of the adjacency map. The blob holds the edge list as
  given (half the size, bytes a pure function of the input); the
  unchanged `new` rebuilds the map from it.
- Not a replacement for `Symmetric::new`. In-memory stacks
  (`build_employee_generic_store`, `dog_impl.rs`'s spike) have no path
  and keep using it. Consequence: a durable stack assembled with `new`
  writes no blob — closed by convention (the domain helpers use
  `create`/`open`), not by the type system.
- Not a stack-level manifest naming a stack's files, and not a record of
  which relation (`R`, `Marker`) the blob holds — both deferred, the
  second together with `STORAGE-015`'s identical open question.
- Not multi-writer coordination beyond atomic rename, last writer wins.

## Context and terminology

- **Edge blob**: the companion file at `edges_path` — a 20-byte header
  (`GENEDGE\0` magic, `u32` little-endian blob version, currently `1`,
  then a `u64` little-endian fingerprint) followed by the `bincode`
  encoding of `Vec<(R::Id, R::Id)>` in caller order.
- **Fingerprint**: FNV-1a 64 over the streamed `bincode` encoding of the
  edge list — the same hash and streaming the `.records` blobs use.
  Order is part of it deliberately: order is observable through
  `neighbors`'s result order, so a reordered list *is* a different layer.
- **Current blob**: readable header, this build's blob version, and a
  fingerprint equal to that of the edge list `open` was handed.
- **`<path>.edges`**: the single-relation convention (`edge_blob::
  edges_path`), beside `<path>.records`. `Symmetric` takes the path as an
  argument rather than deriving it because a stack may hold two
  symmetric relations over one store, which need two distinct files.

## Requirements

- `STORAGE-016-FR-001` (design `SYMPORT-FR-001`): `Symmetric::create(
  inner, edges, edges_path)` writes the full `edges` slice, in the order
  given, to `edges_path` as a `bincode` `Vec<(R::Id, R::Id)>` behind the
  `STORAGE-014` v0.2.0 20-byte header — magic `GENEDGE\0` (distinct from
  `DOGBLOB\0`, `GENBLOB\0`, `DOGMMAP\0`, `GMMAPST\0`), blob version `1`,
  FNV-1a 64 fingerprint — via the shared `encode_image`, then returns
  `Self::new(inner, edges)`. An existing file at `edges_path` is always
  replaced.
- `STORAGE-016-FR-002` (design `SYMPORT-FR-002`):
  `Symmetric::read_portable_edges(edges_path) -> Result<Vec<(R::Id,
  R::Id)>, DurabilityError>` returns the persisted edge list in persisted
  order, touching only the edge blob, never writing.
- `STORAGE-016-FR-003` (design `SYMPORT-FR-003`): `Symmetric::
  open_portable(inner, edges_path) -> Result<Self, DurabilityError>` is
  exactly `Ok(Self::new(inner, &Self::read_portable_edges(edges_path)?))`.
- `STORAGE-016-FR-004` (design `SYMPORT-FR-004`): `Symmetric::open(inner,
  edges, edges_path)` keeps the blob current: header read, fingerprint
  compare, rewrite only when stale, missing, or unreadable (which heals a
  pre-feature directory holding `.mmap` and `.records` but no `.edges`).
  It returns `Self::new(inner, edges)` — on this path the adjacency is
  always built from the caller's edges, never from the blob.
- `STORAGE-016-FR-005` (design `SYMPORT-FR-005`): every edge-blob
  failure at `read_portable_edges`/`open_portable` time — missing, short,
  wrong magic, wrong version, body not matching the header fingerprint,
  `bincode` decode failure — is `DurabilityError::RecordBlobUnreadable {
  path, cause }` with `path` naming the edge blob and `cause` naming
  which. Never a panic, never a silently edgeless layer, no new error
  variant.
- `STORAGE-016-FR-006` (design `SYMPORT-FR-006`): the four functions live
  in one separate `impl` block bounded `R::Id: Serialize +
  DeserializeOwned`. `Symmetric::new`, the `Neighbors` impl, and every
  forwarding impl keep their bounds exactly; no existing call site
  changes. Every `R::Id` in this crate (`Uuid`, integers) satisfies the
  bound.
- `STORAGE-016-FR-007` (design `SYMPORT-FR-007`):
  `create_employee_production_stack` and `open_employee_production_stack`
  keep their signatures and use `Symmetric::create`/`open` with
  `edges_path = <path>.edges`; the new
  `open_employee_production_stack_portable(path)` builds the whole stack
  from `path`, `<path>.records`, and `<path>.edges`, reusing
  `GenericMmapStore::read_portable_records` for the records (read once,
  for the core store and for `Reversed`) and `Symmetric::open_portable`
  for the edges — no duplicated stack-building code.
- `STORAGE-016-FR-008` (design `SYMPORT-FR-008`): `neighbors` results
  after `open_portable` are identical to the original's, including
  order — the blob preserves edge order, and `new` pushes adjacency
  entries in edge order.

## Architecture and interfaces

- `src/generic/edge_blob.rs` (new, `pub(crate)`, unconditional): `MAGIC
  = GENEDGE\0`, `BLOB_VERSION = 1`, `EDGES_SUFFIX = ".edges"`;
  `EdgeBlob<'a, Id: Serialize>` borrowing `&'a [(Id, Id)]` with `new`,
  `fingerprint() -> Result<u64, DurabilityError>` (streams `bincode`
  into `Fnv1a64`, no allocation), `encode() -> Result<EncodedRecordBlob,
  DurabilityError>`, and `is_current_at(&Path) -> bool` (one fingerprint
  pass plus a 20-byte read, never the body; a serialization failure
  counts as "not current" so `open`'s rewrite surfaces it as a proper
  error); free `read<Id: DeserializeOwned>(&Path)` (header check, body
  hashed and verified against the header, then decode) and
  `edges_path(&Path)`. 12 tests.
- `src/durability/record_blob.rs`: unchanged — its `pub(crate)`
  machinery (`encode_image`, `parse_header`, `EncodedRecordBlob::write`,
  `Fnv1a64`, `HEADER_LEN`) gains a third call site. The
  `RecordBlobUnreadable` doc comment in `src/durability/mod.rs` gains one
  sentence naming the edge blob.
- `src/generic/store.rs`: one added `impl` block (`create`, `open`,
  `read_portable_edges`, `open_portable`) with its `use` lines, plus a
  new `#[cfg(test)] mod tests` — the file had none — with 6 tests over an
  in-memory `BaseStore` inner (the blob is independent of `S`).
- `src/generic/mod.rs`: declares `pub(crate) mod edge_blob;`.
- `src/generic_spike/employee_impl.rs` (research-gated): the two
  existing helpers switch constructors; `open_employee_production_stack_
  portable` added; 5 tests.
- No new dependency; `bincode`/`serde` and the existing error variant
  are reused unchanged.

## Data/state and invariants

- The edge blob reflects the last edge list a `create`/`open` was
  handed, in that order. After `open_portable`, the blob is untouched
  (bytes and mtime).
- A blob's header fingerprint equals the FNV-1a 64 of its body bytes —
  `encode` establishes it, `read` verifies it, and the streamed
  fingerprint of the in-memory edges equals both (pinned by a test). A
  change to the encoding or the hash is a `BLOB_VERSION` bump.
- The edge list is immutable through the layer: `Symmetric` has no
  mutating method. The same load-bearing assumption `ADR-0016` and
  `ADR-0017` name, named again as the revisit trigger.
- Three files travel together for the `Employee` stack. Copying `<path>`
  and `<path>.records` without `<path>.edges` is a typed error naming the
  edge blob from `open_portable`; `open_employee_production_stack(
  employees, edges, path)` on that directory still works and heals it.
- The blob does not record `R`, `Marker`, or `Id`'s type — the
  `STORAGE-015` trust model.
- Blob/edge-list disagreement is not a failure mode: `open` builds from
  the caller's edges and only writes; `open_portable` builds from the
  blob and never writes.

## Errors, failure, recovery, and observability

- `create`/`open`: `DurabilityError::Io` for any blob write failure;
  `DurabilityError::Serde` if the edge list can't be serialized
  (propagated, never `expect`ed).
- `read_portable_edges`/`open_portable`: `RecordBlobUnreadable` with the
  causes `cannot read file` / `magic number mismatch` / `blob version
  mismatch` / `fingerprint mismatch` / `body does not decode`.
- A `create` whose inner store already succeeded but whose blob write
  fails returns the error and drops the inner store; the inner store's
  files are valid on disk and a retried `create` or an `open` rebuilds.
- Every blob write goes to a temp file, is `fsync`'d, then renamed into
  place — the shared write path. A crash mid-write never leaves a partial
  blob at `edges_path`.
- No `unwrap`/`expect` outside `#[cfg(test)]`.

## Security, privacy, and compatibility

- Purely additive: no existing signature or bound changes. A pre-feature
  `Employee` directory opens through the existing helper and gains its
  `.edges` file on that first `open`.
- Forward-detecting: a blob with a newer `BLOB_VERSION` is refused with a
  `version` cause, never partially decoded.
- `GENEDGE\0` differs from every other magic in the crate, so pointing
  `read` at a `.records`, `.mmap`, or `Dog` blob fails on the header
  before `bincode` sees a byte — and vice versa.
- Synthetic data only; no network surface.

## Acceptance criteria

- `create_employee_production_stack(employees, edges, path)` then
  `open_employee_production_stack_portable(path)` returns a stack whose
  `get`/`filter_eq`/`scan`/`update`/`parent`/`children`/`neighbors`
  results are identical to the original's — `neighbors` including order
  — with no `employees` or `edges` argument.
- `<path>`, `<path>.records`, and `<path>.edges` copied to a fresh
  directory → `open_employee_production_stack_portable` there succeeds
  with the same answers.
- The same call against a directory missing only `<path>.edges` fails
  with `RecordBlobUnreadable` naming the edge blob path; the existing
  `open_employee_production_stack` on that directory succeeds and writes
  it, after which the portable call succeeds.
- `Symmetric::open` rewrites the blob only when the edge list changed
  (bytes and mtime unchanged otherwise); a reordered list counts as
  changed; `create` over an existing blob always rewrites.
- A `GENBLOB\0` or `DOGBLOB\0` file at `edges_path` is a magic error, not
  a decode attempt.
- `git diff src/generic/store.rs` adds one `impl` block (with its
  imports and a private `PortableEdges<R>` return-type alias) and a new
  test module, and removes nothing;
  `tests/mmap_record_identity_keying.rs`, `record_blob.rs`'s 9 generic
  and 12 `Dog` tests, and `mmap_store.rs`'s 14 tests pass unmodified.
- `dog_impl.rs` and `build_employee_generic_store` are untouched.
- The edge blob's own unit tests cover: path derivation; round trip
  through `Uuid` pairs in order; the streamed fingerprint equalling the
  header's; a reordered or different list not current; an empty list;
  missing file; short file; `GENBLOB\0` and `DOGBLOB\0` magic; wrong
  version; tampered body; truncated body.

## Verification plan

- `cargo test` (default features) and `cargo test --all-features`: the
  12 `generic::edge_blob` tests, 6 `generic::store` tests, 5
  `employee_impl` tests, plus every pre-existing test, passing.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo doc --no-deps` (no new warnings): clean.
- Cost: not measured, per the design — the mechanism is the one
  `STORAGE-015` measured at ~4% of `open` for records, an edge is 32
  bytes to a record's ~76, and no published Criterion group times the
  `Employee` stack's constructors. If a reason appears, the measurement
  goes in `RESULTS.md` beside the `STORAGE-015` figures.

## Traceability

Implements: ADR-0018 / `SYMMETRIC-EDGE-PORTABILITY-DESIGN.md`
(`SYMPORT-FR-001..008` ↔ `STORAGE-016-FR-001..008`, one-to-one).
Depends on: `STORAGE-012` (the layer extended), `STORAGE-014`/
`STORAGE-015` (the blob machinery shared and the record-side portable
helpers composed). Feeds: the resolution of `GENERIC-STORE-PORTABILITY-
DESIGN.md`'s first open question and `STORAGE-015`'s "Symmetric-level
edge companion" open question.

One place the implementation diverges from the design's sketch, on
purpose: the sketch's `fn fingerprint(&self) -> u64` is
`Result<u64, DurabilityError>`, because `bincode::serialize_into` can
fail and the crate forbids `expect` outside tests. `is_current_at` treats
that failure as "not current" so `open` reports it through the rewrite
rather than swallowing it.

## Open questions

- Whether the blob should record which relation (`R`, `Marker`) it holds
  — deferred together with `STORAGE-015`'s identical question, to be
  resolved together (the owner's queued follow-up 2, "blob schema tag").
  **Accepted** as proposed in `BLOB-SCHEMA-TAG-DESIGN.md` / `ADR-0019`
  (this spec's v0.2.0, next: `GENEDGE\0` version 2 with a schema-tag
  header field).
- Whether a stack should carry a manifest naming its files — not
  proposed; the helper's docs name the three, and a missing one is a
  typed error naming which.

## Change history

- 0.1.0 (2026-09-02): Initial accepted draft, alongside the real
  implementation (`src/generic/edge_blob.rs`, one `impl` block in
  `src/generic/store.rs`, the `Employee` helpers in
  `src/generic_spike/employee_impl.rs`) and 23 new tests. Registers the
  design ADR-0018 accepted on 2026-09-02 as requirements; records the one
  deliberate deviation from the design sketch under "Traceability".
