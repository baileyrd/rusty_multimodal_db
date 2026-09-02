# ADR-0017: Add file portability to `GenericMmapStore` — persist the full record set alongside the existing one-field mmap file

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner approved the design as proposed, no changes requested)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` (the full
  design document this ADR summarizes),
  `docs/decisions/ADR-0016-production-store-file-portability-proposal.md`
  and `docs/design/PRODUCTION-STORE-PORTABILITY-DESIGN.md` (the
  `ProductionStore` treatment this proposal ports, implemented as
  `STORAGE-014` v0.1.0/v0.2.0), `docs/design/GENERIC-SCHEMA-DESIGN.md`
  §4.2 (where the generic library first hit the one-durable-field wall),
  ADR-0006 (the original "ages-only" scope-down), ADR-0009 (the generic
  schema library)
- Supersedes/Superseded by: none. Extends, does not reverse, ADR-0009's
  layering and ADR-0016's design — `GenericMmapStore`'s own `.mmap` file
  format is unchanged by this proposal (see "Decision" below).

## Context

`ADR-0016` closed the file-portability gap for `ProductionStore` and named,
twice, the half it deliberately left open: *"This design does not extend
to `crate::generic`/`GenericMmapStore`, which has the identical limitation
(`GENERIC-SCHEMA-DESIGN.md` §4.2) but a different, generic-over-`Record`
API shape — a separate, later decision if pursued."* Its revisit trigger —
*"Revisit if: `crate::generic`/`GenericMmapStore` needs the identical
treatment"* — is what the owner queued as follow-up (b), `PROJECT-STATUS.md`
item 63, with "1 then 2" once `RECORD-BLOB-FINGERPRINT` (item 1) merged.

The gap is the same one: `GenericMmapStore<R, IndexMarker, ScanMarker>`
persists exactly one fixed-width `(R::Id, R::ScanValue)` slot per record.
For `Order`, `customer_id`/`status`/`created_at_unix_ms`/`discount_cents`
never reach disk; `create(records, path)` and `open(records, path)` both
require the caller to already hold the full `Vec<R>`, and the relation
layer above (`Reversed`, in `OrderProductionStack`) is rebuilt from that
same caller-supplied vector on every open. The file alone cannot
reconstruct a store.

The API shape is what makes this a separate decision rather than a
mechanical port. `RecordBlob` (`src/durability/record_blob.rs`) knows it
holds `DogRecord`s: it fingerprints `id`/`breed`/edges by hand and skips
`age`. A generic `R` gives a blob no view inside a record, and
`GenericMmapStore` owns no edge list — relations are layers stacked above
it (`ADR-0009`). Both differences force choices `ADR-0016` never had to
make, and a change to a public generic type's bounds plus a new on-disk
format are the kind of consequential, hard-to-reverse decision this
project's convention treats as ADR-worthy. This ADR proposes a design and
authorizes no implementation, the same posture `ADR-0016` took.

## Decision drivers

- **Additive to the file format, not a rewrite of it.** Five correctness
  rounds (`GENERIC-MMAP-RECORD-IDENTITY-FIX` through
  `GENERIC-MMAP-APPEND-SLOT-RACE-FIX`) closed `GenericMmapStore`'s
  `GMMAPST\0`/schema-version-2 slot format. This proposal adds a second
  file next to it, exactly as `STORAGE-014` did next to `MmapAgeStore`'s,
  and touches `mmap_store.rs` only in `create`/`open`'s bodies, their
  bounds, and two new functions.
- **Solve the write-once problem, not the mutable-N-fields one.**
  `GENERIC-SCHEMA-DESIGN.md` §4.2's "N mmap-backed fields" redesign is a
  different problem (mutable, fixed-width, in-place). Nothing in
  `crate::generic` mutates a record's non-`ScanMarker` fields after
  construction — `UpdateField::update` writes only the mmap-backed one —
  so, as in `ADR-0016`, the everything-else data is immutable and a plain
  serialize/deserialize round trip is the whole mechanism.
- **Reuse `STORAGE-014`'s validated pieces at their second call site.**
  The 20-byte magic/version/FNV-1a header, `companion_path`
  (`<path>.records`), the temp-then-`sync_all`-then-rename write, and
  `DurabilityError::RecordBlobUnreadable` all exist and are tested. A
  second real call site is this project's own threshold for sharing rather
  than duplicating.
- **Respect `ADR-0009`'s layering.** Relation state belongs to the layer
  that indexes it. The core store's blob carries records only; a
  `Symmetric` edge list is not smuggled into it.
- **Design-first, then owner acceptance, then a spec and code** — the
  `ADR-0016` → `STORAGE-014` precedent, followed exactly.

## Considered options

See `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md`'s "Considered
options" section for the full reasoning. Summarized:

1. **Where the record set lives.** Extending the `.mmap` slot to carry the
   whole record is impossible for variable-length fields without the
   string-heap format `ADR-0006` declined, and would reopen a closed
   format for data that never mutates in place. Rejected. **Chosen: a
   companion blob**, `bincode`-serialized `Vec<R>` at `<path>.records`,
   magic `GENBLOB\0`, `BLOB_VERSION = 1`, the `STORAGE-014` v0.2.0
   20-byte header layout.
2. **How to fingerprint a record the blob cannot see inside.** A new
   per-type trait method hand-walking immutable fields adds one more
   easy-to-get-wrong method to a library whose forwarding boilerplate is
   already its named ergonomic cost — rejected for now, **named as the
   fallback** (later measured at the owner's request and closed as not
   warranted: −7 ms per million records; see "Acceptance and
   implementation"). `std::hash::Hash` through FNV is unstable across Rust
   versions for an on-disk value — rejected. **Chosen: FNV-1a over the
   streamed `bincode` encoding** (an `io::Write` impl on the existing
   `Fnv1a64`): no allocation, no per-type code, one bound the blob needs
   anyway. It includes the mmap-backed field, so a reopen with regenerated
   scan values rewrites the blob the `Dog` design would skip — a cost, not
   a correctness issue (the `.mmap` file stays that field's truth), and no
   in-crate caller does it.
3. **Tighten `create`/`open`'s bounds vs. a parallel constructor pair.** A
   separate `create_portable`/`open_portable` pair leaving `create`/`open`
   bound-free would let a plain `open` leave an existing blob silently
   stale — the exact gap `ADR-0016` rejected. **Chosen: add `Serialize +
   DeserializeOwned` to the existing bounds**; `create` always writes,
   `open` always checks. This is the one breaking change, named: every
   record type used with `GenericMmapStore` must be serializable. Every
   such type in this crate already is or is one derive line away;
   `publish = false` means there are no others.
4. **What `open_portable` returns when the stack above needs records.**
   Returning only `Self` and pulling records back out of the store gives a
   `HashMap`'s nondeterministic order, which would make `Reversed`'s
   per-parent child order vary run to run — rejected. **Chosen:
   `read_portable_records(path) -> Vec<R>` as a public step** (the blob
   preserves persisted order), plus `open_portable(path) -> Self` built on
   it; the domain helper reads once and reuses the existing
   `open_order_production_stack(records, path)`.
5. **Where the shared machinery lives.** `Fnv1a64`, `companion_path`, the
   header encode/parse, and the rename-write are private to
   `record_blob.rs` today. **Chosen: they become `pub(crate)`** (or move to
   a small crate-internal module — an implementation-time placement
   detail), parameterized by magic. `RecordBlob`'s tests and behavior are
   unchanged; a visibility change to a closed module, named as such.

## Decision

- `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md` records the full
  proposed design: a companion `<path>.records` blob holding
  `bincode`-serialized `Vec<R>` behind a `GENBLOB\0`/version/FNV-1a
  header, written at `create()` and refreshed by `open(records, path)`
  only when the fingerprint says the record set changed; two new additive
  associated functions, `GenericMmapStore::read_portable_records(path)`
  and `GenericMmapStore::open_portable(path)` (the latter implemented as
  `open(read_portable_records(path)?, path)`); and one new domain helper,
  `open_order_production_stack_portable(path)` in `order_customer.rs`.
- `create`/`open`'s signatures are unchanged; their bounds gain
  `R: Serialize + DeserializeOwned` (and `R::Id` likewise). In-crate record
  types (`Order`, `Employee`, the doctest `Widget`, the harness binaries'
  records) gain a derive line each. `GenericMmapStore`'s own `.mmap`
  format and `GenericProductionStore` are unchanged.
- No new dependency and no new error variant: `serde`/`bincode` (already
  present) and `DurabilityError::RecordBlobUnreadable` (already present)
  are reused.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No source file is modified by this ADR itself. Per the
  `ADR-0016` → `STORAGE-014` precedent, the next unit registers a new spec
  (`STORAGE-015`) and a real implementation packet before any code
  changes.
- This design does not persist a `Symmetric` layer's edge list. `Reversed`
  — the only relation layer the promoted `Order`/`Customer` domain uses —
  derives everything it needs from the records. `Employee`'s durable stack
  (research-gated spike material) uses `Symmetric` with an external edge
  list; a portable helper for it, if added, keeps the edge list as its one
  remaining argument. A `Symmetric`-level companion is a separate, later
  decision (see the design document's "Open questions").

## Consequences

### Positive

- Closes the generic half of the gap `ADR-0016` named as its own
  deliberate limitation, on the design the owner already accepted and
  that `STORAGE-014` already measured — materially lower risk than a new
  mechanism.
- `OrderProductionStack` — the promoted reference domain — becomes
  reopenable from a path alone, including every field the `.mmap` file
  could never supply, with `Reversed`'s parent/child results identical to
  the original's.
- Zero risk to `GenericMmapStore`'s five-round-hardened slot format, its
  `COMMITTED`-marker crash safety, or its `O_APPEND` multi-process story —
  none of them is touched.
- Steady-state `open` cost is a 20-byte header read plus a streamed
  serialization, not a full serialize-and-byte-compare: the v0.2.0 shape
  from the start, never the v0.1.0 one.

### Negative / tradeoffs

- **A breaking bound tightening on a public generic type.** Every `R`
  used with `GenericMmapStore` must implement `Serialize +
  DeserializeOwned`. Justified by `publish = false` and by every in-crate
  type being one derive line away — but it is a real API change, not
  additive, and it is the one this proposal chooses over a silently-stale
  parallel constructor.
- **The fingerprint includes the mmap-backed field.** A caller that
  reopens with regenerated scan values pays a spurious blob rewrite. No
  in-crate caller does; named so it can be measured if one ever does. The
  trait-method fingerprint was the fallback for this and for the
  fingerprint's CPU cost; the CPU-cost case is measured and closed
  below, and a whole-record trait walk could not exclude the mmap-backed
  field marker-independently anyway (`Order` has three scannable
  fields; which one a store maps is the store's `ScanMarker`, not the
  record's to know).
- **Two files must travel together for `open_portable`** — a typed
  `RecordBlobUnreadable`, never a partial store, and plain
  `open(records, path)` still works and heals the missing blob. Same
  tradeoff `ADR-0016` accepted.
- **`Symmetric` edges are not covered.** `Employee`'s durable stack is not
  fully path-portable under this proposal — a named, deliberately
  out-of-scope limitation, not an oversight.
- **Multi-writer blob semantics are last-writer-wins**, inheriting exactly
  the scope `GenericMmapStore`'s own docs give its multi-process guarantee
  (slot creation only). Each blob is self-consistent; a later
  `open_portable` sees one process's view.
- The immutability assumption (nothing mutates a persisted field through
  the store) is load-bearing, as in `ADR-0016` — named as a revisit
  trigger below.

## Validation and revisit triggers

- **This proposal's own validation**: design-only, matching `ADR-0016` —
  it ports an accepted, implemented, measured design onto an existing,
  well-tested module using already-validated pieces (`STORAGE-014`'s
  header, hash, write path, and error variant), so the implementation's
  own test suite is the direct verification, per the design document's
  "Verification plan."
- **Real validation, post-acceptance**: a new spec (`STORAGE-015`); a
  generic blob module; `mmap_store.rs`/`order_customer.rs` tests covering
  round trip, missing-companion, wrong-magic (a `DOGBLOB\0` file is a
  magic error, not a decode attempt), and rewrite-only-when-changed; the
  same throwaway release-build measurement `STORAGE-014` used
  (`create`/`open`/`open_portable` at 1K/100K/1M `Order` records, median
  of 7, 3 at 1M) recorded in `RESULTS.md`; `tests/
  mmap_record_identity_keying.rs`, `RecordBlob`'s 12 tests and
  `production.rs`'s 6 portability tests passing unmodified.
- Revisit if: a future round adds a way to mutate a non-`ScanMarker`
  field, the id, or the index value after construction — the blob's
  immutability assumption would need real rework.
- Revisit if: `Symmetric`'s edge list needs persisting for a promoted
  (non-spike) domain — a `Symmetric`-level companion or a stack-level
  blob, a separate decision. *Resolved 2026-09-02, ahead of any promoted
  domain, as the owner's first queued follow-up: a `Symmetric`-level
  `<path>.edges` companion (`ADR-0018`, `STORAGE-016` v0.1.0). The
  `Employee` spike's durable stack is now portable as three files.*
- Revisit if: `open`'s steady-state delta lands nearer `STORAGE-014`
  v0.1.0's +27% than v0.2.0's +0.3–4% at 1M — the per-type trait-method
  fingerprint (considered option 2's fallback) is the named next step, not
  a different architecture. *Measured in place at ~4% (2026-09-02, below):
  on v0.2.0's side. The fallback itself saves ~7 ms per million records
  and is closed as not warranted; this trigger now re-arms only for a
  record type whose serialized width is far above `Order`'s ~76 B — a
  domain with large variable-length fields — and the first step is the
  same in-place A/B, not the trait.*
- Revisit if: a second store type ever needs the blob to know which `R`
  it holds — a schema tag in the header is a `BLOB_VERSION` bump, not a
  redesign.

## Acceptance and implementation

- 2026-09-02: accepted as proposed. The next unit registers `STORAGE-015`
  and implements per `docs/design/GENERIC-STORE-PORTABILITY-DESIGN.md`.
- 2026-09-02: implemented as `GENERIC-STORE-PORTABILITY` (`STORAGE-015`
  v0.1.0). `src/generic/record_blob.rs` (new — `GENBLOB\0`, blob version
  1, `GenericRecordBlob<'a, R>` borrowing the record slice, streamed
  `bincode`-into-`Fnv1a64` fingerprint, `read`/`blob_path`);
  `STORAGE-014`'s `HEADER_LEN`/`Fnv1a64`/`parse_header`/`encode_image`/
  `companion_path`/`EncodedRecordBlob` became `pub(crate)` at their
  second call site, `RecordBlob`'s 12 tests unchanged;
  `GenericMmapStore::create`/`open` write/refresh the blob and gain the
  `R: Serialize + DeserializeOwned` bound; `read_portable_records(path)`
  and `open_portable(path)` added; `open_order_production_stack_portable
  (path)` in `order_customer.rs`; serde derives on `Order`/`OrderStatus`/
  `Customer`/`Employee`/`Department`. The `.mmap` slot format is untouched
  (the diff removes only the six bound lines).
- Two deliberate departures from the design's sketch, recorded in the
  spec's "Traceability": no bound is added on `R::Id` (only `Vec<R>` is
  ever serialized, and `R`'s own derive already covers its id field), and
  no `Employee` portable helper is added (the `Symmetric` edge list is out
  of scope per this ADR; the helper waits for that decision).
- **The `open`-cost revisit trigger above appeared tripped** (superseded
  by the next entry). Release build,
  throwaway measurement, median of 7/7/3: `open` +80–108% at 1K (a
  ~0.1 ms floor on a 0.15 ms call), +19–33% at 100K (10–17 ms to
  stream-encode 100K records into the hasher), and −4% to +2% at 1M
  across three after-runs — but the published 20-sample Criterion
  `generic_production_open` group, run twice in the same session against
  a `git stash`-isolated before-run, puts the 1M delta at +24–27% (about
  300 ms on 1.25 s). `RESULTS.md`'s `### GenericMmapStore file
  portability (STORAGE-015)` subsection records both and treats the
  Criterion figure as the trustworthy one (20 samples over 3; the linear
  extrapolation from 100K lands on its side of zero). That is nearer
  v0.1.0's +27% than v0.2.0's +0.3–4%, so the named next step — the
  per-type trait-method fingerprint of considered option 2's fallback,
  not a different architecture — is now the owner's call; not built
  here, since it adds to the record traits' API. `create` roughly doubles
  at every size, the blob write itself (~76 B/record, 3× the `.mmap`
  file).
- Unlike `STORAGE-014`, two published Criterion groups *do* time these
  constructors: `generic_production_create` and `generic_production_open`
  (added by the record-identity-keying round). Their numbers move by
  roughly the deltas above; `RESULTS.md` records the before/after pair.
- 2026-09-02, **the fallback measured and closed as not warranted.** The
  owner asked for the trait-method fingerprint; before building it this
  round measured what it could save at 1M `Order` records (release
  build, 5-sample medians). An in-place A/B — the same binary with
  `is_current_at`'s `fingerprint()` stubbed to a header-only check —
  puts `open` at 1,222 ms with the fingerprint and 1,170 ms without: the
  shipped fingerprint is ~52 ms, **4% of `open`**, not the ~300 ms the
  Criterion pair implied (that group, re-run on unchanged code, moved
  +8.8% against itself — drift larger than the fingerprint). In
  isolation: streamed bincode 79 ms; a hand-walk over every `Order`
  field 72 ms (**−7 ms**, the same ~76 B/record hashed, only serde's
  per-field dispatch saved); id + customer + status 42 ms; id alone
  21 ms — each cheaper row hashes less of the record, and a reopen with
  a changed non-hashed field would then keep the blob silently stale,
  the gap `ADR-0016` rejected. Offered as a three-way choice (build
  whole-record for ~7 ms; build id-only for ~50 ms at that cost; close),
  the owner chose to close. Consequence: the streamed fingerprint of
  considered option 2 stays, `BLOB_VERSION` stays 1, `STORAGE-015` stays
  v0.1.0, no record-trait API is added, and the revisit trigger above is
  re-read against the in-place figure (4%, v0.2.0's side). `RESULTS.md`'s
  `#### Follow-up: the trait-method fingerprint, measured and not built`
  carries the tables. No code change in this round.
