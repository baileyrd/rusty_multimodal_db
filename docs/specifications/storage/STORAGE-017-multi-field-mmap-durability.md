# STORAGE-017 — Multi-field mmap durability (`MmapScanned` per-field slot files over a shared `SlotFile`)

- Version: 0.1.0
- Status: Accepted
- Owners: baileyrd
- Depends on: `STORAGE-012` (the generic library: `ScannableField`,
  `Scanned`, `forward_scannable_pairs!`, `GenericMmapStore` and its
  `GMMAPST\0` version-2 slot format), `STORAGE-015` v0.2.0 (the tagged
  `.records` blob and `SchemaTag` this spec's foreign-file guard leans
  on; the portable-open path this spec extends to N files), ADR-0020 and
  `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md` (both Accepted —
  the design this spec turns into requirements), ADR-0006 (the one-
  mutable-field decision whose revisit trigger this spec answers)
- Supersedes: none. Extends, does not reverse, `ADR-0006`: the
  single-field `GenericMmapStore` stays exactly as scoped; a second
  durable field is a second layer with a second file.

## Purpose and scope

`GenericMmapStore<R, IndexMarker, ScanMarker>` (`src/generic/mmap_store.rs`)
owns every record, one equality index, and exactly *one* mmap-backed
scannable field. Every other field of `R` is durable through the
companion `.records` blob but immutable through the store: for `Order`,
`amount_cents` was the only field that was durable, mutable, and
exposed as scannable at once. `ADR-0006` chose that scope deliberately
and named its revisit trigger — *"a future record shape needs more than
one mutable field persisted"*. `ADR-0020` accepted this design as the
answer.

This spec adds, additively, the shape the accepted design proposes: a
new store layer, **`MmapScanned<S, R, Marker>`** (`src/generic/
mmap_scanned.rs`), the durable twin of `Scanned<S, R, Marker>` — one
more mutable, durable, scannable field over any inner store, backed by
its own slot file in exactly `GenericMmapStore`'s existing `GMMAPST\0`
version-2 format. The file mechanics both stores share are extracted
into a `pub(crate)` **`SlotFile<Id, V>`** (`src/generic/slot_file.rs`)
with `GenericMmapStore` delegating to it and no behaviour change.
`forward_scannable_pairs!` gains a `for MmapScanned;` arm so the
cross-marker forwards are generated, not hand-written. The reference
domain's `OrderProductionStack` gains `DiscountCents` as a second
durable, mutable field, with `Amount` unchanged and `CreatedAt`
deliberately left in-memory. The nine requirements below are the design
document's `MFMD-FR-001..009`, renumbered into this spec's namespace.

## Non-goals

- Not a change to any on-disk format: `GMMAPST\0` version 2,
  `GENBLOB\0` version 2, `GENEDGE\0` version 2 are all unchanged
  (`STORAGE-017-FR-006`). Every existing file opens as before.
- Not a per-file schema tag on the slot file. The design's option (b)
  — `GMMAPST\0` version 3 with a 28-byte tagged header — is the named
  revisit, not this unit (see "Open questions").
- Not a change to `GenericMmapStore`'s public surface, its own file, or
  its blob. Its tests are textually unchanged and are the guard on the
  `SlotFile` extraction.
- Not a unification of `GenericMmapStore` with `MmapScanned` over
  `Indexed<BaseStore>` plus a blob. The design defers that to its own
  ADR.
- Not a multi-field atomic update. Two `update` calls on two layers are
  two independent in-place writes; a crash between them is visible as
  one field new and one old — the same invariant `GenericMmapStore`
  plus a `Scanned` layer already had, now durable on both sides.
  Making the pair atomic is `ADR-0013`'s transaction territory.
- Not a variable-width mutable durable field (`String`). Out of scope;
  the design names the blob-rewrite or WAL fallback.
- Not a stack-level manifest naming a stack's files — deferred, with
  the file count as the trigger (two slot files plus a blob is still a
  naming convention on one constructor).

## Context and terminology

- **Slot file**: a `GMMAPST\0` file — 12-byte header (8-byte magic,
  `u32` little-endian `SCHEMA_VERSION`, currently `2`) followed by
  fixed-width slots `[id][value][COMMITTED]`, `Id::BYTE_WIDTH +
  V::BYTE_WIDTH + 1` bytes each. `GenericMmapStore` writes one for its
  `ScanMarker`; each `MmapScanned` layer writes one for its `Marker`.
- **`SlotFile<Id, V>`**: the `pub(crate)` owner of one mapped slot file
  — `MmapMut` plus path — carrying the constants, offsets, header
  read/write, committed-slot read, and the two halves of reconciliation
  ("committed pairs keyed by id"; "append these slots, report each
  landed position"). Both stores delegate to it.
- **`MmapScanned<S, R, Marker>`**: the layer. Owns a `SlotFile<R::Id,
  R::ScanValue>` and a `position_index: HashMap<R::Id, usize>` over an
  inner store `S`; forwards everything it does not own.
- **Reconciliation**: `open`'s by-id merge of the file's committed slots
  against the caller's `records` — reuse a committed slot; append a
  fresh one for a record without one; leave a committed slot whose id
  is not in `records` in place and ignore it; treat an uncommitted
  (torn) slot as absent and re-append its record. Per file,
  independent of every other file in the stack.
- **Gapless**: every slot in a file is live (`slot_count ==
  position_index.len()`), enabling the `chunks_exact` scan fast path
  that walks the mapping in file order; otherwise `scan` sorts the live
  positions and reads each.
- **Slot-width mismatch**: a file whose slot data (body after the
  header) is not a whole number of this layer's slots. Refused by
  `MmapScanned::open` with the new `DurabilityError::SlotWidthMismatch`.
- **`<path>.discount_cents.mmap`**: the `Order` domain's naming rule for
  its `DiscountCents` file, derived from the base `path` by
  `order_customer::discount_cents_path`, beside `<path>` (the `Amount`
  store) and `<path>.records` (the blob). The layer itself is
  path-agnostic; the rule is the domain's.

## Requirements

- `STORAGE-017-FR-001` (design `MFMD-FR-001`): a stack can expose more
  than one `ScannableField` marker of one record type as mutable *and*
  durable, each backed by a fixed-width in-place write into its own
  mapping. `MmapScanned::update` is a `position_index` lookup plus
  `SlotFile::write_value` — one bounded copy, no allocation, no syscall
  — the same cost shape as `GenericMmapStore::update`.
- `STORAGE-017-FR-002` (design `MFMD-FR-002`): each layer's values
  survive process exit without an explicit flush (the page cache's
  guarantee, as for `GenericMmapStore`) and `Flush::flush` on the layer
  `msync`s its own file, then forwards to the inner store, so a flush
  on the stack reaches every slot file.
- `STORAGE-017-FR-003` (design `MFMD-FR-003`): `GetById::get` through
  a `MmapScanned` layer returns `inner.get(id)` with this layer's field
  patched in via `ScannableField::set_scannable_value` from the slot
  file — so a stack of N layers returns a record whose every durable
  field reflects its latest `update`, not the blob's or the caller's
  `records`' value.
- `STORAGE-017-FR-004` (design `MFMD-FR-004`): `MmapScanned::open(inner,
  records, path)` reconciles its file against `records` by id with the
  same cases `GenericMmapStore::open` has — reuse, append-for-missing,
  ignore-stale — and the same torn-slot handling (an uncommitted slot
  is invisible and its record re-appended, through the same `O_APPEND`
  path). Appended slots are seeded from each record's own
  `scannable_value`. Every failure returns before any slot is appended.
- `STORAGE-017-FR-005` (design `MFMD-FR-005`):
  `open_order_production_stack_portable(path)` — read the blob via
  `GenericMmapStore::read_portable_records`, then open the stack as
  `open_order_production_stack` does — works for the two-file stack
  from `<path>`, `<path>.records`, and `<path>.discount_cents.mmap`
  alone, with no blob rewrite when the records are current.
- `STORAGE-017-FR-006` (design `MFMD-FR-006`): no on-disk format
  changes. `slot_file.rs` carries the same `MAGIC`, `SCHEMA_VERSION =
  2`, `HEADER_LEN`, and `COMMITTED` that `mmap_store.rs` did; the
  `GenericMmapStore` byte-level tests pass textually unchanged.
- `STORAGE-017-FR-007` (design `MFMD-FR-007`): `OrderProductionStack`
  is `Reversed<MmapScanned<GenericMmapStore<Order, Status, Amount>,
  Order, DiscountCents>, Customer, Order, BelongsToCustomer>`.
  `create_order_production_stack`, `open_order_production_stack`, and
  `open_order_production_stack_portable` keep their signatures; the
  first two create/open the `DiscountCents` file at `discount_cents_path
  (path)`. `Amount` stays in `GenericMmapStore`'s file; `CreatedAt`
  stays in-memory (it is scannable through `OrderGenericStore` and
  neither scannable nor updatable through the production stack — a
  compile-time refusal, not a runtime one).
- `STORAGE-017-FR-008` (design `MFMD-FR-008`): adding a durable field to
  a domain costs one layer in the stack type, one constructor line
  deriving its path, and one `forward_scannable_pairs!(for MmapScanned;
  R; …)` invocation entry. `forward_scannable_pairs!` takes the layer as
  a parameter (`for Scanned;` / `for MmapScanned;`; the original
  bare-record spelling still means `Scanned`) and generates the O(pairs)
  cross-marker `ScanField`/`UpdateField` forwards; no hand-written pair
  impl exists in `order_customer.rs` or `mmap_scanned.rs`.
- `STORAGE-017-FR-009` (design `MFMD-FR-009`): a foreign file is refused
  by name rather than misread. The layer writes no blob and its file
  carries no tag, so the guarantee rests on two things: (a) `create`/
  `open` sit in the one `impl` block bounded `R: SchemaTag`, tying the
  layer to a record type whose stack carries a tagged blob, and the
  domain's portable path reads that blob *first*, before any slot file
  is touched; (b) `open` refuses a file whose slot data is not a whole
  number of `Id::BYTE_WIDTH + ScanValue::BYTE_WIDTH + 1`-byte slots with
  `DurabilityError::SlotWidthMismatch { path, body_len, slot_width }`.
  The check is weak by design (a foreign width can divide the same body
  length) and is documented as such.

## Architecture and interfaces

- `src/generic/slot_file.rs` (new, `pub(crate)`): `MAGIC`,
  `SCHEMA_VERSION`, `HEADER_LEN`, `COMMITTED` (moved verbatim from
  `mmap_store.rs`); `SlotFile<Id, V> { mmap: MmapMut, path: PathBuf }`
  with `slot_width()`, `slot_offset(position)`, `read_value`,
  `write_value`, `slot_bytes()` (the body after the header — each owner
  keeps its own `chunks_exact` fast path over it), `slot_count()`,
  `trailing_partial_bytes()`, `is_gapless(live_count)`, `path()`,
  `flush()`, `create(path, slots: ExactSizeIterator<(Id, V)>)`,
  `open(path)` (header check only), `committed_pairs() -> HashMap<Id,
  (usize, V)>`, and `append_committed_slots(slots) -> Result<Vec<usize>>`.
  Private: `write_slot_into`, `is_committed`, `append_committed_slot`,
  `write_header`, `read_header`. No tests of its own — `mmap_store.rs`'s
  16 tests are the guard, unchanged.
- `src/generic/mmap_store.rs`: `GenericMmapStore` holds a `SlotFile`
  instead of a raw `MmapMut` + path and delegates every file operation;
  `create`/`open`/`scan`/`update`/`get`/`flush` semantics unchanged.
  Module docs gain a closing section on the shared engine and on the
  one policy divergence (this store tolerates trailing partial bytes;
  the layer refuses them). Tests textually unchanged; test-only imports
  now come from `slot_file`.
- `src/generic/mmap_scanned.rs` (new, `pub`): the struct; `inner()`,
  `inner_mut()`, `path()`; the `R: SchemaTag`-bounded `create`/`open`;
  `ScanField<R, Marker>`, `UpdateField<R, Marker>`, `GetById<R>`,
  `Flush`; generic `FilterEq`, `Neighbors`, `Children` forwards
  (`Parent` is the blanket impl). 8 tests, `research`-gated fixtures
  over `Order`.
- `src/generic/store.rs`: `forward_scannable_pairs!` gains two entry
  arms — `(for Scanned; $record; …)` and `(for MmapScanned; $record;
  …)` — ahead of the original `($record; …)` arm (which now expands to
  `for Scanned`; the `for` arms must come first because `for` also
  opens a `for<'a>` type and a `ty` matcher fails hard on it). The
  layer path travels through the `@rotate`/`@pairs` accumulators as one
  bracketed `tt` and is opened only in `@impl_pair`, which calls
  `Layer::inner`/`inner_mut` on the forward.
- `src/durability/mod.rs`: new variant `DurabilityError::
  SlotWidthMismatch { path: PathBuf, body_len: usize, slot_width:
  usize }` — *"mmap slot file at {path}: {body_len} bytes of slot data
  is not a whole number of {slot_width}-byte slots (written for another
  record shape, or truncated mid-slot)"*.
- `src/generic/order_customer.rs` (research-gated): the second
  `forward_scannable_pairs!(for MmapScanned; …)` invocation; the new
  `OrderProductionStack` alias; `pub fn discount_cents_path`; a private
  `layer_and_reverse(core, orders, path, open)` shared by `create`/
  `open`; docs on all three constructors. 1 added test, 1 extended,
  plus 2 compile-time pair checks.
- `src/generic/mod.rs`: `pub mod mmap_scanned;`, `pub(crate) mod
  slot_file;`, `pub use mmap_scanned::MmapScanned;`.
- `benches/generic_production.rs`: two new Criterion groups,
  `generic_production_scan_layer` and `generic_production_update_layer`,
  timing `scan`/`update` of `DiscountCents` through the layer beside the
  existing `scan`/`update` of `Amount` through the core; the existing
  `create`/`open` groups now time the two-file stack.
- No new dependency; `memmap2` is already Tier 2's.

## Data/state and invariants

- One slot file per durable field, each self-contained. Files of one
  stack share nothing on disk; positions in one file are unrelated to
  positions in another (a record re-appended to one file after a torn
  slot may sit at a different position than in the others). Nothing
  reads across files by position.
- The companion blob remains an immutable snapshot of all fields at
  `create`/heal time. For a durable scannable field the blob's value is
  stale after the first `update` and is never consulted once a committed
  slot exists — reconciliation reuses the slot. `GenericMmapStore::open`'s
  blob currency check still compares the caller's `records` against the
  blob, so a caller who hands `open` freshly re-read records does not
  trigger a rewrite (pinned: acceptance criterion 4).
- Update atomicity is per field; `Flush` order is top-down (this layer's
  `msync`, then inner). No ordering guarantee between files is offered
  or needed.
- Gapless detection, torn-slot repair, and stale slots are per file.
- The `Order` stack's three files travel together: `<path>`,
  `<path>.records`, `<path>.discount_cents.mmap`. A missing
  `DiscountCents` file is an `Io` (`NotFound`) error from `open`;
  `create` writes all three.

## Errors, failure, recovery, and observability

- `MmapScanned::create`: `DurabilityError::Io` if the parent can't be
  created, the file can't be created/sized/mapped, or the initial flush
  fails.
- `MmapScanned::open`: `Io` (missing, unmappable, append failure);
  `InvalidMagic`; `SchemaVersionMismatch`; `SlotWidthMismatch` naming
  the path. All returned before any slot is appended.
- A crash during `create` leaves a partial file: a short header is an
  error, a partial trailing slot is a `SlotWidthMismatch` (the layer
  has no blob to fall back on, so it prefers refusal to a guess — the
  one policy divergence from `GenericMmapStore`, which ignores trailing
  bytes). A crash during `open`'s append leaves an uncommitted slot,
  invisible next time and re-appended.
- Failure part-way through a stack's `open` (core opens, layer does
  not) surfaces as the layer's error; the core's appends and blob heal,
  if any, have landed and are correct on their own. Nothing is rolled
  back; nothing needs to be.
- `path()` on the layer names its file. No other new observability.
- No `unwrap`/`expect` outside `#[cfg(test)]`.

## Security, privacy, and compatibility

- Purely additive at the file level: every existing `.mmap` and blob
  opens unchanged. `GenericMmapStore`'s type and API are unchanged.
- `OrderProductionStack` is a public type alias (research-gated) whose
  definition changes; any caller naming it re-compiles. The three
  constructors' signatures do not change. `open_order_production_stack`
  on a pre-feature directory (base file + blob, no `DiscountCents`
  file) fails with an `Io` (`NotFound`) error — it does not heal it,
  because the layer's `open` has no `create` fallback (design: a
  missing/short file is an error, as for `GenericMmapStore`). Recreate
  with `create_order_production_stack`.
- `Scanned`, `Indexed`, `Symmetric`, `Reversed` are untouched beyond the
  macro's generated impls.
- One more file of `(id, value)` pairs per durable field, same content
  class as today's `.mmap`; the single-process exclusive-access
  assumption now applies per file.
- Synthetic data only; no network surface.

## Acceptance criteria

Numbered as in the design document.

1. `MmapScanned<GenericMmapStore<Order, Status, Amount>, Order,
   DiscountCents>`: `create`, `update` `Amount` and `DiscountCents` on
   one record, drop, `open` — `get` returns both new values;
   `scan::<Amount>` and `scan::<DiscountCents>` each reflect their own
   update and not the other's. Also through the full
   `OrderProductionStack` with a `flush` before drop.
2. `open` reconciliation for the layer's file: a record missing from the
   file is appended and readable; a persisted id absent from `records`
   is ignored by `scan`; a slot with its commit byte cleared is
   re-appended (via the `HEADER_LEN`/slot-offset arithmetic the byte
   tests share).
3. `flush` on the layer reaches both files (observed through the file
   bytes after `flush`, no process exit).
4. `create_order_production_stack` + `update`s + `flush` + drop, then
   `open_order_production_stack_portable(path)` returns a stack whose
   `get` reflects every update; the blob's bytes are unchanged; a
   second portable open reads the same values.
5. A file whose body is not a whole number of the layer's slots (a
   `Narrow { id: u32, weight: i64 }` file of 13-byte slots read as
   25-byte `Order` slots) is refused by `open` with an error whose
   `Display` names the path; a file truncated mid-slot is refused the
   same way; a file with a bogus magic is `InvalidMagic`.
6. Every existing `GenericMmapStore` test passes textually unchanged
   against the extracted `SlotFile`; `benches/generic_production.rs`'s
   `scan` (gapless fast path) and `update` groups are within noise of
   the pre-extraction baseline (`RESULTS.md`).
7. `forward_scannable_pairs!(for MmapScanned; Order; …)` generates all
   six ordered cross-marker pairs — pinned by compile-time
   `_mmap_pair_exists` checks in `order_customer.rs`'s tests; no
   hand-written pair impl exists.
8. `MmapScanned<BaseStore<Order>, Order, Amount>` works: the layer does
   not depend on `GenericMmapStore`.

## Verification plan

- `cargo test --all-features`: the 8 `generic::mmap_scanned` tests
  (criteria 1, 2, 3, 5, 8), 1 new and 1 extended `generic::order_customer`
  test plus 2 compile-time pair checks (criteria 1, 4, 7), and every
  pre-existing test — 316 lib tests — passing. `cargo test` (default features): 124
  lib + 2 integration, unchanged in count.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo doc --no-deps` (no new warnings): clean.
- Criterion, `--features research --bench generic_production`, before
  and after: the core's `scan`/`update` within noise (criterion 6;
  confirmed by a back-to-back `git stash`-isolated pair); new
  `scan_layer`/`update_layer` rows beside `scan`/`update`, landing
  where they do; `get`/`parent` carrying the second layer's per-call
  cost (+22–28% at 1K/100K, +67% at 1M); the two-file `create`/`open`
  cost (+15–42%). Recorded in `RESULTS.md`'s `## Generic schema
  library` section.

## Traceability

Implements: ADR-0020 / `MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`
(`MFMD-FR-001..009` ↔ `STORAGE-017-FR-001..009`, one-to-one). Answers
`ADR-0006`'s revisit trigger without reversing it. Depends on:
`STORAGE-012` (the layer shapes and the store extended),
`STORAGE-015` v0.2.0 (the tagged blob and `SchemaTag` the foreign-file
guard leans on; the portable path extended). Feeds: the design's
deferred unification of `GenericMmapStore` and the per-file tag
revisit.

Where the implementation settles something the design left to it:

- **Slot-width refusal is a new variant.** The design allowed "an
  `InvalidMagic`-style variant reused or a small new one added".
  `SlotWidthMismatch { path, body_len, slot_width }` is the new one: no
  existing variant carries both the path and the arithmetic that
  explains the refusal.
- **The `chunks_exact` fast path stays in each owner.** `SlotFile`
  exposes `slot_bytes()`; `GenericMmapStore::scan` and
  `MmapScanned::scan` each keep their own gapless fast path over it,
  so the extraction does not touch the benchmark-pinned loop.
- **Trailing partial bytes: permissive in the store, refused in the
  layer.** `GenericMmapStore` keeps ignoring them (its tests pin that);
  `MmapScanned::open` refuses them, folding the truncated-mid-slot case
  into the slot-width check because the layer has no blob to fall back
  on.
- **Macro spelling.** The design offered "generalized to take the layer
  type as a parameter, or a sibling macro". It is the former:
  `forward_scannable_pairs!(for MmapScanned; …)`, with the original
  spelling preserved as `for Scanned`.
- **`CreatedAt` through the durable stack is a compile error.** The
  design said it "stays in-memory only"; in practice
  `OrderProductionStack` has no `ScanField`/`UpdateField` for
  `CreatedAt` at all, since `GenericMmapStore` owns only `Amount` and
  nothing in the stack owns `CreatedAt`. The refusal is at compile time,
  which is stronger than the design asked for.
- **`open` does not heal a missing layer file.** A pre-feature `Order`
  directory does not open through `open_order_production_stack`; see
  "Security, privacy, and compatibility". The design's error model
  ("a missing/short header is an error") is followed; the domain is
  research-gated, so no user data is affected.

## Open questions

- Whether the layer's file should carry `R::SCHEMA_TAG` directly
  (`GMMAPST\0` version 3, a 28-byte header like the blobs'). Not in
  this unit: the `SchemaTag` bound plus the blob-first portable path
  covers the realistic mistake, and a `.mmap` version bump would refuse
  every existing file. Revisit if a slot file is ever opened without a
  tagged blob in the same stack.
- Whether N files warrant the stack manifest `ADR-0018` deferred.
  Deferred, with the file count as the trigger.
- Whether `GenericMmapStore` should be re-expressed as `MmapScanned`
  over `Indexed<BaseStore>` plus a blob. Its own ADR, if ever; the
  `SlotFile` extraction has already removed the duplication that would
  motivate it.
- Whether `open_order_production_stack` should heal a missing layer file
  by creating it from `orders` (the way `GenericMmapStore::open` heals
  a missing blob). Not proposed: a slot file's values may be newer than
  `orders`', so silently recreating one from `orders` would look like
  a heal and be a data loss. If a domain needs it, it is an explicit
  `create_layer_if_missing` on that domain's constructor, spec'd then.
- A variable-width mutable durable field. Out of scope; the design's
  option 3 (blob rewrite, or the crate's WAL) is the named fallback.

## Change history

- 0.1.0 (2026-09-02): Initial accepted draft, alongside the real
  implementation (`src/generic/slot_file.rs`, `src/generic/
  mmap_scanned.rs`, the `SlotFile` delegation in `src/generic/
  mmap_store.rs`, the `for Layer;` arms of `forward_scannable_pairs!`
  in `src/generic/store.rs`, the `SlotWidthMismatch` variant, the
  `Order` stack changes in `src/generic/order_customer.rs`, two
  Criterion groups) and 9 new tests, 1 extended, plus 2 compile-time
  checks.
  Registers the design ADR-0020 accepted on 2026-09-02 as requirements;
  records the six implementation calls the design left open under
  "Traceability".
