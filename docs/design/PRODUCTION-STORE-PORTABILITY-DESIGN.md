# `ProductionStore` File Portability Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-01 — the owner
  approved the design as proposed, no changes requested). Acceptance
  authorizes the design; implementation still requires its own unit
  (registering `STORAGE-014` and a planning packet) before any code is
  written — see ADR-0016's "Decision" section. See
  `ADR-0016-production-store-file-portability-proposal.md` for the
  decision record this document backs.
- Date: 2026-09-01
- Related: `docs/decisions/ADR-0006-tier-2-durability-architectures.md`
  (the original "ages-only" scope-down and its own revisit trigger this
  proposal responds to), `docs/design/GENERIC-SCHEMA-DESIGN.md` §4.2 (the
  same wall hit a second time, with `Order`), ADR-0008
  (`ProductionStore`), `RESULTS.md`'s `## External database comparison`
  (the SQLite/DuckDB portability comparison that surfaced this gap)

## Purpose and scope

`ProductionStore`'s `.mmap` file persists only `age` — `id`/`breed` and
`littermate_of` edges are never written to disk. `ProductionStore::open`
requires the caller to already hold the full `Vec<DogRecord>`/
`Vec<(Uuid, Uuid)>` in memory; the file alone cannot reconstruct a
`ProductionStore`. This is a real, named gap against SQLite's/DuckDB's
own `.db` files, which are fully self-describing (`SqliteConn::open(path)`
needs nothing else — see `benches/external_db.rs`) — copying
`ProductionStore`'s directory to another machine and reopening it with no
companion in-memory dataset does not work today.

This proposal adds a second, purely additive way to open a
`ProductionStore`: from disk alone, no caller-supplied `records`/`edges`.
The existing `create`/`open(records, edges, path)` signatures are
unchanged and keep their exact current semantics (identity-keyed
reconciliation against a possibly-different caller-supplied dataset — the
capability `MMAP-AGE-STORE-IDENTITY-FIX` built).

**In scope for this proposal:**

- Persisting `id`/`breed`/`littermate_of` edges to disk, alongside the
  existing, unchanged `ages.mmap` file `MmapAgeStore` already owns.
- A new constructor, `ProductionStore::open_portable(path)`, that needs
  only a path — no caller-supplied dataset.
- Crash-safety for the new persisted data, reusing the exact
  write-to-temp-then-atomic-rename mechanism the columnar
  `MmapAgeStore` redesign (`MMAP-AGE-STORE-IDENTITY-FIX`) already
  validated, rather than inventing a new one.

**Explicitly out of scope, named directly:**

- Mutating `breed` or the record/edge set after `create()`. No such
  method exists anywhere in this crate today (only `update_age` mutates
  anything) — this proposal doesn't add one, and the persisted-blob
  design below depends on that being true (see "Data/state and
  invariants").
- Changing `MmapAgeStore`'s own on-disk format. It stays exactly as
  `MMAP-AGE-STORE-IDENTITY-FIX` left it — this proposal adds a second,
  separate file, not a modification to a closed round's own format.
- Full SQL-parity-style portability (arbitrary schema evolution, a
  version-migration story beyond the existing `SCHEMA_VERSION` check). Out
  of scope per `docs/FUTURE-GROWTH.md`'s existing "not full SQL parity"
  line, same as `STORAGE-013`.

## Non-goals

- Not a replacement for `create`/`open(records, edges, path)`. Both stay
  exactly as they are — this is a third constructor, not a signature
  change to the existing two. Every existing benchmark/test call site is
  unaffected; none needs to change.
- Not a generalization to `crate::generic`/`GenericMmapStore`. That
  module has the identical one-durable-field scope-down (see
  `GENERIC-SCHEMA-DESIGN.md` §4.2) and would need this same treatment,
  but porting it is a separate, later decision — see "Open questions."
- Not a fix for `MmapAgeStore`'s own file being unreadable by anything
  but this crate (no external tool reads `.mmap` files the way `sqlite3`
  reads a `.db` file) — "portable" here means "the directory alone is
  sufficient to reopen a `ProductionStore`," not "readable by other
  programs."

## Context and terminology

`ADR-0006`'s own revisit trigger named this directly: *"Revisit if: a
future record shape needs more than one mutable field persisted — at that
point mmap's and `redb`'s ages-only scope-down would need real redesign
(the string-heap/fixed-layout problem this ADR chose not to solve), not
just an incremental extension."* `GENERIC-SCHEMA-DESIGN.md` §4.2 hit that
exact wall with `Order` (which wants both `amount` and `status` mutable)
and confirmed the string-heap problem is real *for mutable, variable-length
fields specifically* — `status: OrderStatus` works because it's a
fixed-size enum, not because the problem was solved.

**The key reframing this proposal makes**: `breed` (and `id`, and
`littermate_of` edges) are never mutated in this crate — only `age` is.
ADR-0006's rejected string-heap design was solving a harder problem than
this proposal needs to: *mutable*, in-place, fixed-width-mmap-compatible
variable-length storage. Immutable data doesn't need mmap's in-place
mutation properties at all. `SnapshotFullStore`
(`src/durability/snapshot_full.rs`) already proves this crate can persist
full `DogRecord`s (including `breed: String`) via a plain `bincode`
serialize/deserialize round trip, no string-heap format required — it
just doesn't get `MmapAgeStore`'s zero-loss-window per-write durability,
because it re-serializes its *entire* state (including the mutable `age`
cache) on every checkpoint, not just what changed.

This proposal takes the piece of `SnapshotFullStore`'s approach that
actually fits the problem (bincode-serialize the *immutable* fields once)
and leaves `MmapAgeStore`'s per-write, zero-loss-window mutable-`age`
path completely untouched — getting both properties by splitting them
onto two files rather than trying to force one format to do both jobs.

## Requirements

- `PORTABILITY-FR-001`: `id`, `breed`, and `littermate_of` edges are
  persisted to disk at `create()` time, in a location derivable from the
  existing `path` argument (a sibling file, not a second caller-supplied
  path — see "Architecture and interfaces").
- `PORTABILITY-FR-002`: `ProductionStore::open_portable(path) ->
  Result<Self, DurabilityError>` reconstructs a fully working
  `ProductionStore` from `path` alone — no `records`/`edges` argument.
  Every `DogStore`/`ConcurrentStore` method behaves identically to a
  store built via `create`/`open` with the same logical dataset.
- `PORTABILITY-FR-003`: `MmapAgeStore`'s own file format and the existing
  `create`/`open(records, edges, path)` signatures are unchanged — zero
  modification to `src/durability/mmap_store.rs`'s on-disk layout or
  public API, verified by diff.
- `PORTABILITY-FR-004`: The new persisted data survives a crash mid-write
  at `create()` time with the same guarantee `MMAP-AGE-STORE-IDENTITY-FIX`
  already validated for `MmapAgeStore`'s own rewrite path
  (write-to-temp-then-atomic-rename) — `path` never observes a partial
  write.
- `PORTABILITY-FR-005`: `open_portable` distinguishes "file doesn't exist
  or isn't from this format" (a new, distinctly-named `DurabilityError`
  variant, not conflated with `MmapAgeStore`'s existing `InvalidMagic`)
  from "file exists but ages are stale relative to it," matching this
  crate's existing "no unwrap/expect outside tests, no silent wrong
  answers" discipline.

## Architecture and interfaces

### Considered options

**Where the immutable data lives: extend `MmapAgeStore`'s own file
format vs. a separate companion file.**

1. *Extend `MmapAgeStore`'s columnar format with a string-heap region for
   `breed` and an edge-list region, in the same file.* Rejected — this is
   exactly the redesign ADR-0006 declined once and `GENERIC-SCHEMA-DESIGN.md`
   §4.2 confirmed is real, non-incremental work, *and* it's solving a
   problem this data doesn't have: `breed`/edges never mutate, so they
   never need mmap's in-place-fixed-width property that string-heap
   offsets exist to preserve *under mutation*. Also would touch a closed
   round's own file format (`MMAP-AGE-STORE-IDENTITY-FIX`), which this
   crate's convention (see that round's own precedent for touching
   `GenericMmapStore` only when a real bug was found) treats as a real
   cost, not a free edit.
2. *A separate, sibling file (e.g. `<path>` for ages, a derived
   `<path>.records` for the immutable blob), bincode-serialized
   `(Vec<DogRecord>, Vec<(Uuid, Uuid)>)`, written once at `create()`.*
   Chosen — reuses `DogRecord`'s existing `Serialize`/`Deserialize`
   derive (already present, from `STORAGE-008`/`STORAGE-009`) and the
   already-justified `bincode` dependency; zero changes to
   `MmapAgeStore`'s own format; the "immutable, write-once" property
   means there's no in-place-mutation problem to solve at all — a plain
   `std::fs::write`/`std::fs::read` (via `write_via_rename` for
   crash-safety) is sufficient, the same complexity class
   `SnapshotFullStore`'s own `checkpoint`/`open` already uses.

**Whether to cap `breed`'s length instead (avoiding a second file
entirely).**

1. *Cap `breed` to a fixed maximum length, pad/truncate, map it directly
   into `MmapAgeStore`'s existing fixed-width region.* Rejected — an
   unapproved record-shape change (`DogRecord` is not this proposal's to
   redefine), and doesn't solve edges at all (`littermate_of` is a
   variable-length list per record, the identical problem one level up).
2. *Companion file, no length cap, `DogRecord` unchanged.* Chosen — see
   above.

**Whether to replace `open`'s signature vs. add a new constructor.**

1. *Change `open(records, edges, path)` to `open(path)`, dropping the
   dataset arguments.* Rejected for this proposal — breaking, and would
   silently drop the identity-keyed-reconciliation-against-a-possibly-
   different-dataset capability `MMAP-AGE-STORE-IDENTITY-FIX` exists
   specifically to provide (e.g. a caller that regenerates its dataset
   from an external source of truth and wants the file's ages
   reconciled against it, not just replayed as-is). Every existing
   benchmark/test call site would also need updating for a capability
   they don't need.
2. *Add `open_portable(path)`, additive, alongside the unchanged
   `create`/`open`.* Chosen — matches this project's own repeated
   "additive, not a rewrite" decision driver (ADR-0006, ADR-0010).
   Existing callers are completely unaffected.

**Whether `open`'s existing reconciliation path should also refresh the
companion blob when it detects a changed record set.**

1. *No — the blob only ever reflects `create()`-time state.* Rejected —
   would silently make `open_portable` return stale `breed`/edge data
   after a legitimate `open(records, edges, path)` call with a
   genuinely different dataset (the exact scenario
   `MMAP-AGE-STORE-IDENTITY-FIX` was built to handle correctly), a real
   correctness gap for a feature whose whole point is being trustworthy
   without the original caller-supplied dataset in hand.
2. *Yes — `open(records, edges, path)` rewrites the companion blob
   whenever it detects the record set changed (the same condition that
   already triggers `MmapAgeStore`'s own file rewrite).* Chosen — this
   is the one place `PORTABILITY-FR-001`'s "persisted at `create()` time"
   requirement needs a small addition: also re-persisted whenever
   `open`'s existing reconciliation logic would otherwise leave the
   portable blob stale. No new write path — piggybacks on the rewrite
   `open` already performs in that case.

### Proposed shape

```rust
// src/durability/mmap_store.rs — MmapAgeStore's own file format: UNCHANGED.

// New, in a new module (name TBD at implementation time, e.g.
// src/durability/record_blob.rs), reusing the write-to-temp-then-rename
// pattern MmapAgeStore's own `write_via_rename` already established:
struct RecordBlob {
    records: Vec<DogRecord>,
    edges: Vec<(Uuid, Uuid)>,
}

impl RecordBlob {
    fn write(&self, path: &Path) -> Result<(), DurabilityError> {
        // bincode::serialize + write-to-temp-then-rename, mirroring
        // MmapAgeStore::write_via_rename exactly.
    }

    fn read(path: &Path) -> Result<Self, DurabilityError> {
        // bincode::deserialize; a missing/corrupt file is a new,
        // distinctly-named DurabilityError variant (PORTABILITY-FR-005).
    }
}

// src/production.rs
impl ProductionStore {
    // Unchanged.
    pub fn create(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>, path: &Path)
        -> Result<Self, DurabilityError> { /* ...; also RecordBlob::write(companion_path(path)) */ }

    // Unchanged.
    pub fn open(records: Vec<DogRecord>, edges: Vec<(Uuid, Uuid)>, path: &Path)
        -> Result<Self, DurabilityError> { /* ...; RecordBlob::write(...) only when MmapAgeStore's own reconciliation detects a changed record set */ }

    // New.
    pub fn open_portable(path: &Path) -> Result<Self, DurabilityError> {
        let RecordBlob { records, edges } = RecordBlob::read(&companion_path(path))?;
        Self::open(records, edges, path)
    }
}
```

`open_portable` is implemented entirely in terms of the existing `open`
— it just sources `records`/`edges` from the companion blob instead of
from a caller. This means `open_portable` gets `open`'s existing
identity-keyed age-reconciliation logic for free, with no duplicated
code path to keep in sync.

`companion_path(path)` derives the sibling file's name from `path` (e.g.
appending `.records`) — a fixed, documented convention, not a second
caller-supplied path, so a `ProductionStore` directory is portable as a
unit (copy both files together) without the caller needing to track two
paths.

## Data/state and invariants

- The companion blob is genuinely immutable after `create()` except when
  `open`'s existing reconciliation detects a changed record set (see
  "Considered options" above) — this holds *only* because no method in
  this crate mutates `breed`, `id`, or the edge set. If a future round
  ever adds such a mutation, this design would need revisiting (see
  ADR-0016's revisit triggers) — named here explicitly, not assumed to
  keep holding forever.
- Two files must travel together for `open_portable` to work: the
  existing `ages.mmap` and the new companion blob. Copying only one is a
  user error this design surfaces as a clear, typed error
  (`PORTABILITY-FR-005`), not a silent wrong answer or a panic.
- No change to `MmapAgeStore`'s own crash-safety story — the mutable
  `age` path keeps its existing zero-loss-window, per-write durability
  guarantee (`ADR-0008`'s own headline finding) completely unchanged.

## Errors, failure, recovery, and observability

- A missing or corrupt companion blob at `open_portable` time is a new
  `DurabilityError` variant, distinct from `MmapAgeStore`'s existing
  `InvalidMagic`/`SchemaVersionMismatch` (which describe the *ages* file,
  not this one) — same "fail loud, typed, not silent" standard this
  crate holds itself to throughout `src/durability/**`.
- A crash mid-write to the companion blob at `create()`/reconciling-`open()`
  time can never leave a partial file at the real path — same
  write-to-temp-then-atomic-rename guarantee `MmapAgeStore`'s own
  columnar rewrite path already provides, reused directly rather than
  re-derived.
- Not covered by this proposal: what happens if the companion blob and
  the `ages.mmap` file disagree about which ids exist (e.g. the blob was
  copied from a different `create()` than the ages file it's paired
  with) — `open`'s existing reconciliation-by-id logic handles this the
  same way it already handles any other caller-supplied/file mismatch
  (ids in the blob but not the file get their `create()`-time starting
  age; ids in the file but not the blob are silently dropped, matching
  `MMAP-AGE-STORE-IDENTITY-FIX`'s own documented "a record missing from
  reopen is invisible, not erroring" behavior) — no new failure mode,
  just the existing one reached via a new entry point.

## Security, privacy, and compatibility

Not applicable beyond what already applies to `MmapAgeStore`'s own file —
synthetic, locally-generated data; no network exposure; no change to this
crate's trust model (a `ProductionStore` directory is exactly as trusted
as the process that created it, same as today).

## Acceptance criteria

- `ProductionStore::create(records, edges, path)` followed by
  `ProductionStore::open_portable(path)` (no `records`/`edges` supplied
  to the portable call) returns a store whose `get`/`scan_ages`/
  `same_breed`/`neighbors` results are identical to the original,
  including `breed` values — the property `MmapAgeStore` alone cannot
  provide today.
- Copying both files (the `.mmap` file and its companion blob) to a
  fresh directory and calling `open_portable` there succeeds — the real
  "move the directory to another machine" property this proposal exists
  to add, verified directly, not just inferred from the round-trip test
  above.
- `open_portable` against a directory missing the companion blob (or
  with only the legacy `.mmap` file from before this feature existed)
  fails with the new, distinctly-named error — never a panic, never a
  silently-empty store.
- `MmapAgeStore`'s own existing test suite (all 11 tests) passes
  unchanged — zero regression to the closed round this proposal builds
  next to, not into.
- No change to any existing `ProductionStore::create`/`open` call site
  anywhere in this crate — verified by diff.

## Verification plan

- Real, compiled, tested code once accepted — matching this crate's own
  precedent for storage-layer decisions (unlike `SERVER-QUERY-LAYER-DESIGN`'s
  standalone-scratch-probe verification, appropriate for a new
  public-facing wire protocol; this proposal extends an existing,
  already-well-tested module in the same crate, so a compiled scratch
  probe adds little the real implementation's own test suite won't
  already cover more directly).
- New unit tests in whatever module holds `RecordBlob` (round-trip,
  crash-mid-write-via-fault-injection-if-the-existing pattern from
  `MMAP-AGE-STORE-IDENTITY-FIX` transfers directly, missing/corrupt-file
  error path) plus `src/production.rs` tests covering
  `open_portable`'s full round trip and the "directory copied to a fresh
  location" scenario named in Acceptance criteria.
- A real before/after benchmark comparison of `create()`'s own cost (now
  writes two files, not one) — `create()` is never in any existing
  benchmark's timed `b.iter()` closure (matching every other durability
  variant's own convention), so this is a one-time, informational
  measurement, not expected to move any existing `RESULTS.md` table.

## Traceability

A new spec (next available: `STORAGE-014`) would be registered once this
design is accepted, per `SERVER-QUERY-LAYER-DESIGN.md`'s own precedent
(no spec for the design document itself; a spec is registered as a
separate step once a design is accepted, right before real implementation
begins).

## Open questions

- Whether `crate::generic`/`GenericMmapStore` needs the identical
  treatment is a separate, later decision — it has the same
  one-durable-field limitation (`GENERIC-SCHEMA-DESIGN.md` §4.2) but a
  different, generic-over-`Record` API shape this proposal doesn't
  attempt to generalize into.
- The companion blob's own naming/location convention
  (`companion_path(path)`) is sketched above but not finalized — the
  real implementation should pick a convention that reads clearly
  alongside the existing `.mmap` file, not necessarily literally
  `<path>.records`.
- Whether `open_portable`'s reconciliation-triggered blob rewrite (see
  "Considered options") adds a meaningful cost to `open`'s existing path
  in the common case (record set unchanged, no rewrite needed) is
  unmeasured — expected to be zero, since the rewrite only fires when
  `MmapAgeStore`'s own logic already decided a rewrite was needed for
  the ages file, but not confirmed until implemented.
- This design does not address `GenericMmapStore`'s or any Tier 1/2
  durability variant's own portability — scoped to `ProductionStore`/
  `MmapAgeStore` specifically, the crate's own recommended production
  pick, matching where the owner's original portability question was
  asked.

## Implementation status

Implemented 2026-09-02 as `PRODUCTION-STORE-PORTABILITY` (`STORAGE-014`
v0.1.0; `ADR-0016` "Acceptance and implementation"). Resolutions of the
open questions above, in order:

- `GenericMmapStore`: still a separate, later decision — unchanged.
- Naming convention: finalized as literally `<path>.records`
  (`src/durability/record_blob.rs::companion_path`), with
  `<companion>.rewrite-tmp` as the crash-safe temp file. The two files
  sort adjacently, which turned out to read clearly enough on its own.
- The reconciliation-triggered rewrite cost: **measured, and not zero.**
  `MmapAgeStore`'s own rewrite decision is private to it, and this round
  deliberately left `mmap_store.rs` untouched, so `open` cannot learn
  whether the ages file was rewritten. It instead serializes the record
  set and byte-compares it against the on-disk blob on every call,
  rewriting only on a mismatch (which also heals a pre-`STORAGE-014`
  directory holding only the ages file). Release build, median of 7
  samples (3 at 1M): `open` +27%/+30%/+27% at 1K/100K/1M records, `create`
  +15%/+68%/+78%; `open_portable` between the old and new `open`. Full
  table and verdict in `RESULTS.md`'s `### ProductionStore file
  portability (STORAGE-014)` subsection. Named follow-up, not built at
  the time: a content hash in the blob header so `open` compares a
  fingerprint instead of a full serialization (a `BLOB_VERSION` bump).
  **Built as `STORAGE-014` v0.2.0 (blob version 2)**: a `u64` FNV-1a
  fingerprint of the immutable content follows the version in a 20-byte
  header; `open` reads the header and compares fingerprints, serializing
  only when stale; version-1 blobs are upgraded in place. Re-measured:
  `open` +0.3-4% at 1M (was +27%). Details in the spec's v0.2.0 change
  entry and `RESULTS.md`'s `#### Follow-up: header fingerprint`
  subsection.
- Tier 1/2 durability variants: still out of scope — unchanged.

## Change history

- 2026-09-02: "Implementation status" updated — the header-fingerprint
  follow-up is built (`STORAGE-014` v0.2.0, blob version 2); `open`'s
  steady-state cost is no longer a full serialize-and-compare.
- 2026-09-02: "Implementation status" addendum — implemented, open
  questions resolved or explicitly carried, measured `open` cost recorded.
- 2026-09-01: Initial proposal, in response to the owner asking to
  pursue `ProductionStore` file portability from `RESULTS.md`'s/
  `PROJECT-STATUS.md`'s own standing open question.
