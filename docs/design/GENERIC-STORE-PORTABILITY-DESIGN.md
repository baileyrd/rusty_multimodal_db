# `GenericMmapStore` File Portability Design (Proposed)

- Status: **Proposed** (awaiting owner review). Acceptance would authorize
  the design only; implementation would still require its own unit
  (registering `STORAGE-015` and a planning packet) before any code is
  written — the same posture `ADR-0016` took. See
  `ADR-0017-generic-store-file-portability-proposal.md` for the decision
  record this document backs.
- Date: 2026-09-02
- Related: `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` and
  `docs/decisions/ADR-0016-production-store-file-portability-proposal.md`
  (the `ProductionStore` treatment this document ports, implemented as
  `STORAGE-014` v0.1.0/v0.2.0), `docs/design/GENERIC-SCHEMA-DESIGN.md`
  §4.2 (where the generic library first hit the one-durable-field wall,
  with `Order`), `ADR-0006` (the original "ages-only" scope-down),
  `ADR-0009` (the generic schema library), `STORAGE-012` (the generic
  library's spec)

## Purpose and scope

`GenericMmapStore<R, IndexMarker, ScanMarker>` (`src/generic/mmap_store.rs`)
persists exactly one thing per record: a fixed-width `(R::Id,
R::ScanValue)` slot behind a `GMMAPST\0` magic + `u32` schema-version
header. Everything else a record carries — for `Order`, that is
`customer_id`, `status`, `created_at_unix_ms`, `discount_cents` — never
reaches disk. Both constructors, `create(records, path)` and
`open(records, path)`, require the caller to already hold the full
`Vec<R>`; the file alone cannot reconstruct a store. Relation layers
stacked above it (`Reversed<.., Customer, Order, BelongsToCustomer>` in
`OrderProductionStack`) are rebuilt from that same caller-supplied `Vec<R>`
on every open. This is the exact gap `PRODUCTION-STORE-PORTABILITY-DESIGN`
closed for `ProductionStore` — `ADR-0016` named it, twice, as the
deliberately-out-of-scope generic half, and `PROJECT-STATUS.md`'s item 63
is the owner's "1 then 2" queuing it as follow-up (b).

This proposal adds the same purely additive capability to the generic
library: a companion record blob written next to the `.mmap` file, and a
way to reopen a `GenericMmapStore` (and, through the existing domain
helpers, a whole production stack) from the path alone. The existing
`create`/`open(records, path)` signatures are unchanged and keep their
exact current semantics — identity-keyed reconciliation by id, the
append-on-missing path, the commit-marker crash-safety story, all of it.

**In scope for this proposal:**

- Persisting the full `Vec<R>` to disk at `create()` time, in a location
  derived from the existing `path` argument (`<path>.records`, the
  `STORAGE-014` convention, reused unchanged).
- Two new, additive associated functions on `GenericMmapStore`:
  `read_portable_records(path) -> Vec<R>` and `open_portable(path) ->
  Self`, needing only a path.
- One new domain helper per reference stack that has a durable variant
  today: `open_order_production_stack_portable(path)` in
  `order_customer.rs` (and its `Employee` twin in `generic_spike/
  employee_impl.rs`, to the extent §"Non-goals" allows — see there).
- Keeping the blob current through the existing `open(records, path)`
  path, using the `STORAGE-014` v0.2.0 header-fingerprint mechanism so
  the steady-state cost is a 20-byte read, not a full serialize-and-
  compare.
- Crash safety for the new file, reusing `STORAGE-014`'s
  write-to-temp-then-atomic-rename write path verbatim.

**Explicitly out of scope, named directly:**

- Changing `GenericMmapStore`'s own `.mmap` format (`GMMAPST\0`, schema
  version 2, the `[id][value][COMMITTED]` slot layout). Five rounds of
  correctness work closed that format (`GENERIC-MMAP-RECORD-IDENTITY-FIX`
  through `GENERIC-MMAP-APPEND-SLOT-RACE-FIX`); this proposal adds a second
  file next to it, exactly as `STORAGE-014` did next to `MmapAgeStore`'s.
- Making more than one field durable. `GENERIC-SCHEMA-DESIGN.md` §4.2's
  "N mmap-backed fields" redesign is a different problem (mutable,
  fixed-width, in-place) from the one this proposal solves (immutable
  everything-else, write-once). Same reframing as `ADR-0016`: the fields
  this blob persists are never mutated through the store — only the one
  `ScanMarker` field is, and it stays in the `.mmap` file.
- Persisting the edge list a `Symmetric<.., R, Marker>` layer is built
  from. See "Non-goals" for why, and "Open questions" for what it would
  take.
- Any change to `GenericProductionStore<S>` (`src/generic/production.rs`).
  It is path-agnostic by design — it wraps whatever `S` a domain helper
  builds — so portability lands entirely below it. Its doctest gains an
  `open_portable` line; its code does not change.

## Non-goals

- Not a replacement for `create`/`open(records, path)`. Both stay exactly
  as they are; `open_portable` is implemented in terms of `open`. Every
  existing call site — `order_customer.rs`, `employee_impl.rs`, the four
  crash-safety/multi-process harness binaries, `tests/
  mmap_record_identity_keying.rs`, the `Widget` doctest — keeps compiling
  with no argument changes (one derive line each on the record types, see
  "Requirements", `GPORT-FR-006`).
- Not a design for persisting `Symmetric`'s edge list. `Reversed` — the
  only relation layer the promoted `Order`/`Customer` reference domain
  uses — needs nothing beyond the records themselves (`Reversed::new(inner,
  children: &[C])` reads `ChildOf::parent_id` off each child). `Symmetric::
  new(inner, edges: &[(R::Id, R::Id)])` takes an external edge list that
  no record carries; only `Employee`'s `CollaboratesWith` (research-gated
  spike material, `src/generic_spike/employee_impl.rs`) uses it on a
  durable stack today. Putting an edge list into `GenericMmapStore`'s own
  blob would make the core store carry a relation layer's data — the
  layering `ADR-0009` established says relation state belongs to the
  layer that indexes it. So `Employee`'s portable helper, if added, takes
  the edge list as its one remaining argument and is honest in its name
  about that; a `Symmetric`-level companion is a separate, later decision.
- Not full multi-writer coordination for the blob. `GenericMmapStore`'s
  own module docs scope its multi-process guarantee to slot *creation*
  via `O_APPEND` and name everything broader as out of scope
  (`docs/FUTURE-GROWTH.md`); the blob inherits exactly that scope. Two
  processes that `open` with different record sets each rewrite the blob
  (atomic rename, last writer wins, each blob self-consistent) — a later
  `open_portable` sees one process's view, and records only the other
  process appended are invisible to it, exactly as they are to any
  `open(records, path)` whose `records` omit them today. Named, not hidden.
- Not "readable by other programs" — same meaning of portable as
  `STORAGE-014`: the `.mmap` + `.records` pair is sufficient to reopen the
  store; nothing else reads either file.

## Context and terminology

`STORAGE-014` split `ProductionStore`'s persistence into two files with
two different jobs: the mutable, fixed-width `age` stays in
`MmapAgeStore`'s per-write, zero-loss-window `.mmap`; the immutable
`id`/`breed`/edges go into a write-once `bincode` blob whose 20-byte
header carries a magic, a version, and a 64-bit FNV-1a content fingerprint
so a reopen can tell "same record set" from "changed" without
deserializing anything. That is the design this document ports, with two
structural differences the generic library forces:

1. **The blob's contents are a type parameter, not `DogRecord`.**
   `RecordBlob` serializes `Vec<DogRecord>` and walks `id`/`breed`/edges
   by hand to fingerprint them, skipping `age`. A generic `R` gives the
   blob no view inside a record — it can serialize `R` (given serde
   bounds) and nothing more. So the generic fingerprint is FNV-1a over the
   streamed `bincode` encoding of the records, and it *includes* the
   mmap-backed field. Consequence and cost: see "Data/state and
   invariants".
2. **Relations are layers, not constructor arguments.** `ProductionStore::
   create(records, edges, path)` owns its edge list; `GenericMmapStore`
   never sees one. What `open_portable` must return is therefore not
   just a store but the records the layers above are built from — which
   is why the design exposes `read_portable_records` as its own public
   step, rather than only `open_portable`.

Terminology: "the blob" is the companion `<path>.records` file; "stale"
means the blob's header fingerprint differs from the fingerprint of the
record set an `open` call was handed; "portable" has `STORAGE-014`'s
meaning, above.

## Requirements

- `GPORT-FR-001`: `GenericMmapStore::create(records, path)` also writes
  the full `records` to `<path>.records` (the `STORAGE-014`
  `companion_path` convention, reused), behind a header of its own magic
  (distinct from `DOGBLOB\0`, so a `Dog` blob and a generic blob can never
  be mistaken for one another), a `u32` blob version starting at 1, and a
  `u64` FNV-1a 64 content fingerprint — the `STORAGE-014` v0.2.0 header
  layout, 20 bytes, same field order.
- `GPORT-FR-002`: `GenericMmapStore::read_portable_records(path) ->
  Result<Vec<R>, DurabilityError>` returns the persisted records, in the
  order they were persisted, touching only the blob — never the `.mmap`
  file, never writing.
- `GPORT-FR-003`: `GenericMmapStore::open_portable(path) -> Result<Self,
  DurabilityError>` is exactly `open(read_portable_records(path)?, path)`
  — it inherits `open`'s identity-keyed reconciliation, its
  header/version checks, and its append-on-missing path unchanged, with
  no second code path to keep in sync.
- `GPORT-FR-004`: `open(records, path)` keeps the blob current: it reads
  the 20-byte header, compares fingerprints, and rewrites the blob only
  when stale (which also heals a pre-feature directory holding only the
  `.mmap` file). The ordering is `STORAGE-014` v0.2.0's: header check →
  encode only if stale → open the `.mmap` file → write only if stale, so
  an `.mmap` error never clobbers a valid blob.
- `GPORT-FR-005`: Every blob failure — missing, short, wrong magic, wrong
  version, body not matching the header fingerprint, `bincode` decode
  failure — is the existing `DurabilityError::RecordBlobUnreadable { path,
  cause }`, never a panic, never a silently-empty store; the `.mmap`
  file's own `InvalidMagic`/`SchemaVersionMismatch` stay distinct, as
  they already are.
- `GPORT-FR-006`: The one new requirement on record types: `R: Serialize
  + DeserializeOwned` (and `R::Id` likewise, which every existing `Id` in
  this crate — `Uuid`, integers — already satisfies) joins
  `GenericMmapStore`'s existing bounds. `Order`, `OrderStatus`, `Customer`,
  `Employee` and its enums, and the doctest's `Widget` gain
  `#[derive(Serialize, Deserialize)]`; nothing else about them changes.
- `GPORT-FR-007`: `src/generic/mmap_store.rs`'s slot layout, header,
  `write_slot_into`/`append_committed_slot`/`is_committed`, and the
  reconciliation loop are byte-for-byte unchanged in behavior; the only
  edits to `create`/`open` are the added blob write/check, verified by
  the existing suite (`tests/mmap_record_identity_keying.rs` and the
  module's own tests) passing unmodified.
- `GPORT-FR-008`: A new domain helper `open_order_production_stack_portable
  (path)` in `order_customer.rs` builds the full `OrderProductionStack`
  from the path alone, by way of `read_portable_records` +
  `open_order_production_stack` (no duplicated stack-building code).

## Architecture and interfaces

### Considered options

**Where the persisted record set lives: inside the `.mmap` file vs. a
companion blob.**

1. *Extend the `.mmap` slot to carry the whole record* — impossible for
   variable-length fields (`Customer::name: String`) without the
   string-heap format `ADR-0006` declined and `GENERIC-SCHEMA-DESIGN.md`
   §4.2 confirmed is a real redesign; and it would reopen a five-round
   closed format for data that never mutates in place. Rejected.
2. *A companion blob, `bincode`-serialized `Vec<R>`, `<path>.records`.*
   Chosen — the `STORAGE-014` answer, for the `STORAGE-014` reason: the
   data is write-once, so `SnapshotFullStore`'s plain serialize/
   deserialize round trip is the whole mechanism, and `MmapAgeStore`'s
   temp-then-rename write path already makes it crash-safe.

**How to fingerprint a record the blob cannot see inside.**

1. *A new trait method, `fn fingerprint_into(&self, hasher)`, hand-walking
   each record's immutable fields, as `RecordBlob::fingerprint` does for
   `DogRecord`.* Rejected for this proposal — one more hand-written,
   easy-to-get-subtly-wrong method per record type, on a library whose
   §4.5 forwarding-boilerplate tax is already its named ergonomic cost;
   and the thing it buys (excluding the mmap-backed field so an
   `update`d value never counts as a changed record set) matters only for
   a caller that reopens with *regenerated* scan values — see "Data/state
   and invariants". Named as the fallback if that ever measures as a
   real cost.
2. *`std::hash::Hash` through an FNV hasher.* Rejected — `Hash`'s byte
   stream is explicitly not stable across Rust versions (the same reason
   `STORAGE-014` rejected `DefaultHasher`), and this value lives on disk.
3. *FNV-1a over the streamed `bincode` encoding of `records`, via an
   `io::Write` impl on the existing `Fnv1a64`.* Chosen — no allocation,
   no file read beyond the header, no per-type code, one bound (`Serialize`)
   the blob needs anyway. The serialization CPU cost is still paid on
   every `open`; expected to land between `STORAGE-014` v0.1.0's +27% and
   v0.2.0's +0.3–4% at 1M and measured at implementation, not assumed.

**Whether to tighten `create`/`open`'s bounds or add a parallel
constructor pair.**

1. *A separate impl block with the serde bounds holding `create_portable`
   /`open_portable`, leaving `create`/`open` bound-free.* Rejected — a
   store created by plain `create` would have no blob, and a plain
   `open(records, path)` with a changed record set would leave an
   existing blob stale: exactly the "silently stale `open_portable`"
   correctness gap `ADR-0016` rejected on its fourth considered option.
2. *Add `Serialize + DeserializeOwned` to the existing bounds; `create`
   always writes, `open` always checks.* Chosen. It is a bound tightening
   on a public generic type, so it is named as the one breaking change
   (`GPORT-FR-006`): every record type used with `GenericMmapStore` must
   be serializable. Every such type in this crate already is or is one
   derive line away; `publish = false` means there are no others. A store
   that cannot serialize its records cannot be portable, and `Dog` pays
   this already.

**What `open_portable` returns when the stack above needs the records.**

1. *`open_portable(path) -> Self` only, with the domain helper pulling
   records back out of the store.* Rejected — `GenericMmapStore` holds
   its records in a `HashMap`, so any order it could hand back is
   nondeterministic, which would make `Reversed`'s per-parent `Vec<C::Id>`
   order (and so `Children::children`'s result order) vary run to run.
2. *`read_portable_records(path) -> Vec<R>` as a public step, plus
   `open_portable(path) -> Self` built on it.* Chosen — the blob preserves
   the persisted order, the domain helper reads once and reuses the
   existing `open_order_production_stack(records, path)`, and a caller
   with no relation layer gets the one-call form.

**Where the shared machinery lives.** `Fnv1a64`, `companion_path`, the
header encode/parse, and the temp-then-rename write are all private to
`src/durability/record_blob.rs` today. This is their second real call
site, which is this project's own threshold for sharing rather than
duplicating: they become `pub(crate)` (or move to a small crate-internal
module — placement is an implementation-time detail), parameterized by
magic where needed. `RecordBlob`'s own tests and behavior are unchanged;
this is a visibility change to a closed module, named as such.

### Proposed shape

```rust
// src/durability/record_blob.rs — RecordBlob's format and behavior: UNCHANGED.
// Fnv1a64 (now also `impl io::Write`), companion_path, the header
// encode/parse, and the temp-then-rename write become crate-internal
// shared helpers.

// New, e.g. src/generic/record_blob.rs:
const MAGIC: [u8; 8] = *b"GENBLOB\0";   // distinct from DOGBLOB\0
const BLOB_VERSION: u32 = 1;
// header: MAGIC (8) + BLOB_VERSION (u32 LE) + fingerprint (u64 LE) = 20 bytes

pub(crate) struct GenericRecordBlob<R> { records: Vec<R> }

impl<R: Serialize + DeserializeOwned> GenericRecordBlob<R> {
    fn fingerprint(&self) -> u64;          // FNV-1a over the streamed bincode encoding
    fn encode(&self) -> Result<Encoded, DurabilityError>;
    fn read(path: &Path) -> Result<Self, DurabilityError>;   // RecordBlobUnreadable on every failure
    fn is_current_at(&self, path: &Path) -> bool;            // 20-byte read + fingerprint compare
}

// src/generic/mmap_store.rs
impl<R, IndexMarker, ScanMarker> GenericMmapStore<R, IndexMarker, ScanMarker>
where
    R: IndexedField<IndexMarker> + ScannableField<ScanMarker> + Clone
        + Serialize + DeserializeOwned,          // the one new bound (GPORT-FR-006)
    R::Id: MmapFieldValue + Serialize + DeserializeOwned,
    R::ScanValue: MmapFieldValue,
{
    // Unchanged signature; encodes the blob before `records` is consumed,
    // writes it after the .mmap file (the STORAGE-014 ordering).
    pub fn create(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError>;

    // Unchanged signature; header check -> encode if stale -> existing
    // open -> write if stale (STORAGE-014 v0.2.0's ordering).
    pub fn open(records: Vec<R>, path: &Path) -> Result<Self, DurabilityError>;

    // New. Blob only; never touches the .mmap file; never writes.
    pub fn read_portable_records(path: &Path) -> Result<Vec<R>, DurabilityError>;

    // New. Exactly open(read_portable_records(path)?, path).
    pub fn open_portable(path: &Path) -> Result<Self, DurabilityError>;
}

// src/generic/order_customer.rs — new helper, next to the existing two.
pub fn open_order_production_stack_portable(
    path: &Path,
) -> Result<OrderProductionStack, DurabilityError> {
    let orders = GenericMmapStore::<Order, Status, Amount>::read_portable_records(path)?;
    open_order_production_stack(orders, path)
}

// src/generic/production.rs — GenericProductionStore: UNCHANGED.
// Doctest gains: let reopened = GenericProductionStore::new(
//     GenericMmapStore::<Widget, Category, Price>::open_portable(&path)?);
```

`open_portable` reads the blob and then runs the unchanged `open`, whose
own fingerprint check finds the blob current (the records came from it)
and writes nothing. The helper form reads the blob once and hands the
records to the existing stack builder, so `Reversed` is rebuilt from the
same deterministic order it was created from.

## Data/state and invariants

- **The blob reflects the last record set a `create`/`open` was handed,
  fingerprinted whole.** Because the generic fingerprint cannot exclude
  the mmap-backed field, a caller that reopens with regenerated records
  whose scan values differ from create-time values sees a blob rewrite
  the `Dog` design would have skipped. This is a cost, not a correctness
  issue: the `.mmap` file stays the truth for that field on every read
  (`GetById::get`'s write-through), `open` seeds slots from `records` only
  for ids with no slot yet — exactly today's semantics — and
  `open_portable` never writes. No call site in this crate regenerates
  scan values between `create` and `open`; the spurious-rewrite case is
  named so it can be measured if a caller ever does.
- **The persisted fields are immutable through the store.** Nothing in
  `crate::generic` mutates a record's id, index value, parent id, or any
  non-`ScanMarker` field after construction; `UpdateField::update` writes
  only the mmap-backed field. This is the load-bearing assumption, the
  same one `ADR-0016` named, and it is named here as a revisit trigger.
- **Two files must travel together.** Copying only the `.mmap` file is a
  typed `RecordBlobUnreadable`, never a partial store — and plain
  `open(records, path)` on that directory still works and heals it.
- **The blob does not record `R`.** Reading an `Order` blob as an
  `Employee` is a caller error, surfaced as a decode failure when the
  encodings differ and not at all when they happen to coincide — the
  same trust model as the `.mmap` file, which records slot widths only.
  Named in "Open questions".
- **No change to `GenericMmapStore`'s own crash-safety or multi-process
  story** — the `COMMITTED` marker, the `O_APPEND` slot append, and their
  documented limits are untouched; the blob's own write is atomic by
  rename, and its multi-writer behavior is last-writer-wins as stated in
  "Non-goals".

## Errors, failure, recovery, and observability

- Every blob failure is `DurabilityError::RecordBlobUnreadable { path,
  cause }` with a distinguishing cause (`cannot read file`/`magic`/
  `version`/`fingerprint`/`decode`), reusing the `STORAGE-014` variant —
  no new variant, since the failure modes are identical and the `path`
  field already names which file.
- A crash mid-write at `create`/reconciling-`open` time never leaves a
  partial blob at the real path (temp file → `write_all` → `sync_all` →
  rename, any stale temp consumed) — `STORAGE-014`'s write path, reused.
- Blob/`.mmap` disagreement about which ids exist is not a new failure
  mode: `open`'s reconciliation handles it as it handles any
  caller-supplied/file mismatch today — ids in the blob but not the file
  get an appended slot seeded from the blob's value; slots whose ids the
  blob lacks are inert and invisible, the module's existing documented
  behavior.

## Security, privacy, and compatibility

Not applicable beyond what applies to the `.mmap` file — locally
generated data, no network exposure, a store directory exactly as trusted
as the process that created it. Compatibility: the `.mmap` format is
unchanged; the one API change is the bound tightening in `GPORT-FR-006`,
`publish = false`, every in-crate record type covered.

## Acceptance criteria

- `create(records, path)` then `open_portable(path)` returns a store whose
  `get`/`filter_eq`/`scan`/`update` results, and — through
  `open_order_production_stack_portable` — `parent`/`children` results,
  are identical to the original's, including non-durable fields
  (`customer_id`, `status`, `created_at_unix_ms`, `discount_cents`) the
  `.mmap` file alone could never have supplied.
- Copying `<path>` and `<path>.records` to a fresh directory and calling
  `open_portable` there succeeds.
- `open_portable` against a directory holding only the `.mmap` file
  fails with `RecordBlobUnreadable` naming the companion path; `open(
  records, path)` on the same directory succeeds and writes the blob, after
  which `open_portable` succeeds.
- `open` rewrites the blob only when the record set changed — bytes and
  mtime unchanged otherwise; a blob written by one `create` and reopened
  via `open_portable` is not rewritten.
- `tests/mmap_record_identity_keying.rs` and `mmap_store.rs`'s own tests
  pass unmodified; `git diff` of `mmap_store.rs` touches only `create`/
  `open`'s bodies, the bounds, and the two new functions.
- `RecordBlob`'s 12 tests and `production.rs`'s 6 portability tests pass
  unmodified — the visibility changes to `record_blob.rs` alter no
  behavior.

## Verification plan

- Real, compiled, tested code once accepted, as `STORAGE-014` was: unit
  tests in the new blob module (round trip through `Order`; missing/
  short/wrong-magic/wrong-version/fingerprint-mismatch/truncated all typed
  errors; a `DOGBLOB\0` file is a magic error, not a decode attempt),
  `mmap_store.rs` tests for `open_portable`/`read_portable_records` and
  the rewrite-only-when-changed property, `order_customer.rs` tests for
  the stack-level helper, and a doctest update in `production.rs`.
- The same throwaway release-build measurement `STORAGE-014` used:
  `create`/`open`/`open_portable` at 1K/100K/1M `Order` records, before
  and after, median of 7 samples (3 at 1M), recorded in `RESULTS.md`. The
  number that matters is `open`'s steady-state delta (the streamed-
  serialization fingerprint); if it lands nearer v0.1.0's +27% than
  v0.2.0's +0.3–4%, the trait-method fingerprint (considered option 1)
  is the named next step, not a different architecture.
- `crash_writer`/`crash_safety_harness`/`multiprocess_writer`/
  `multiprocess_harness` build and their documented behavior is
  unchanged — they exercise `create`/`open`, whose slot semantics this
  proposal does not touch.

## Traceability

A new spec (next available: `STORAGE-015`) would be registered once this
design is accepted, per the `STORAGE-014` precedent — no spec for the
design document itself.

## Open questions

- Whether a `Symmetric` layer's edge list should get its own companion
  (a `Symmetric`-level `<path>.edges`, or a stack-level blob) so
  `Employee`'s durable stack becomes fully portable — separate decision;
  this proposal leaves that helper honest about its one remaining
  argument.
- Whether the blob should record which `R` it holds (a caller-supplied
  schema tag, or the slot widths as a weak check) — deferred; the `.mmap`
  file makes the same trust assumption today.
- Whether the streamed-serialization fingerprint's `open` cost is
  acceptable at 1M — measured at implementation, with the trait-method
  fingerprint as the named fallback.
- Whether `GenericProductionStore` should grow a path-taking constructor
  of its own — not proposed; it is deliberately store-agnostic, and the
  domain helpers are where paths already live.

## Implementation status

Not implemented. Proposed 2026-09-02; awaiting owner acceptance.

## Change history

- 2026-09-02: Initial proposal, follow-up (b) of `PROJECT-STATUS.md` item
  63 — the owner's "1 then 2" after `PRODUCTION-STORE-PORTABILITY`.
