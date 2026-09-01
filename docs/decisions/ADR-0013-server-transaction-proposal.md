# ADR-0013: Add atomic multi-operation transactions to the server/query layer

- Status: **Proposed** — awaiting owner review. Not accepted; nothing in
  this ADR or its linked design document authorizes implementation.
- Date: 2026-09-01
- Deciders: baileyrd (pending)
- Related: `docs/design/SERVER-TRANSACTION-DESIGN.md` (the full design
  document this ADR summarizes), `docs/decisions/ADR-0010-server-query-layer-proposal.md`
  (names "no transaction semantics" directly in its own Consequences),
  `docs/decisions/ADR-0012-server-authentication-proposal.md` (the
  immediately preceding design-first-then-implement round this ADR
  follows the same cadence as), `docs/specifications/server/SERVER-001-query-layer.md`
  (the spec this proposal would extend, not modify, until accepted),
  `docs/FUTURE-GROWTH.md` ("Path to a server / query layer" — names
  "session/transaction semantics across multiple requests" as
  "genuinely new," not an incremental extension)
- Supersedes/Superseded by: none. Extends (does not supersede) ADR-0010's
  "no transaction semantics" consequence — this ADR is a revisit of that
  gap, the same way ADR-0012 was a revisit of ADR-0010's "no
  authentication" gap.

## Context

The owner picked three next directions at once after `SERVER-QUERY-LAYER`
v0.4.0 landed: a schema-driven client library, authentication/
authorization, and this proposal — session/transaction semantics. The
client library was bounded/additive and shipped directly as `SERVER-001`
v0.5.0, no new ADR needed. Authentication/authorization and this
proposal are both named "genuinely new," not incremental, in
`docs/FUTURE-GROWTH.md`'s own "Path to a server / query layer" section —
both change this crate's consistency or security posture in a way a
client could come to depend on, the same "hard to reverse once a client
depends on it" reasoning ADR-0010 itself used to justify a design-only-
first pass before `SERVER-QUERY-LAYER` was implemented. Authentication/
authorization already followed that treatment (ADR-0012, Accepted and
implemented as `SERVER-001` v0.6.0). This ADR gives session/transaction
semantics — the third and last of the owner's three picks — the
identical treatment: **it authorizes a design, not implementation
code**, matching this project's `adr-cadence.md` Regime 1 discipline for
a consequential decision during active major development.

ADR-0010's own Consequences named this directly: "no transaction
semantics" alongside "no authentication, no authorization, no transport
encryption" as an explicit, deliberately-unresolved gap at acceptance
time. Authentication/authorization is now closed (ADR-0012). This ADR is
the next revisit — not of ADR-0010 as a whole (its protocol/framing/
concurrency-model decisions are unchanged and out of scope here), only
of its "no transaction semantics" consequence.

## Decision drivers

- **Deliver a real, useful guarantee, not a token gesture, but the
  smallest real slice of it** — the same discipline `SERVER-AUTH-DESIGN`
  applied when it picked a shared-secret token over a full identity
  system. "Session/transaction semantics" as `docs/FUTURE-GROWTH.md`
  frames it is bigger than what this proposal delivers; this ADR is
  explicit about exactly which slice it takes and which it defers.
- **No new, unbounded liveness risk.** This project's thread-per-
  connection, one-writer-lock-per-store model has no fairness or
  preemption story for a client that stops responding. A design that
  would hold an exclusive lock open across an unbounded number of client
  round trips (a true interactive "session") introduces a real
  denial-of-service surface this project has never accepted anywhere
  else in the server layer. This proposal is scoped specifically to
  avoid that, by bounding every transaction to one request message.
- **No new lock at the server layer** — the same principle ADR-0010
  itself established ("Concurrency across client connections adds no
  new lock: it collapses onto whatever `RwLock` the wrapped store
  already manages internally"). This proposal's atomicity mechanism
  reuses that existing lock, held for a longer, but still request-
  bounded, critical section — it does not add a second lock anywhere.
- **Honest about what "transaction" does and doesn't mean here.** A
  reader seeing "transactions" added to this crate could reasonably
  assume full ACID semantics. This proposal delivers atomicity and
  isolation with respect to concurrent access; it does not deliver
  crash-atomicity across a batch. Both need to be named plainly, the
  same way `SERVER-AUTH-DESIGN` was explicit that authentication alone
  doesn't deliver transport encryption.
- **Correctly attribute the real cost.** Unlike `SERVER-AUTH`'s
  implementation (purely additive at the server layer), this proposal's
  atomicity mechanism requires a real, if minimal, new primitive on the
  storage layer itself (`ProductionStore`/`GenericProductionStore`) —
  already-accepted, "closed" modules. That cost belongs in this decision
  record up front, not discovered partway through an implementation
  round.

## Considered options

See `docs/design/SERVER-TRANSACTION-DESIGN.md`'s own "Architecture and
interfaces" section for the full reasoning. Summarized:

1. **Batch shape**: a multi-round-trip interactive session
   (`BeginTransaction`/several requests/`Commit`/`Rollback`, matching
   `docs/FUTURE-GROWTH.md`'s literal framing most closely) — rejected
   for this proposal specifically because of the unbounded liveness risk
   named above; a real, larger design in its own right, not a small
   extension of this one — vs. **one request message carries the whole
   batch** (`Request::Transaction { updates }`) — **chosen**, bounded by
   construction, reuses the existing `Request`/`Response` shape exactly.
2. **Rollback mechanism**: sequential apply with no rollback (rejected —
   delivers no atomicity) vs. an undo log (considered, rejected as
   unneeded complexity) vs. **validate-then-apply**, exploiting this
   crate's own "records are never deleted at runtime, schema is fixed"
   invariant so no time-of-check-to-time-of-use race exists to guard
   against — **chosen**.
3. **Locking mechanism**: a new server-layer-only lock (rejected —
   violates ADR-0010's own "no new lock at this layer" principle, and
   doesn't achieve real isolation unless every write is rerouted through
   it anyway) vs. **a new, minimal storage-layer primitive** reusing the
   existing internal lock for a longer, request-bounded critical section
   — **chosen**, at the real, named cost of touching `src/production.rs`/
   `src/generic/production.rs`, both already-accepted and closed.

## Decision

- `docs/design/SERVER-TRANSACTION-DESIGN.md` records the full proposed
  design: `Request::Transaction { updates: Vec<TransactionOp> }`, each
  `TransactionOp` shaped exactly like `Request::UpdateField`'s own
  fields; every operation's precondition checked before any write is
  applied (`Response::Ok` on full success, `Response::TransactionFailed { index, code, message }`
  naming the first failing operation on any failure, with no write
  applied in that case); `TokenClass::ReadWrite` required, same rule
  `UpdateField` already has; one new `ErrorCode::RecordNotFound`
  variant, reachable only through the new response shape.
- **A new, minimal storage-layer primitive is part of this proposal**,
  not deferred to "someday": `ProductionStore`/`GenericProductionStore`
  would gain a way to hold one continuously-held exclusive-access
  critical section spanning multiple logical operations, reusing each
  store's existing internal lock. This is the real mechanism the
  atomicity/isolation guarantee depends on, and it is real work against
  already-accepted, "closed" storage-layer modules (`STORAGE-011`/
  `STORAGE-012`) — named here explicitly so accepting this ADR means
  accepting that cost, not discovering it mid-implementation.
- **This proposal does not deliver crash-atomicity across a batch.** A
  process crash between two of a batch's writes landing on stable
  storage can leave a partial batch durably applied. Documented as a
  real, named limitation, not silently assumed away — see the design
  document's own "Security, privacy, and compatibility" section.
- **This proposal does not deliver a multi-round-trip interactive
  session.** Every operation in a transaction must be known and
  submitted together in one request message. The literal "session" half
  of `docs/FUTURE-GROWTH.md`'s framing is deliberately deferred — see
  "Considered options" above and "Validation and revisit triggers"
  below.
- No new dependency — this proposal reuses `Request`/`Response`/
  `ErrorCode`'s existing shapes and each store's existing internal lock.
- **Acceptance of this ADR authorizes the design, not implementation
  code.** No existing source file is modified by this ADR itself. Per
  ADR-0010's/ADR-0012's own precedent, a real implementation would
  extend `SERVER-001` with new FRs (or register a dedicated spec), and
  would extend `STORAGE-011`/`STORAGE-012` to record the storage-layer
  primitive, as its own follow-up unit, only after this design is
  explicitly accepted.

## Consequences

### Positive

- Closes a real, named gap (ADR-0010's own "no transaction semantics"
  consequence, `docs/FUTURE-GROWTH.md`'s "session/transaction semantics"
  item) with a bounded, honestly-scoped slice rather than either
  ignoring it indefinitely or over-committing to a full interactive
  session design this project isn't ready to take the liveness-risk
  cost of.
- Delivers a real, useful atomicity/isolation guarantee for the one
  mutating operation kind this protocol has (`UpdateField`) — not a
  token gesture; a real concurrent stress test (per the design
  document's own Acceptance criteria) would have to hold under
  contention for this to be considered done.
- No new dependency, no new lock at the server layer — reuses exactly
  what `ProductionStore`/`GenericProductionStore` already manage
  internally, extended in scope rather than duplicated.
- Bounded by construction against the liveness risk a true interactive
  session would carry — every transaction's critical section has the
  same bounded-by-one-request-message shape every other request already
  has.

### Negative / tradeoffs

- **Touches already-accepted, "closed" storage-layer modules**
  (`src/production.rs`/`src/generic/production.rs`, `STORAGE-011`/
  `STORAGE-012`) — a materially bigger footprint than `SERVER-AUTH`'s
  purely server-layer-additive implementation. A real cost, flagged here
  rather than discovered mid-implementation.
- **No crash-atomicity** — a process crash mid-batch can leave a partial
  batch durably applied. A caller relying on this as a full ACID
  transaction would be relying on a guarantee this proposal does not
  make.
- **Does not deliver the literal "session" half** of what
  `docs/FUTURE-GROWTH.md` named — a deliberate, named scope-down, not a
  silent reinterpretation, but real functionality (an interactive,
  multi-round-trip transaction) that some readers of the original
  three-direction pick might have expected is not part of this proposal.
- Batches are restricted to `UpdateField` only — no transactional reads,
  no other mutating operation kind (none exists today either way).
- The exact shape of the new storage-layer primitive is sketched, not
  fixed, in the linked design document — a real implementation-time API
  design question this ADR does not resolve.

## Validation and revisit triggers

- **This proposal is design-only, matching ADR-0010's/ADR-0012's own
  precedent** — no implementation, no test suite yet, no storage-layer
  code touched. Unlike ADR-0009's/ADR-0010's own proposals, no
  standalone scratch-crate compile probe was built for this one, for the
  same reason ADR-0012 didn't build one: the proposed protocol additions
  (one new `Request`/`Response` variant each, one new `ErrorCode`
  variant) are incremental extensions of `SERVER-001`'s existing,
  already-compiling shapes. The one piece of this proposal that *is*
  genuinely new — the storage-layer critical-section primitive — is
  judged low-risk enough not to warrant a probe either, since it's a
  scope change to an existing lock (broader critical section), not a new
  synchronization primitive or a new type-system structure.
- Revisit if: the owner wants a true multi-round-trip interactive
  session — a real, larger design (idle timeouts, a forced-abort policy,
  probably a cap of one open transaction per connection, a real fairness
  story) would need its own proposal, not a small extension of this one.
- Revisit if: crash-atomicity across a batch becomes a real need — would
  need a combined-journal or two-phase-commit-style mechanism, a real
  durability redesign, not a small extension of this proposal's
  validate-then-apply mechanism.
- Revisit if: a domain ever needs a second mutating operation kind
  beyond `UpdateField` — `TransactionOp` would need to grow beyond the
  one shape this proposal fixes.
