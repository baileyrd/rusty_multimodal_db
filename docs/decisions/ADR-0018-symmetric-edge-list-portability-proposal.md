# ADR-0018: Persist `Symmetric`'s edge list in its own companion blob — make `Employee`'s durable stack reopenable from its path alone

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner approved the design as proposed, no changes requested)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (the full
  design document this ADR summarizes),
  `docs/decisions/ADR-0017-generic-store-file-portability-proposal.md`
  and `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (the
  `GenericMmapStore` treatment this proposal completes, implemented as
  `STORAGE-015` v0.1.0 — its "Open questions" names this decision
  first), `docs/decisions/ADR-0016-production-store-file-portability-proposal.md`
  (where `ProductionStore`'s own edges were persisted inside
  `RecordBlob`), ADR-0009 (the generic schema library and its layering
  rule), `STORAGE-014` v0.2.0 (the header, hash, and write path reused a
  third time here), `STORAGE-015` (the generic record blob this design
  sits beside)
- Supersedes/Superseded by: none. Extends, does not reverse, ADR-0009's
  layering and ADR-0017's design — `GenericMmapStore`'s `.mmap` file,
  its `.records` blob, and `Symmetric::new` are all unchanged by this
  proposal (see "Decision" below).

## Context

`ADR-0017` closed the generic half of the file-portability gap and named
the piece it left open, in its decision and again in its consequences:
*"This design does not persist a `Symmetric` layer's edge list ... A
`Symmetric`-level companion is a separate, later decision"* and
*"`Employee`'s durable stack is not fully path-portable under this
proposal — a named, deliberately out-of-scope limitation."* Its revisit
trigger — *"`Symmetric`'s edge list needs persisting for a promoted
(non-spike) domain"* — has not literally fired (`Employee` is still
research-gated), but the owner queued this as the first of four
follow-ups after `GENERIC-STORE-FINGERPRINT-MEASUREMENT` ("1, 2, 3, then
4"), and it is the first open question `GENERIC-STORE-PORTABILITY-DESIGN`
lists.

The gap is specific and small. `Reversed` derives everything it indexes
from the records (`ChildOf::parent_id`), so `OrderProductionStack` became
fully portable under `STORAGE-015` with no relation-layer work.
`Symmetric<S, R, Marker>` (`src/generic/store.rs`) is the one relation
layer whose state is *not* derivable from records: `Symmetric::new(inner,
edges)` takes an external `&[(R::Id, R::Id)]`, builds a `HashMap<R::Id,
Vec<R::Id>>` adjacency, and holds it in memory only. `EmployeeProductionStack`
(`Reversed` over `Symmetric` over `GenericMmapStore`) therefore still
needs the caller to hold the collaboration edge list in both of its
durable helpers, and `STORAGE-015` gave `Employee` no portable helper at
all rather than one that was "portable except for the edges."

`STORAGE-015` also said why the edges were not simply added to
`GenericMmapStore`'s blob: `ADR-0009`'s layering puts state in the layer
that indexes it. That rule is what makes this a design decision rather
than a two-line field addition — the answer has to be a file the
`Symmetric` layer owns, and `Symmetric` has no path of its own to put it
at. A new on-disk format and a new public constructor triple on a
library type are the kind of consequential decision this project's
convention treats as ADR-worthy. This ADR proposes a design and
authorizes no implementation, the posture `ADR-0016` and `ADR-0017` both
took.

## Decision drivers

- **The layer that indexes the edges persists them.** `ADR-0009`'s
  layering, applied consistently: the core store's blob carries records
  only (`STORAGE-015`), and a relation layer with state of its own
  carries that state in a file of its own.
- **No new mechanism.** The 20-byte magic/version/FNV-1a header,
  `Fnv1a64`'s streamed `io::Write` fingerprint, `encode_image`/
  `parse_header`, the temp-then-`sync_all`-then-rename write, and
  `DurabilityError::RecordBlobUnreadable` are all `pub(crate)` and tested
  at two call sites already. This is their third, and it adds nothing to
  them.
- **Additive everywhere.** `Symmetric::new` and every existing impl keep
  their bounds and behavior; the new functions live in a separately
  bounded `impl` block; `GenericMmapStore`, `.mmap`, `.records`,
  `Reversed`, and the in-memory `Symmetric` stacks (`dog_impl.rs`,
  `build_employee_generic_store`) are untouched.
- **A stack may hold more than one `Symmetric`.** Two symmetric relations
  over one record type are two layers with two edge lists; the design
  must not bake in a single file name that would make them collide.
- **Design-first, then owner acceptance, then a spec and code** — the
  `ADR-0016` → `STORAGE-014` and `ADR-0017` → `STORAGE-015` precedent,
  followed exactly.

## Considered options

See `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md`'s "Considered
options" section for the full reasoning. Summarized:

1. **Where the edge list lives.** Inside `GenericMmapStore`'s `.records`
   blob as a second field — rejected: the core store would carry a
   relation layer's data against `ADR-0009`, `GENBLOB\0`'s version would
   bump for every store that has no `Symmetric`, and two symmetric
   relations over one store could not be expressed. A stack-level blob
   written by each domain helper (the `RecordBlob` shape) — rejected: it
   duplicates the records already in `.records` and moves persistence
   out of the library into every domain. **Chosen: a `Symmetric`-level
   companion blob at a caller-supplied path** — magic `GENEDGE\0`,
   `BLOB_VERSION = 1`, the shared 20-byte header, body a `bincode`
   `Vec<(R::Id, R::Id)>`.
2. **Edge list or adjacency map.** Persisting the `HashMap<R::Id,
   Vec<R::Id>>` — rejected: twice the size (every edge under both
   endpoints) and nondeterministic iteration order, so two `create`s of
   the same edges would produce different bytes and fingerprints.
   **Chosen: the edge list as given, order preserved and part of the
   fingerprint**; the unchanged `new` rebuilds the map from it exactly
   as it does from a caller's slice today. A reordered list is a changed
   list, deliberately — order is observable through `neighbors`.
3. **How the path reaches `Symmetric`.** A fixed convention derived
   inside `Symmetric` — impossible (no path to derive from) and it would
   collide for two layers. A `Path`-carrying trait on `S` — rejected: a
   new trait every store type must implement, for a value the domain
   helper already holds. **Chosen: a caller-supplied `edges_path`
   argument**, with the single-relation convention `<path>.edges` in a
   small crate-internal helper (`edges_path(path)`) so every domain
   helper spells it the same way.
4. **Fold the blob into `new` or add a parallel triple.** Changing `new`
   to take `Option<&Path>` — rejected: the in-memory stacks would pass
   `None` forever and every call site changes for no behavior change.
   **Chosen: a parallel `create`/`open`/`open_portable` triple (plus the
   public `read_portable_edges` step) in a separately bounded `impl`
   block, `new` untouched.** This is the parallel-constructor shape
   `ADR-0017` rejected for `GenericMmapStore`, and the reason it is right
   here is the reason it was wrong there: `GenericMmapStore::open`
   already had a path, so refreshing the blob on every `open` cost the
   caller nothing; `Symmetric::new` has no path, so there is nothing for
   it to refresh. The residual gap — a durable stack assembled with `new`
   writes no blob — is closed by convention in the domain helpers and
   named as such.
5. **Which error variant.** A new `DurabilityError::EdgeBlobUnreadable`
   — rejected: a new public enum variant (breaking for exhaustive
   matches) for failure modes identical to the record blob's, already
   distinguished by the `path` field. **Chosen: reuse
   `RecordBlobUnreadable { path, cause }`** with `path` naming the edge
   blob; the variant's docs gain one sentence.

## Decision

- `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` records the full
  proposed design: a companion edge blob (`GENEDGE\0`, version 1, the
  `STORAGE-014` v0.2.0 20-byte header via the shared helpers, body a
  `bincode` `Vec<(R::Id, R::Id)>` in the order given) at a caller-supplied
  `edges_path`; a crate-internal blob module beside `record_blob.rs`
  with `edges_path(path) -> <path>.edges` as the single-relation
  convention; and four new additive associated functions on
  `Symmetric` in a separately bounded `impl` block (`R::Id: Serialize +
  DeserializeOwned`, this block only): `create(inner, edges, edges_path)`
  (always writes, then `new`), `open(inner, edges, edges_path)` (header
  read, fingerprint compare, rewrite only when stale, missing, or
  unreadable, then `new(inner, edges)` — the adjacency is always built
  from the caller's edges on this path), `read_portable_edges(edges_path)
  -> Vec<(R::Id, R::Id)>` (blob only, never writes, persisted order), and
  `open_portable(inner, edges_path)` (exactly `Ok(Self::new(inner,
  &Self::read_portable_edges(edges_path)?))`).
- `Symmetric::new`, the `Neighbors` impl, and every forwarding impl are
  unchanged in bounds and behavior. `GenericMmapStore`, its `.mmap` file,
  its `.records` blob (`GENBLOB\0` stays version 1; `STORAGE-015` stays
  v0.1.0), `Reversed`, `dog_impl.rs`, and `build_employee_generic_store`
  are unchanged.
- `create_employee_production_stack` and `open_employee_production_stack`
  keep their signatures and switch from `Symmetric::new` to
  `Symmetric::create`/`Symmetric::open` with `edges_path(path)`; one new
  helper, `open_employee_production_stack_portable(path)`, builds the
  whole `EmployeeProductionStack` from `<path>`, `<path>.records`, and
  `<path>.edges`, reading the record blob once (for the core store and
  for `Reversed`, as `open_order_production_stack_portable` does) and the
  edge blob once.
- No new dependency and no new error variant: `serde`/`bincode` and
  `DurabilityError::RecordBlobUnreadable` are reused; the variant's docs
  gain one sentence naming the edge blob.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No source file is modified by this ADR itself. Per the
  `ADR-0017` → `STORAGE-015` precedent, the next unit registers a new
  spec (`STORAGE-016`) and a real implementation packet before any code
  changes.
- Not decided here, deferred deliberately: a schema tag recording which
  relation a blob holds (the same open question `STORAGE-015` deferred —
  to be resolved together, and queued by the owner as follow-up 2 of the
  same four — since accepted and implemented as
  `BLOB-SCHEMA-TAG-DESIGN.md` / `ADR-0019`, `STORAGE-016` v0.2.0); a
  stack-level manifest naming a stack's files.

## Consequences

### Positive

- Closes the one gap `ADR-0017` named as its own deliberate limitation,
  on the mechanism `STORAGE-014` and `STORAGE-015` already validated and
  measured — the third use of the same header, hash, and write path,
  with no new mechanism to get wrong.
- `EmployeeProductionStack` — the only durable stack in this crate that
  uses `Symmetric` — becomes reopenable from a path alone, `neighbors`
  results identical to the original's including order.
- Zero change to `GenericMmapStore`'s hardened slot format, to
  `STORAGE-015`'s blob, to `Symmetric::new`, or to any existing call
  site: the `store.rs` diff is one added `impl` block; every existing
  test suite passes unmodified.
- `ADR-0009`'s layering survives intact and is now applied to
  persistence too: records in the core store's file, each relation
  layer's own state in its own file, and the domain helper — not the
  library — deciding names.
- Steady-state `open` cost is a 20-byte header read plus a streamed
  serialization of a slice of 32-byte pairs — smaller than the record
  fingerprint `STORAGE-015` measured at ~4% of `open`.

### Negative / tradeoffs

- **A by-convention gap, not a type-system one.** A durable stack
  assembled with `Symmetric::new` writes no edge blob and refreshes
  none; the domain helpers are the guard. `ADR-0017` closed the
  analogous gap by tightening `open`'s bounds because `open` had a path;
  `new` has none, so the same fix is not available here. Named in the
  design's "Non-goals"; the third open question there records the
  eventual type-level closure (an in-memory variant of the triple
  replacing `new`) as not proposed.
- **Three files must travel together for the `Employee` stack** (`<path>`,
  `<path>.records`, `<path>.edges`), and nothing on disk records that
  fact. A missing `.edges` is a typed `RecordBlobUnreadable` naming it,
  never a stack with silently empty adjacency, and the existing
  `open_employee_production_stack(employees, edges, path)` heals it.
- **The blob does not record `R`, `Marker`, or `Id`.** Reading one
  symmetric relation's blob as another's is a caller error surfaced only
  when the encodings differ — `STORAGE-015`'s trust model, and the same
  schema-tag question, deferred here to be resolved together.
- **`RecordBlobUnreadable` now names two kinds of file.** A caller
  distinguishes them by `path`; the variant's name is slightly less
  literal than it was. Accepted over a breaking new variant.
- **Multi-writer blob semantics are last-writer-wins**, as the record
  blob's are; each blob is atomic by rename and self-consistent.
- The immutability assumption (nothing adds or removes edges through the
  layer after construction; `Symmetric` has no mutating method) is
  load-bearing, as in `ADR-0016`/`ADR-0017` — named as a revisit trigger
  below.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching `ADR-0017` —
  it applies an accepted, implemented, measured mechanism one layer up,
  using only already-validated `pub(crate)` pieces, so the
  implementation's own test suite is the direct verification, per the
  design document's "Verification plan."
- **Real validation, post-acceptance**: a new spec (`STORAGE-016`); the
  edge blob module's unit tests (round trip through `Uuid` pairs; missing/
  short/wrong-magic/wrong-version/fingerprint-mismatch/truncated as typed
  errors; a `GENBLOB\0` file is a magic error; order preserved);
  `store.rs` tests for the triple over an in-memory inner store (round
  trip with order, rewrite-only-when-changed, reorder-counts-as-changed,
  missing blob healed by `open`); research-gated `employee_impl.rs` tests
  for each acceptance criterion in the design document; every existing
  `store.rs`, `mmap_store.rs`, `record_blob.rs`, `tests/
  mmap_record_identity_keying.rs` test passing unmodified. A cost
  measurement is not planned unless the implementation shows a reason
  (an edge is 32 bytes to a record's ~76; no published Criterion group
  times the `Employee` stack's constructors); if taken, it goes in
  `RESULTS.md` beside the `STORAGE-015` figures.
- Revisit if: a future round adds a way to add or remove edges through
  `Symmetric` after construction — the blob's immutability assumption
  would need real rework, and `open`'s "always build from the caller's
  edges" rule would need a merge story.
- Revisit if: a second `Symmetric` layer appears in one durable stack —
  the caller-supplied path is designed for it, but the `<path>.edges`
  convention helper would need a second name, and the manifest open
  question becomes more pressing.
- Revisit if: the schema-tag question is resolved for `STORAGE-015` — the
  same `BLOB_VERSION` bump applies here, at the same time. *Tripped:
  `ADR-0019` proposes exactly that, both blobs to version 2 in one unit;
  accepted as proposed and implemented as `STORAGE-016` v0.2.0.*
- Revisit if: a caller is found assembling a durable stack with
  `Symmetric::new` and relying on portability — the by-convention gap
  would then be real, and the design's third open question (retire `new`
  for an in-memory variant of the triple) becomes the next step.

## Acceptance and implementation

- 2026-09-02: accepted as proposed. The next unit registers `STORAGE-016`
  and implements per `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md`.
- 2026-09-02: implemented as `STORAGE-016` v0.1.0 in this PR
  (`src/generic/edge_blob.rs`, one impl block on `Symmetric` in
  `src/generic/store.rs`, the `Employee` helper switch in
  `src/generic_spike/employee_impl.rs`). One deviation from the design's
  sketch, recorded in the spec's Traceability: `EdgeBlob::fingerprint`
  returns `Result` rather than `u64`, because hashing encodes each `Id`
  through `bincode` and that encode can fail.
