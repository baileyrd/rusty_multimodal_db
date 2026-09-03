# ADR-0033: Optimistic read-set validation for sessions — snapshot isolation from other connections' commits

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): a third `BeginWith`
  bit, `SESSION_SNAPSHOT_ISOLATION`, optimistic read-set validation
  checked and applied atomically at `Commit`; (b) tracking only the
  single most recent read and (c) closing as not warranted declined;
  no changes requested). Acceptance authorizes the design;
  implementation follows as its own unit — see "Acceptance and
  implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN.md`
  (the full design this ADR summarizes), `ADR-0024` (the session
  mechanism; `SESS-FR-007`'s own "committed state only" requirement),
  `ADR-0027` (read-your-writes — closed `SESS-FR-007`'s first half,
  left the second, isolation from other connections, unnamed as a
  trigger), `ADR-0025`/`ADR-0026` (the journal and group commit — the
  precedent for checking and applying atomically inside one exclusive
  section, and for `apply_transaction`'s own signature evolving),
  `ADR-0030` (`STV-FR` — the precedent for a `ConnectionStore` hook
  added for a session-scoped concern), `docs/FUTURE-GROWTH.md`.
- Supersedes/Superseded by: none. Extends `ConnectionStore::apply_transaction`'s
  signature (every implementor updated); changes no store, no wire
  shape beyond one appended flag bit and one appended `ErrorCode`.

## Context

The owner picked "real transactions" from `docs/FUTURE-GROWTH.md` as
the direction to pursue. Read against what this repository has built
since that document was written, most of its own framing is already
closed: `Request::Transaction`/sessions give true N-operation
atomicity, the journal and group commit give it crash-safety, and
read-your-writes lets a session see its own pending writes. What is
still true and still unaddressed is narrower: a session's reads have
no protection from what another connection commits in between — the
second half of `SESS-FR-007`, which `ADR-0027` left standing on
purpose rather than closing.

This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0032` took.

## Decision drivers

- Close a real, already-named half-gap (`SESS-FR-007`) rather than
  reach for the full "MVCC or log-based design"
  `docs/FUTURE-GROWTH.md` itself frames as a multi-year effort —
  proportionate scope over an impressive one.
- Reuse the exact atomic check-then-apply window `ADR-0025`/`ADR-0026`
  already built for write validation, rather than invent a new
  locking story.
- Never hold a lock across a network round trip — the constraint
  `ADR-0024` itself established and every session mechanism since has
  kept.
- Detect conflicts, don't try to prevent them — optimistic, not
  pessimistic, concurrency control; a session pays nothing unless it
  opts in, and pays only at `Commit`, never per read.

## Considered options

1. **Optimistic read-set validation at `Commit`, checked and applied
   atomically inside the existing exclusive section — proposed.**
   Cheap, no storage-engine change, directly reuses `JRN-FR-002`'s own
   atomicity guarantee extended to cover reads.
2. **Hold a shared read lock for the session's lifetime.** Trivially
   correct, rejected outright: a session spans client round trips
   with no bound on duration, so this blocks every writer for as long
   as any snapshot-isolated session is open — exactly the failure
   mode `ADR-0024` designed the whole session mechanism to avoid.
3. **Real MVCC — versioned records, a snapshot pointer, garbage
   collection.** The correct general answer and
   `docs/FUTURE-GROWTH.md`'s own named multi-year effort; rejected
   this round as disproportionate to what has actually been asked
   for, and because it is the first `SERVER-001` round that would
   need to touch `src/store/**` at all.
4. **Close as not warranted.** A legitimate, smaller choice, left to
   the owner.

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/protocol.rs`: `SESSION_SNAPSHOT_ISOLATION = 4` (a third
  `BeginWith` bit, composing with the existing two);
  `PROTOCOL_VERSION` moves to 7 (a flag bit is introduced at a
  version, `ADR-0022`'s own clarification, taken again at `STV-FR`'s
  precedent); `ErrorCode` gains `Conflict`, reported as
  `Response::TransactionFailed { index: 0, code: Conflict }` — the
  same sentinel-index shape `ErrorCode::Journal` already established
  for a commit-level failure not tied to one staged operation.
- `src/server/mod.rs`: a per-connection `snapshot_reads:
  Option<HashMap<(RecordId, FieldRef), ScanValue>>`, alongside
  `session`/`read_your_writes`/`validate_on_stage`; every session
  `GetById` (independent of read-your-writes) records the *raw,
  committed* value it returned, before any read-your-writes overlay,
  bounded at `MAX_TRACKED_READS = 4096` (the `MAX_STAGED_OPS`
  precedent — a constant, not a config); a `NotFound` read is never
  tracked (no phantom-read protection this round, named).
- `ConnectionStore::apply_transaction` gains one parameter,
  `read_set: &[(RecordId, FieldRef, ScanValue)]` (empty when unused,
  zero added cost on that path — the `validate_op` precedent). Inside
  the same exclusive section the batch's own write validation and
  apply already use, each adapter re-reads every tracked key and
  compares it to the recorded value before applying anything; any
  mismatch refuses the whole commit atomically with that check.
- Every implementor (`DogConnectionStore`, `OrderConnectionStore`,
  `EmployeeConnectionStore`, this crate's own test doubles) updated;
  no external implementor exists.
- `SERVER-001`'s next minor / FR (`ISO-FR-001`–`008`); `SESS-FR-007`'s
  second half resolved by pointer; `SPEC-REGISTRY`, `TRACEABILITY`,
  `ROADMAP` (`SERVER-SESSION-SNAPSHOT-ISOLATION`), `PROJECT-STATUS`.
- Tests per the design's verification plan, including a real
  two-connection integration test proving a conflicting commit from
  another connection is caught, a non-conflicting one is not, and the
  read-your-writes/snapshot-isolation combination behaves correctly
  together.
- No `Cargo.toml`, store (`src/store/**`), or `serve`-signature
  change.

## Consequences

### Positive

- Closes a real, previously-named gap (`SESS-FR-007`'s second half)
  rather than leaving it permanently restated, the same discipline
  this session applied to every other multi-ADR-restated trigger.
- A session gets a real, useful isolation guarantee — "nothing I
  looked at moved before I committed" — at the cost of one comparison
  per tracked read, paid once, only when opted in.
- Composes for free with the access log: `Commit`'s outcome already
  flows through `outcome_of`'s existing exhaustive match, so a
  conflict is visible in an operator's access log with zero new code
  in `access.rs` — a small, concrete piece of evidence the design's
  pieces fit together cleanly rather than by coincidence.

### Negative / tradeoffs

- **`ConnectionStore::apply_transaction`'s signature changes** — every
  implementor updated, the same bounded mechanical cost `validate_op`'s
  own addition already paid at `STV-FR`.
- **No phantom-read protection, no set-read isolation.** A session
  relying only on `FilterEq`/`ScanField`, or on records not yet
  created, gets no benefit from this feature — named plainly rather
  than implied to be covered.
- **Optimistic, not pessimistic**: a conflict is only caught at
  `Commit`, not prevented from happening — a session doing a lot of
  work between a read and its own commit can lose that work to a
  conflict it could not have seen coming. The client owns retry, as
  it already does for every other typed commit failure.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0032`. Every claim about the current code (`SESS-FR-007`'s
  exact wording, the `GetById` session-intercept's raw-then-overlay
  order, `apply_transaction`'s current signature and its adapters'
  `with_exclusive` sections, `ErrorCode::Journal`'s sentinel-index
  precedent, `outcome_of`'s exhaustive match) read from `main`
  `649d708`. No probe: the mechanism is a map, a comparison, and a
  check-then-apply reuse of an already-proven atomic window, not new
  concurrency machinery needing empirical validation before
  committing to a shape.
- Revisit if: phantom-read protection is wanted — a range/existence
  lock or a versioned index, a real design of its own.
- Revisit if: `FilterEq`/`ScanField` isolation is wanted enough to pay
  its re-validation cost at `Commit` — `RYW-FR`'s own identical
  trigger, restated here for the read-set side.
- Revisit if: optimistic detection proves insufficient for a real
  workload (too many conflicts, work lost too often) — real MVCC
  becomes the next question, not a smaller tweak to this design.
- Revisit if: `MAX_TRACKED_READS` is hit in practice — the
  `MAX_STAGED_OPS` precedent: a constant until a real report decides
  otherwise.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — optimistic
  read-set validation, a third `BeginWith` bit, protocol version 7;
  **(b)** accept but scope down further — track only the single most
  recent `GetById` per session rather than a full read set, cheaper
  but far less useful (most sessions read more than one thing before
  deciding what to write); **(c)** close as not warranted — restate
  `SESS-FR-007`'s second half as still open, build nothing. Proposed
  in PR #173.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implementation follows as `SERVER-001`'s next minor / FR, per
  `docs/design/SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN.md`.
