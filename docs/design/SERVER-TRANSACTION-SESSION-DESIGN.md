# Server Transaction Session and Crash-Atomic Batch Design (Accepted)

- Status: **Accepted** (both parts promoted from Proposed on 2026-09-02 —
  the owner approved each as proposed, option (a) of both `ADR-0024`
  and `ADR-0025`; the lock-held session, closing either, and always-on
  journaling declined; no changes requested). Two decisions, each
  accepted on its own: **Part A**, a multi-round-trip transaction
  session (`ADR-0024`); **Part B**, crash-atomicity for a committed
  batch (`ADR-0025`). Acceptance authorizes each design; implementation
  follows as its own unit per part — see each ADR's "Acceptance and
  implementation" section.
- Date: 2026-09-02
- Related: `docs/design/SERVER-TRANSACTION-DESIGN.md` / `ADR-0013`
  (`Request::Transaction`, `SERVER-001` v0.7.0 / FR-017 — whose first
  two revisit triggers, *a true multi-round-trip interactive session*
  and *crash-atomicity across a batch*, this document answers),
  `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md` / `ADR-0022`
  (`PROTOCOL_VERSION = 2`, the append-only rules, and the named trigger
  *the first version-3 variant* — Part A is that variant),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.13.0,
  `src/durability/hybrid.rs` (the snapshot-plus-never-truncated-WAL
  shape Part B's journal borrows), `src/durability/mmap_store.rs` and
  `docs/design/MULTI-FIELD-MMAP-DURABILITY-DESIGN.md` / `STORAGE-017`
  (the in-place, per-slot, `COMMITTED`-marker durability a batch is
  made of), `docs/FUTURE-GROWTH.md` items 1–2.

## Purpose and scope

`SERVER-001` v0.7.0 delivered one bounded slice of "transactions": a
batch of `UpdateField`-shaped writes, known up front and sent in one
request, applied all-or-nothing under one continuously held exclusive
lock. `ADR-0013` named the two things it deliberately did not deliver
and made each a revisit trigger:

1. *A true multi-round-trip interactive session* — open a transaction,
   issue several requests against it, then commit or roll back. Rejected
   then because holding the exclusive lock across an unbounded number of
   client round trips is a liveness risk this project has never
   accepted anywhere.
2. *Crash-atomicity across a batch* — a process crash between two of a
   batch's slot writes reaching disk leaves a partial batch durably
   applied. Rejected then as "a combined-journal or two-phase-commit-
   style mechanism, a real durability redesign."

The owner selected both as one design round. This document takes each
in turn. The two are independent: a session without journaling is
exactly as durable as today's `Transaction`; journaling without a
session hardens today's `Transaction` alone. They are presented
together because they share a vocabulary (a *batch*: the ordered
`Vec<TransactionOp>` that `apply_transaction` validates then applies)
and because Part B's journal is written at the one moment both paths
already converge on: the instant before `apply_transaction` runs.

**In scope:**

- **Part A.** A session that gives the multi-round-trip *shape* without
  holding any lock across round trips: the server *stages* a
  connection's writes in a per-connection buffer and applies them at
  commit exactly as one `Transaction`. Three new requests
  (`Begin`/`Commit`/`Rollback`), one new response (`Staged`), three new
  error codes, `PROTOCOL_VERSION` 3, the per-connection negotiated
  version `ADR-0022` deferred until now, and `SchemaDrivenClient`'s
  first version-gated API.
- **Part B.** A redo journal for batches: the batch is appended and
  `fsync`'d before it is applied, replayed on the next open if the
  process died in between, and checkpointed (mmap `flush` then truncate)
  by size and at open. Opt-in at the domain adapter, no wire change, no
  storage-format change.

**Explicitly out of scope, named directly:**

- A lock-held interactive session (the literal FUTURE-GROWTH framing).
  Weighed and rejected again, for `ADR-0013`'s reason; see Part A's
  considered options for why the buffered shape is not a lesser version
  of it but the one this server's concurrency model can honor.
- Read-your-writes inside a session. A read issued between `Begin` and
  `Commit` sees committed state only. Named as a consequence, not
  hidden; the alternative is a per-connection overlay on every read
  path, a real cost for a use no caller has asked for.
- Any operation kind beyond `UpdateField` in a session or a journal
  entry — today's protocol has exactly one mutating kind, unchanged
  since `ADR-0013`.
- Isolation levels, MVCC, snapshot reads; nested sessions; savepoints;
  cross-domain sessions. Unchanged from `SERVER-TRANSACTION-DESIGN.md`'s
  own list.
- Durability for a single `UpdateField`. Its torn-write safety is the
  per-slot `COMMITTED` marker (`STORAGE-011`/`STORAGE-017`) and its
  page write-back is the OS's, with `flush` to force it. Part B does
  not journal single writes; it journals *batches*, whose atomicity
  is the only guarantee at stake.
- A general durability upgrade (`fsync` on every write, a full WAL for
  all traffic). `docs/FUTURE-GROWTH.md` item 2 names "a real
  transaction manager — likely its own MVCC or log-based design"; Part
  B is the smallest log that makes today's batch guarantee survive a
  crash, not that manager.

## Non-goals

- Not ACID. Part A adds no durability; Part B adds crash-atomicity for
  a batch, not general durability, and only against a crash of *this
  process* on a filesystem that honors `fsync`/`msync`.
- Not a change to `dispatch`'s contract, `ConnectionStore::apply_transaction`'s
  signature, or any store's locking. Part A's commit *is* an
  `apply_transaction` call; Part B wraps that call.
- Not a fairness or preemption story for the existing thread-per-
  connection model. Part A is designed so it needs none.

## Context and terminology

- **Batch.** `Vec<TransactionOp>`, ordered; validated then applied by
  `ConnectionStore::apply_transaction` under one `with_exclusive`
  closure; failure names the first failing index. Both `Request::Transaction`
  (today) and a session's `Commit` (Part A) produce one.
- **Staged write.** A `TransactionOp` held in a per-connection buffer
  between `Begin` and `Commit`, not yet validated against the store and
  not applied. No lock is held while anything is staged.
- **Negotiated version.** `min(client, PROTOCOL_VERSION)` from the
  first-frame `Hello`, or 1 for a silent client (`PROTO-FR-004`). At
  version 2 the server keeps only "was the first frame read"; the first
  gated variant is what makes it keep the number (`ADR-0022`'s trigger).
- **The durability a batch is made of.** Each `UpdateField` is one
  in-place write of a fixed-width slot in an `.mmap` file, keyed by
  record id, with a `COMMITTED` marker; the OS writes pages back
  asynchronously and `flush` (`msync`) forces it. A batch is N such
  writes under one lock. Between the first and the last reaching disk
  there is a window in which a crash leaves some of the N durable and
  the rest not — the exact gap `TXN-FR-007` named.
- **Redo, not undo.** Every operation is an idempotent overwrite of a
  slot keyed by an id that is never deleted at runtime and a field whose
  type is fixed by the schema. Re-applying an already-applied operation
  is a no-op; applying one that never landed produces the same state it
  would have. A log of *intended* writes, replayed in order, therefore
  restores the all-or-nothing outcome with no record of prior values —
  the reason `ADR-0013` could reject an undo log then and this design
  can still reject one now.
- **`hybrid.rs`'s shape.** Append per write, never truncate the WAL
  except at a checkpoint that first makes the snapshot durable, replay
  everything after the last checkpoint on open. Part B's journal is that
  shape with the batch as the unit and the `.mmap` files as the
  snapshot.

---

## Part A — a buffered transaction session (`ADR-0024`)

### Requirements

- `SESS-FR-001`: Three new request variants, appended at the next
  indices (`Begin` 11, `Commit` 12, `Rollback` 13), one new response
  variant (`Staged { index: u32 }` at 11), and three new error codes
  (`NoSession` 6, `SessionOpen` 7, `SessionFull` 8), all introduced at
  protocol version 3; `PROTOCOL_VERSION` becomes 3; the version table
  and golden vectors are extended; every version-1 and -2 vector is
  unchanged (`ADR-0022` rules 1–2).
- `SESS-FR-002`: `Begin` opens a session on the connection: an empty
  per-connection buffer. While a session is open, every `UpdateField`
  the auth gate admits is appended to the buffer and answered
  `Staged { index }` — its position in the eventual batch — with
  nothing applied, no lock taken, and no validation against the store.
  `Commit` turns the buffer into one batch and calls
  `apply_transaction` exactly as `Request::Transaction` does, answering
  `Ok` or `TransactionFailed { index, .. }` (the staged index), and
  closes the session either way. `Rollback` discards the buffer and
  closes the session. Disconnecting with a session open discards it.
- `SESS-FR-003`: **No lock across round trips.** The only lock a session
  ever takes is `apply_transaction`'s own, at `Commit`, for exactly the
  duration a `Request::Transaction` of the same batch would take. A
  connection that opens a session and stalls holds nothing any other
  connection waits on.
- `SESS-FR-004`: Session state errors are typed, and the connection
  stays open: `Commit`/`Rollback` with no session → `NoSession`;
  `Begin` with one open → `SessionOpen`; `Request::Transaction` with one
  open → `SessionOpen` (one batch at a time per connection); the
  buffer at `MAX_STAGED_OPS` (a `pub const`, proposed 4096) →
  `SessionFull`, the offending write not staged, the session still open.
- `SESS-FR-005`: Gating is unchanged and applies per request: staging
  an `UpdateField` and `Commit` require `TokenClass::ReadWrite`
  (`AUTH-FR-003`/`TXN-FR-004`); `Begin` and `Rollback` require only an
  authenticated connection. `Authenticate` mid-session is allowed and
  takes effect for later requests, as today.
- `SESS-FR-006`: **Version gating, both directions** — the first use of
  `ADR-0022` rules 3 and 4. `handle_connection` keeps the negotiated
  version per connection; a `Begin`/`Commit`/`Rollback` on a connection
  negotiated below 3 (including a silent, version-1 client) is
  `Malformed`, and `Staged` and the three new codes are never sent on
  such a connection (they cannot arise without a `Begin`). `dispatch`
  maps the three to `Unsupported`, as it maps `Authenticate` and
  `Hello`. `SchemaDrivenClient` refuses to start a session unless
  `server_protocol_version() >= 3`, with `ClientError::Unsupported`.
- `SESS-FR-007`: Reads inside a session (`GetById`, `ScanField`, …) are
  served against committed state, unchanged; staged writes are
  invisible to every connection, this one included, until `Commit`.
  Documented on `Begin` and in the client library.
- `SESS-FR-008`: `SchemaDrivenClient` gains `begin() -> Result<Session<'_>, ..>`,
  with `Session::update(id, field_name, value) -> Result<u32, ..>`
  (the staged index; the client-side capability checks of `update`
  still apply first), `Session::commit(self)`, `Session::rollback(self)`;
  `Drop` sends a best-effort `Rollback` if neither was called. No
  existing client method changes shape.
- `SESS-FR-009`: `SERVER-001` goes to v0.14.0 with FR-024; no
  `Cargo.toml` change; `framing.rs`, the codec, and every existing
  suite unchanged (they are versions 1–2's regression test).

### Considered options

**What a session holds between round trips.**

1. **The exclusive lock** — `Begin` enters `with_exclusive` and the
   connection thread stays inside it until `Commit`/`Rollback`; every
   `UpdateField` applies immediately; `Rollback` needs an undo log.
   The literal "interactive transaction." Rejected: every other
   connection blocks on this one client's next frame; a stalled or
   malicious client is a denial of service the thread-per-connection
   model cannot preempt; an idle timeout would need a second thread or
   a socket timeout on every read, and a forced abort mid-`with_exclusive`
   would have to unwind the closure — all of `ADR-0013`'s reasons,
   still true. Offered as option (b) of `ADR-0024` only so the owner
   sees the price named, not because it is buildable safely here.
2. **A buffer of intended writes (proposed).** Staging costs a `Vec`
   push; committing is today's `Transaction`. Isolation is identical to
   `Request::Transaction`'s (full serialization for the batch's
   duration); atomicity is identical; the liveness profile is identical
   to any other request. What is lost relative to option 1 is
   read-your-writes and stage-time validation, both named.
3. **Client-side batching only** — no server change; `SchemaDrivenClient`
   accumulates a `Vec<TransactionOp>` and sends one `Transaction`. The
   status quo, made ergonomic. Rejected as the proposal because it
   gives a raw-protocol client nothing and a library client nothing it
   cannot already write in three lines; offered as option (c) of
   `ADR-0024` (close as not warranted) since it is a legitimate answer.

**Where staged writes are validated.**

1. **At commit only (proposed)** — `apply_transaction` already validates
   every operation and names the first failure by index; `Staged { index }`
   gives the client the correlation. No trait change.
2. **At stage time, against the schema** — check field/type via
   `describe()` and existence via `get()` per staged write. Rejected:
   duplicates commit validation, adds a `ConnectionStore` obligation or
   a per-connection schema lookup on the hot path, and cannot make
   commit validation unnecessary (the batch is still re-checked under
   the lock). A client wanting early feedback can `GetById` first, as
   today.

**Where the session lives.**

1. **In `handle_connection` (proposed)** — `Option<Vec<TransactionOp>>`
   beside `authenticated`, intercepted before `dispatch` exactly as
   `Authenticate` and `Hello` are; `Commit` calls
   `store.apply_transaction`. `dispatch` and every adapter unchanged.
2. **In `ConnectionStore`** (`begin`/`stage`/`commit` methods). Rejected:
   a store has no per-connection identity; it would need a session id
   and a map, a second piece of shared mutable state for no benefit.

### Proposed shape

`src/server/protocol.rs`:

```rust
pub const PROTOCOL_VERSION: u32 = 3;
pub const MAX_STAGED_OPS: usize = 4096;

pub enum Request { …, Hello { .. } /* 10 */, Begin /* 11 */, Commit /* 12 */, Rollback /* 13 */ }
pub enum Response { …, Hello { .. } /* 10 */, Staged { index: u32 } /* 11 */ }
pub enum ErrorCode { …, RecordNotFound /* 5 */, NoSession /* 6 */, SessionOpen /* 7 */, SessionFull /* 8 */ }
```

Version table row 3: `+ Request::{Begin, Commit, Rollback} (11–13),
Response::Staged (11), ErrorCode::{NoSession, SessionOpen, SessionFull}
(6–8)`. Golden vectors for each (`Begin` is `0b 00 00 00`; `Staged { 2 }`
is `0b 00 00 00 02 00 00 00`).

`src/server/mod.rs`, `handle_connection` — after the `Hello` intercept:

```rust
let mut negotiated: u32 = 1;                 // set by the Hello intercept
let mut session: Option<Vec<TransactionOp>> = None;
…
// after the auth gate and the ReadOnly gate (Commit joins UpdateField/Transaction there):
match req {
    Request::Begin | Request::Commit | Request::Rollback if negotiated < 3 => Malformed,
    Request::Begin => if session.is_some() { SessionOpen } else { session = Some(Vec::new()); Ok },
    Request::Rollback => if session.take().is_some() { Ok } else { NoSession },
    Request::Commit => match session.take() {
        None => NoSession,
        Some(batch) => match store.apply_transaction(&batch) { Ok(()) => Ok, Err((i, c)) => TransactionFailed { i, c, .. } },
    },
    Request::UpdateField { id, field, value } if session.is_some() => {
        let buf = session.as_mut();
        if buf.len() >= MAX_STAGED_OPS { SessionFull } else { buf.push(TransactionOp { id, field, value }); Staged { index } }
    }
    Request::Transaction { .. } if session.is_some() => SessionOpen,
    other => dispatch(store, other),
}
```

`src/server/client.rs`:

```rust
impl SchemaDrivenClient {
    /// Requires `server_protocol_version() >= 3` (`ClientError::Unsupported("session")` otherwise).
    pub fn begin(&mut self) -> Result<Session<'_>, ClientError>;
}
pub struct Session<'a> { client: &'a mut SchemaDrivenClient, open: bool }
impl Session<'_> {
    pub fn update(&mut self, id: RecordId, field_name: &str, value: ScanValue) -> Result<u32, ClientError>;
    pub fn commit(self) -> Result<(), ClientError>;     // TransactionFailed → ClientError::Server(code, msg) carrying the index in the message
    pub fn rollback(self) -> Result<(), ClientError>;
}
impl Drop for Session<'_> { /* best-effort Rollback if still open; errors ignored */ }
```

A `TransactionFailed` at commit needs a client-side shape naming the
index; `ClientError` gains `TransactionFailed { index: u32, code: ErrorCode, message: String }`
(one additive variant, the FR-022 precedent) rather than folding the
index into a string.

### Data/state and invariants

- Per-connection state grows by one `u32` (negotiated version) and one
  `Option<Vec<TransactionOp>>` bounded by `MAX_STAGED_OPS`; nothing is
  shared across connections; nothing survives the connection.
- Invariant: no lock is held by a connection while `session.is_some()`
  except inside `apply_transaction` during `Commit` — the same interval
  a `Request::Transaction` holds it.
- Invariant: a staged write is never observable, by any connection,
  before `Commit` returns `Ok`.
- Invariant: on a connection negotiated below 3, no version-3 variant
  is ever sent by the server.

### Errors, failure, recovery, and observability

- Every session-state misuse is a typed error with the connection open
  (`SESS-FR-004`); a `Commit` failure reports the staged index and
  leaves nothing applied, exactly as `Transaction` does.
- A client disconnect discards the buffer; nothing to recover, nothing
  to log (the server logs nothing per connection today).
- A version-3 request on an older connection is `Malformed` with the
  connection open, the `Hello`-misuse precedent.

### Security, privacy, and compatibility

- No new liveness or resource risk beyond `MAX_STAGED_OPS × size_of::<TransactionOp>`
  per connection — bounded, and smaller than one `MAX_FRAME_BYTES` frame.
- The auth gate is unchanged and evaluated per request; a session
  cannot stage what the connection could not apply.
- Backward compatible by construction: a version-1/2 client sees no
  change; every existing suite and `benches/server.rs` are unchanged.

### Acceptance criteria (Part A)

1. Golden vectors for `Begin`/`Commit`/`Rollback`/`Staged`/the three
   codes; every version-1/2 vector unchanged; `PROTOCOL_VERSION == 3`.
2. Over a real socket: `Begin`, three `UpdateField`s answered
   `Staged { 0..3 }` with a concurrent connection's reads unchanged
   throughout, `Commit` → `Ok`, the writes now visible to every
   connection.
3. `Commit` with an invalid staged operation → `TransactionFailed`
   naming its staged index, nothing applied, session closed.
4. `Rollback` discards; a disconnect with a session open discards; a
   second `Begin` → `SessionOpen`; `Commit`/`Rollback` without one →
   `NoSession`; `Transaction` inside a session → `SessionOpen`;
   `MAX_STAGED_OPS + 1` writes → `SessionFull` with the session intact.
5. A `ReadOnly` connection: `Begin` ok, `UpdateField` `Unauthorized`
   (nothing staged), `Commit` `Unauthorized`.
6. A silent (version-1) client and a version-2 `Hello` client each get
   `Malformed` for `Begin`, connection open.
7. `SchemaDrivenClient::begin` on a version-3 server round-trips a
   session; against a server reporting 2 it is `Unsupported` without a
   frame sent; `Session`'s `Drop` rolls back.
8. Every pre-existing suite and bench unchanged; no `Cargo.toml` change.

### Verification plan (Part A)

- `tests/server_transaction_integration.rs` gains a session section
  for criteria 2–6 on the `Dog` domain; the concurrent-visibility check
  in criterion 2 reuses the file's existing multi-connection harness.
- `tests/server_protocol_version.rs` gains criterion 6 (the first real
  use of rule 3/4 gating) and a version-2-pinned client check: a client
  that says `Hello { 2 }` negotiates 2 and cannot `Begin`.
- `tests/server_schema_driven_client.rs` gains criterion 7 on all three
  domains.
- No benchmark: staging is a `Vec` push; commit is the measured
  `Transaction` path (FR-018).

---

## Part B — crash-atomic batches via a redo journal (`ADR-0025`)

### Requirements

- `JRN-FR-001`: A domain adapter may be constructed with a **batch
  journal** (`DogConnectionStore::with_journal(store, path)`, and the
  same on `OrderConnectionStore`/`EmployeeConnectionStore`). Without
  one, behavior is exactly v0.13.0's (`TXN-FR-007`'s named gap stands).
- `JRN-FR-002`: With a journal, `apply_transaction` **appends the
  validated batch to the journal and `fsync`s it before applying any
  write**, inside the same `with_exclusive` closure (validate → append
  + `fsync` → apply). A batch that fails validation is never journaled.
- `JRN-FR-003`: **Replay on open.** `with_journal` reads the journal
  before serving: every complete entry is re-applied in order (an
  idempotent overwrite per operation), the store is `flush`ed, and the
  journal is truncated. A torn trailing entry (a crash mid-append,
  before the `fsync` returned) is detected by its length prefix or
  decode failure and discarded — it was never acknowledged.
- `JRN-FR-004`: **Checkpoint by size.** When the journal exceeds
  `JOURNAL_CHECKPOINT_BYTES` (a `pub const`, proposed 1 MiB) after an
  append, the adapter `flush`es the store (`msync` of every `.mmap`
  file in the stack — `Flush` is own-then-inner) and truncates the
  journal, still inside the exclusive section. The journal therefore
  holds exactly the batches since the last point at which every slot
  write was known durable.
- `JRN-FR-005`: **The guarantee.** After a crash of the server process
  at any instant, the next `with_journal` open leaves every committed
  batch (one whose `Commit`/`Transaction` was answered `Ok`) either
  fully applied or fully replayed, never partially — provided the
  filesystem honors `fsync`/`msync`. A batch whose `Ok` was never sent
  may or may not be present, exactly as a single `UpdateField` today.
- `JRN-FR-006`: Journal format: magic `TXNJRNL\0`, `u32` format version
  1, then entries `[u32 len][codec(Vec<TransactionOp>)]` — `crate::codec`,
  the encoding `STORAGE-018` pins, so a journal written by one build
  replays under the next per `STORAGE-018`'s rules. `TransactionOp`
  already has a golden vector.
- `JRN-FR-007`: Single `UpdateField`s are not journaled; `Request::Transaction`
  and a session `Commit` (Part A) are — they are the only batches.
- `JRN-FR-008`: No wire change, no `PROTOCOL_VERSION` change, no
  storage-format change (`GMMAPST\0` 2, the ages file, the blobs), no
  `Cargo.toml` change, no `ConnectionStore` signature change. The
  journal is a sidecar file the adapter owns.
- `JRN-FR-009`: `SERVER-001` goes to v0.15.0 with FR-025 (or v0.14.0 if
  Part A is declined); `TXN-FR-007` and `SERVER-001`'s v0.7.0 open
  question are resolved by pointer for journaled adapters and restated
  for unjournaled ones.

### Considered options

**Mechanism.**

1. **Redo journal of intended writes (proposed).** Correct because every
   operation is an idempotent overwrite keyed by an undeletable id (see
   "Context"); one `fsync` per batch; replay is `apply` without
   `validate`; no prior values recorded.
2. **Undo log** — record prior values, roll back on open. Rejected: needs
   a read of every target before the write, a second format for
   values, and buys nothing redo does not, given idempotence.
3. **Two-phase commit across the `.mmap` files** — a prepare marker per
   file. Rejected: a format change to every slot file (`STORAGE-011`,
   `-012`, `-017` all say the format is unchanged) for what a sidecar
   achieves.
4. **`flush` (`msync`) after every batch, no journal.** Rejected: an
   `msync` after the writes does not close the window *between* the
   writes; it only shortens the tail. The journal closes it by making
   the intent durable *before* the first write.
5. **Journal everything (single writes too).** Rejected: doubles the
   cost of the crate's headline nanosecond write for a guarantee a
   single slot write already has by construction (the `COMMITTED`
   marker); named as the future step if per-write `fsync` durability
   is ever wanted (`hybrid.rs` variant 1 already measures it).

**Placement.**

1. **The domain adapter, opt-in (proposed).** The adapter is the only
   layer that both sees a batch (`apply_transaction`) and knows how to
   apply one operation to its store (it already maps `FieldRef` →
   `update_age`/`update_field`), so it is the only layer that can
   replay one. It also owns the store handle for `flush`. A
   `pub(crate)` `server::journal::BatchJournal { path, file, bytes }`
   helper is shared by the three adapters.
2. **The store (`ProductionStore`/`GenericProductionStore`).** Rejected:
   a store has no notion of `TransactionOp` or `FieldRef` and cannot
   replay a server-level operation without importing the server layer
   into `STORAGE-011`/`-012` — an inversion. A storage-level journal
   would need its own operation vocabulary and a second replay path.
3. **`serve`, as a fifth parameter.** Rejected: the journal belongs to
   one store/adapter pair (its path is derived from the store's), not
   to the listener; and `serve` is generic over `ConnectionStore`.

**Always-on vs. opt-in.** Proposed opt-in via the constructor, because
the cost is a real `fsync` per batch (hundreds of microseconds to
milliseconds, against FR-018's measured microseconds) and because the
crate's own `dog_server`, tests, and benches must keep their current
shape by default. `ADR-0025` offers always-on as option (b); the design
recommends against it for those two reasons but notes it is the safer
default for anyone who reads "transaction" as ACID.

### Proposed shape

```rust
// src/server/journal.rs (pub(crate))
pub const JOURNAL_MAGIC: &[u8; 8] = b"TXNJRNL\0";
pub const JOURNAL_FORMAT_VERSION: u32 = 1;
pub const JOURNAL_CHECKPOINT_BYTES: u64 = 1 << 20;

pub(crate) struct BatchJournal { file: File, path: PathBuf, len: u64 }
impl BatchJournal {
    pub(crate) fn open(path: &Path) -> Result<(Self, Vec<Vec<TransactionOp>>), JournalError>; // replays complete entries, drops a torn tail
    pub(crate) fn append(&mut self, batch: &[TransactionOp]) -> Result<(), JournalError>;   // write + fsync
    pub(crate) fn needs_checkpoint(&self) -> bool;
    pub(crate) fn truncate(&mut self) -> Result<(), JournalError>;                          // after the store flushed
}

// src/server/dog.rs (and order.rs, employee.rs)
impl DogConnectionStore {
    pub fn with_journal(store: ProductionStore, journal_path: &Path) -> Result<Self, JournalError>; // replay → flush → truncate
}
fn apply_transaction(&self, updates) {
    self.store.with_exclusive(|inner| {
        validate(updates)?;                                   // as today
        if let Some(j) = &self.journal { j.lock().append(updates)?; }   // fsync before the first write
        apply(updates);                                       // as today
        if journal.needs_checkpoint() { inner.flush()?; journal.truncate()?; }
        Ok(())
    })
}
```

A journal I/O failure at append is reported as the batch's failure
(`TransactionFailed { index: 0, code: ErrorCode::Io?, .. }`) with
nothing applied — a new `ErrorCode::Journal` (version-3 or, if Part A
is declined, version-3 anyway: it is the next index) is the honest
shape; the design proposes it and `ADR-0025` records it as the one wire
addition, gated like any appended code. `dog_server` gains an optional
`SERVER_TXN_JOURNAL_PATH` variable.

### Data/state and invariants

- The journal holds exactly the batches applied since the last
  checkpoint; the store's `.mmap` files are the snapshot. Replay of the
  journal onto the snapshot yields the post-commit state, whatever
  subset of the slot writes had reached disk.
- Invariant: an entry is fully on disk (`fsync` returned) before any
  operation of it is applied; an entry is truncated only after every
  operation of it is known durable (`flush` returned).
- The lock discipline is unchanged: append, apply, and checkpoint all
  happen inside the one `with_exclusive` closure the batch already
  held.

### Errors, failure, recovery, and observability

- Replay errors at open (unreadable file, bad magic/version, an
  operation the adapter cannot apply — impossible for a batch it
  validated, but checked) fail `with_journal`, never a panic; the
  adapter does not start on a journal it cannot honor.
- A torn tail is silent by design (it was never acknowledged).
- No per-batch logging; a checkpoint is not observable except by the
  journal's size.

### Security, privacy, and compatibility

- The journal contains record ids and field values in `codec` form —
  the same data the `.mmap` files hold, in the same directory, under
  the same permissions. No token, no key.
- Compatible by construction: no journal path, no change. A journaled
  adapter over an unjournaled store's files is fine (the journal starts
  empty); the reverse (opening without `with_journal` after a crash)
  simply forgoes replay — documented as the one way to lose the
  guarantee, and why `dog_server` reads the variable at every start.

### Acceptance criteria (Part B)

1. `with_journal` on all three adapters; `BatchJournal`'s format as
   specified with a golden header; a torn tail discarded.
2. A journaled `Transaction` and a journaled session `Commit` each
   append-then-apply; the journal holds the batch after `Ok`; a failed
   validation journals nothing.
3. **The crash test**: apply a batch through a journaled adapter, then
   discard the adapter *without* `flush` and, on a copy of the `.mmap`
   file taken mid-batch (a test hook that snapshots the file after the
   k-th slot write, or a batch whose first write is forced to page-out
   while later ones are not), reopen with `with_journal` and observe
   every operation applied. If a deterministic mid-batch snapshot
   cannot be produced without a test-only hook, the criterion is met
   by the pair: (i) a journal replayed onto a pristine copy of the
   pre-batch files yields the full post-batch state, and (ii) a journal
   replayed onto the post-batch files is a no-op — together they show
   replay is correct for every intermediate state, since each slot is
   independent.
4. Checkpoint: after `JOURNAL_CHECKPOINT_BYTES` the journal is empty
   and the store was flushed; replay after a checkpoint applies nothing.
5. Unjournaled adapters, every existing suite, and `benches/server.rs`
   unchanged; a `--features server,research` bench row for a journaled
   `Transaction` (FR-018's harness, one more row) records the `fsync`
   cost honestly in `RESULTS.md`.
6. No `Cargo.toml`, storage-format, or `ConnectionStore` signature
   change.

### Verification plan (Part B)

- `src/server/journal.rs` unit tests: header, round trip, torn tail,
  checkpoint threshold.
- `tests/server_transaction_integration.rs` gains a journaled section
  for criteria 2–4 (criterion 3 via the file-copy pair unless a hook is
  cheap).
- `benches/server.rs` gains the journaled row (criterion 5); `RESULTS.md`
  records it under the `Request::Transaction` follow-up.

---

## Traceability

- Part A → `SERVER-001` v0.14.0 / FR-024 (`SESS-FR-001`–`009`),
  `ADR-0024`; cites `ADR-0022` (the first version-3 variant) and
  resolves `ADR-0013`'s first trigger and `SERVER-TRANSACTION-DESIGN.md`'s
  last open question.
- Part B → `SERVER-001` v0.15.0 / FR-025 (`JRN-FR-001`–`009`),
  `ADR-0025`; resolves `ADR-0013`'s second trigger and `TXN-FR-007` for
  journaled adapters.
- Roadmap: `SERVER-TRANSACTION-SESSION-DESIGN` (this document), then
  `SERVER-TRANSACTION-SESSION` and `SERVER-TRANSACTION-JOURNAL` as
  separate implementation units, in the order the owner accepts them.

## Open questions

- `MAX_STAGED_OPS` (4096) and `JOURNAL_CHECKPOINT_BYTES` (1 MiB) are
  proposed constants, not measured; the journaled bench row is the
  place to revisit the second.
- Whether `Session::update` should also accept the same client-side
  capability check `update` does before staging (proposed yes — it is
  free) or defer everything to commit.
- Whether a journal I/O failure should close the connection rather than
  answer a typed error — proposed typed error, connection open, since
  the store is untouched.
- Read-your-writes inside a session: out of scope here; if ever wanted,
  a per-connection overlay on `GetById`/`ScanField` is the shape, at a
  real cost on every read path.
- Whether Part B's journal should one day absorb single writes
  (`hybrid.rs` variant 1's guarantee) — named, not proposed.

## Change history

- 2026-09-02: Initial proposal, in response to the owner selecting the
  transaction-session / crash-atomicity design round as the third of
  four next directions, after mTLS (`ADR-0023`, implemented as
  `SERVER-001` v0.13.0) and before the reconnect-without-hello fallback.
  (PR #137.)
- 2026-09-02: Both parts accepted as proposed ("1, 1"). No content
  change. Implementation order: Part A as `SERVER-001` v0.14.0 /
  FR-024, then Part B as v0.15.0 / FR-025. (PR #138.)
- 2026-09-02: Part A implemented as `SERVER-001` v0.14.0 / FR-024 (PR
  #139), per the verification plan: acceptance criteria 1–8 hold as
  written, no deviation. Criterion 7's "against a server reporting 2"
  half is covered by the raw-protocol gating test in
  `tests/server_protocol_version.rs` (a `Hello { 2 }` client), since no
  real server negotiates below its own version; the client-side gate is
  one comparison. `ADR-0024`'s acceptance log carries the same note.
