# ADR-0025: Crash-atomic batches — an opt-in redo journal at the domain adapter, `fsync`'d before the first write

- Status: **Accepted** (promoted from Proposed on 2026-09-02 — the owner
  approved the design as proposed, option (a): an opt-in redo journal
  at the domain adapter, `fsync` before the first write, replay on
  open, checkpoint by size; (b) always-on and (c) close as not
  warranted declined; no changes requested). Acceptance authorizes the
  design; implementation follows as its own unit, after `ADR-0024`'s —
  see "Acceptance and implementation" below.
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part B
  (the full design this ADR summarizes), `ADR-0013` /
  `docs/design/SERVER-TRANSACTION-DESIGN.md` (`TXN-FR-007`, the named
  gap; its second revisit trigger — *crash-atomicity across a batch* —
  is what this answers), `ADR-0024` (the independent session decision
  from the same round; a session `Commit` is a batch this journal
  covers), `ADR-0006` / `src/durability/hybrid.rs` (the
  snapshot-plus-never-truncated-WAL shape this borrows),
  `STORAGE-011` / `STORAGE-017` / `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`
  (the per-slot `COMMITTED`-marker durability a batch is made of, and
  the "atomic multi-field updates" non-goal that document named),
  `ADR-0021` / `STORAGE-018` (`crate::codec`, the journal's encoding),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.13.0,
  `docs/FUTURE-GROWTH.md` item 2.
- Supersedes/Superseded by: none. Adds a sidecar to the domain
  adapters; changes no store, format, wire, or lock.

## Context

A `Request::Transaction` is N in-place slot writes under one lock. Each
write is torn-write-safe on its own (the `COMMITTED` marker) and the OS
writes pages back when it likes; `flush` forces it. Between the first
and the last of the N reaching disk, a crash of the server process
leaves some applied and some not — `ADR-0013` named this plainly
("a process crash between two of a batch's writes landing on stable
storage can leave a partial batch durably applied") and made it a
revisit trigger, expecting "a combined-journal or two-phase-commit-style
mechanism, a real durability redesign."

It is smaller than that, because of an invariant this crate has relied
on since `ADR-0013`: every operation is an idempotent overwrite of a
fixed-width slot keyed by a record id that is never deleted at runtime,
for a field whose type the schema fixes. Re-applying an applied
operation changes nothing; applying one that never landed produces
what it would have. So a log of *intended* writes, made durable before
the first write and replayed in order on the next open, restores the
all-or-nothing outcome from any intermediate state — with no prior
values recorded and no change to any slot file. `src/durability/hybrid.rs`
already has the shape: append per write, never truncate except at a
checkpoint that first makes the snapshot durable, replay everything
since on open. Here the unit is the batch and the snapshot is the
`.mmap` files.

Placement is the real decision. The only layer that both sees a batch
and knows how to apply one operation to its store is the domain adapter
(`apply_transaction` already maps `FieldRef` to `update_age`/
`update_field`); a store knows neither `TransactionOp` nor `FieldRef`.
So the journal is the adapter's, opt-in, a sidecar file.

The owner selected this design round as the third of four directions.
This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0024` took.

## Decision drivers

- Make "answered `Ok`" mean "fully applied or fully replayed" for a
  batch, across a crash of this process — the one guarantee
  `TXN-FR-007` withheld.
- Change no format that works (`GMMAPST\0` 2, the ages file, the blobs),
  no lock, no wire shape, no `ConnectionStore` signature.
- Pay for the guarantee only where it is asked for: one `fsync` per
  batch, on adapters constructed with a journal; the crate's headline
  single-write cost untouched.
- Reuse what exists: `crate::codec` for the entries, `Flush` for the
  checkpoint, the `hybrid.rs` discipline for truncation.

## Considered options

1. **A redo journal of intended writes, at the adapter, opt-in** —
   proposed. Inside the same `with_exclusive` closure: validate; append
   the batch and `fsync`; apply; if the journal exceeds
   `JOURNAL_CHECKPOINT_BYTES`, `flush` the store and truncate. On
   `with_journal(store, path)`: replay every complete entry, `flush`,
   truncate; a torn tail is discarded (never acknowledged). Format:
   `TXNJRNL\0`, `u32` version, `[u32 len][codec(Vec<TransactionOp>)]`.
2. **An undo log.** Rejected: needs a read before every write and a
   second value format, and buys nothing redo does not, given
   idempotence.
3. **Two-phase commit across the slot files** (a prepare marker per
   file). Rejected: a format change to every `.mmap` for what a sidecar
   achieves.
4. **`flush` after every batch, no journal.** Rejected: an `msync`
   after the writes shortens the window but does not close it; only
   durable intent *before* the first write closes it.
5. **Journal single writes too.** Rejected: doubles the cost of the
   nanosecond write for a guarantee a single slot already has; named
   as the future step if per-write `fsync` durability is wanted
   (`hybrid.rs` variant 1 measures it).
6. **Placement in the store** (`ProductionStore`/`GenericProductionStore`).
   Rejected: the store cannot replay a server-level operation without
   importing the server layer — an inversion of `STORAGE-011`/`-012`.
7. **Placement on `serve`** as a fifth parameter. Rejected: the journal
   belongs to one adapter/store pair, and `serve` is generic over
   `ConnectionStore`.
8. **Always-on rather than opt-in.** Weighed: the safer default for
   anyone reading "transaction" as ACID, but a real `fsync` per batch
   (hundreds of microseconds to milliseconds against FR-018's measured
   microseconds), and every existing test, bench, and `dog_server`
   would take it. Offered as option (b); the design recommends opt-in.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/journal.rs` (`pub(crate)`): `JOURNAL_MAGIC`,
  `JOURNAL_FORMAT_VERSION = 1`, `JOURNAL_CHECKPOINT_BYTES = 1 MiB`;
  `BatchJournal::{open (replays, drops a torn tail), append (write +
  fsync), needs_checkpoint, truncate}`; `JournalError { Io, Format,
  Codec }`.
- `src/server/{dog,order,employee}.rs`: `with_journal(store, path)`
  constructors (replay → `flush` → truncate); `apply_transaction`
  gains the append-before-apply and checkpoint-after-apply steps
  inside its existing closure; a journal I/O failure at append is the
  batch's failure with nothing applied, reported as a new
  `ErrorCode::Journal` (the next index; version-gated like any appended
  code) in `TransactionFailed { index: 0, .. }`.
- `src/bin/dog_server.rs`: optional `SERVER_TXN_JOURNAL_PATH`.
- `SERVER-001` v0.15.0, FR-025 (`JRN-FR-001`–`009`) — v0.14.0 if
  `ADR-0024` is declined; `TXN-FR-007` and the v0.7.0 open question
  resolved by pointer for journaled adapters and restated for
  unjournaled ones; `ADR-0013`'s second trigger taken;
  `MULTI-FIELD-MMAP-DURABILITY-DESIGN.md`'s "atomic multi-field
  updates" non-goal pointed here; `SPEC-REGISTRY`, `TRACEABILITY`,
  `ROADMAP` (`SERVER-TRANSACTION-JOURNAL`), `PROJECT-STATUS`.
- Tests per the design's verification plan, including the crash
  criterion by file-copy pair (replay onto pre-batch files yields the
  full state; replay onto post-batch files is a no-op) unless a
  deterministic mid-batch snapshot hook is cheap; one journaled row in
  `benches/server.rs` and `RESULTS.md` recording the `fsync` cost.
- No `Cargo.toml`, storage-format, wire-shape (beyond the one appended
  error code), lock, or `ConnectionStore` signature change.

## Consequences

### Positive

- A committed batch survives a crash whole, on any adapter that asks
  for it — `docs/FUTURE-GROWTH.md` item 2's "do these N atomically"
  made true against a crash, without the transaction manager it
  predicted.
- No store, format, or lock touched; the mechanism is one sidecar file
  and two steps inside a closure that already exists.
- The cost is visible and chosen: one `fsync` per batch, measured in
  the bench row, paid only by journaled adapters.

### Negative / tradeoffs

- **Opt-in means off by default**, including in `dog_server` unless the
  variable is set. A reader who assumes `Transaction` is crash-atomic
  without checking is still wrong on an unjournaled adapter; the docs
  say so in both places.
- **Opening without `with_journal` after a crash forgoes replay.** The
  journal is inert until an adapter reads it; a deployment must open
  the same way every time. Named, and the reason `dog_server` reads
  the variable at every start.
- **One `fsync` per batch** — milliseconds on spinning disks, hundreds
  of microseconds on NVMe — against FR-018's microsecond batches.
- Guarantees hold only against a crash of this process on a filesystem
  that honors `fsync`/`msync`; not a general durability upgrade, not
  protection for single writes.
- Replay time at open is bounded by `JOURNAL_CHECKPOINT_BYTES`, a
  constant chosen without measurement; the bench row is where it gets
  revisited.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0024`. One claim is made from reasoning rather than running —
  that redo replay is correct from every intermediate state — and it
  rests on the same "no runtime deletion, fixed schema, idempotent
  slot overwrite" invariant `ADR-0013` already relied on; the file-copy
  test pair in the acceptance criteria is its check.
- Revisit if: per-write durability (journaling single `UpdateField`s)
  becomes wanted — option 5, with `hybrid.rs` variant 1's numbers as
  the cost estimate.
- Revisit if: a domain gains a mutating operation that is not an
  idempotent overwrite (an append, a delete, a counter) — redo replay
  would no longer be trivially correct and this journal would need
  sequence numbers or an undo component.
- Revisit if: the journal's `fsync` cost dominates a real workload — a
  group-commit (one `fsync` per several batches, across connections)
  is the standard answer and a separate design.
  *Fired by `RESULTS.md`'s FR-025 rows (throughput flat at ~3.3k
  batches/s from 1 to 64 connections) and taken up as `ADR-0026` /
  `docs/design/SERVER-JOURNAL-GROUP-COMMIT-DESIGN.md`, proposed in
  this PR: the `fsync` leaves the exclusive section, one leader syncs
  for everyone, applies stay in journal order. No change to this
  decision; the journal's format and guarantees are unchanged there.*
- Revisit if: always-on is judged the right default after the bench
  row lands — a one-line constructor change, but a decision.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — an opt-in
  redo journal at the domain adapter, `fsync` before the first write,
  replay on open, checkpoint by size; **(b)** accept always-on — the
  same journal, unconditional on every adapter and in `dog_server`,
  taking the `fsync` cost everywhere for the safer default; **(c)**
  close as not warranted — `TXN-FR-007`'s named gap stands as the
  documented limitation, `ADR-0013`'s trigger stays armed. Proposed in
  PR #137.
- 2026-09-02: accepted as proposed (option (a); (b) and (c) declined).
  Implemented after `ADR-0024`'s unit, as `SERVER-001` v0.15.0 /
  FR-025, per `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part
  B. (PR #138.)
- 2026-09-02: implemented as `SERVER-001` v0.15.0 (FR-025) in PR #141
  — `src/server/journal.rs` (`BatchJournal`, `CheckpointFlush`,
  `JournalError`, the constants), `with_journal` on all three adapters
  with the append-before-apply and checkpoint steps inside
  `apply_transaction`'s existing closure, `SERVER_TXN_JOURNAL_PATH`,
  `ErrorCode::Journal`, the `dog-jrnl-txn` bench rows in `RESULTS.md`.
  Seven unit tests and one integration test; every acceptance
  criterion holds. One clarification, not a deviation: `ErrorCode::Journal`
  is an appended variant, so `ADR-0022`'s rule 2 bumps `PROTOCOL_VERSION`
  to 4 and rule 3 downgrades it to `Unsupported` on a connection below 4
  (`downgrade_for_version`) — this ADR named the index and the gating
  but not the bump. The partial-checkpoint case (flush or truncate
  failing after the writes) leaves a longer journal that replays
  idempotently, as the design's invariants allow. Full sweep green (347
  lib tests, 340 + 7).
