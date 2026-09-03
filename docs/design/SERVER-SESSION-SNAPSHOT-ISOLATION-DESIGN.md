# Server Session Snapshot Isolation Design (Proposed)

- Status: **Proposed**
- Date: 2026-09-03
- Related: `ADR-0024` / `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`
  (the session mechanism this design extends; `SESS-FR-007` — *reads
  inside a session see committed state only* — is the requirement
  this design finishes closing), `ADR-0027` /
  `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md` (closed the
  first half of `SESS-FR-007`'s gap — a session's own pending writes
  — explicitly leaving the second half, isolation from *other*
  connections' commits, unnamed as a trigger and unaddressed),
  `ADR-0025`/`ADR-0026` (the crash-atomic journal and group commit —
  the precedent for extending `apply_transaction`'s own signature and
  for validating and applying atomically inside one exclusive
  section), `ADR-0030` (`STV-FR` stage-time validation — the other
  precedent for a session-scoped `ConnectionStore` hook),
  `docs/FUTURE-GROWTH.md`'s "Path to SQLite/DuckDB parity" (*Individual
  writes are crash-safe and torn-write-safe today, but "do these N
  operations atomically, or roll all of them back" doesn't exist as a
  concept. This needs a real transaction manager* — the owner's
  starting point for this round; see "Context" below for how much of
  that framing this repository has since closed on the write side).

## Purpose and scope

The owner picked "real transactions" as the direction to pursue from
`docs/FUTURE-GROWTH.md`. Read literally against the document's own
words, most of that gap has already closed since it was written:
`Request::Transaction` and interactive sessions give N operations
true all-or-nothing atomicity (`ADR-0013`, `ADR-0024`), the redo
journal and group commit give that atomicity crash-safety
(`ADR-0025`, `ADR-0026`), and read-your-writes lets a session see its
own pending writes (`ADR-0027`). What remains is narrower and more
precise than the original prose: **a session has no protection from
what happens between its own reads and its own commit.** Two
`GetById`s in the same session can return different values for the
same field if another connection commits in between, and nothing
today lets a session detect that its decision to write was made
against data that has since moved. This is `SESS-FR-007`'s own
second half — *reads inside a session see committed state only* — a
requirement `ADR-0027` deliberately left standing rather than closed
(its own "Non-goals": read-your-writes answers "do I see my own
pending write," not "has anyone else's commit landed since I looked").

**In scope:**

- Detecting, at `Commit`, whether any record/field a session's own
  `GetById` calls returned has been changed by a commit from another
  connection (or another of this connection's own non-transactional
  writes) since it was read — and refusing the whole batch if so,
  atomically with the commit decision itself.
- A third `BeginWith` flag, `SESSION_SNAPSHOT_ISOLATION`, opt-in and
  composable with the two that already exist.
- The `ConnectionStore` trait change needed to check and apply in one
  exclusive section, following `STV-FR`'s and the journal's own
  precedent for growing that trait's surface.

**Out of scope (see "Non-goals")**: full MVCC, phantom-read
protection (a record created after being read as absent), `ScanField`/
`FilterEq`/relation-read isolation, snapshotting for read-only
connections with no session, retrying a conflicted transaction
automatically.

## Non-goals

- **Full MVCC.** `docs/FUTURE-GROWTH.md`'s own framing — "this needs a
  real transaction manager — likely its own MVCC or log-based design"
  — describes a genuine storage-engine rewrite: versioned records (or
  copy-on-write snapshots), a garbage-collection/vacuum story, and a
  real cost model for keeping old versions alive under a long-running
  reader. That is the actual multi-year effort the document names,
  and this design does not attempt it. What follows is optimistic
  conflict *detection*, not multi-version storage: exactly one copy
  of every record exists at all times, exactly as today.
- **Phantom-read protection.** A session that reads "no record with
  this id" and later commits gets no signal if that id is created in
  the meantime — only `GetById` calls that returned a real record are
  tracked (see `ISO-FR-005`). A real answer needs either a range/
  predicate lock or a versioned index, neither of which exists here;
  named as an open question, not solved.
- **`ScanField`/`FilterEq`/relation-read isolation.** Only `GetById`
  is tracked, for the identical reason `RYW-FR` only overlays
  `GetById`: a set-shaped read has no fixed identity to re-check
  cheaply at `Commit` — re-validating a `FilterEq` would mean
  re-running the whole scan, an O(n) cost per commit this design does
  not want to force onto every batch. A session that only ever issues
  set reads gets no isolation improvement from this feature; it keeps
  `SESS-FR-007`'s existing committed-state behavior, unchanged.
- **Automatic retry.** A conflicted commit fails outright, the same
  way a `RecordNotFound`/`Malformed`/`Journal` failure already does —
  the client decides whether to re-read and retry. No server-side
  retry loop, matching this crate's own "the client library, not the
  server, owns retry policy" posture everywhere else.
- **Isolating plain (non-session) reads.** A `GetById` outside any
  session is, and stays, a single committed-state read with no
  concept of "since when" — nothing to isolate.

## Context and terminology

**What `SESS-FR-007` already says**, read from `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`:
*"Reads inside a session (`GetById`, `ScanField`, …) are [committed
state only]."* `ADR-0027`'s own design doc restates this as the thing
read-your-writes does *not* change for anything but a session's own
`GetById` overlay, and lists among its rejected options nothing that
would have closed the isolation-from-others half — that half was
simply never a trigger any accepted design named. This design is the
first to name it.

**What the session already tracks**, read from `src/server/mod.rs`'s
`handle_connection`: a per-connection `session: Option<Vec<TransactionOp>>`
(staged writes), `read_your_writes: Option<Vec<FieldRef>>` (the
updatable-field set, when that bit is on), and `validate_on_stage: bool`.
The `Request::GetById` session intercept, active whenever
`read_your_writes.is_some() && session.is_some()`, calls
`dispatch(store, Request::GetById { id })` for the raw, committed
result and *then* overlays staged writes onto it via
[`overlay_staged`] before answering — the raw, pre-overlay value is
exactly what this design needs to remember.

**What `Commit` already does**: takes the staged `Vec<TransactionOp>`
and calls `ConnectionStore::apply_transaction(&batch)`, which — inside
one `with_exclusive` section per domain adapter — validates every
operation against current state and applies the whole batch, or
applies nothing and reports which staged index failed
(`Response::TransactionFailed { index, code }`). A journaled adapter
appends to its journal and (since `ADR-0026`) waits for group-commit
durability inside that same window before applying. `ErrorCode::Journal`
already established the "index 0, not a real staged-op index" shape
for a failure that isn't about any one operation (`JRN-FR-002`) — the
shape this design's own new failure reuses.

## Requirements

- `ISO-FR-001` — **A third `BeginWith` bit.**
  `pub const SESSION_SNAPSHOT_ISOLATION: u32 = 4`, composing with
  `SESSION_READ_YOUR_WRITES` (1) and `SESSION_VALIDATE_ON_STAGE` (2)
  — `flags: 7` is every bit on. Unknown bits stay `Malformed`
  (unchanged rule). Introducing a flag bit is introducing a bit of
  wire meaning exactly as a variant is (the clarification `ADR-0022`
  already carries, taken at `STV-FR`'s own precedent): `PROTOCOL_VERSION`
  moves from 6 to **7**, the version table gains row 7, and `BeginWith`
  with this bit set is `Malformed` on a connection negotiated below 7.
- `ISO-FR-002` — **Read-set tracking.** While a session opened with
  `SESSION_SNAPSHOT_ISOLATION` is active, every `GetById` on that
  connection — independent of whether `SESSION_READ_YOUR_WRITES` is
  also set — records the *raw, committed* `(id, field) → value` pairs
  `dispatch` actually returned, before any read-your-writes overlay,
  into the session's own read set. A second read of the same
  `(id, field)` replaces the earlier entry — the read set always
  holds the most recent value this connection has actually seen for
  each key, which is the correct baseline for "has this changed since
  I last looked."
- `ISO-FR-003` — **Validated and applied atomically at `Commit`.**
  When the read set is non-empty, `Commit` passes it to
  `ConnectionStore::apply_transaction` alongside the staged batch.
  Inside the *same* exclusive section the batch's own write validation
  and apply already use, the adapter re-reads each tracked
  `(id, field)` and compares it to the recorded value; any mismatch
  refuses the whole commit — nothing applied, exactly the "checked
  and applied together, or neither" guarantee the journal already
  gives the write side, extended to cover the read side too. Reported
  as `Response::TransactionFailed { index: 0, code: ErrorCode::Conflict }`
  — the same sentinel-index shape `ErrorCode::Journal` established
  for a failure that is not about one specific staged operation.
- `ISO-FR-004` — **Bounded read-set size.** `pub const
  MAX_TRACKED_READS: usize = 4096` (the `MAX_STAGED_OPS` precedent: a
  constant, not a config, until a real report says otherwise). Past
  the cap, a `GetById` for a *new* `(id, field)` key is simply not
  added to the read set — existing tracked keys keep updating on
  re-read, the request itself still succeeds and still returns a
  value, and `Commit` still runs; the session just loses the ability
  to detect a conflict on whatever went untracked. A graceful
  degradation of the guarantee, never a refused read or a failed
  session.
- `ISO-FR-005` — **Only a found `GetById` is tracked.** A `GetById`
  that returns `Response::NotFound` adds nothing to the read set — no
  phantom-read protection this round (see "Non-goals"); named, not
  silently assumed.
- `ISO-FR-006` — **`ConnectionStore::apply_transaction` grows one
  parameter**: `read_set: &[(RecordId, FieldRef, ScanValue)]`, empty
  whenever snapshot isolation is off — the identical "no branch taken,
  no cost" shape `validate_op`'s own addition established at `STV-FR`.
  Every implementor (`DogConnectionStore`, `OrderConnectionStore`,
  `EmployeeConnectionStore`, and this crate's own test doubles) is
  updated; no external implementor of this trait exists.
- `ISO-FR-007` — **Composes with the other two bits.** A session may
  set any combination of `SESSION_READ_YOUR_WRITES`,
  `SESSION_VALIDATE_ON_STAGE`, and `SESSION_SNAPSHOT_ISOLATION`
  independently; each mechanism reads its own state and none
  changes another's behavior. In particular, validating a staged
  write (`STV-FR`) and validating the read set (this design) are two
  different checks — a session can catch a bad *write* at stage time
  and a stale *read* at commit time, on the same batch.
- `ISO-FR-008` — **Cost and compatibility.** With the bit unset (the
  default), the read set is never populated, `apply_transaction`'s
  new parameter is always an empty slice, and the cost is one
  `Option`/bit check per `GetById`, matching every prior opt-in
  addition's own "no branch, no cost" bar. No `Cargo.toml`, store, or
  `serve`-signature change; the one wire addition is `ErrorCode::Conflict`
  and the flag bit, both version-gated exactly as `ADR-0022`'s
  compatibility rules require.

## Considered options

**How to give a session isolation from concurrent commits.**

1. **Optimistic read-set validation at `Commit` — proposed.** Cheap
   (one comparison per tracked read, paid once, at commit), no
   storage-engine change, reuses the exact atomic check-then-apply
   window the journal already established. Detects conflicts; does
   not prevent them from happening, only from being committed over.
2. **Hold a shared read lock for the whole session.** Trivially
   correct — nothing can change under a lock — but a session spans
   multiple client round trips with no bound on how long it stays
   open, so this would block every writer on the store for as long as
   any snapshot-isolated session is open. Rejected: this is exactly
   the failure mode `ADR-0024` itself named as the reason the
   session design holds no lock at all except at `Commit` for the
   interval a plain `Transaction` already holds it (*"The only lock a
   session ever takes is `apply_transaction`'s own at `Commit`"*) —
   this option would be the first thing in this crate's whole history
   to hold a lock across a network round trip.
3. **Real MVCC — versioned records, a snapshot pointer at `Begin`,
   garbage collection of old versions.** The correct, general answer,
   and `docs/FUTURE-GROWTH.md`'s own named "multi-year effort."
   Rejected this round on proportionality: it is a storage-layer
   rewrite (`src/store/**` has never changed for any `SERVER-001`
   round to date, a fact this spec's own acceptance criteria have
   repeatedly verified), needs a real vacuum/compaction story this
   crate has never needed before, and nothing has asked for full
   repeatable-read or phantom-read protection yet — named as the real
   answer if optimistic detection turns out not to be enough.
4. **Close as not warranted — restate `SESS-FR-007`'s gap, build
   nothing.** A legitimate, smaller-footprint choice; the owner may
   still take it.

**Where the read set lives.**

1. **Per-connection state in `handle_connection`, alongside `session`/
   `read_your_writes`/`validate_on_stage` — proposed.** Matches every
   other session-scoped mechanism already there; no new type needed
   beyond a small map.
2. **Inside `ConnectionStore` itself.** Rejected: the store has no
   concept of a connection or a session today, and giving it one
   would be a far bigger change than tracking a handful of key-value
   pairs on the connection's own stack.

**Failure shape for a conflict.**

1. **`Response::TransactionFailed { index: 0, code: Conflict }` —
   proposed.** Reuses `ErrorCode::Journal`'s own precedent exactly: a
   commit-level failure that is not about one specific staged
   operation gets index 0 and its own code, not a fabricated index
   into the write batch.
2. **A new `Response` variant naming which read conflicted.** Would
   leak which record/field triggered the conflict on a Wire response
   to a client whose own token might only cover `ReadOnly` for that
   record — no case has asked for this level of detail yet, and it
   grows the wire surface for a diagnostic a client can already get
   by re-reading; rejected, named as an open question if ever wanted.

## Proposed shape

```rust
// src/server/protocol.rs
pub const SESSION_SNAPSHOT_ISOLATION: u32 = 4;   // third BeginWith bit
pub const PROTOCOL_VERSION: u32 = 7;             // ISO-FR-001

pub enum ErrorCode {
    // ...unchanged variants...
    Conflict,   // ISO-FR-003: a tracked read changed before Commit
}

// src/server/mod.rs
pub const MAX_TRACKED_READS: usize = 4096;   // ISO-FR-004

// per-connection state, alongside session/read_your_writes/validate_on_stage
let mut snapshot_reads: Option<HashMap<(RecordId, FieldRef), ScanValue>> = None;

// at BeginWith: snapshot_reads = (flags & SESSION_SNAPSHOT_ISOLATION != 0).then(HashMap::new);
// at Commit/Rollback: snapshot_reads = None;

// the GetById session intercept gains a second, independent branch
// (or the existing RYW branch also records into snapshot_reads when it's Some):
// after dispatch(store, GetById { id }) returns Response::Record { id, fields },
// for each (field, value) in &fields, if snapshot_reads.is_some() and under
// MAX_TRACKED_READS distinct keys, snapshot_reads[(id, field)] = value.clone();
// (recorded BEFORE the RYW overlay, if any, is applied)

// Commit:
let read_set: Vec<(RecordId, FieldRef, ScanValue)> = snapshot_reads
    .take()
    .map(|m| m.into_iter().map(|((id, f), v)| (id, f, v)).collect())
    .unwrap_or_default();
match store.apply_transaction(&batch, &read_set) {
    Ok(()) => Response::Ok,
    Err((index, code)) => Response::TransactionFailed { index, code },
}

// trait ConnectionStore (src/server/mod.rs)
fn apply_transaction(
    &self,
    updates: &[TransactionOp],
    read_set: &[(RecordId, FieldRef, ScanValue)],   // ISO-FR-006, empty when unused
) -> Result<(), (usize, ErrorCode)>;

// each adapter's apply_transaction, inside its existing with_exclusive section,
// gains one step before applying: for (id, field, expected) in read_set, re-read
// the current value and compare; on any mismatch, return Err((0, ErrorCode::Conflict))
// without applying anything — the exact place validate_batch's own existence
// check already runs, just one step earlier.
```

## Data/state and invariants

- The read set is per-connection, per-session, in-memory only — never
  persisted, never journaled, cleared on `Commit` or `Rollback` exactly
  like `read_your_writes`/`validate_on_stage`.
- A tracked entry's value is always the *most recently read* value for
  that `(id, field)` on this connection — re-reading the same key
  updates it, so `Commit` only ever compares against what the session
  actually last saw, not a stale first-read snapshot.
- `apply_transaction`'s read-set check and its write validate/apply run
  under the same lock acquisition (the adapter's existing
  `with_exclusive`/`with_journal` section) — no window exists between
  "read set confirmed unchanged" and "batch applied" for another
  commit to land. This is the same atomicity property `JRN-FR-002`
  already gives the write side, and this design adds nothing new to
  reason about there — it is the same critical section, one more
  check inside it.
- `MAX_TRACKED_READS` bounds memory, never correctness: a session
  whose read set overflowed the cap still commits or fails on exactly
  the keys it did track: it never falsely reports a conflict, and
  never falsely claims a guarantee it isn't providing.

## Errors, failure, recovery, and observability

- `ErrorCode::Conflict` is a normal, typed refusal — the connection
  stays open, the session is cleared (matching every other `Commit`
  outcome), and the client may re-read and retry a fresh session.
- No new sink, no new gate: `Commit`'s outcome (Ok or
  `Err(ErrorCode::Conflict)`) already flows through the existing
  access-log machinery unmodified — `outcome_of` already maps every
  `Response::TransactionFailed { code, .. }` to `Outcome::Err(code)`
  (`ACC-FR-004`'s existing exhaustive match), so a conflict is visible
  in an operator's access log the moment this ships, with zero new
  code in `access.rs`. The audit log is unaffected — it records
  admission/authentication/authorization decisions, not dispatched-
  request outcomes, and `Commit` is a dispatched request either way.

## Security, privacy, and compatibility

- No new secret, id, or value crosses the wire that a session
  couldn't already see — the read set is built entirely from values
  the connection's own `GetById` calls already returned to it, and
  `Conflict` names no record, field, or value in its response
  (`ISO-FR-003`'s failure shape, "Considered options" above).
- Backward compatible by construction: `SESSION_SNAPSHOT_ISOLATION`
  unset is byte-for-byte today's behavior (`ISO-FR-008`); a pre-v7
  connection cannot request it and never sees `ErrorCode::Conflict`.

## Acceptance criteria

1. `SESSION_SNAPSHOT_ISOLATION`/`PROTOCOL_VERSION = 7`/`ErrorCode::Conflict`
   exist exactly as specified; the version table and golden vectors
   updated; `flags: 7` composes all three bits; an unknown bit stays
   `Malformed`; the bit is `Malformed` below version 7.
2. A session's `GetById` on `id`/`field`, followed by a *different*
   connection committing a write to that same `id`/`field`, followed
   by this session's own unrelated `Commit`, fails with
   `Response::TransactionFailed { index: 0, code: Conflict }` and
   applies nothing — including when the session's own batch touches
   entirely different records than the one it read.
3. The same sequence with no intervening write from anywhere commits
   normally — a snapshot-isolated session pays no false conflicts.
4. A `GetById` returning `NotFound` is not tracked (no false conflict
   when that id is later created by someone else) — `ISO-FR-005`.
5. `MAX_TRACKED_READS` holds under more distinct reads than the cap:
   the session keeps working, keeps committing/failing correctly on
   whatever it did track, and never fails purely because it read too
   much.
6. `SESSION_READ_YOUR_WRITES` and `SESSION_SNAPSHOT_ISOLATION`
   together: the read set records the raw, pre-overlay value, and a
   session sees its own staged writes exactly as before while still
   being protected from everyone else's commits.
7. With the bit unset, every existing test in
   `tests/server_transaction_integration.rs`/
   `tests/server_schema_driven_client.rs`/`tests/server_protocol_version.rs`
   is unchanged — `apply_transaction`'s new parameter is always empty
   and changes no existing outcome.
8. No `Cargo.toml`, store (`src/store/**`), or `serve`-signature
   change; `ConnectionStore` gains one parameter on one existing
   method, implemented identically in shape by all three adapters.

## Verification plan

- `src/server/mod.rs` unit tests: the read-set-recording branch
  (records raw value, not the RYW-overlaid one; `NotFound` untracked;
  a repeated read replaces the earlier entry; the `MAX_TRACKED_READS`
  cap degrades gracefully).
- Each adapter (`dog.rs` at minimum, the pattern shared by
  `order.rs`/`employee.rs`): a unit test driving `apply_transaction`
  directly with a read set that does and does not match current
  state, confirming atomicity with the existing write-validation step
  (a conflicting read set refuses the batch even when every write in
  it would itself have been valid).
- `tests/server_transaction_integration.rs`: a real two-connection
  test — connection A reads, connection B commits a conflicting
  write, connection A commits and gets `Conflict`; a second test
  where B's write does not conflict and A commits normally; a third
  combining `SESSION_READ_YOUR_WRITES` and `SESSION_SNAPSHOT_ISOLATION`
  on one session.
- `tests/server_protocol_version.rs`: the version pin (7), the
  unknown-bit-below-7 gate, the golden vector for the new flag.

## Traceability

- → `SERVER-001` next minor / FR (`ISO-FR-001`–`008`), `ADR-0033`;
  closes `SESS-FR-007`'s second half (isolation from other
  connections) that `ADR-0027` left standing; answers the owner's
  "real transactions" pick from `docs/FUTURE-GROWTH.md`'s "Path to
  SQLite/DuckDB parity" section, narrowed to the bounded slice this
  document scopes — full MVCC and phantom-read protection remain open.
- Roadmap: `SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN` (this document),
  then `SERVER-SESSION-SNAPSHOT-ISOLATION` as the implementation unit
  if accepted.

## Open questions

- Whether phantom-read protection (a `NotFound` read later
  contradicted by a create) is ever wanted — named, not solved; would
  need a range or existence lock this design does not build.
- Whether `FilterEq`/`ScanField` isolation is ever wanted enough to
  justify its own re-validation cost at `Commit` — a real, separate
  design if it comes up, matching `RYW-FR`'s own identical open
  question for its overlay.
- Whether real MVCC is ever warranted — the answer if optimistic
  detection's "refuse and let the client retry" posture turns out not
  to be enough for a real workload; not decided here.
- Whether a conflicted commit should report *which* read conflicted —
  rejected this round on the same secrecy-shape grounds `AUD-FR`
  established for the audit log elsewhere; open if a real caller asks.

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting
  "real transactions" from `docs/FUTURE-GROWTH.md` as the direction to
  pursue, scoped down from that document's own "MVCC or log-based
  design" framing to the bounded, session-machinery-native slice this
  document proposes.
