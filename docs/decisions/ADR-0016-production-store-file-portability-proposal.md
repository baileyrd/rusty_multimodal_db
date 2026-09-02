# ADR-0016: Add file portability to `ProductionStore` — persist `breed`/edges alongside the existing `age`-only mmap file

- Status: **Accepted** (promoted from Proposed on 2026-09-01 — the owner approved the design as proposed, no changes requested)
- Date: 2026-09-01
- Deciders: baileyrd
- Related: `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` (the full
  design document this ADR summarizes), `docs/decisions/ADR-0006-tier-2-durability-architectures.md`
  (the original "ages-only" scope-down and its own revisit trigger this
  proposal responds to), `docs/design/GENERIC-SCHEMA-DESIGN.md` §4.2 (the
  identical wall hit a second time, with `Order`), ADR-0008
  (`ProductionStore`)
- Supersedes/Superseded by: none. Extends, does not reverse, ADR-0006's
  decision — `MmapAgeStore`'s own ages-only file format is unchanged by
  this proposal (see "Decision" below).

## Context

The owner asked to pursue `ProductionStore` file portability — a standing
open question this project's own docs have carried since
`MMAP-AGE-STORE-IDENTITY-FIX`: *"`ProductionStore`'s file still not
portable/self-contained the way SQLite's/DuckDB's are — persists only
`age`, not the full record."* `RESULTS.md`'s `## External database
comparison` section made the gap concrete by direct contrast: SQLite's/
DuckDB's own `.db` files are fully self-describing (`open(path)` alone
reconstructs everything); `ProductionStore::open` requires the caller to
already hold the full `Vec<DogRecord>`/`Vec<(Uuid, Uuid)>` in memory,
because the `.mmap` file itself only ever persisted `age`.

This is not a new gap — `ADR-0006` scoped it out explicitly when
`MmapAgeStore` was first built (*"needs either a string-heap-with-offsets
file format ... or capping `breed`'s length ... neither is minimal"*),
and named its own revisit trigger for exactly this situation: *"a future
record shape needs more than one mutable field persisted."*
`GENERIC-SCHEMA-DESIGN.md` §4.2 hit that trigger once already, with
`Order`, and confirmed the string-heap problem is real for **mutable**
variable-length fields. This ADR follows `AGENTS.md`'s own ADR-cadence
guidance (this project is in active bootstrap/major-development, so
default to writing one) — a change to `ProductionStore`'s public
constructor surface and a new on-disk file format are both the kind of
consequential, hard-to-reverse decision this project's convention treats
as ADR-worthy, matching how `ADR-0010`/`ADR-0009` proposed a design
before implementation for comparably weighty decisions.

## Decision drivers

- **Additive, not a rewrite — of either `MmapAgeStore`'s file format or
  `ProductionStore`'s existing public API.** `MMAP-AGE-STORE-IDENTITY-FIX`
  closed out a real correctness round on `MmapAgeStore`'s own format
  recently; this decision should not reopen it without a genuinely new
  reason, matching this project's own established "don't touch a closed
  module without a real reason" pattern.
- **Solve the problem this data actually has, not the harder one
  ADR-0006 already declined.** `breed`/`id`/edges are never mutated
  anywhere in this crate — only `age` is. The string-heap complexity
  ADR-0006 rejected exists specifically to support in-place mutation of
  variable-length data; immutable data doesn't need it.
- **Reuse already-validated pieces of this project's own prior work.**
  `DogRecord` already derives `Serialize`/`Deserialize`;
  `SnapshotFullStore` already proves a plain `bincode` round trip works
  for full records; `MmapAgeStore`'s own columnar rewrite path already
  established a write-to-temp-then-atomic-rename crash-safety pattern —
  this proposal reuses all three rather than inventing new mechanisms.
- **Design-first for a new on-disk format and public constructor**,
  matching `ADR-0006`'s/`ADR-0009`'s/`ADR-0010`'s own precedent: a file
  format and a public API are both comparably hard to reverse once real
  data exists in the wild (even if that "wild" is just this crate's own
  test/bench suite) — this ADR proposes a design and authorizes no
  implementation, the same posture `ADR-0010` took for the server layer.

## Considered options

See `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md`'s "Considered
options" section for the full reasoning. Summarized:

1. **Extend `MmapAgeStore`'s own columnar file format** with a
   string-heap region for `breed` and an edge-list region, in the same
   file. Rejected — this is the exact redesign `ADR-0006` already
   declined once, and it solves a harder problem (in-place mutation)
   than this immutable data actually needs; it would also touch a
   recently-closed round's own file format without a new correctness
   reason.
2. **Cap `breed`'s length** and map it directly into `MmapAgeStore`'s
   existing fixed-width region. Rejected — an unapproved record-shape
   change, and doesn't solve variable-length edges at all.
3. **A separate, sibling companion file**, bincode-serialized
   `(Vec<DogRecord>, Vec<(Uuid, Uuid)>)`, written once at `create()` (and
   re-written only when `open`'s existing reconciliation logic detects a
   genuinely changed record set). **Chosen.** Zero changes to
   `MmapAgeStore`'s own format; reuses already-justified
   dependencies/derives/patterns throughout.
4. **Replace `open`'s signature** (`open(path)`, dropping `records`/
   `edges`) vs. **add a new, additive constructor**
   (`open_portable(path)`). The former was rejected — breaking, and would
   silently drop the identity-keyed-reconciliation capability
   `MMAP-AGE-STORE-IDENTITY-FIX` exists to provide. **The latter was
   chosen** — every existing call site is unaffected.

## Decision

- `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` records the full
  proposed design: a new companion file (bincode-serialized
  `id`/`breed`/edges, write-to-temp-then-atomic-rename crash safety,
  reusing `MmapAgeStore`'s own established pattern), a new
  `ProductionStore::open_portable(path)` constructor implemented in
  terms of the existing `open`, and zero changes to `MmapAgeStore`'s own
  file format or to `create`/`open`'s existing signatures.
- No new dependency. `bincode`/`serde` (already present) are reused;
  `DogRecord`'s `Serialize`/`Deserialize` derive (already present) is
  reused unchanged.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No existing source file is modified by this ADR itself. Per
  this project's own established pattern (`STORAGE-012` following
  `GENERIC-SCHEMA-DESIGN`'s acceptance, `SERVER-001` following
  `ADR-0010`'s), the next unit registers a new spec (`STORAGE-014`) and a
  real implementation packet before any code changes.
- This design does not extend to `crate::generic`/`GenericMmapStore`,
  which has the identical limitation (`GENERIC-SCHEMA-DESIGN.md` §4.2)
  but a different, generic-over-`Record` API shape — a separate, later
  decision if pursued (see the design document's "Open questions").

## Consequences

### Positive

- Closes a real, honestly-named gap against SQLite's/DuckDB's own file
  portability — the exact comparison `RESULTS.md`'s external-database
  section made directly, without engineering the numbers to look better
  than they are.
- Reuses `SnapshotFullStore`'s already-proven "bincode-serialize a full
  `DogRecord`" approach and `MmapAgeStore`'s already-proven crash-safe
  rewrite mechanism, rather than designing either from scratch —
  materially lower implementation risk than a from-scratch string-heap
  format would carry.
- Zero risk to `MmapAgeStore`'s own closed, recently-hardened file
  format or to `ADR-0008`'s headline finding (`MmapAgeStore`'s per-write,
  zero-loss-window durability) — the mutable-`age` path is completely
  untouched by this design.
- Fully additive to `ProductionStore`'s public API — every existing
  caller (every benchmark, every test, every doctest) needs zero changes.

### Negative / tradeoffs

- **Two files must travel together for `open_portable` to work.**
  Copying only the `.mmap` file (e.g. from a pre-this-feature backup)
  produces a clear, typed error, not a silent partial reconstruction —
  but it is a real new failure mode a caller has to understand, absent
  under the existing `create`/`open` constructors.
- **The companion blob's immutability assumption is load-bearing, not
  incidental.** This design works because no method in this crate
  mutates `breed`, `id`, or the edge set today. If a future round ever
  adds such a mutation, this design needs real revisiting — named
  explicitly here as a revisit trigger, not silently assumed to keep
  holding.
- `create()`'s own cost grows (now writes two files, not one) — never
  timed in any existing benchmark's `b.iter()` closure, so expected to be
  informational only, but a real, new cost worth measuring once
  implemented, not just assumed negligible.
- Does not address `crate::generic`/`GenericMmapStore`'s identical gap —
  a real, named, deliberately out-of-scope limitation of this proposal,
  not an oversight.

## Validation and revisit triggers

- **This proposal's own validation**: design-only. No standalone scratch
  probe (unlike `ADR-0009`/`ADR-0010`'s new-public-protocol proposals) —
  this design extends an existing, already-well-tested module with
  already-validated pieces (`DogRecord`'s derive, `bincode`,
  `write_via_rename`'s pattern), so the real implementation's own test
  suite is the more direct verification, per the design document's own
  "Verification plan."
- **Real validation, post-acceptance**: a new spec (`STORAGE-014`), real
  implementation (`src/production.rs`, a new module for the companion
  blob), a full before/after benchmark of `create()`'s own cost, and
  `MmapAgeStore`'s existing 11-test suite passing unchanged (zero
  regression to the closed round this proposal builds next to).
- Revisit if: a future round adds a way to mutate `breed`, `id`, or the
  record/edge set after `create()` — this design's "the companion blob
  is immutable except via `open`'s existing reconciliation" assumption
  would need real rework, not just an incremental patch.
- Revisit if: `crate::generic`/`GenericMmapStore` needs the identical
  treatment — a separate, later decision, not automatically covered by
  this ADR.
- Revisit if: the companion blob's write cost at `create()`/reconciling-
  `open()` time turns out to be non-negligible at scale (unmeasured by
  this proposal) — at that point a cheaper incremental-update format
  (rather than a full re-serialize) would be the natural next step, not
  a different architecture.

## Acceptance and implementation

- 2026-09-02: implemented as `PRODUCTION-STORE-PORTABILITY` (`STORAGE-014`
  v0.1.0). `src/durability/record_blob.rs` (new), `ProductionStore::create`
  /`open`/`open_portable` in `src/production.rs`, and
  `DurabilityError::RecordBlobUnreadable` in `src/durability/mod.rs`;
  `src/durability/mmap_store.rs` untouched, confirmed by an empty diff.
- Naming convention finalized: the companion blob is literally
  `<path>.records` (so `dogs.mmap` pairs with `dogs.mmap.records`) — the
  two files sort adjacently and the pairing is visible without reading
  either. The rewrite temp file is `<companion>.rewrite-tmp`.
- **One divergence from this ADR's expectation, measured not assumed**: the
  "Validation and revisit triggers" bullet above treated the blob's write
  cost at `create()`/reconciling-`open()` time as unmeasured; the design
  document's own open question predicted the common-case `open` cost to be
  zero, on the theory that the blob rewrite could ride on `MmapAgeStore`'s
  own rewrite decision. It cannot without touching `MmapAgeStore` (that
  decision is private), and this round's whole point was not touching it.
  `open` therefore serializes the record set and byte-compares it against
  the on-disk blob on every call. Release build, median of 7 samples (3 at
  1M): `create` +15%/+68%/+78% and `open` +27%/+30%/+27% at 1K/100K/1M
  records; `open_portable` lands between the old and new `open`; the blob
  is ~5.6× the ages file (~118 B/record). Full table in `RESULTS.md`'s
  `### ProductionStore file portability (STORAGE-014)` subsection. No
  benchmarked hot path is affected — nothing times `create`/`open` inside
  `b.iter()`.
- The "cheaper incremental-update format" revisit trigger is not yet
  tripped: the named follow-up if `open`'s steady-state cost ever matters
  is a content hash in the blob header (a `BLOB_VERSION` bump), not a
  different architecture. Not built this round; no caller has asked.
- `GenericMmapStore` remains out of scope, per the last revisit bullet.
