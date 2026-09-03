# ADR-0026: Group commit for the batch journal — the `fsync` leaves the exclusive section, one leader syncs for everyone, applies stay in journal order

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): leader/follower group
  commit with no delay, the `fsync` outside the exclusive section,
  ordered apply, quiescent checkpoint; (b) a fixed commit delay and (c)
  close as not warranted declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit — see
  "Acceptance and implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-JOURNAL-GROUP-COMMIT-DESIGN.md` (the
  full design this ADR summarizes), `ADR-0025` /
  `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part B (the redo
  journal, `SERVER-001` v0.15.0 / FR-025; its third revisit trigger —
  *the journal's `fsync` cost dominates a real workload; a group
  commit is the standard answer and a separate design* — is what this
  answers), `RESULTS.md` "Journaled `Request::Transaction` follow-up
  (FR-025)" (the measurement that fired it: latency +254 µs, throughput
  flat at ~3.3k batches/s from 1 to 64 connections), `ADR-0013` /
  `docs/design/SERVER-TRANSACTION-DESIGN.md` (`with_exclusive` and the
  "no runtime deletion, idempotent overwrite" invariant), `ADR-0024`
  (a session `Commit` is one of the two batch shapes this covers),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.16.0.
- Supersedes/Superseded by: none. Amends one sentence of Part B's
  "Data/state and invariants" — *the lock discipline is unchanged:
  append, apply, and checkpoint all happen inside the one
  `with_exclusive` closure* — by pointer; changes no format, store,
  wire, or lock type.

## Context

`ADR-0025` made a batch crash-atomic by appending it to a redo journal
and `fsync`ing before the first slot write, inside the exclusive
section the batch already held. It accepted one `fsync` per batch with
open eyes and asked for the number. `RESULTS.md` produced it: the
`fsync` costs ~254 µs on this container, and — the finding — journaled
throughput is *flat* from 1 to 64 connections, because the `fsync` runs
under the store's write lock, so every other connection's batch, read,
and single write queues behind it. `ADR-0025` named this exact profile
as its trigger and named the answer.

A throwaway probe (recorded in the design doc, never committed) ran the
answer before proposing it: a leader/follower group commit on the same
storage goes from a flat ~4.6k `fsync`s/s to ~29k batches/s at 64
threads with an average group of 16, while a single thread loses
nothing; a fixed 100 µs commit delay doubles the group size, adds no
throughput, and halves the lone thread's rate.

The owner selected this design round as the first of four directions.
This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0025` took.

## Decision drivers

- Stop holding the store's write lock across an `fsync`: a journaled
  server should not stall every reader and single writer for a quarter
  of a millisecond per batch.
- Let concurrent batches share one `fsync` without a timer, a delay, or
  a thread — self-tuning, so one connection pays exactly what it pays
  today.
- Keep every guarantee `ADR-0025` made: durable before the first write,
  idempotent replay, torn tail dropped, checkpoint only after the store
  flushed — and add the one a concurrent journal newly needs: the
  order the journal replays is the order the store applied.
- Change no format, store, wire, or lock; leave the unjournaled path
  byte-identical.

## Considered options

1. **Leader/follower group commit, no delay, ordered apply** —
   proposed. Validate with per-call reads (sound by the invariant);
   append under the journal mutex, taking a sequence; the first waiter
   with no sync in flight syncs once for everyone appended so far,
   the rest wait on a condvar; apply under `with_exclusive` in
   sequence order (a turn gate — free, since durability is monotone in
   sequence); checkpoint only when no later entry is appended.
2. **The same with a fixed commit delay** (the leader waits before
   syncing to collect followers). Rejected as the default on the
   probe's numbers; offered as option (b) because it is the variant
   PostgreSQL and MySQL both expose and a reader will ask.
3. **A committer thread** on a tick. Rejected: option 2 plus a thread
   the adapter cannot stop (no shutdown story since `ADR-0010`).
4. **Do nothing** — the journal is opt-in, the numbers are container
   numbers. Offered as option (c). The design's answer: the
   reader/writer stall is real on any journaled server, not only a
   throughput number.
5. **Unordered apply** (take the lock whenever durable). Rejected: two
   batches on one slot could replay in the opposite order to how they
   applied, so a crash could change a winner a reader had already
   seen. The gate is free; there is no trade.
6. **One journal per connection.** Rejected: replay order across files
   is undefined — the very property option 5 was rejected for losing.
7. **Append inside the exclusive section, sync outside it.** Equivalent
   to the proposal with one extra write-lock acquisition per batch;
   kept as the fallback if validation turns out to need a locked store.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/journal.rs`: `BatchJournal::append_unsynced` and `sync`
  (format, `open`, `truncate`, `CheckpointFlush`, `JournalError`
  unchanged); a `pub(crate) CommitGroup { Mutex<GroupState>, Condvar
  durable, Condvar turn }` with `GroupState { journal, appended,
  durable, syncing, next_apply }` and one `commit(batch, apply)` that
  runs append → group `fsync` → ordered apply → quiescent checkpoint
  (`GRP-FR-001`–`004`), shared by the three adapters.
- `src/server/{dog,order,employee}.rs`: `journal: Option<CommitGroup>`;
  the journaled arm of `apply_transaction` validates with per-call
  reads, then `commit`s with `with_exclusive` inside the apply closure;
  the unjournaled arm is untouched. `validate_batch` generalized to
  `&impl DogStore` (it uses only `get`).
- Failure semantics as v0.15.0's, per group: an `fsync` failure fails
  every covered batch with `ErrorCode::Journal`, nothing applied, the
  next batch tries again (`GRP-FR-005`). No new error code, no
  `PROTOCOL_VERSION` change.
- `SERVER-001` v0.17.0, FR-027 (`GRP-FR-001`–`008`); `ADR-0025`'s third
  trigger and Part B's lock-discipline sentence resolved by pointer;
  `SPEC-REGISTRY`, `TRACEABILITY`, `ROADMAP`
  (`SERVER-JOURNAL-GROUP-COMMIT`), `PROJECT-STATUS`.
- Tests per the design's verification plan — a `#[cfg(test)]` sync
  hook for the held-leader, grouping, and failure criteria; a
  many-thread ordering test; the file-copy replay pair reused for
  ordering and the deferred checkpoint — and the `dog-jrnl-txn` rows
  re-measured in `RESULTS.md` beside the v0.15.0 rows.
- No `Cargo.toml`, journal-format, storage-format, wire, lock-type, or
  `ConnectionStore` change.

## Consequences

### Positive

- A journaled server no longer stalls readers and single writers
  behind each batch's `fsync` — the store's write lock is held for
  microseconds of apply, never across a sync.
- Concurrent batches share `fsync`s in proportion to their
  concurrency, with no knob: the probe's 6.4× at 64 threads, and a
  lone connection at exactly today's cost.
- The ordering guarantee is stated and tested rather than accidental:
  replay order equals apply order.
- Nothing on disk changes; a v0.15.0 journal replays unchanged.

### Negative / tradeoffs

- **More moving parts in the hot path**: two condvars and a sequence
  counter where v0.15.0 had a never-contended mutex. Bounded — every
  wait has a named monotone condition — but a real increase in the
  code a reader must hold in their head. The design's invariants list
  is the reading guide.
- **Validation leaves the lock.** Sound by the invariant, and the
  design says so twice, but it is a second place (after `ADR-0025`'s
  redo argument) where correctness rests on "no runtime deletion"
  rather than on a held lock. If that invariant ever breaks
  (`ADR-0025`'s second trigger), this design breaks with it, and the
  fallback (option 7) is a one-function change.
- **A checkpoint needs a gap in arrivals.** Under sustained load the
  journal can exceed `JOURNAL_CHECKPOINT_BYTES` for a while; replay at
  the next open is correspondingly longer. Bounded by arrival gaps,
  which real workloads have; named.
- **An `fsync` failure now fails a group, not one batch.** The
  semantics per batch are unchanged (never `Ok`, may or may not be
  present); the blast radius per failure is wider by the group size.
- Panic inside an apply strands the turn gate — but the store lock is
  already poisoned in that case, so nothing that worked before is lost;
  a `Drop` guard is the cheap hardening the implementation may add.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0025`, but with the mechanism *run* rather than reasoned about:
  the probe's numbers in the design doc are this decision's evidence,
  and the one claim it does not exercise — that ordered apply and
  quiescent checkpoint preserve `ADR-0025`'s invariants under
  concurrency — is what the acceptance criteria's hook-driven tests
  and replay pair check.
- Revisit if: a real workload wants a commit delay after all — option
  2 is a constant and a `sleep` in the leader; the probe's numbers are
  the argument to re-run first.
- Revisit if: an `fsync` failure is ever observed in practice — the
  fail-stop question in the design's open questions becomes a
  decision.
- Revisit if: the store gains a shutdown/drain lifecycle — a committer
  thread (option 3) becomes possible, though not necessarily better.
- Revisit if: a domain gains a non-idempotent mutating operation
  (`ADR-0025`'s second trigger) — validation returns under the lock
  (option 7) and the journal needs sequence numbers on disk.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed —
  leader/follower group commit, no delay, the `fsync` outside the
  exclusive section, ordered apply, quiescent checkpoint; **(b)**
  accept with a fixed commit delay — the same, plus a
  `GROUP_COMMIT_DELAY` constant the leader waits before syncing, for
  larger groups at the lone connection's expense; **(c)** close as not
  warranted — the `fsync` stays inside the exclusive section,
  `ADR-0025`'s trigger stays armed for a real workload. Proposed in
  PR #145.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined —
  "a, a, a, a" across `ADR-0026`–`ADR-0029`). Implemented next, as
  `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-JOURNAL-GROUP-COMMIT-DESIGN.md`. (PR #153.)
- 2026-09-03: implemented as `SERVER-001` v0.17.0 (FR-027) in this PR
  — `CommitGroup` in `src/server/journal.rs` (append under the journal's
  mutex, leader/follower `fsync` outside the exclusive section, a turn
  gate for ordered apply, a quiescent checkpoint re-checked under the
  lock, v0.15.0's failure semantics per group, a turn guard on unwind,
  a `#[cfg(test)]` pre-sync hook), the three adapters' journaled path
  restructured around it, the unjournaled path untouched. Five unit
  tests and one integration test; every acceptance criterion 1–8 holds;
  no deviation. One implementation note: the hook runs after a leader
  takes the lead and *before* it reads how far to sync — the slot option
  (b)'s delay would occupy — so batches appended while a leader is held
  are covered by its one sync, as criterion 2 states. Measured:
  `dog-jrnl-txn` 304.3 µs and 3,169 / 5,906 / 14,276 / 15,322 batches/s at
  1 / 4 / 32 / 64 connections (v0.15.0: 320.7 µs; 3,182 / 3,843 / 3,395
  / 3,337). Full sweep green (352 lib tests, 347 + 5).
