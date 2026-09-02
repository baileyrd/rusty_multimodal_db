# Multi-Field mmap Durability Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed: the per-field `MmapScanned` layer with
  the `SlotFile` extraction, over duplication, over the multi-slot
  single-file layout, over closing as not worth building; no changes
  requested). Implementation is the next unit, as `STORAGE-017` v0.1.0.
  See `docs/decisions/ADR-0020-multi-field-mmap-durability-proposal.md`
  for the decision record this document backs.
- Date: 2026-09-02
- Related: `docs/decisions/ADR-0006-tier-2-durability-architectures.md`
  (the decision whose revisit trigger this answers — *"a future record
  shape needs more than one mutable field persisted"*),
  `docs/design/GENERIC-SCHEMA-DESIGN.md` §4.1/§4.2 and
  `docs/decisions/ADR-0009-generic-schema-design-proposal.md` (the
  generic library; §4.2 confirms the trigger bites with `Order` and
  sketches, without designing, "N separate flat mmap-backed arrays, one
  per field"), `STORAGE-012` (`GenericMmapStore`, `Scanned`,
  `forward_scannable_pairs!`), `STORAGE-015` v0.2.0 and
  `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (the `.records`
  companion blob whose immutable-snapshot role bounds what "durable"
  means for the non-mmap fields), `STORAGE-016` v0.2.0 (the
  caller-supplied-path precedent for a layer's own file),
  `docs/decisions/ADR-0019-blob-schema-tag-proposal.md` (the tag the
  new file would carry)

## Purpose and scope

`GenericMmapStore<R, IndexMarker, ScanMarker>` persists exactly one
mutable field per record: the `ScanMarker` value, in a fixed-width slot
of its `.mmap` file, updated in place. Every other field of `R` comes
from the `.records` companion blob (`STORAGE-015`), which is an immutable
snapshot taken at `create` and rewritten only when `open` finds it stale
or foreign — never on `update`. So through the store, exactly one field
is mutable *and* durable; the rest are durable but read-only.

`ADR-0006` scoped Tier 2 durability to `age` only and named the trigger
for revisiting that: *"a future record shape needs more than one mutable
field persisted — at that point mmap's and `redb`'s ages-only scope-down
would need real redesign (the string-heap/fixed-layout problem this ADR
chose not to solve), not just an incremental extension."*
`GENERIC-SCHEMA-DESIGN` §4.2 found that `Order` — the very first second
domain — is that record shape: a real order domain plausibly wants both
`amount_cents` and `status`, or `amount_cents` and `discount_cents`,
mutable and durable. `ADR-0009` confirmed the trigger as *"necessary
(not hypothetical) but not designed in this pass."* `OrderProductionStack`
today makes `Amount` durable and leaves `CreatedAt` and `DiscountCents`
in-memory only, with the module docs of `mmap_store.rs` saying so
explicitly. `PROJECT-STATUS` item 22 has carried this as "genuinely
unscoped follow-up work" since the generic library landed, and item 68
names it as the owner's third queued follow-up ("1, 2, 3, then 4").

This document scopes it. It weighs the three shapes `PROJECT-STATUS`
item 68 names — a multi-slot `.mmap` layout, per-field `.mmap` files, and
widening the blob's role — against the code as it is, and proposes one.
Per the `ADR-0016`–`ADR-0019` precedent, it authorizes no implementation;
that is a separate unit after the owner's call, with a spec
(`STORAGE-017`) registered at implementation time.

The question is narrower than `ADR-0006`'s phrasing suggests. The
"string-heap/fixed-layout problem" is two problems: (a) more than one
mutable field, and (b) a mutable field of variable width. `GenericMmapStore`
already answered (b) by construction — a durable field is whatever
implements `MmapFieldValue`, which is fixed-width `Copy` (`u32`, `i64`,
`Uuid` today), and `GENERIC-SCHEMA-DESIGN` §4.2 recorded that constraint
as load-bearing. This design addresses (a) only, for fields that already
satisfy `MmapFieldValue`. Variable-width mutable fields stay out of scope
(see Non-goals), and are the reason option 3 below is discussed at all.

## Non-goals

- Variable-width mutable durable fields (`String`, `Vec<_>`). Every mmap
  shape here needs a fixed-width slot; a string heap is a different
  design and a different ADR. `bincode`-blob rewriting (option 3) is the
  only shape that handles them and is rejected here for the fixed-width
  case on cost grounds; it remains the fallback if a variable-width
  mutable field is ever required.
- Atomic multi-field updates. Today one `update` is one in-place
  fixed-width write. Nothing here changes that: two fields updated in
  sequence are two independent writes, and a crash between them leaves
  the first applied and the second not. Named as an invariant below, not
  solved — a transactional update across fields is `ADR-0013`'s domain
  (server-side transactions), not the storage layer's.
- Changing `GenericMmapStore`'s `.mmap` format (`GMMAPST\0`,
  `SCHEMA_VERSION` 2) or the `.records` blob (`GENBLOB\0` version 2).
  Option 1 would; the proposed option 2 does not.
- Making the `IndexedField` (`Status`) mutable. The index is immutable
  after construction in every backend in this crate; a mutable indexed
  field is index maintenance, a separate concern.
- Extending `redb` or the LSM variant. `ADR-0006`'s trigger names both;
  only mmap graduated into the production default (`ADR-0008`) and the
  generic library (`ADR-0009`). The other two stay at `age`-only as
  benchmark baselines.
- `ProductionStore`/`MmapAgeStore` (the `Dog` domain). `Dog` has one
  mutable field. Nothing there changes.
- A stack-level manifest listing the files a stack owns. It becomes more
  attractive with N+2 files (see Open questions) and is still
  `ADR-0018`'s open question, not this design's.

## Context and terminology

- **`GenericMmapStore`** (`src/generic/mmap_store.rs`): header
  `[GMMAPST\0][SCHEMA_VERSION u32 LE]` (12 bytes), then slots of
  `[id R::Id::BYTE_WIDTH][value R::ScanValue::BYTE_WIDTH][COMMITTED u8]`.
  `create` truncates, writes header and one committed slot per record,
  then the companion blob. `open` maps the file, reads every committed
  `(id, value)` keyed by id, reconciles against the caller's `records`
  (reuse a persisted position; append a slot for a record with none;
  ignore a persisted id with no record — the "stale" case), remaps, then
  rewrites the blob only if it was not current. `update` overwrites the
  value bytes in place; `get` clones the in-memory record and patches
  the scannable field from the mapping via `set_scannable_value`; `scan`
  takes a `chunks_exact` fast path when `is_gapless`; `flush` is one
  `msync`.
- **`Scanned<S, R, Marker>`** (`src/generic/store.rs`): the in-memory
  layer with the same shape — `position_index` + `cache: Vec<ScanValue>`
  — stacked over any inner store; `get` patches on the way up; `Flush`
  forwards. Its `ScanField`/`UpdateField` for *other* markers cannot be
  one generic impl (coherence, `E0119`) and are generated per ordered
  marker pair by `forward_scannable_pairs!`.
- **`OrderProductionStack`** = `Reversed<GenericMmapStore<Order, Status,
  Amount>, Customer, Order, BelongsToCustomer>`; constructors
  `create_order_production_stack`, `open_order_production_stack`,
  `open_order_production_stack_portable` in `order_customer.rs`.
  `CreatedAt` and `DiscountCents` are `ScannableField` markers on `Order`
  that the production stack does not expose at all.
- **`MmapFieldValue`** (`src/generic/mmap_field.rs`): `BYTE_WIDTH`,
  `write_le`, `read_le`. The fixed-width contract every mmap slot value
  meets.
- **Durable, mutable, exposed** are three different properties of a field
  through a stack. Today for `Order`: `amount_cents` is all three;
  `status` is durable and exposed, immutable; `customer_id`, `created_at`,
  `discount_cents` are durable (in the blob) but neither mutable nor
  exposed as scannable through the production stack. This design's goal
  is to let more than one field be all three.

## Requirements

Requirement ids use the `MFMD-FR-` prefix (multi-field mmap durability).
A spec (`STORAGE-017`) would carry them at implementation time.

- `MFMD-FR-001` — A stack can expose more than one `ScannableField`
  marker of one record type as mutable *and* durable, each backed by a
  fixed-width in-place mmap write, with the same per-write cost shape
  `GenericMmapStore::update` has today (one bounded copy, no
  allocation, no syscall).
- `MFMD-FR-002` — Each durable field's value survives process exit
  without an explicit flush (the page cache's guarantee, as today) and is
  forced to disk by `Flush::flush` (the `msync` guarantee, as today).
- `MFMD-FR-003` — `GetById::get` returns a record whose every durable
  scannable field reflects its latest `update`, not the value the
  companion blob or the caller's `records` held (write-through
  consistency, `STORAGE-012`'s existing rule, extended to N fields).
- `MFMD-FR-004` — Reopening reconciles each durable field by record
  identity, with the same three cases `GenericMmapStore::open` has
  (reuse, append-for-missing, ignore-stale) and the same torn-slot
  handling (an uncommitted slot is invisible and re-appended).
- `MFMD-FR-005` — The portable-open path (`read_portable_records` then
  open) works for a stack with N durable fields from its files alone.
- `MFMD-FR-006` — No change to any existing on-disk format:
  `GMMAPST\0` version 2, `GENBLOB\0` version 2, `GENEDGE\0` version 2 all
  unchanged; every existing file opens as before.
- `MFMD-FR-007` — `OrderProductionStack` gains at least one more durable
  field (`DiscountCents` — the domain's stated refund-adjustment case) as
  the in-crate proof, with `create`/`open`/`open_portable` constructors
  updated; `Amount` stays where it is.
- `MFMD-FR-008` — Adding a durable field to a domain costs one layer in
  the stack type, one constructor line, and one macro entry — no
  hand-written forwarding impls (the `forward_scannable_pairs!` rule).
- `MFMD-FR-009` — Every new file is tagged with `R::SCHEMA_TAG`
  (`ADR-0019`) or is opened only alongside a tagged blob, so a foreign
  file is refused by name rather than misread.

## Architecture and interfaces

### Considered options

**Option 1 — a multi-slot `.mmap` layout (one file, wider slots).**
`GenericMmapStore<R, IndexMarker, Layout>` where `Layout` is a per-domain
type describing N durable markers; the slot becomes
`[id][v1][v2]…[vN][COMMITTED]`; `SCHEMA_VERSION` bumps to 3. Per-marker
`ScanField<R, M>`/`UpdateField<R, M>` come from one generic impl bounded
on a `DurableSlot<R, M> { const OFFSET: usize }` trait that `Layout`
implements once per marker — and those N impls must be concrete per
domain (a blanket `impl<M1, M2> DurableSlot<M1> for (M1, M2)` overlaps
with the `M2` one when `M1 == M2`, the same `E0119` wall
`forward_scannable_pairs!` exists for), so a `durable_fields!` macro
generates them: O(N) impls, one macro entry per field.

- For: one file, one header, one reconciliation pass, one `msync`; a
  row-shaped read of several fields of one record touches one cache line.
- Against: a format change on the file `ADR-0008`'s production default
  descends from — every existing `.mmap` is refused (`SchemaVersionMismatch`;
  the crate's convention is detection, not migration, and unlike the
  blobs `open` cannot heal it, since the old file's one value per slot
  cannot fill the new slot's N). `slot_width`, `slot_offset`,
  `read_value`, `write_value`, `write_slot_into`, `append_committed_slot`,
  `create`, `open`, and `scan` all become layout-generic; the benchmark-
  pinned `chunks_exact` fast path reads a wider stride — a single-field
  scan over `Order` with three `i64` fields walks 41-byte slots to use 8
  bytes of each, versus 25-byte slots today. Every test that patches
  bytes by offset is re-pinned. This is the "real redesign" `ADR-0006`
  predicted, in its most invasive form, and it turns the store into a
  small row store — the AoS layout `ADR-0001` measured as "bad for
  single-column scans (touches every field of every record)", against
  the SoA layout every scan-shaped result in this project since has
  favoured.

**Option 2 — per-field `.mmap` files via a composable layer
(proposed).** A new store layer, `MmapScanned<S, R, Marker>`, the durable
twin of `Scanned<S, R, Marker>`: it owns one `.mmap` file in exactly
`GenericMmapStore`'s existing format (`GMMAPST\0`, version 2, the same
`[id][value][COMMITTED]` slot) for its one marker, stacks over any inner
store, and forwards everything else. `GenericMmapStore` keeps its file
and its one `ScanMarker` unchanged; a second durable field is a second
file. This is the shape `GENERIC-SCHEMA-DESIGN` §4.2 sketched — *"N
separate flat mmap-backed arrays, one per field, same shape as §4.1's
multiple-`Scanned`-layers stacking"* — and the shape `ADR-0009` chose for
every other capability: one layer, one concern, composed by type.

- For: no format change (`MFMD-FR-006` for free); `GenericMmapStore`'s
  1,600 lines of crash-safety reasoning and its benchmarks are untouched;
  each field's scan walks only its own 25-byte-stride file (column
  layout, the project's measured preference); a domain adds a field by
  adding a layer (`MFMD-FR-008`); the layer is reusable over any inner
  store, including an in-memory `BaseStore` — durable fields without a
  `GenericMmapStore` at all.
- Against: N files plus the blob, N reconciliation passes at open (each
  a `HashMap` keyed by id over the file, O(records)), N `msync`s per
  `flush`; the caller (or the domain constructor) names N paths; the
  cross-marker forwarding is O(N²) generated impls, as `Scanned`'s is.
  The slot/header/commit/reconcile machinery must either be duplicated
  from `GenericMmapStore` or factored out of it — see Proposed shape.

**Option 3 — widen the blob's role.** Make the `.records` companion blob
the durable home for the additional mutable fields: `update` rewrites
the blob (or appends to it). This is the only shape that could also
carry a variable-width field.

- For: no new file format, no slot machinery, handles `String`.
- Against: a full `bincode` encode + fingerprint + write per `update`
  — milliseconds against the nanoseconds `ADR-0006` called *"the standout
  result of the entire durability pass"*; it is not mmap durability at
  all but a snapshot-per-write. An append-only variant is a write-ahead
  log, which the crate already has (`STORAGE-008`, Tier 1) with its own
  measured cost profile and checkpoint story; re-deriving it under the
  blob's name would be a second WAL. Rejected for fixed-width fields;
  kept as the named fallback for a variable-width mutable field if one
  is ever required (Open questions).

### Proposed shape

Option 2. Sketch, not code — names may move at implementation.

**A shared slot file.** Extract the file mechanics of `GenericMmapStore`
into a `pub(crate)` helper, tentatively `generic::slot_file::SlotFile<Id,
V>`: the constants (`MAGIC`, `SCHEMA_VERSION`, `HEADER_LEN`,
`COMMITTED`), `slot_width`/`slot_offset`, `read_value`/`write_value`,
`write_slot_into`, `append_committed_slot`, `write_header`/`read_header`,
`is_committed`, `is_gapless`, and the two halves of `open`'s file work —
"read committed pairs keyed by id" and "append slots for these missing
records, reporting each landed position." `GenericMmapStore` delegates
to it with no behavior change; its existing tests and benchmarks are the
guard. The alternative — duplicating those ~600 lines into the new layer
— is faster to write and doubles the surface a future crash-safety fix
has to touch; the extraction is preferred, with duplication the fallback
if it disturbs the `chunks_exact` fast path measurably.

**The layer.**

```rust
pub struct MmapScanned<S, R, Marker>
where
    R: ScannableField<Marker>,
    R::Id: MmapFieldValue,
    R::ScanValue: MmapFieldValue,
{
    inner: S,
    position_index: HashMap<R::Id, usize>,
    file: SlotFile<R::Id, R::ScanValue>,   // owns the MmapMut + path
    _marker: PhantomData<Marker>,
}

impl<S, R, Marker> MmapScanned<S, R, Marker> where R: SchemaTag + … {
    /// Truncate-and-write `path` from `records`' current values, in
    /// `records`' order — mirrors `GenericMmapStore::create` minus the blob.
    pub fn create(inner: S, records: &[R], path: &Path) -> Result<Self, DurabilityError>;
    /// Map `path`, reconcile by id against `records` (reuse / append /
    /// ignore-stale, torn slots invisible), remap — mirrors `open` minus the blob.
    pub fn open(inner: S, records: &[R], path: &Path) -> Result<Self, DurabilityError>;
    pub fn inner(&self) -> &S;
    pub fn inner_mut(&mut self) -> &mut S;
    pub fn path(&self) -> &Path;
}
```

Impls, each the durable analogue of `Scanned`'s:

- `ScanField<R, Marker>`: the `is_gapless` fast path or the sorted-
  positions fallback, exactly `GenericMmapStore::scan`.
- `UpdateField<R, Marker>`: `position_index` lookup, `write_value`.
- `GetById<R>`: `inner.get(id)` then `set_scannable_value(read_value(pos))`
  — the patch on the way up (`MFMD-FR-003`), so a stack of
  `MmapScanned<MmapScanned<GenericMmapStore<…>>>` patches each field in
  turn.
- `Flush`: own `msync`, then `inner.flush()` (`MFMD-FR-002`).
- `FilterEq<R, IndexMarker>`, `Neighbors`, `Children`: generic forwards,
  as `Scanned` has.
- `ScanField`/`UpdateField` for *other* markers: `forward_scannable_pairs!`
  generalized to take the layer type as a parameter (or a sibling
  `forward_mmap_scannable_pairs!` sharing its rotating-accumulator
  body), invoked once per domain alongside the existing one.

**No blob of its own.** The layer does not write a `.records` blob; the
record set is the inner store's concern (`GenericMmapStore`'s blob, or
nothing for an in-memory inner). The file it writes is tagged: the
version-2 `GMMAPST\0` header has no tag field (`ADR-0019` deferred the
`.mmap` tag because a `.mmap` is never opened without its blob), and
this layer's file *is* opened with only a `records` slice — so
`MFMD-FR-009` is met either by (a) the layer's `open` refusing a file
whose slot width does not match `Id::BYTE_WIDTH + ScanValue::BYTE_WIDTH
+ 1` (a weak check, free, catches most cross-domain mistakes) plus the
requirement that the stack's `open_portable` reads its records from a
tagged blob first, or (b) a per-file tag, which would be `GMMAPST\0`
version 3 and is exactly what Non-goals rules out for this unit. (a) is
proposed; (b) is the revisit.

**Paths.** Caller-supplied per layer, the `STORAGE-016` precedent. The
domain constructor derives siblings from one base path so a stack still
has one name: for `Order`, `<path>` (the `Amount` store), `<path>.records`
(the blob), `<path>.discount_cents.mmap` (the new layer). The naming rule
is the domain's, documented on the constructor; the layer itself is
path-agnostic.

**`OrderProductionStack` (`MFMD-FR-007`).**

```rust
pub type OrderProductionStack = Reversed<
    MmapScanned<GenericMmapStore<Order, Status, Amount>, Order, DiscountCents>,
    Customer, Order, BelongsToCustomer>;
```

`create_order_production_stack(orders, path)` creates the core, then
`MmapScanned::create(core, &orders, &discount_path(path))`, then
`Reversed::new`. `open_*` mirrors it with `open`;
`open_order_production_stack_portable` is unchanged in shape (read the
blob, then `open`). `CreatedAt` stays in-memory only — it is a timestamp
that does not change after creation, and leaving one `ScannableField`
non-durable keeps the "durable is opt-in per field" property visible in
the reference domain.

### Why not make `GenericMmapStore` itself the layer?

A tempting unification: `GenericMmapStore<R, I, M>` = `MmapScanned<Indexed<
BaseStore<R>, R, I>, R, M>` + blob. It would leave one slot-file
implementation and one durable layer. It is not proposed here because
`GenericMmapStore`'s public surface (`create`/`open`/`open_portable`/
`read_portable_records`, the `is_gapless` fast path over its own
`records`, its benchmarks in `benches/`) is `STORAGE-012`/`STORAGE-015`'s
contract, and re-expressing it as a stack is a refactor with its own ADR.
The shared `SlotFile` extraction gets most of the de-duplication without
touching that contract; full unification is a revisit trigger.

## Data/state and invariants

- One `.mmap` file per durable field, each self-contained: header, slots,
  commit markers, positions. Files of one stack share nothing on disk;
  positions in one file are unrelated to positions in another (each
  reconciles independently, and a record appended to one file at open
  because its slot was torn may sit at a different position than in the
  others). Nothing reads across files by position.
- The companion blob remains an immutable snapshot of all fields at
  `create`/heal time. For a durable scannable field, the blob's value is
  stale after the first `update` and is never consulted for that field
  once a persisted slot exists — reconciliation reuses the slot, exactly
  as `GenericMmapStore` does for `ScanMarker` today. `open`'s currency
  check (`STORAGE-015-FR-004`) still compares the caller's `records`
  against the blob, so a caller who hands `open` freshly re-read records
  does not trigger a rewrite.
- Update atomicity is per field. A stack-level "update two fields" is two
  `update` calls; a crash between them leaves the first durable. The
  single-copy-not-torn assumption `GenericMmapStore::is_committed`
  documents applies to each file separately.
- `Flush` order is top-down: the outermost layer's `msync` first, then
  inner. No ordering guarantee between files is offered or needed —
  each field's durability is independent.
- Torn-slot repair, stale slots, and gapless detection are per file.
  `is_gapless` in one file says nothing about another.

## Errors, failure, recovery, and observability

- `create` and `open` return `DurabilityError` with the same variants
  `GenericMmapStore` uses: `InvalidMagic`, `SchemaVersionMismatch`, `Io`.
  A slot-width mismatch (the weak foreign-file check) is a new cause
  under an existing variant if one fits (`SchemaVersionMismatch` does
  not; an `InvalidMagic`-style "wrong file" variant may be reused or a
  small new one added — implementation's call, spec'd then).
- A crash during `create` leaves a truncated or partial file; `open`
  treats a missing/short header as an error, a partial slot as absent,
  as today.
- A crash during `open`'s append-for-missing leaves an uncommitted slot,
  invisible next time and re-appended — `GenericMmapStore`'s repair, per
  file.
- Failure part-way through a stack's `open` (file 1 opens, file 2 does
  not) surfaces as the second file's error; file 1's appends, if any,
  have landed and are correct on their own. Nothing is rolled back;
  nothing needs to be.
- Observability: none new. `path()` on the layer names its file for
  error messages and tooling.

## Security, privacy, and compatibility

- **Compatibility**: every existing `.mmap` and blob opens unchanged
  (`MFMD-FR-006`). `GenericMmapStore`'s type and API are unchanged;
  `OrderProductionStack`'s type alias changes (a public type — any
  caller naming it re-compiles; the constructors' signatures do not
  change). `Scanned`, `Indexed`, `Symmetric`, `Reversed` are untouched
  beyond the new macro's generated impls.
- **Privacy/security**: one more file of `(id, value)` pairs per durable
  field, same content class as today's `.mmap`. No new trust assumptions
  beyond `GenericMmapStore`'s single-process exclusive-access one, now
  per file.
- **Dependencies**: none new (`memmap2` is already Tier 2's).

## Acceptance criteria

1. `MmapScanned::<GenericMmapStore<Order, Status, Amount>, Order,
   DiscountCents>` round-trips: `create`, `update` both `Amount` and
   `DiscountCents` on one record, drop, `open` — `get` returns both new
   values; `scan::<Amount>` and `scan::<DiscountCents>` each reflect
   their update and not the other's.
2. `open` reconciliation for the new file: a record missing from the file
   is appended and readable; a persisted id absent from `records` is
   ignored by `scan`; a slot with its commit byte cleared is re-appended.
3. `flush` on the stack `msync`s both files (observable via a
   `test`-only hook or via the file's mtime/data after a `flush` with no
   process exit — the existing test's technique).
4. `open_order_production_stack_portable(path)` after `create` +
   `update`s + drop returns a stack whose `get` reflects every update,
   with no blob rewrite (mtime unchanged).
5. A file whose slot width does not match the layer's `Id`/`ScanValue`
   widths is refused by `open` with an error naming the path.
6. Every existing `GenericMmapStore` test passes unchanged against the
   extracted `SlotFile`; `benches/` numbers for `scan` (gapless fast
   path) and `update` are within noise of the current baseline.
7. `forward_scannable_pairs!` (or its sibling) generates the cross-marker
   forwards for `MmapScanned` from one invocation per domain; no
   hand-written pair impl exists in `order_customer.rs`.
8. An in-memory inner (`MmapScanned<BaseStore<Order>, Order, Amount>`)
   works — the layer does not depend on `GenericMmapStore`.

## Verification plan

- Unit tests in the new module for criteria 1, 2, 3, 5, 8 (fixtures under
  `research`, as `mmap_store.rs`'s are).
- `order_customer.rs` tests for criteria 4 and 7.
- `cargo test`, `cargo test --all-features`, and the `mmap_record_identity_
  keying` integration test unchanged for criterion 6, plus one Criterion
  run of the existing generic-store benchmarks before and after the
  `SlotFile` extraction (criterion 6's second half). If the extraction
  moves the gapless `scan` fast path beyond noise, fall back to
  duplication and record why.
- One new benchmark row: `update` and `scan` through `MmapScanned` over
  `GenericMmapStore`, to show the layer adds no per-write cost and to put
  a number on the N-file `open` (two-file reconciliation at 100K/1M).
- The full sweep green.

## Traceability

| Requirement | Where it lands (implementation unit) |
|---|---|
| `MFMD-FR-001`, `-002`, `-003` | `src/generic/mmap_scanned.rs` (`ScanField`, `UpdateField`, `GetById`, `Flush`) |
| `MFMD-FR-004` | `src/generic/slot_file.rs` (extracted from `mmap_store.rs`); `MmapScanned::open` |
| `MFMD-FR-005`, `-007` | `src/generic/order_customer.rs` constructors and `OrderProductionStack` |
| `MFMD-FR-006` | no constant changes; existing format tests |
| `MFMD-FR-008` | `forward_scannable_pairs!` generalization in `src/generic/store.rs` |
| `MFMD-FR-009` | slot-width check in `MmapScanned::open`; blob-first `open_portable` |
| Spec | new `STORAGE-017` v0.1.0, `SPEC-REGISTRY` |

## Open questions

- Whether the new file should carry `R::SCHEMA_TAG` directly (`GMMAPST\0`
  version 3, a 28-byte header like the blobs'). Proposed: not in this
  unit — the weak slot-width check plus the blob-first portable path
  covers the realistic mistake, and a `.mmap` version bump would refuse
  every existing file. Revisit if a `.mmap` is ever opened without a
  tagged blob in the same stack.
- Whether N files warrant the stack manifest `ADR-0018` deferred. Two
  files plus a blob is still a naming convention on one constructor;
  five would not be. Deferred, with the count as the trigger.
- Whether `GenericMmapStore` should later be re-expressed as `MmapScanned`
  over `Indexed<BaseStore>` plus a blob (the unification above). Not
  here; its own ADR if the `SlotFile` extraction makes it obviously
  cheap.
- The `SlotFile` extraction's exact boundary — whether `scan`'s
  `chunks_exact` fast path lives in the helper or stays in each store.
  Implementation's call, guarded by criterion 6.
- A variable-width mutable durable field. Out of scope; option 3 (blob
  rewrite, or the crate's WAL) is the named fallback, and a string heap
  is a separate design if one is ever justified by a domain.

## Change history

- 2026-09-02: Proposed.
- 2026-09-02: Accepted as proposed by the owner, no changes requested.
