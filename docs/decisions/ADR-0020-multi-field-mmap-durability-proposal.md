# ADR-0020: More than one mutable durable field per record — a per-field `MmapScanned` layer over the existing `.mmap` format

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner approved the design as proposed, option 2 with the `SlotFile` extraction, over the duplication variant, the multi-slot layout, and closing it; no changes requested)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md` (the full
  design document this ADR summarizes),
  `docs/decisions/ADR-0006-tier-2-durability-architectures.md` (whose
  revisit trigger — *"a future record shape needs more than one mutable
  field persisted"* — this answers),
  `docs/decisions/ADR-0009-generic-schema-design-proposal.md` and
  `docs/design/GENERIC-SCHEMA-DESIGN.md` §4.1/§4.2 (the generic library;
  §4.2 confirms the trigger bites with `Order` and sketches the per-field
  shape), `STORAGE-012` (`GenericMmapStore`, `Scanned`,
  `forward_scannable_pairs!`), `STORAGE-015` v0.2.0 and `ADR-0017` (the
  `.records` companion blob — an immutable snapshot, which is what makes
  every non-`ScanMarker` field read-only through the store),
  `STORAGE-016` v0.2.0 and `ADR-0018` (the caller-supplied-path
  precedent for a layer's own file), `ADR-0019` (the blob schema tag)
- Supersedes/Superseded by: none. Extends `ADR-0006`'s and `ADR-0009`'s
  mmap scope from one durable mutable field to N, without changing any
  format either produced.

## Context

`ADR-0006` scoped every Tier 2 durability variant to the `age` field
only, and said when to come back: *"Revisit if: a future record shape
needs more than one mutable field persisted — at that point mmap's and
`redb`'s ages-only scope-down would need real redesign (the string-heap/
fixed-layout problem this ADR chose not to solve), not just an
incremental extension."* `GENERIC-SCHEMA-DESIGN` §4.2 found the trigger
met by the first second domain tried: a real order domain wants
`amount_cents` and `status`, or `amount_cents` and `discount_cents`,
mutable and durable. `ADR-0009` recorded it as *"confirmed necessary
(not hypothetical) but not designed in this pass,"* and
`GenericMmapStore` shipped with exactly one `ScanMarker` in its `.mmap`
file, its module docs pointing at that unscoped follow-up.
`OrderProductionStack` makes `Amount` durable; `CreatedAt` and
`DiscountCents` are in-memory only. `PROJECT-STATUS` item 22 has carried
this since the generic library landed; item 68 names it as the owner's
third queued follow-up ("1, 2, 3, then 4") and frames the choice:
*"weighing a multi-slot `.mmap` layout against per-field `.mmap` files
against widening the blob's role."*

Two things narrowed the problem since `ADR-0006` wrote that sentence.
`GenericMmapStore` already settled the fixed-layout half by construction:
a durable field is whatever implements `MmapFieldValue` (fixed-width
`Copy`), and §4.2 recorded that as a load-bearing constraint. And
`STORAGE-015`'s companion blob made every field durable-but-immutable,
so the remaining gap is precisely "mutable *and* durable for more than
one fixed-width field" — not strings, not a general record format.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0019` all took, and the one
`PROJECT-STATUS` item 68 asks for.

## Decision drivers

- Answer `ADR-0006`'s revisit trigger with a scoped design rather than
  leaving it "genuinely unscoped" indefinitely.
- Keep the per-write cost shape `ADR-0006` called the standout result of
  the durability pass: one bounded in-place copy, no allocation, no
  syscall.
- Do not change a format that works. `GMMAPST\0` version 2 is the file
  `ADR-0008`'s production default descends from; `GENBLOB\0` version 2
  just landed.
- Follow the library's own architecture (`ADR-0009`): one capability,
  one layer, composed by type — `Scanned` stacks; its durable twin should
  too.
- Column layout for scan-shaped reads (`ADR-0001`'s AoS/SoA result),
  which every scan number in this project since has confirmed.
- Adding a durable field to a domain must not cost hand-written
  forwarding impls (the `forward_scannable_pairs!` rule).

## Considered options

1. **A multi-slot `.mmap` layout** — one file, slot
   `[id][v1]…[vN][COMMITTED]`, `SCHEMA_VERSION` 3, a per-domain layout
   type with macro-generated `DurableSlot<R, M> { OFFSET }` impls (the
   same `E0119` wall `forward_scannable_pairs!` exists for, at O(N)).
   One file, one reconciliation, one `msync`. But it refuses every
   existing `.mmap` (detection, not migration, and `open` cannot heal a
   slot that now needs N values from one), makes every slot helper,
   `create`, `open`, and the benchmark-pinned `scan` fast path
   layout-generic, and walks a wider stride to scan one field — the AoS
   shape `ADR-0001` measured as bad for single-column scans. The most
   invasive form of the "real redesign" `ADR-0006` predicted.
2. **Per-field `.mmap` files via a composable layer** — proposed. A new
   `MmapScanned<S, R, Marker>`, the durable twin of `Scanned`: one file
   per marker in exactly `GenericMmapStore`'s existing format, stacked
   over any inner store, `get` patching its field on the way up, `Flush`
   flushing its own file then forwarding, cross-marker forwards
   generated by `forward_scannable_pairs!` generalized to the layer
   type. The slot/header/commit/reconcile machinery is extracted from
   `GenericMmapStore` into a `pub(crate)` `SlotFile` helper both use
   (duplication the fallback if the extraction disturbs the fast path).
   The shape §4.2 already sketched. Costs N files, N reconciliations, N
   `msync`s, and a naming convention on the domain constructor.
3. **Widen the blob's role** — rewrite (or append to) the `.records`
   blob on `update`. The only shape that could carry a variable-width
   field, and not mmap durability at all: a full encode + fingerprint +
   write per update, milliseconds against nanoseconds; the append
   variant is a second WAL beside `STORAGE-008`'s. Rejected for
   fixed-width fields; the named fallback for a variable-width one.

## Decision

Accepted: option 2. Concretely, at implementation:

- `src/generic/slot_file.rs` (`pub(crate)`): the file mechanics of
  `GenericMmapStore` — constants, slot arithmetic, read/write,
  commit-marker append, header check, "read committed pairs by id",
  "append slots for missing records reporting positions" — with
  `GenericMmapStore` delegating and its tests and benchmarks unchanged.
- `src/generic/mmap_scanned.rs`: `MmapScanned<S, R, Marker>` with
  `create(inner, &records, path)` / `open(inner, &records, path)`,
  `inner`/`inner_mut`/`path`; `ScanField`/`UpdateField` for its marker,
  `GetById` patched via `set_scannable_value`, `Flush` own-then-inner,
  generic forwards for `FilterEq`/`Neighbors`/`Children`; `R: SchemaTag`
  on the file constructors. No blob of its own. A slot-width check on
  `open` as the weak foreign-file guard; the tagged blob, read first on
  the portable path, is the strong one.
- `forward_scannable_pairs!` generalized to take the layer type (or a
  sibling macro sharing its body).
- `OrderProductionStack` becomes `Reversed<MmapScanned<GenericMmapStore<
  Order, Status, Amount>, Order, DiscountCents>, Customer, Order,
  BelongsToCustomer>`; the three constructors derive
  `<path>.discount_cents.mmap` from the base path; `CreatedAt` stays
  in-memory only, deliberately.
- No format changes: `GMMAPST\0` 2, `GENBLOB\0` 2, `GENEDGE\0` 2.
- A new spec, `STORAGE-017` v0.1.0, carrying the design's `MFMD-FR-001`
  to `-009`, registered at implementation.

## Consequences

### Positive

- `ADR-0006`'s revisit trigger has a scoped answer; `PROJECT-STATUS`
  item 22 stops being "genuinely unscoped."
- Per-write cost is unchanged in shape and, by the design's own
  benchmark criterion, in measure.
- Nothing on disk changes; every existing file opens as before.
- A domain adds a durable field by adding a layer: one type parameter,
  one constructor line, one macro entry.
- The layer works over an in-memory inner too — durable fields without
  a `GenericMmapStore`.
- Each field's scan walks only its own file (column layout).

### Negative / tradeoffs

- N + 1 files per stack and a naming convention on the domain
  constructor; the stack manifest `ADR-0018` deferred gets more
  attractive with each field.
- N reconciliation passes at `open` (each O(records) with a `HashMap`)
  and N `msync`s per `flush`. The design asks for a two-file number at
  100K/1M.
- The cross-marker forwarding stays O(N²) generated impls per domain.
- No multi-field atomic update — as today, named as an invariant, not
  solved (that is `ADR-0013`'s territory, not storage's).
- The `SlotFile` extraction touches `GenericMmapStore`'s implementation
  (not its API) on a benchmark-pinned path; criterion 6 in the design
  guards it, and duplication is the fallback.
- The new `.mmap` carries no tag of its own (a `.mmap` version bump
  would refuse every existing file); the slot-width check is weak by
  design, and the tagged blob is what catches a foreign directory.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching
  `ADR-0017`–`ADR-0019`; written from direct investigation of
  `GenericMmapStore`'s `create`/`open`/query impls, `Scanned`,
  `forward_scannable_pairs!`, `OrderProductionStack` and its
  constructors, and `ADR-0006`/§4.2's exact wording.
- **Real validation, post-acceptance**: `STORAGE-017` v0.1.0; the
  design's eight acceptance criteria as tests (round trip of two
  durable fields; per-file reconciliation and torn-slot repair; two-file
  `flush`; portable open after updates with no blob rewrite; slot-width
  refusal; every existing `GenericMmapStore` test and benchmark
  unchanged through the `SlotFile` extraction; macro-generated forwards
  only; an in-memory inner); one Criterion row for `update`/`scan`
  through the layer and two-file `open`.
- Revisit if: a domain needs a variable-width mutable durable field —
  option 3 (blob rewrite or the crate's WAL) or a string heap, a
  separate ADR.
- Revisit if: a `.mmap` is ever opened without a tagged blob in the same
  stack — `GMMAPST\0` version 3 with the 8-byte tag.
- Revisit if: a stack reaches enough files that one constructor's naming
  rule stops being adequate — the manifest.
- Revisit if: the `SlotFile` extraction makes re-expressing
  `GenericMmapStore` as `MmapScanned` over `Indexed<BaseStore>` plus a
  blob obviously cheap — its own ADR.
- Revisit if: a workload needs several fields of one record read
  together at scan speed — option 1's row shape, as an additional store,
  not a replacement.

## Acceptance and implementation

- 2026-09-02: accepted as proposed (option 2, the per-field `MmapScanned`
  layer with the `SlotFile` extraction; the duplication variant, the
  multi-slot single-file layout, and closing as not worth building all
  declined). The next unit registers `STORAGE-017` v0.1.0 and implements
  per `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`.
- 2026-09-02: implemented as `STORAGE-017` v0.1.0 in PR #114 —
  `src/generic/slot_file.rs` (the extraction), `src/generic/mmap_scanned.rs`
  (the layer), the `for Layer;` arms of `forward_scannable_pairs!`, the
  `SlotWidthMismatch` variant, and `OrderProductionStack` gaining
  `DiscountCents`. No format change; `GenericMmapStore`'s tests textually
  unchanged. The implementation calls the design left open (the new
  error variant, the fast path staying in each owner, the layer refusing
  trailing partial bytes, the macro spelling, `CreatedAt` as a
  compile-time refusal, no heal of a missing layer file) are recorded in
  the spec's "Traceability" section. Measured (`RESULTS.md`, `## Generic
  schema library`): the core's `scan`/`update` unmoved by the extraction;
  the second field costs one more slot read per `get` (+22–28% at
  1K/100K, +67% at 1M on the two-field stack, `parent` in step) and one
  more file per `create`/`open` (+15–42%). The `open` cost is the
  N-reconciliation-passes tradeoff "Consequences" names; the per-`get`
  cost is one it did not name — the same per-layer price `Scanned`'s
  write-through fix measured for the in-memory layer (`RESULTS.md`,
  43–88%), paid here on a slot-file read. Recorded as an amendment, not
  folded into the accepted text.
