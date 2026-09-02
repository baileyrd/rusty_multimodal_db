# `Symmetric` Edge-List Portability Design (Proposed)

- Status: **Proposed** — awaiting owner review. Acceptance would
  authorize the design only; implementation still requires its own unit
  (registering `STORAGE-016` and a planning packet) before any code is
  written — the posture `ADR-0016` and `ADR-0017` both took. See
  `ADR-0018-symmetric-edge-list-portability-proposal.md` for the decision
  record this document backs.
- Date: 2026-09-02
- Related: `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` and
  `docs/decisions/ADR-0017-generic-store-file-portability-proposal.md`
  (the `GenericMmapStore` treatment this document completes, implemented
  as `STORAGE-015` v0.1.0 — its "Open questions" names this decision
  first), `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` and
  `ADR-0016` (where `ProductionStore`'s own `littermate_of` edges were
  persisted, as part of `RecordBlob`), `ADR-0009` (the generic schema
  library and its layering rule), `STORAGE-014` v0.2.0 (the header, hash,
  and write path reused a third time here), `STORAGE-015` (the generic
  companion blob this design sits beside)

## Purpose and scope

`STORAGE-015` made `GenericMmapStore` reopenable from its path alone and
made `OrderProductionStack` — `GenericMmapStore` under a `Reversed`
layer — fully portable, because `Reversed` derives everything it indexes
from the records (`ChildOf::parent_id`). It deliberately stopped there.
`Symmetric<S, R, Marker>` (`src/generic/store.rs`) is the one relation
layer whose state is *not* derivable from records: `Symmetric::new(inner,
edges: &[(R::Id, R::Id)])` takes an external edge list, builds a
`HashMap<R::Id, Vec<R::Id>>` adjacency from it, and holds it in memory
only. `EmployeeProductionStack` (`src/generic_spike/employee_impl.rs`,
research-gated) stacks `Reversed` over `Symmetric` over
`GenericMmapStore`, and both of its durable helpers —
`create_employee_production_stack(employees, collaboration_edges, path)`
and `open_employee_production_stack(employees, collaboration_edges, path)`
— still require the caller to hold the edge list. After `STORAGE-015` the
`employees` argument became redundant (the `.records` blob holds them);
the `collaboration_edges` argument did not. `STORAGE-015`'s spec says why
in its own non-goals: *"the layering `ADR-0009` established says that
state belongs to the layer that indexes it"* — so the edge list was not
put into `GenericMmapStore`'s blob, and `Employee` got no portable helper
at all rather than one that was "portable except for the edges."

This proposal gives `Symmetric` its own companion blob and its own
`create`/`open`/`open_portable` triple, so the layer that indexes the
edge list is the layer that persists it, and `Employee`'s durable stack
becomes reopenable from `path` alone. It is the third use of the
`STORAGE-014` v0.2.0 header/fingerprint/atomic-write machinery and adds
no new mechanism.

**In scope for this proposal:**

- A new companion blob holding the edge list a `Symmetric` layer was
  built from, at a caller-supplied path, behind the shared 20-byte
  magic/version/FNV-1a header with a magic of its own.
- Three new, additive associated functions on `Symmetric`:
  `create(inner, edges, edges_path)`, `open(inner, edges, edges_path)`,
  and `open_portable(inner, edges_path)`, plus the public read step
  `read_portable_edges(edges_path) -> Vec<(R::Id, R::Id)>` that
  `open_portable` is built on — the `STORAGE-015` shape, one level up.
- `Symmetric::new(inner, edges)` unchanged, still bound-free, still the
  constructor for in-memory stacks that have no path.
- The `Employee` durable helpers switched from `Symmetric::new` to
  `Symmetric::create`/`open` (signatures unchanged), and one new helper,
  `open_employee_production_stack_portable(path)`, building the whole
  `EmployeeProductionStack` from the path alone.
- Keeping the blob current through `open`, with the header-fingerprint
  check so the steady-state cost is a 20-byte read.

**Explicitly out of scope, named directly:**

- Any change to `GenericMmapStore`, its `.mmap` file, or its `.records`
  blob (`STORAGE-015` v0.1.0 stays v0.1.0; `GENBLOB\0` stays version 1).
  The edge blob is a third file, not a change to either of the two.
- Any change to `Reversed`, which needs nothing persisted.
- Persisting the adjacency map itself. The blob holds the edge list as
  given; the map is rebuilt from it by the unchanged `new`, exactly as it
  is today. See "Considered options".
- A stack-level manifest that names all of a stack's files. Three files
  travel together (`<path>`, `<path>.records`, `<path>.edges`); nothing
  records that fact on disk. Named in "Open questions".
- The `DogRecord` generic spike's `Symmetric` (`src/generic_spike/dog_impl.
  rs`) — it is in-memory only, has no path, and keeps using `new`.

## Non-goals

- Not a change to how `Symmetric` answers `neighbors` — the adjacency
  map, its construction, and `Neighbors::neighbors` are byte-for-byte
  unchanged; the new constructors all end in the existing `new`.
- Not a replacement for `Symmetric::new`. The in-memory stacks
  (`build_employee_generic_store`, `dog_impl.rs`'s spike stack) have no
  path and no file; `new` stays for them. Consequence, named honestly: a
  durable stack assembled with `new` instead of `open` writes no blob
  and refreshes none. `GenericMmapStore` could close that gap by
  tightening `open`'s bounds because `open` already had a path;
  `Symmetric::new` has none, so the gap is closed by convention (the
  domain helpers use `create`/`open`) rather than by the type system.
  See "Considered options" for the alternative that was rejected.
- Not multi-writer coordination for the edge blob. Same scope as
  `STORAGE-015`'s blob: atomic rename, last writer wins, each blob
  self-consistent.
- Not "readable by other programs" — `STORAGE-014`'s meaning of portable
  throughout: the three files are sufficient to reopen the stack; nothing
  else reads any of them.

## Context and terminology

`ProductionStore` never had this problem because `RecordBlob` (`STORAGE-
014`) holds `records` *and* `edges` in one blob — `ProductionStore::
create(records, edges, path)` owns both. The generic library split that
ownership on purpose (`ADR-0009`): the core store owns records, each
relation layer owns the index it answers from. So the generic answer
cannot be "put the edges in the record blob" without undoing that split;
it has to be "give the layer that owns the edges its own file."

Two consequences of the layering shape drive the design:

1. **`Symmetric` never sees a path.** It wraps an already-constructed
   `S`; nothing in its type says whether `S` is a `GenericMmapStore` at
   some path or an in-memory `Scanned`. So the edge blob's path is an
   argument, supplied by whoever assembles the stack — the domain helper,
   which already holds `path` and can derive `<path>.edges` from it.
2. **A stack may hold more than one `Symmetric`.** Two symmetric
   relations over the same record type (two `Marker`s) are two layers
   with two edge lists; a fixed `<path>.edges` convention baked into
   `Symmetric` would make them collide. A caller-supplied path lets the
   helper name them (`<path>.edges`, `<path>.collab.edges`, whatever the
   domain needs) while the single-relation case keeps the plain
   convention.

Terminology: "the edge blob" is the companion file at `edges_path`;
"stale" means its header fingerprint differs from the fingerprint of the
edge list an `open` call was handed; "portable" has `STORAGE-014`'s
meaning.

## Requirements

- `SYMPORT-FR-001`: `Symmetric::create(inner, edges, edges_path)` writes
  the full `edges` slice, in the order given, to `edges_path` as a
  `bincode`-serialized `Vec<(R::Id, R::Id)>` behind a 20-byte header: a
  magic of its own (distinct from `DOGBLOB\0` and `GENBLOB\0`, so no
  blob of one kind can be read as another), a `u32` blob version starting
  at 1, and a `u64` FNV-1a 64 fingerprint of the streamed encoding — the
  `STORAGE-014` v0.2.0 layout, same field order, via the shared
  `encode_image`.
- `SYMPORT-FR-002`: `Symmetric::read_portable_edges(edges_path) ->
  Result<Vec<(R::Id, R::Id)>, DurabilityError>` returns the persisted
  edge list in persisted order, touching only the edge blob, never
  writing.
- `SYMPORT-FR-003`: `Symmetric::open_portable(inner, edges_path) ->
  Result<Self, DurabilityError>` is exactly `Ok(Self::new(inner,
  &Self::read_portable_edges(edges_path)?))`.
- `SYMPORT-FR-004`: `Symmetric::open(inner, edges, edges_path)` keeps the
  blob current: header read, fingerprint compare, rewrite only when
  stale, missing, or unreadable (which heals a pre-feature directory
  holding the `.mmap` and `.records` files but no `.edges`). It then
  returns `Self::new(inner, edges)` — the adjacency is always built from
  the caller's edges, never from the blob, on this path.
- `SYMPORT-FR-005`: Every edge-blob failure — missing, short, wrong
  magic, wrong version, body not matching the header fingerprint,
  `bincode` decode failure — is `DurabilityError::RecordBlobUnreadable {
  path, cause }` with `path` naming the edge blob, never a panic, never a
  silently edgeless layer. See "Considered options" for why no new
  variant.
- `SYMPORT-FR-006`: The new functions live in a separate `impl` block
  bounded `R::Id: Serialize + DeserializeOwned`. `Symmetric::new`, the
  `Neighbors` impl, and every forwarding impl keep their current bounds
  exactly; no existing call site changes. Every `R::Id` in this crate
  (`Uuid`, integers) already satisfies the bound.
- `SYMPORT-FR-007`: `create_employee_production_stack` and
  `open_employee_production_stack` keep their signatures and use
  `Symmetric::create`/`Symmetric::open` with `edges_path = <path>.edges`;
  a new `open_employee_production_stack_portable(path)` builds the whole
  stack from `path`, `<path>.records`, and `<path>.edges`, reusing
  `GenericMmapStore::read_portable_records` for the records and
  `Symmetric::open_portable` for the edges — no duplicated stack-building
  code beyond the two constructor calls.
- `SYMPORT-FR-008`: `neighbors` results after `open_portable` are
  identical to the original's, including order — the blob preserves edge
  order, and `new` pushes adjacency entries in edge order, so the
  rebuilt `Vec<R::Id>` per id is the same sequence.

## Architecture and interfaces

### Considered options

**Where the edge list lives.**

1. *Inside `GenericMmapStore`'s `.records` blob, as a second field.*
   Rejected — the core store would carry a relation layer's data, the
   thing `STORAGE-015`'s non-goals and `ADR-0017`'s decision drivers both
   refused on `ADR-0009`'s layering; it would also bump `GENBLOB\0`'s
   version for every store that has no `Symmetric` at all, and it cannot
   express two symmetric relations over one store.
2. *A stack-level blob written by the domain helper, holding records and
   edges together (the `RecordBlob` shape).* Rejected — it duplicates the
   records already in `.records`, and it moves persistence out of the
   library into each domain helper, so every new domain re-solves it.
3. *A `Symmetric`-level companion at a caller-supplied path.* Chosen —
   the layer that indexes the edges persists them; the mechanism is the
   library's, once; the helper supplies the name.

**What the blob holds: the edge list or the adjacency map.**

1. *The `HashMap<R::Id, Vec<R::Id>>` adjacency.* Rejected — twice the
   size (every edge appears under both endpoints), and `HashMap`
   iteration order is nondeterministic, so the blob's bytes and
   fingerprint would differ between two `create`s of the same edges.
2. *The edge list as given, `Vec<(R::Id, R::Id)>`.* Chosen — half the
   size, deterministic bytes for deterministic input, and the unchanged
   `new` rebuilds the map from it exactly as it does from a caller's
   slice today.

**How the path reaches `Symmetric`.**

1. *A fixed convention derived inside `Symmetric`* — impossible, since
   `Symmetric` has no path to derive from, and it would collide for two
   symmetric layers in one stack.
2. *A `Path`-carrying trait on `S`* (so `Symmetric` could ask its inner
   store where it lives). Rejected — a new trait every store type would
   have to implement, for a value the domain helper already holds.
3. *A caller-supplied `edges_path` argument.* Chosen. The single-relation
   convention `<path>.edges` lives in a small crate-internal helper next
   to the blob code so every domain helper spells it the same way.

**Whether to fold the blob into `Symmetric::new` (a path parameter on the
existing constructor) or add a parallel triple.**

1. *Change `new` to take `Option<&Path>`.* Rejected — the in-memory
   stacks (`build_employee_generic_store`, `dog_impl.rs`) have no file and
   would pass `None` forever; every existing call site changes for no
   behavior change.
2. *A parallel triple, `create`/`open`/`open_portable`, in a separately
   bounded `impl` block; `new` untouched.* Chosen. This is the parallel-
   constructor shape `ADR-0017` rejected for `GenericMmapStore`, and the
   reason it is right here is the reason it was wrong there: `GenericMmap
   Store::open` already had a path, so refreshing the blob on every
   `open` cost the caller nothing; `Symmetric::new` has no path, so there
   is nothing for it to refresh. The residual gap — a durable stack
   assembled with `new` writes no blob — is closed by convention in the
   domain helpers and named in "Non-goals".

**Which error variant.**

1. *A new `DurabilityError::EdgeBlobUnreadable`.* Rejected — a new public
   enum variant (a breaking change for exhaustive matches) for a failure
   whose modes are identical to the record blob's, distinguished already
   by the `path` field.
2. *Reuse `RecordBlobUnreadable { path, cause }`.* Chosen — the `path`
   names `<path>.edges`; the variant's own docs gain one sentence saying
   the edge blob reports through it too.

### Proposed shape

```rust
// New, e.g. src/generic/edge_blob.rs — crate-internal, beside record_blob.rs.
const MAGIC: [u8; 8] = *b"GENEDGE\0";   // distinct from DOGBLOB\0 and GENBLOB\0
const BLOB_VERSION: u32 = 1;
// header: MAGIC (8) + BLOB_VERSION (u32 LE) + fingerprint (u64 LE) = 20 bytes,
// via the shared encode_image/parse_header; body: bincode Vec<(Id, Id)>.

pub(crate) struct EdgeBlob<'a, Id: Serialize> { edges: &'a [(Id, Id)] }

impl<'a, Id: Serialize> EdgeBlob<'a, Id> {
    fn fingerprint(&self) -> u64;               // FNV-1a over the streamed bincode encoding
    fn encode(&self) -> Result<EncodedRecordBlob, DurabilityError>;
    fn is_current_at(&self, path: &Path) -> bool; // 20-byte read + fingerprint compare
}
pub(crate) fn read<Id: DeserializeOwned>(path: &Path) -> Result<Vec<(Id, Id)>, DurabilityError>;
pub(crate) fn edges_path(path: &Path) -> PathBuf;   // <path>.edges, the single-relation convention

// src/generic/store.rs — Symmetric::new and every existing impl: UNCHANGED.
impl<S, R, Marker> Symmetric<S, R, Marker>
where
    R: SymmetricRelation<Marker>,
    R::Id: Serialize + DeserializeOwned,          // this block only (SYMPORT-FR-006)
{
    /// Writes `edges` to `edges_path`, then `new(inner, edges)`.
    pub fn create(inner: S, edges: &[(R::Id, R::Id)], edges_path: &Path)
        -> Result<Self, DurabilityError>;

    /// Header check -> encode if stale -> write if stale -> `new(inner, edges)`.
    pub fn open(inner: S, edges: &[(R::Id, R::Id)], edges_path: &Path)
        -> Result<Self, DurabilityError>;

    /// Blob only; never writes.
    pub fn read_portable_edges(edges_path: &Path)
        -> Result<Vec<(R::Id, R::Id)>, DurabilityError>;

    /// Exactly `Ok(Self::new(inner, &Self::read_portable_edges(edges_path)?))`.
    pub fn open_portable(inner: S, edges_path: &Path) -> Result<Self, DurabilityError>;
}

// src/generic_spike/employee_impl.rs — existing two helpers keep their
// signatures, switch Symmetric::new -> Symmetric::create / Symmetric::open
// with edges_path(path). New:
pub fn open_employee_production_stack_portable(
    path: &Path,
) -> Result<EmployeeProductionStack, DurabilityError> {
    let employees =
        GenericMmapStore::<Employee, DepartmentField, SalaryCents>::read_portable_records(path)?;
    let core = GenericMmapStore::<Employee, DepartmentField, SalaryCents>::open(
        employees.clone(), path)?;
    let symmetric =
        Symmetric::<_, Employee, CollaboratesWith>::open_portable(core, &edges_path(path))?;
    Ok(Reversed::<_, Employee, Employee, ReportsTo>::new(symmetric, &employees))
}
```

`open_portable` on the helper reads the record blob once (for the core
store *and* for `Reversed`, as `open_order_production_stack_portable`
does), and the edge blob once. `Symmetric::open`'s own check finds the
blob current when the edges came from it, so a `create` → `open_portable`
→ `open(same edges)` sequence writes the edge blob exactly once.

## Data/state and invariants

- **The edge blob reflects the last edge list a `create`/`open` was
  handed, in that order.** Order is part of the fingerprint: the same
  edges in a different order are a "changed" edge list and rewrite the
  blob. This is deliberate — order is observable through `neighbors`'s
  result order, so a reordered list *is* a different layer.
- **The edge list is immutable through the layer.** Nothing in
  `crate::generic` adds or removes edges after construction; `Symmetric`
  has no mutating method. The same load-bearing assumption `ADR-0016` and
  `ADR-0017` named, named again as a revisit trigger.
- **Three files must travel together for the `Employee` stack.** Copying
  `<path>` and `<path>.records` without `<path>.edges` is a typed
  `RecordBlobUnreadable` naming the edge blob from `open_portable`, never
  a stack with silently empty adjacency; `open_employee_production_stack(
  employees, edges, path)` on that directory still works and heals it.
- **The blob does not record `R`, `Marker`, or `Id`'s type.** Reading one
  symmetric relation's blob as another's is a caller error, surfaced as a
  decode failure only when the encodings differ — the `STORAGE-015`
  trust model, and the same open question (a schema tag) it deferred; a
  fix there would apply here the same way.
- **No change to `GenericMmapStore`'s crash-safety or multi-process
  story**; the edge blob's write is atomic by rename and last-writer-wins
  across processes, as its sibling's is.

## Errors, failure, recovery, and observability

- Every edge-blob failure is `DurabilityError::RecordBlobUnreadable {
  path, cause }` with `path` naming `edges_path` and the same
  distinguishing causes as the record blob's (`cannot read file`/
  `magic`/`version`/`fingerprint`/`decode`).
- A crash mid-write never leaves a partial blob at `edges_path`: temp
  file → `write_all` → `sync_all` → rename, the shared write path.
- A `create` whose inner store construction already succeeded but whose
  edge-blob write fails returns the error and drops the inner store; the
  `.mmap`/`.records` files are valid on disk and a retried `create` or an
  `open` rebuilds. No ordering interaction with the inner store's own
  files exists — `Symmetric` receives `inner` already built.
- Blob/edge-list disagreement is not a failure mode: `open` always builds
  from the caller's edges and only *writes* the blob; `open_portable`
  builds from the blob and never writes.

## Security, privacy, and compatibility

Not applicable beyond what applies to the `.records` blob: locally
generated data, no network exposure, a directory exactly as trusted as
the process that created it. Compatibility: purely additive — no
existing signature or bound changes; a pre-feature `Employee` directory
opens through the existing helper and gains its `.edges` file on that
first `open`.

## Acceptance criteria

- `create_employee_production_stack(employees, edges, path)` then
  `open_employee_production_stack_portable(path)` returns a stack whose
  `get`/`filter_eq`/`scan`/`update`/`parent`/`children`/`neighbors`
  results are identical to the original's — `neighbors` including order
  — with no `employees` or `edges` argument.
- Copying `<path>`, `<path>.records`, and `<path>.edges` to a fresh
  directory and calling `open_employee_production_stack_portable` there
  succeeds.
- The same call against a directory missing only `<path>.edges` fails
  with `RecordBlobUnreadable` naming the edge blob path; the existing
  `open_employee_production_stack(employees, edges, path)` on that
  directory succeeds and writes it, after which the portable call
  succeeds.
- `Symmetric::open` rewrites the blob only when the edge list changed
  (bytes and mtime unchanged otherwise); a reordered edge list counts as
  changed.
- A `GENBLOB\0` or `DOGBLOB\0` file at `edges_path` is a magic error,
  not a decode attempt.
- `git diff` of `src/generic/store.rs` adds one `impl` block and nothing
  else; every existing `store.rs` test, `tests/mmap_record_identity_
  keying.rs`, `record_blob.rs`'s 9 generic and 12 `Dog` tests, and
  `mmap_store.rs`'s 14 tests pass unmodified.
- `dog_impl.rs` and `build_employee_generic_store` are untouched.

## Verification plan

- Unit tests in the new blob module (round trip through `Uuid` pairs;
  missing/short/wrong-magic/wrong-version/fingerprint-mismatch/truncated
  as typed errors; a `GENBLOB\0` file is a magic error; order preserved).
- `store.rs` tests for `Symmetric::create`/`open`/`open_portable`/
  `read_portable_edges` over an in-memory inner store (the blob is
  independent of what `S` is, so these need no `.mmap` file): round trip
  with order, rewrite-only-when-changed, reorder-counts-as-changed,
  missing blob healed by `open`.
- `employee_impl.rs` tests (research-gated, as the module is) for the
  stack-level helper: the acceptance criteria above, one test each.
- Cost: a throwaway release-build measurement of `open` with and without
  the edge check at 1M edges is not planned unless the implementation
  shows a reason — the mechanism is the one `STORAGE-015` measured at ~4%
  of `open` for records, and an edge is 32 bytes to a record's ~76. If
  measured, it goes in `RESULTS.md` beside the `STORAGE-015` figures. No
  published Criterion group times the `Employee` stack's constructors.

## Traceability

A new spec (next available: `STORAGE-016`) would be registered once this
design is accepted, per the `STORAGE-015` precedent — no spec for the
design document itself.

## Open questions

- Whether a stack should carry a manifest naming its files (three for
  `Employee` today) so "copy the store" is one instruction rather than a
  convention — not proposed; the domain helper's docs name the three
  files, and a missing one is a typed error naming which.
- Whether the blob should record which relation (`R`, `Marker`) it holds
  — the same schema-tag question `STORAGE-015` deferred, deferred here
  for the same reason and to be resolved together if resolved.
- Whether `Symmetric::new` should eventually be removed in favor of an
  in-memory variant of the triple, closing the by-convention gap in
  "Non-goals" by type — not proposed; `new` has two in-memory callers
  and no path.

## Change history

- 2026-09-02: Initial proposal — the first of the four follow-ups the
  owner queued after `GENERIC-STORE-FINGERPRINT-MEASUREMENT` ("1, 2, 3,
  then 4"), and the first open question `GENERIC-STORE-PORTABILITY-
  DESIGN` left.
