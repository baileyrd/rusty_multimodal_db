# Server Transaction Design (Proposed)

- Status: **Proposed** — not yet reviewed or accepted. Nothing in this
  document authorizes implementation; see
  `docs/decisions/ADR-0013-server-transaction-proposal.md`'s own
  "Decision" section.
- Date: 2026-09-01
- Related: `docs/decisions/ADR-0013-server-transaction-proposal.md` (the
  decision record this document backs), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (names this gap directly in its own Consequences: "no transaction
  semantics"), `docs/decisions/ADR-0012-server-authentication-proposal.md`/
  `docs/design/SERVER-AUTH-DESIGN.md` (the immediately preceding
  design-first-then-implement round this document follows the same
  cadence as), `docs/specifications/server/SERVER-001-query-layer.md`
  (the spec this design would extend, once accepted),
  `docs/FUTURE-GROWTH.md` (names "session/transaction semantics across
  multiple requests" as "genuinely new," not incremental)

## Purpose and scope

The owner picked three next directions for the server/query layer at
once: a schema-driven client library (done, `SERVER-001` v0.5.0),
authentication/authorization (done, `SERVER-001` v0.6.0, ADR-0012), and
this — session/transaction semantics, the third and last. `docs/FUTURE-GROWTH.md`
names it "genuinely new," the same category authentication/authorization
was in, so this follows the identical design-first, stop-for-review
cadence ADR-0012 itself followed after ADR-0010: a design document plus
a Proposed ADR, explicitly no implementation authorized, before any
owner review.

`docs/FUTURE-GROWTH.md`'s own framing is broader than what this document
proposes: "session/transaction semantics across multiple requests...a
protocol has to define what a 'connection' guarantees across several of
them." This document proposes the smallest real slice of that which is
still a genuine, useful guarantee — **one new request kind,
`Request::Transaction`, that applies a batch of `UpdateField`-shaped
writes atomically: all of them succeed, or none of them do, and no other
connection ever observes a partially-applied batch.** It deliberately
does **not** propose the literal "session" half of the FUTURE-GROWTH.md
framing (a connection opening a transaction, issuing several separate
request messages against it, then committing or rolling back) — see
"Non-goals" below for why, and ADR-0013's own "Considered options" for
the full reasoning.

**In scope for this proposal:**

- A new `Request::Transaction { updates: Vec<TransactionOp> }` request
  kind, where each `TransactionOp` has exactly the shape
  `Request::UpdateField` already has (`id`, `field`, `value`) — a batch
  of writes, all known and submitted together in one request message.
- All-or-nothing application: every operation's precondition (record
  exists, field is known and updatable, value's type matches) is checked
  before any write is applied; if every check passes, every write is
  applied; if any check fails, none are.
- Isolation from concurrent connections for the duration of one
  `Transaction` request: no other connection's write can be observed
  interleaved between two of a batch's writes.
- A new, minimal storage-layer primitive (`ProductionStore`/
  `GenericProductionStore`) that lets a caller hold one continuously-held
  exclusive-access critical section spanning multiple logical
  operations — the real mechanism the atomicity/isolation guarantee
  above depends on. This is real work at the storage layer, not
  purely additive at the server layer the way `SERVER-AUTH`'s
  implementation was — flagged explicitly, not glossed over, since it's
  a materially bigger footprint than either prior server-layer-only
  round.

**Explicitly out of scope, named directly rather than left implicit**
(per ADR-0013's own "Considered options"):

- Multi-round-trip interactive sessions — `Request::BeginTransaction`,
  several separate request messages against an open transaction, then
  `Request::Commit`/`Request::Rollback`, with transaction state held
  open across all of them. Deferred to a real, separate future decision
  (see "Revisit triggers" in ADR-0013) — this proposal requires every
  operation in a transaction to be known and submitted together in one
  request message instead.
- Crash-atomicity across a batch — a process crash between two of a
  batch's individual writes landing on stable storage can leave a
  partial batch durably applied. This proposal delivers atomicity with
  respect to concurrent access (a real, useful guarantee, backed by a
  real lock), not atomicity with respect to a process crash mid-batch
  (which would need a combined-journal or two-phase-commit-style
  mechanism this proposal does not build).
- Isolation levels, MVCC, or snapshot reads. The single exclusive lock
  this proposal reuses already gives the strongest possible isolation
  (full serialization) for a transaction's duration — there is no
  weaker, more-concurrent option being deferred, because none is needed
  to satisfy this proposal's own guarantee.
- Read operations inside a transaction, or any operation kind beyond
  `UpdateField` — today's protocol has exactly one mutating operation
  kind (`UpdateField`); a transaction is a batch of writes decided by
  the client upfront, not an interactive session where a read informs a
  later write. A client wanting that today issues an ordinary `GetById`
  first, then decides what `Transaction` to submit — two round trips,
  no state held open between them.
- Nested transactions, savepoints, or partial commit of a batch.
- Cross-domain transactions — a single `Transaction` request only ever
  addresses the one domain its connection is already talking to, same
  as every other request kind.

## Non-goals

- Not a claim that this design, once implemented, delivers ACID
  transactions in the conventional database sense. It delivers atomicity
  and isolation with respect to concurrent access; it explicitly does
  not deliver durability/crash-atomicity across a batch — see "Purpose
  and scope" above and "Security, privacy, and compatibility" below.
- Not a general-purpose session concept for the server/query layer.
  Authentication state (`SERVER-AUTH`, already implemented) remains the
  only state a connection carries across requests; a `Transaction`
  request adds no new per-connection state — it's a single request/
  response round trip like every other request kind, just one that
  happens to carry more than one logical operation.
- Not a replacement for `SERVER-001`'s existing protocol/framing/
  concurrency-model decisions, or for `SERVER-AUTH-DESIGN.md`'s own
  authentication/authorization decisions. Length-prefixed `bincode`
  framing, thread-per-connection dispatch, the `ConnectionStore` trait
  shape (extended, not replaced), and `AuthConfig`/`TokenClass` are all
  unchanged by this proposal.

## Context and terminology

Every domain adapter (`Dog`, `Order`/`Customer`, `Employee`) already
implements `ConnectionStore`, and each of its methods (`get`,
`update_field`, etc.) is independently self-contained: it acquires
whatever lock the underlying store (`ProductionStore`/
`GenericProductionStore`) manages internally, does its work, and
releases it, all within one call. This is exactly right for a single
operation, and it is the reason a naive "call `update_field` N times in
a loop" cannot deliver atomicity: another connection's write can
interleave between any two of those N calls, and a failure partway
through the loop leaves some writes applied and others not, with no way
to undo the ones that already landed.

- **Transaction (this proposal's sense)**: a batch of `UpdateField`-shaped
  writes, submitted together in one `Request::Transaction` message,
  applied all-or-nothing, isolated from every other connection's writes
  for the duration of that one request's processing. Not a
  multi-round-trip concept — see "Non-goals" above.
- **`TransactionOp`**: one operation within a transaction — `id`,
  `field`, `value`, the same three fields `Request::UpdateField` already
  carries.
- **Precondition check**: the same existence/field-known/type-match
  checks `update_field` already performs, run without mutating, against
  every operation in a batch before any of them is applied.
- **The "no runtime deletion" invariant**: no domain this crate serves
  ever deletes a record after a store is constructed — every dataset's
  id set is fixed at construction time, only field *values* mutate. This
  is what makes precondition checks *stay valid* across a batch: as long
  as every check and every write in one transaction happen under one
  continuously-held exclusive lock acquisition, no id can appear or
  disappear between the check and the corresponding write, and no
  field's type can change either (the schema itself is compile-time
  fixed). Without this invariant, a validate-then-apply design would
  need to re-validate after every write (or accept a real
  time-of-check-to-time-of-use race) — this crate does not have that
  problem, and this proposal deliberately exploits that rather than
  building machinery to guard against a race that cannot occur here.

## Requirements

- `TXN-FR-001`: A new `Request::Transaction { updates: Vec<TransactionOp> }` /
  `Response` pair. Full success returns `Response::Ok`, matching
  `UpdateField`'s own success shape. A failed operation returns
  `Response::TransactionFailed { index, code, message }` naming the
  first operation (by index into `updates`) that failed its precondition
  check and why — never a partial success, never a panic.
- `TXN-FR-002`: **Atomicity with respect to concurrent access**: for the
  duration of one `Transaction` request's processing, no other
  connection's write is observable as interleaved with any operation in
  the batch. Backed by one continuously-held exclusive-access critical
  section at the storage layer (`TXN-FR-006`), not per-operation
  locking.
- `TXN-FR-003`: **All-or-nothing application**: every operation's
  precondition (record exists, field is known and supports
  `UpdateField`, value's type matches the field's real type) is checked
  before any write in the batch is applied. If every check passes, every
  write is applied. If any check fails, no write in the batch is
  applied — not even the ones that would have succeeded.
- `TXN-FR-004`: `Request::Transaction` requires `TokenClass::ReadWrite`
  when authentication is configured — a `ReadOnly` token gets
  `ErrorCode::Unauthorized` for the whole request, not evaluated
  per-operation. Extends `AUTH-FR-003`'s existing rule (any request that
  can write requires `ReadWrite`) to this new request kind; does not
  reopen or modify `AUTH-FR-003` itself.
- `TXN-FR-005`: `ErrorCode` gains one new variant, `RecordNotFound`,
  reachable only via `Response::TransactionFailed`. A single,
  non-transactional `Request::UpdateField` keeps returning
  `Response::NotFound` for an unknown id exactly as it does today — this
  proposal changes no existing request kind's behavior.
- `TXN-FR-006`: A new, minimal storage-layer primitive
  (`ProductionStore`/`GenericProductionStore`, exact shape an
  implementation-time decision — see "Architecture and interfaces"
  below) providing one continuously-held exclusive-access critical
  section spanning multiple logical operations. Each domain adapter's
  `ConnectionStore::apply_transaction` implementation (`TXN-FR-001`'s
  real mechanism) uses this primitive rather than calling `update_field`
  in a loop.
- `TXN-FR-007`: **Explicitly named, not silently assumed solved**: a
  process crash between two of a batch's individual writes landing on
  stable storage can leave a partial batch durably applied. This
  proposal delivers atomicity with respect to concurrent access; it does
  not deliver crash-atomicity across a batch. See "Non-goals" and
  "Security, privacy, and compatibility."

## Architecture and interfaces

### Considered options

**Batch shape: interactive multi-round-trip session vs. one atomic
request message.**

1. *Multi-round-trip session* (`Request::BeginTransaction` returns a
   handle; subsequent requests reference it; `Request::Commit`/
   `Request::Rollback` finalize). Matches the literal "session" framing
   in `docs/FUTURE-GROWTH.md` most closely. **Rejected for this
   proposal**: holding a connection's exclusive lock open across an
   unbounded number of client round trips creates a real liveness risk
   this project has never accepted anywhere else in the server layer — a
   slow, stalled, or malicious client that never sends `Commit` blocks
   every other connection indefinitely (this crate's concurrency model
   has exactly one writer lock per store; there is no fairness or
   preemption story for a client that simply stops responding mid-
   transaction). Solving that properly needs real machinery (idle
   timeouts, a forced-abort policy, probably a cap of one open
   transaction per connection) — a real, larger design in its own right,
   not a small extension of this one. Named as a real revisit trigger in
   ADR-0013, not ruled out forever.
2. *One request message carries the whole batch* (`Request::Transaction { updates }`).
   **Chosen.** Bounded by construction: the server never holds a lock
   open waiting on the network — the entire critical section is bounded
   by however long this one request's own validate-and-apply work takes,
   the same risk profile as any single existing request. Reuses the
   existing `Request`/`Response` shape exactly (one new variant each),
   the same "small, bounded extension of what already exists" pattern
   `ADR-0011`'s `DescribeSchema` and `ADR-0012`'s `Authenticate` both
   used.

**Rollback mechanism: undo log vs. validate-then-apply.**

1. *Sequential apply, no rollback.* Rejected outright — doesn't deliver
   atomicity at all; a mid-batch failure would leave an arbitrary prefix
   of the batch applied.
2. *Undo log* (record each write's previous value before applying it;
   on a later failure, apply the recorded previous values back in
   reverse). Considered — works for arbitrary future operation kinds,
   not just `UpdateField`. Rejected for this proposal as unnecessary
   complexity: it requires `update_field` to also report the value it
   overwrote (an API change beyond what committing this batch needs),
   and it has to handle the undo step itself failing, which
   validate-then-apply avoids entirely by never applying a write it
   isn't already sure will succeed.
3. *Validate-then-apply* (check every operation's precondition before
   applying any write). **Chosen.** Exploits this crate's own "no
   runtime deletion, fixed schema" invariant (see "Context and
   terminology" above): as long as every check and every write happen
   under one continuously-held lock acquisition, a precondition checked
   first stays true when its write is applied later in the same batch —
   no time-of-check-to-time-of-use race to guard against, so no undo
   machinery is needed.

**Locking mechanism: a new server-layer lock vs. a new storage-layer
primitive.**

1. *A new lock owned by the server layer* (e.g. each `*ConnectionStore`
   adapter wraps its store in an additional `Mutex`/`RwLock` used only
   for transactions). **Rejected.** This project's own established
   principle (`docs/decisions/ADR-0010-server-query-layer-proposal.md`,
   `src/server/mod.rs`'s own module doc comment) is that the server
   layer adds no new lock, reusing whatever locking the wrapped store
   already manages internally. A second, server-layer-only lock either
   doesn't actually prevent interleaving from a plain, non-transactional
   `Request::UpdateField` on another connection (which wouldn't touch
   the new lock at all unless *every* write is rerouted through it,
   defeating the "no new lock" principle even more directly), or it
   requires exactly that rerouting — either way, a real violation of an
   already-accepted decision driver, not a small addition alongside it.
2. *A new primitive on the storage layer itself* (`ProductionStore`/
   `GenericProductionStore` gain a way to hold one exclusive-access
   critical section across multiple logical operations, reusing the
   `RwLock` each already manages internally). **Chosen.** Correct
   isolation by construction — the same lock every other read/write
   already contends for, held continuously instead of re-acquired per
   operation. The real cost, named honestly: this touches
   `src/production.rs`/`src/generic/production.rs`, both already-
   accepted, "closed" modules (`STORAGE-011`/`STORAGE-012`, Implemented/
   Verified) — a materially bigger footprint than `SERVER-AUTH`'s purely
   server-layer-additive implementation, and (after the `Employee`
   round's real `Neighbors`-forwarding fix to `crate::generic`) the
   second time this project would deliberately reopen an already-closed
   storage-layer module, this time by design rather than because a gap
   was found while building something else.

### Proposed shape

```rust
// src/server/protocol.rs -- additions, not a rewrite of the existing enums

struct TransactionOp {
    id: RecordId,
    field: FieldRef,
    value: ScanValue,
}

enum Request {
    // ...every existing variant, unchanged...
    Transaction { updates: Vec<TransactionOp> },
}

enum ErrorCode {
    // ...every existing variant, unchanged...
    RecordNotFound, // an operation's id doesn't exist -- only reachable via TransactionFailed
}

enum Response {
    // ...every existing variant, unchanged...
    TransactionFailed { index: usize, code: ErrorCode, message: String }, // no operation was applied
}
```

```rust
// src/server/mod.rs -- ConnectionStore gains one new method; dispatch
// gains one new arm. Auth gating (handle_connection) treats Transaction
// exactly like UpdateField: requires TokenClass::ReadWrite.

trait ConnectionStore {
    // ...every existing method, unchanged...

    /// Apply every operation in `updates` atomically: every precondition
    /// (id exists, field known and updatable, value type matches) is
    /// checked before any write is applied; either every write in
    /// `updates` is applied, or none are. `Err((index, code))` names the
    /// first operation that failed its precondition check.
    fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)>;
}

// dispatch's new arm, sketched:
// Request::Transaction { updates } => match store.apply_transaction(&updates) {
//     Ok(()) => Response::Ok,
//     Err((index, code)) => Response::TransactionFailed {
//         index,
//         code,
//         message: /* same per-code messages err_response already uses */,
//     },
// }
```

```rust
// src/production.rs / src/generic/production.rs -- a new, minimal
// primitive each domain adapter's apply_transaction uses (illustrative
// shape, exact API an implementation-time decision -- see "Open
// questions"):

impl ProductionStore {
    /// Runs `f` with exclusive access held for `f`'s entire duration --
    /// the same internal lock every other `&self` method on this store
    /// already acquires and releases per call, exposed here as one
    /// continuous critical section instead of many short ones.
    pub fn with_exclusive<R>(&self, f: impl FnOnce(&mut Self) -> R) -> R {
        // ...
    }
}
// GenericProductionStore<S> would gain the equivalent.
```

`DogConnectionStore::apply_transaction` (and the `Order`/`Employee`
equivalents) would call `self.store.with_exclusive(|store| { ...check
every op, then apply every op, all against `store` directly, no
re-locking between them... })`.

## Data/state and invariants

- No new per-connection state — a `Transaction` request is one request/
  response round trip like every other kind; `handle_connection`'s
  existing `Option<TokenClass>` auth state is untouched by this
  proposal.
- No new persistent state or on-disk format — every byte a transaction
  writes is written through the exact same field-write path a single
  `UpdateField` already uses; this proposal changes *when* the store's
  internal lock is held, not *what* gets written or how.
- The storage-layer primitive (`with_exclusive` or equivalent) is the
  one new piece of state-adjacent machinery this proposal introduces,
  and it introduces no new lock — it changes the *scope* of the existing
  one from "one operation" to "the caller's whole closure."

## Errors, failure, recovery, and observability

- A failed precondition check returns `Response::TransactionFailed { index, code, message }`
  with no write applied — never a partial success, never a panic,
  matching `dispatch`'s existing "typed error, not a panic" discipline.
- A `ReadOnly`-authenticated connection attempting `Request::Transaction`
  gets `ErrorCode::Unauthorized` for the whole request, evaluated before
  any operation in the batch is even looked at — the same "rejected
  before any work is attempted" shape `AUTH-FR-003` already established
  for `UpdateField`.
- **Named, not silently assumed solved**: a process crash between two of
  a batch's writes landing on stable storage can leave a partial batch
  durably applied on disk — see `TXN-FR-007` and "Security, privacy, and
  compatibility" below. No mechanism in this proposal detects or repairs
  that after the fact.
- Out of scope, named rather than silently assumed solved: a maximum
  batch size (an unbounded `Vec<TransactionOp>` is bounded only by
  `framing::MAX_FRAME_BYTES`, the same resource-exhaustion guard every
  other request already gets, not a transaction-specific limit); any
  metric or log distinguishing transactional writes from ordinary ones.

## Security, privacy, and compatibility

- **This design does not, by itself, deliver ACID transactions.** It
  delivers atomicity and isolation with respect to concurrent access,
  backed by a real, continuously-held exclusive lock; it explicitly does
  not deliver durability/crash-atomicity across a batch — a process
  crash mid-batch can leave a partial batch durably applied. Both facts
  need to be true together before a caller could rely on this as a
  conventional database transaction; this proposal only delivers the
  first.
- No interaction with `SERVER-AUTH`'s posture beyond `TXN-FR-004`
  (`Transaction` requires `ReadWrite`, same rule `UpdateField` already
  has) — this proposal introduces no new authentication or authorization
  concept of its own.
- No interaction with the still-open transport-encryption gap (ADR-0012)
  either way — a `Transaction` request's contents are exactly as
  plaintext-on-the-wire as every other request already is.
- Backward compatible by construction: no existing request kind's shape
  or behavior changes. A server that never receives a `Request::Transaction`
  behaves identically to one built without this proposal at all.

## Acceptance criteria

(For the eventual implementation unit, once this design is accepted —
not attempted by this proposal itself.)

- A batch where every operation's precondition passes applies every
  write and returns `Response::Ok`; a subsequent `GetById` for each
  touched record reflects every write.
- A batch where one operation's precondition fails (unknown id, unknown
  field, or a type-mismatched value) returns `Response::TransactionFailed`
  naming that operation's index and `ErrorCode`; a subsequent `GetById`
  for every record in the batch — including ones whose own operation
  would have succeeded — shows no write from that batch took effect.
- A concurrent read/write stress test (matching this project's own
  flagship-stress-test discipline): while one connection's transaction
  is being applied, no other connection ever observes a state where some
  but not all of that transaction's writes are visible.
- A `ReadOnly`-authenticated connection's `Request::Transaction` gets
  `ErrorCode::Unauthorized` without any operation in the batch being
  evaluated (verify no partial application occurs even though the
  request was rejected before reaching `apply_transaction`).
- No existing test in `tests/server_{dog,order,employee}_integration.rs`,
  `tests/server_schema_driven_client.rs`, or `tests/server_auth_integration.rs`
  needs to change — this proposal is purely additive.

## Verification plan

(Also for the eventual implementation unit.)

- Unit tests: `apply_transaction` against a minimal fixture store (all-
  pass, first-operation-fails, middle-operation-fails, last-operation-
  fails — confirming none of the batch's writes land in the failing
  cases), `Request::Transaction`/`Response::TransactionFailed`/
  `ErrorCode::RecordNotFound` round-trip through `bincode`.
- Real end-to-end tests: a genuine `TcpListener`/`TcpStream` pair, all-
  success and first-failure/middle-failure cases, a `ReadOnly`-token
  rejection case, matching the existing `tests/server_*_integration.rs`
  pattern.
- A concurrent stress test: multiple client connections, some issuing
  ordinary `UpdateField` requests and some issuing `Transaction` batches
  against overlapping ids, verified via sequential-replay linearizability
  against a fresh in-memory reference — the same pattern this crate's
  other flagship concurrency tests already use
  (`concurrent_clients_over_the_wire_match_a_sequential_replay`), extended
  to confirm a transaction's writes are never observed as a partial set.

## Traceability

Would implement: the "session/transaction semantics" gap ADR-0010 and
`docs/FUTURE-GROWTH.md` each name — once ADR-0013 is accepted. No spec
registered yet; per ADR-0010's/ADR-0012's own precedent (`SERVER-001`
registered/extended as a separate step after each ADR's acceptance), a
real implementation would extend `SERVER-001` with new FRs (or register
a dedicated spec) as its own follow-up unit, not part of this
design-only pass. The storage-layer primitive (`TXN-FR-006`) would also
need `STORAGE-011`/`STORAGE-012` to record the change, matching how the
`Employee` round's `Neighbors`-forwarding fix bumped `STORAGE-012` to
v0.2.0 with an ADR-0009 addendum rather than being folded silently into
`SERVER-001` alone.

## Open questions

- The exact shape of the storage-layer primitive (`with_exclusive` or
  equivalent) is sketched, not fixed — whether it exposes a closure over
  `&mut Self`, a narrower guard type, or something else entirely is a
  real implementation-time API design question, not decided here.
  Whatever it is, it must not weaken any existing `ProductionStore`/
  `GenericProductionStore` invariant `STORAGE-011`/`STORAGE-012` already
  established.
- A maximum operations-per-transaction limit (beyond the existing
  `MAX_FRAME_BYTES` framing bound) is not proposed here — whether one is
  worth adding (to bound how long one connection can hold the exclusive
  lock, protecting other connections' latency) is an owner's call,
  informed by real benchmark numbers once this is implemented.
- Whether the eventual implementation benchmarks transaction throughput/
  latency the way `benches/server.rs` already does for single requests
  is not decided here — a real, separate question once real code exists
  to measure.
- Whether the multi-round-trip interactive session this proposal
  explicitly rejects (see "Considered options") is ever worth the real
  liveness-management design it would need is entirely the owner's call,
  not decided here — this document only records why it's out of scope
  *for this proposal*, not that it's permanently off the table.

## Change history

- 2026-09-01: Initial proposal, in response to the owner selecting
  session/transaction semantics as the third of three next directions
  (alongside the schema-driven client library, done, and auth/
  authorization, done).
