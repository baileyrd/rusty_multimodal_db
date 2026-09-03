# Server Journal Group Commit Design (Proposed)

- Status: **Proposed** (not yet accepted; no implementation authorized).
  One decision, `ADR-0026`: take the batch journal's `fsync` out of the
  store's exclusive section and share it across concurrent batches as a
  leader/follower group commit, with applies kept in journal order.
  Acceptance authorizes the design; implementation follows as its own
  unit — see `ADR-0026`'s "Acceptance and implementation" section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part B /
  `ADR-0025` (the redo journal this design re-schedules — `SERVER-001`
  v0.15.0 / FR-025, `JRN-FR-001`–`009`; its third revisit trigger, *the
  journal's `fsync` cost dominates a real workload*, is what this
  document answers), `RESULTS.md` "Journaled `Request::Transaction`
  follow-up (FR-025)" (the measurement that fired the trigger),
  `docs/design/SERVER-TRANSACTION-DESIGN.md` / `ADR-0013`
  (`TransactionalStore::with_exclusive`, the one continuously held
  section a batch takes, and the "no runtime deletion, idempotent
  overwrite" invariant everything here rests on), `src/server/journal.rs`
  (`BatchJournal`, `CheckpointFlush`, `JournalError`),
  `src/durability/hybrid.rs` (the crate's earlier note on why eager
  per-write `fsync` is the expensive variant), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0.

## Purpose and scope

`SERVER-001` v0.15.0 (FR-025, `ADR-0025`) made a `Request::Transaction`
or session `Commit` crash-atomic on an adapter built with
`with_journal`: the validated batch is appended to a redo journal and
`fsync`'d *before* its first slot write, inside the same
`with_exclusive` closure the batch already held. The design named the
cost — one `fsync` per batch — and asked for it to be measured rather
than estimated. `RESULTS.md` measured it:

| Row | Latency (µs, 1 conn) | 1 thread | 4 threads | 32 threads | 64 threads |
|---|---:|---:|---:|---:|---:|
| `dog-txn` (unjournaled) | 67.1 | 13,723 | 203,393 | 199,230 | 191,634 |
| `dog-jrnl-txn` (journaled) | 320.7 | 3,182 | 3,843 | 3,395 | 3,337 |

The latency line is the cost the design accepted: ~254 µs of `fsync`
on this container's storage. The throughput line is the finding: it is
**flat at every thread count**, because the append and `fsync` run
inside the exclusive section, so every other connection's batch — and,
less visibly, every other connection's *read* and single `UpdateField`,
which take the same lock — queues behind one `fsync` at a time.
`ADR-0025` anticipated exactly this profile and named the standard
answer as its own revisit trigger: *a group commit — one `fsync` per
several batches, across connections — is the standard answer and a
separate design.* This is that design.

**In scope:**

- Moving the journal append and `fsync` **out of** the store's
  exclusive section on a journaled adapter, so the store is never
  locked across an `fsync`.
- Sharing one `fsync` across every batch appended while the previous
  `fsync` was in flight — a leader/follower group commit with no timer,
  no delay, and no extra thread.
- Keeping applies in journal order, so the state the journal replays
  after a crash is the state every reader saw before it.
- Checkpointing (`JRN-FR-004`) only at a quiescent point, so no
  appended-but-unapplied batch is ever truncated away.
- Re-measuring the `dog-jrnl-txn` rows.

**Out of scope (see "Non-goals")**: journaling single writes, a commit
delay knob, a background writer thread, any change to the journal
format, the store, the wire, or the unjournaled path.

## Non-goals

- **Single-write durability.** Single `UpdateField`s stay unjournaled
  (`JRN-FR-007`); `ADR-0025`'s first trigger is untouched.
- **A commit-delay tunable.** The probe below shows a fixed delay buys
  batching but not throughput here, and costs the lone connection
  half its rate; the design offers it as `ADR-0026`'s option (b) and
  recommends against it. No new configuration surface.
- **A committer thread.** The adapter has no shutdown story
  (`serve`'s drain is an explicit non-goal since `ADR-0010`), so it
  must not own a thread it cannot stop.
- **Format, store, wire, lock-type changes.** `TXNJRNL\0` version 1 is
  unchanged; `TransactionalStore::with_exclusive` is unchanged; no
  `Request`/`Response`/`ErrorCode` variant; `PROTOCOL_VERSION` stays 4;
  `Cargo.toml` untouched.
- **The unjournaled path.** `DogConnectionStore::new` and its siblings
  keep the v0.7.0 validate-then-apply under one exclusive section,
  byte for byte; FR-018's headline numbers are not in play.

## Context and terminology

- **Batch**: the `Vec<TransactionOp>` a `Request::Transaction` carries
  or a session `Commit` assembles — the unit `apply_transaction`
  receives and the journal records (`JRN-FR-007`).
- **Exclusive section**: the closure `TransactionalStore::with_exclusive`
  runs under the store's write lock (`RwLock<MmapAgeStore>` for
  `ProductionStore`; the same shape for `GenericProductionStore`). Every
  `&self` read and every single `UpdateField` on every connection takes
  the matching read or write lock per call.
- **Journal**: `BatchJournal` — one file per adapter, `[u32 len][codec]`
  entries after a 12-byte header, appended and `sync_data`'d by
  `append`, replayed by `open`, truncated by a checkpoint.
- **Sequence**: the position of an entry in the journal, counted from
  the last checkpoint. Replay is in sequence order by construction
  (`open` reads the file front to back).
- **Durable through `n`**: every entry with sequence `≤ n` has had an
  `fsync` return after its bytes were written.
- **The invariant** (from `ADR-0013`, relied on by `ADR-0025`): every
  operation is an idempotent overwrite of a fixed-width slot keyed by
  an id that is never deleted at runtime, for a field whose type the
  schema fixes. Existence is the only precondition that could vary at
  runtime, and it never does.

### What the current code does, read from `main` `3987f9c`

`DogConnectionStore::apply_transaction` (and the identical
`Order`/`Employee` arms):

```rust
self.store.with_exclusive(|inner| {
    Self::validate_batch(inner, updates)?;          // existence + field/type
    match &self.journal {
        None => Self::apply_batch(inner, updates),
        Some(journal) => {
            let mut journal = journal.lock()..;      // never contended: inside with_exclusive
            journal.append(updates)..;               // write + sync_data  ← ~254 µs, lock held
            Self::apply_batch(inner, updates)?;
            if journal.needs_checkpoint() && inner.checkpoint_flush().is_ok() {
                let _ = journal.truncate();
            }
            Ok(())
        }
    }
})
```

The journal mutex exists only to make `BatchJournal` `Sync`; it is
taken inside the exclusive section and so is never contended. That is
the shape this design changes.

### Evidence: a throwaway probe

Before proposing, the mechanism was run rather than reasoned about. A
throwaway program (never committed, `std` only — one file, one mutex,
one condvar) compared three disciplines on this container's storage,
each thread appending a 48-byte entry 300 times:

- **per-batch**: write + `sync_data` under one mutex — v0.15.0's shape;
- **group**: leader/follower — the first waiter whose entry is not yet
  durable and who finds no `fsync` in flight becomes the leader,
  drops the mutex, `sync_data`s once, and marks durable everything
  appended before it started; everyone else waits on a condvar;
- **group + 100 µs**: the same, with the leader sleeping 100 µs before
  syncing to collect more followers (a fixed commit delay).

| Threads | per-batch (ops/s) | group (ops/s) | avg group | group + 100 µs (ops/s) | avg group |
|---:|---:|---:|---:|---:|---:|
| 1 | 4,910 / 5,265 | 5,314 / 5,496 | 1.0 | 2,519 / 2,737 | 1.0 |
| 4 | 4,861 / 4,894 | 8,482 / 8,668 | 2.3 | 7,348 / 8,024 | 3.9 / 4.0 |
| 32 | 4,246 / 4,380 | 23,135 / 25,919 | 12.2 / 11.9 | 22,520 / 26,201 | 19.4 / 20.0 |
| 64 | 4,588 / 4,583 | 29,781 / 29,228 | 16.0 / 16.2 | 27,963 / 27,608 | 27.6 / 29.0 |

(Two runs, both shown; container-class storage, so the absolute
numbers are this container's — the *shape* is the finding.) Three
things the numbers say:

1. **Per-batch is flat**, exactly as `RESULTS.md` found through the
   server: one `fsync` at a time, whatever the concurrency.
2. **Group commit scales** — 6.4× at 64 threads with an average group
   of 16, and the lone connection loses nothing (it is its own leader,
   one `fsync`, no wait).
3. **A commit delay is not worth a knob**: 100 µs roughly doubles the
   group size but adds no throughput (the `fsync` rate, not the group
   size, is the bound once groups are a dozen deep) and halves the
   single-connection rate. Rejected as the default; offered as an
   option because it is the one variant a reader would expect to see
   weighed.

The probe measures only the journal step. Through the server, the
apply step (an exclusive section of microseconds, per `dog-txn`'s
200k/s) and the network round trip sit on either side of it; the
implementation's bench row is where the composed number gets recorded.

## Requirements

- `GRP-FR-001` — **The `fsync` leaves the exclusive section.** On a
  journaled adapter, `apply_transaction` becomes four steps, only the
  third under the store's write lock: (1) validate, using the store's
  ordinary per-call reads; (2) append the batch to the journal under
  the journal's own mutex, taking a sequence number, and wait until
  the journal is durable through that sequence; (3) apply under
  `with_exclusive`, in sequence order; (4) checkpoint if due and
  quiescent. Validation before the exclusive section is sound by the
  invariant: existence never changes, and field/type validity is a
  pure function of the request. `JRN-FR-002`'s guarantee — durable
  before the first slot write — is preserved by step order.
- `GRP-FR-002` — **Leader/follower group commit.** The journal keeps
  `appended` (the last sequence written), `durable` (the last sequence
  known synced), and a `syncing` flag. A batch whose sequence is not
  yet durable and that finds no sync in flight becomes the leader: it
  sets `syncing`, releases the mutex, calls `sync_data` once, retakes
  the mutex, advances `durable` to the `appended` it observed before
  syncing, clears `syncing`, and notifies. Any other batch waits on
  the condvar until `durable ≥` its sequence. No timer, no sleep, no
  thread: a lone batch is its own leader and pays exactly v0.15.0's
  one `fsync`.
- `GRP-FR-003` — **Ordered apply.** Applies happen in sequence order:
  a batch enters `with_exclusive` only when `next_apply ==` its
  sequence, and advances `next_apply` when it leaves. So two batches
  that touch one slot apply in the order the journal will replay them,
  and a reader before a crash and a reader after replay see the same
  winner. The gate costs no extra waiting: durability is monotone in
  sequence (a leader covers every earlier entry), so the batch whose
  turn it is is always already durable.
- `GRP-FR-004` — **Quiescent checkpoint.** A batch checkpoints
  (`checkpoint_flush` then `truncate`, per `JRN-FR-004`) only if, after
  its own apply and while it still holds the apply turn, the journal
  is past `JOURNAL_CHECKPOINT_BYTES` **and** no later entry has been
  appended (`appended ==` its sequence). Otherwise it defers; a later
  batch will find the condition. The journal therefore still holds
  exactly the batches applied since the last checkpoint, never one
  that is appended but not yet applied.
- `GRP-FR-005` — **Failure.** A write failure at append fails that
  batch alone with `ErrorCode::Journal`, nothing applied. An `fsync`
  failure fails every batch the leader was covering with
  `ErrorCode::Journal`, nothing applied; the next batch tries again
  (v0.15.0's per-batch semantics, unchanged). Those entries may or may
  not be on disk and would replay on the next open — exactly
  `JRN-FR-005`'s "a batch whose `Ok` was never sent may or may not be
  present." A poisoned journal mutex is `Journal` for every batch
  after it, as today.
- `GRP-FR-006` — **Cost.** Single-connection cost is v0.15.0's: one
  `fsync`, plus two uncontended mutex acquisitions and one condvar
  check. Readers and single writers on other connections no longer
  wait behind a batch's `fsync` — at v0.15.0 they did, because the
  exclusive section held it.
- `GRP-FR-007` — **Nothing else changes.** Journal format
  (`TXNJRNL\0`, version 1, the golden header), `open`'s replay and
  torn-tail handling, `CheckpointFlush`, `JournalError`,
  `TransactionalStore::with_exclusive`, `ConnectionStore`, the wire,
  `PROTOCOL_VERSION` (4), `Cargo.toml`, and the unjournaled path are
  unchanged. The three adapters share one implementation of the
  discipline; none re-implements it.
- `GRP-FR-008` — `SERVER-001` goes to v0.17.0 with FR-027; the
  `dog-jrnl-txn` rows in `RESULTS.md` are re-measured on the same
  harness with the v0.15.0 rows kept beside them; `ADR-0025`'s third
  trigger is resolved by pointer and Part B's "lock discipline is
  unchanged" invariant is amended by pointer.

## Considered options

**Mechanism.**

1. **Leader/follower group commit, no delay (proposed).** The waiter
   that finds no sync in flight syncs for everyone appended so far.
   Self-tuning: at one connection it is one `fsync` per batch with no
   added latency; under load the group is whoever arrived during the
   last `fsync`. The probe: 6.4× at 64 threads, lone connection
   unchanged.
2. **Leader/follower with a fixed commit delay** (the leader waits
   `GROUP_COMMIT_DELAY` before syncing). Rejected as the default: the
   probe shows bigger groups but no more throughput, and the lone
   connection pays the delay on every batch. Offered as `ADR-0026`'s
   option (b) — it is the one variant PostgreSQL (`commit_delay`) and
   MySQL (`binlog_group_commit_sync_delay`) both expose, so a reader
   will ask.
3. **A dedicated committer thread** that syncs on a tick or when woken.
   Rejected: it is option 2 with a thread the adapter must own and
   stop, and the adapter has no lifecycle to hang that on.
4. **Keep the `fsync` inside the exclusive section** — do nothing.
   `ADR-0026`'s option (c): the journal is opt-in and off by default,
   and `RESULTS.md`'s numbers are container numbers. Honest, and the
   reason this is a design round rather than an implementation; but
   the reader/writer stall behind each `fsync` is a real cost on a
   journaled server that the design also removes, not only a
   throughput number.
5. **One journal file per connection.** Rejected: replay order across
   files is undefined, which is exactly the property `GRP-FR-003`
   exists to protect; and `open` would have to merge.

**Ordering.**

1. **Ordered apply by sequence (proposed).** A gate, not a lock: the
   store's write lock is taken only for the apply itself.
2. **Let applies race** (take `with_exclusive` whenever durable).
   Rejected: two batches on one slot could apply in the opposite order
   to their journal order; after a crash, replay would re-decide the
   winner, and a reader who saw the first winner before the crash sees
   the other after it. `JRN-FR-005` would hold per batch and still be
   a lie in aggregate. The gate is free (durability is monotone), so
   there is no trade to make.
3. **Append under the exclusive section, sync outside it** — validate
   and append inside `with_exclusive` so sequence order equals lock
   order, then release, wait for durability, and re-acquire to apply.
   Equivalent to the proposal with one more write-lock acquisition per
   batch and validation kept under the lock. Not chosen: the invariant
   makes validation outside the lock sound, and one fewer exclusive
   acquisition is one fewer stall for readers. Noted as a fallback if
   implementation finds a reason validation must see a locked store.

**Checkpoint.**

1. **Quiescent only (proposed)** — `appended ==` own sequence after own
   apply.
2. **Checkpoint through the sequence applied so far** (truncate a
   prefix). Rejected: `BatchJournal::truncate` drops the whole file by
   design (`hybrid.rs`'s discipline); a prefix truncation is a
   rewrite, a second format decision, for a checkpoint that is already
   rare (1 MiB of batches).

## Proposed shape

```rust
// src/server/journal.rs — additions; BatchJournal's format and open()
// are untouched.
impl BatchJournal {
    /// Write one entry without syncing; the caller owns the fsync.
    pub(crate) fn append_unsynced(&mut self, batch: &[TransactionOp]) -> Result<(), JournalError>;
    /// `sync_data` on the journal file.
    pub(crate) fn sync(&self) -> Result<(), JournalError>;
}

/// The group-commit discipline, shared by the three adapters.
pub(crate) struct CommitGroup {
    state: Mutex<GroupState>,
    durable: Condvar,   // durable advanced
    turn: Condvar,      // next_apply advanced
}
struct GroupState {
    journal: BatchJournal,
    appended: u64,      // last sequence written
    durable: u64,       // last sequence fsync'd
    syncing: bool,      // a leader is in sync_data
    next_apply: u64,    // the sequence whose turn it is
}

impl CommitGroup {
    pub(crate) fn open(path: &Path) -> Result<(Self, Vec<Vec<TransactionOp>>), JournalError>; // = BatchJournal::open
    /// GRP-FR-001..004: append → group fsync → ordered apply → quiescent checkpoint.
    /// `apply` runs inside the caller's `with_exclusive`; it returns whether the
    /// store flushed (so the group may truncate).
    pub(crate) fn commit<E>(
        &self,
        batch: &[TransactionOp],
        apply: impl FnOnce(Checkpoint) -> Result<(), E>,   // Checkpoint: "flush now?" query the adapter answers
    ) -> Result<(), CommitError<E>>;                        // Journal(JournalError) | Apply(E)
}

// src/server/dog.rs (and order.rs, employee.rs)
pub struct DogConnectionStore<S> { store: S, journal: Option<CommitGroup> }

fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)> {
    match &self.journal {
        None => self.store.with_exclusive(|inner| {              // v0.7.0 path, unchanged
            Self::validate_batch(inner, updates)?;
            Self::apply_batch(inner, updates)
        }),
        Some(group) => {
            Self::validate_batch(&self.store, updates)?;          // per-call reads, no exclusive section
            group.commit(updates, |checkpoint| {
                self.store.with_exclusive(|inner| {
                    Self::apply_batch(inner, updates)?;
                    if checkpoint.due() && inner.checkpoint_flush().is_ok() { checkpoint.flushed(); }
                    Ok(())
                })
            }).map_err(|e| match e { CommitError::Journal(_) => (0, ErrorCode::Journal), CommitError::Apply(e) => e })
        }
    }
}
```

`commit`, step by step, for a batch that becomes sequence `s`:

1. Lock `state`; `append_unsynced`; `appended = s`. (A write error
   returns `Journal` here; `appended` is not advanced.)
2. While `durable < s`: if `!syncing`, become the leader — set
   `syncing`, note `upto = appended`, unlock, `sync`, relock, set
   `durable = max(durable, upto)` (or record the error for every
   sequence in `(durable, upto]`), clear `syncing`, `notify_all`
   `durable`. Otherwise `wait` on `durable`.
3. While `next_apply != s`: `wait` on `turn`. Unlock `state`.
4. Run `apply` (the adapter's `with_exclusive`). Inside it the
   adapter asks `checkpoint.due()` — true iff `journal.needs_checkpoint()
   && appended == s` (read under a brief relock) — and reports a
   successful flush.
5. Relock `state`; if the adapter flushed, `truncate`; `next_apply = s + 1`;
   `notify_all` `turn`.

`validate_batch` is generalized to take `&impl DogStore` (it uses only
`get`) so the same function serves both paths; `Order`/`Employee`
already have `self.store.get::<T>()` for their reads.

## Data/state and invariants

- `durable` is monotone and every sequence `≤ durable` is on disk
  (a leader covers every entry appended before it started).
- An entry is applied only after `durable ≥` its sequence
  (`JRN-FR-002`, preserved by step order).
- Applies occur in sequence order; `next_apply` advances by exactly one
  per batch, in the batch's own thread, after its apply.
- A checkpoint truncates only when `appended == next_apply − 1` and
  after the store flushed — the journal holds exactly the batches
  applied since the last checkpoint, `ADR-0025`'s invariant unchanged.
- The store's write lock is held for an apply (and a rare flush) only,
  never across a `sync`.
- Sequence numbers are in-memory only; the file carries no sequence
  field. Replay order is file order, which is append order, which is
  apply order.
- Lock order: `state` is never held while taking the store lock
  (`commit` unlocks before `apply`); the adapter never takes `state`
  itself. No cycle.
- Validation outside the lock sees existence and field/type only; a
  concurrent single `UpdateField` changes a value, never existence, so
  a batch validated as `Ok` is still applicable when its turn comes —
  the same argument `ADR-0013` made for validate-then-apply *inside*
  one section, now resting explicitly on the invariant.

## Errors, failure, recovery, and observability

- **Crash before the leader's `sync` returns**: the covered batches
  were never answered `Ok`. On open they may be present (complete
  entries the OS happened to write back), torn (the last one), or
  absent; `open` replays the complete ones and drops a torn tail —
  `JRN-FR-003`/`005`, unchanged. Only the final entry can be torn,
  because appends are sequential under one mutex.
- **`sync` fails**: every batch in `(durable, upto]` gets
  `ErrorCode::Journal`, none is applied, `durable` does not advance
  for them, and the next batch appends after them and syncs again.
  Their bytes remain in the file and would replay on the next open
  as "never acknowledged" writes — the same exposure v0.15.0 has for a
  failed `append`. Whether an `fsync` failure should instead fail-stop
  the journal (every later batch refused until restart, the posture
  PostgreSQL adopted after 2018's "fsyncgate") is an open question
  below; the design keeps v0.15.0's semantics.
- **Apply fails** (impossible for a validated batch; guarded): the
  batch is journaled but not applied; `next_apply` still advances so
  later batches proceed; on the next open, replay applies it — which
  is what redo means. The error is reported to the client as today.
- **Panic inside `apply`**: `next_apply` would never advance and every
  later batch would wait forever. The store lock is already poisoned
  in that case (every subsequent `with_exclusive` panics), so the
  adapter is already unusable; the design accepts this and names it —
  a `Drop`-guard that advances `next_apply` on unwind is a cheap
  hardening the implementation may add.
- No per-batch logging; a group's size is not observable except by
  the bench.

## Security, privacy, and compatibility

- Same file, same bytes, same directory, same permissions as v0.15.0.
  No token, no key, no new data at rest.
- A journal written by v0.15.0 replays under this design and vice
  versa: the format is unchanged.
- The unjournaled path is byte-identical; `benches/server.rs`'s
  `dog-txn`/`order-txn`/`employee-txn` rows must not move.
- Fairness: the leader does the work for followers that arrived after
  it; a follower never waits for more than one `fsync` beyond the one
  in flight when it appended. The apply gate is FIFO by construction.

## Acceptance criteria

1. On a journaled adapter, no `fsync` happens while the store's write
   lock is held — shown by a test that holds the leader's `sync` on a
   test-only hook and observes a concurrent `get` and a concurrent
   single `UpdateField` complete on another thread while it is held.
2. Grouping: with the same hook holding one leader, two batches
   appended meanwhile become durable after that leader's single
   `sync` — three batches, one `sync` (a test-only sync counter).
3. Ordered apply: with many threads committing concurrently, the
   sequence each apply observed is strictly increasing, and a journal
   replayed onto pre-batch files (the file-copy pair from Part B)
   yields the same final state the live adapter reached.
4. Quiescent checkpoint: a checkpoint that fires while other batches
   are appended-but-unapplied is deferred, and after it fires no
   appended batch is missing from either the store or the journal.
5. Failure: a `sync` made to fail by the hook fails every batch in the
   group with `ErrorCode::Journal`, applies none, and the next batch
   succeeds.
6. The single-connection `dog-jrnl-txn` latency is within the
   v0.15.0 band; its throughput rows rise with thread count; every
   unjournaled row is within its run-to-run band. Recorded in
   `RESULTS.md` beside the v0.15.0 rows.
7. Every existing test — `journal.rs`'s four, the adapters' replay and
   checkpoint tests, `tests/server_transaction_integration.rs`'s
   journaled section, the golden header — passes unchanged.
8. No `Cargo.toml`, journal-format, storage-format, wire,
   `PROTOCOL_VERSION`, `TransactionalStore`, or `ConnectionStore`
   change; `git diff --stat` limited to `src/server/journal.rs`, the
   three adapters, `benches/server.rs`, tests, `RESULTS.md`, and docs.

## Verification plan

- `src/server/journal.rs` unit tests on `CommitGroup` with a
  `#[cfg(test)]` sync hook (criteria 1, 2, 5) and a many-thread
  ordering test (criterion 3's first half).
- `src/server/dog.rs` tests: the ordering-then-replay pair (criterion
  3's second half) and the deferred checkpoint (criterion 4), on the
  existing `with_journal_replays_onto_pre_batch_files` fixture.
- `tests/server_transaction_integration.rs`: the journaled section
  unchanged (criterion 7), plus one concurrent-clients test through
  the server.
- `benches/server.rs`: the existing `dog-jrnl-txn` rows re-run
  (criterion 6); `RESULTS.md` gets a "group commit" paragraph under
  the FR-025 subsection.

## Traceability

- → `SERVER-001` v0.17.0 / FR-027 (`GRP-FR-001`–`008`), `ADR-0026`;
  resolves `ADR-0025`'s third revisit trigger and amends Part B's
  "lock discipline is unchanged" invariant by pointer.
- Roadmap: `SERVER-JOURNAL-GROUP-COMMIT-DESIGN` (this document), then
  `SERVER-JOURNAL-GROUP-COMMIT` as the implementation unit if
  accepted.

## Open questions

- **Fail-stop on `fsync` failure.** The design keeps v0.15.0's
  "fail the batch, try again next time." The stricter posture refuses
  every later batch until restart, because a later `fsync` cannot
  vouch for pages a failed one dropped. Named, not proposed: it is a
  policy the journal did not have at v0.15.0 either, and belongs to
  whichever round first sees a real `fsync` failure.
- Whether `validate_batch` outside the exclusive section needs a
  fallback (ordering option 3) will be known at implementation; the
  design expects not.
- Whether the apply gate should carry a `Drop` guard for the panic
  case — cheap, and the implementation may include it without a
  design change.
- `JOURNAL_CHECKPOINT_BYTES` is still the unmeasured constant Part B
  named; a quiescent checkpoint makes it slightly rarer under load
  (it needs a gap in arrivals). The bench row is still where it gets
  revisited.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting the
  group-commit design round as the first of four next directions
  ("1, 2, 3, 4") after `SERVER-001` v0.16.0. Evidence from a
  throwaway probe (never committed) recorded above.
