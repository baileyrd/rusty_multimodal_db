# ADR-0024: A buffered transaction session — `Begin`/`Commit`/`Rollback` at protocol version 3, no lock across round trips

- Status: **Proposed** (not yet accepted; authorizes no implementation)
- Date: 2026-09-02
- Deciders: baileyrd
- Related: `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part A
  (the full design this ADR summarizes), `ADR-0013` /
  `docs/design/SERVER-TRANSACTION-DESIGN.md` (`Request::Transaction`;
  its first revisit trigger — *a true multi-round-trip interactive
  session* — is what this answers), `ADR-0022` /
  `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md` (whose first revisit
  trigger — *the first version-3 variant is proposed* — this is; this
  ADR cites it as that trigger required), `ADR-0025` (the independent
  crash-atomicity decision from the same design round),
  `docs/specifications/server/SERVER-001-query-layer.md` v0.13.0,
  `docs/FUTURE-GROWTH.md` item 1.
- Supersedes/Superseded by: none. Extends `ADR-0013`'s batch by a
  second way to assemble one; extends `ADR-0022`'s protocol by its
  first gated variants; changes nothing either already does.

## Context

`Request::Transaction` (v0.7.0) applies a batch all-or-nothing under
one continuously held exclusive lock, and requires the whole batch in
one request. `ADR-0013` rejected the interactive alternative — open a
transaction, issue requests, commit — because holding that lock across
an unbounded number of client round trips is a liveness risk the
thread-per-connection model cannot preempt, and named it as its first
revisit trigger, "a real, larger design (idle timeouts, a forced-abort
policy, probably a cap of one open transaction per connection, a real
fairness story)."

Read literally, that trigger asks for machinery this server does not
have. Read for what a caller actually wants — to accumulate writes over
several round trips and commit them atomically — it does not require
the lock to be held at all. The server can *stage* a connection's
writes in a buffer and apply them at commit exactly as one
`Transaction`. Every guarantee `ADR-0013` delivers is kept (atomicity,
full-serialization isolation for the batch's duration); the one thing a
lock-held session would add — reading your own uncommitted writes — is
the one thing this design gives up, and names.

The three new requests are the first variants introduced after
`PROTOCOL_VERSION = 2`. `ADR-0022` deferred keeping the negotiated
version per connection "until a gated variant exists" and named that
moment as its first revisit trigger. This is it: the server keeps the
number, refuses version-3 requests on older connections, and the client
library gates its first API on `server_protocol_version()`.

The owner selected this design round as the third of four directions.
This ADR proposes a design and authorizes no implementation — the
posture `ADR-0016` through `ADR-0023` took.

## Decision drivers

- Give the multi-round-trip shape without the multi-round-trip lock:
  no new liveness surface, no fairness story needed, no timeout thread.
- Keep `Request::Transaction`'s guarantees and mechanism exactly:
  commit *is* `apply_transaction`.
- Exercise `ADR-0022`'s rules for real — per-connection negotiated
  version, rule 3 and rule 4 gating — on the first variants that need
  them, so the next bump has a worked example.
- Bounded by construction: one session per connection, a hard cap on
  staged writes, nothing shared across connections, nothing surviving
  a disconnect.
- No new dependency; `framing.rs`, the codec, `dispatch`'s contract,
  and every adapter unchanged.

## Considered options

1. **A lock-held interactive session** — `Begin` enters `with_exclusive`
   and stays there; writes apply immediately; `Rollback` needs an undo
   log. Rejected: every other connection blocks on this client's next
   frame; an idle timeout or forced abort would need a second thread
   or a socket timeout on every read and an unwind out of the closure.
   `ADR-0013`'s reasons, unchanged. Offered as option (b) so the price
   is named, not because it is safe to build here.
2. **A server-side buffer of intended writes, applied at commit as one
   batch** — proposed. `Begin` (11) opens a per-connection
   `Vec<TransactionOp>`; an admitted `UpdateField` is staged and
   answered `Staged { index }` (11), nothing applied, no validation,
   no lock; `Commit` (12) runs `apply_transaction` on the buffer —
   `Ok` or `TransactionFailed { index }` naming the staged position —
   and closes the session; `Rollback` (13) discards. `NoSession`,
   `SessionOpen`, `SessionFull` (6–8) for misuse, connection open;
   `MAX_STAGED_OPS` (4096) caps the buffer; `Transaction` inside a
   session is `SessionOpen`. Reads inside a session see committed
   state only. Gating unchanged and per request (`ReadWrite` to stage
   or commit). `PROTOCOL_VERSION = 3`; the server keeps the negotiated
   version and answers `Begin`/`Commit`/`Rollback` on a connection
   below 3 with `Malformed`; `dispatch` maps them to `Unsupported`.
   `SchemaDrivenClient::begin()` returns a `Session` (`update` →
   staged index, `commit`, `rollback`, `Drop` rolls back best-effort)
   and refuses below version 3.
3. **Client-side batching only** — the status quo made ergonomic in the
   library. Rejected as the proposal: a raw-protocol client gains
   nothing, and a library client can already write it. Offered as
   option (c), close as not warranted.
4. **Stage-time validation against the schema** (within option 2).
   Rejected: duplicates the commit-time check that must run anyway
   under the lock, at the cost of a `ConnectionStore` obligation or a
   schema lookup per staged write; `Staged { index }` already lets the
   client correlate a later failure.
5. **Session state on `ConnectionStore`** (within option 2). Rejected:
   a store has no per-connection identity; it would need session ids
   and a map — shared mutable state for nothing.

## Decision

Proposed: option 2. Concretely, at implementation:

- `src/server/protocol.rs`: `PROTOCOL_VERSION = 3`; `MAX_STAGED_OPS`;
  `Request::{Begin, Commit, Rollback}` at 11–13, `Response::Staged {
  index: u32 }` at 11, `ErrorCode::{NoSession, SessionOpen,
  SessionFull}` at 6–8, each with a golden vector; version table row 3;
  every version-1/2 vector unchanged.
- `src/server/mod.rs`: `handle_connection` keeps `negotiated: u32`
  (from the `Hello` intercept, 1 for a silent client) and
  `session: Option<Vec<TransactionOp>>`; intercepts the three requests
  after the auth and `ReadOnly` gates (`Commit` joins
  `UpdateField`/`Transaction` in the `ReadOnly` gate); stages an
  admitted `UpdateField` while a session is open; refuses the three on
  a connection below 3 with `Malformed`; `dispatch` maps them to
  `Unsupported`. No adapter, store, or `ConnectionStore` change.
- `src/server/client.rs`: `begin()` → `Session<'_>` with `update`,
  `commit`, `rollback`, `Drop`; `ClientError::TransactionFailed {
  index, code, message }` (additive); `Unsupported("session")` below
  version 3, no frame sent.
- `SERVER-001` v0.14.0, FR-024 (`SESS-FR-001`–`009`); `ADR-0013`'s
  first trigger and `SERVER-TRANSACTION-DESIGN.md`'s last open question
  resolved by pointer; `ADR-0022`'s first trigger taken (the state and
  the gating branch it predicted); `SPEC-REGISTRY`, `TRACEABILITY`,
  `ROADMAP` (`SERVER-TRANSACTION-SESSION`), `PROJECT-STATUS`.
- Tests per the design's verification plan: a session section in
  `tests/server_transaction_integration.rs` (concurrent-visibility
  included), version gating in `tests/server_protocol_version.rs`
  (including a client pinned at `Hello { 2 }`), the client API in
  `tests/server_schema_driven_client.rs` on all three domains.
- No `Cargo.toml` change; `framing.rs`, the codec, `benches/server.rs`,
  and every existing suite unchanged.

## Consequences

### Positive

- The interactive shape `docs/FUTURE-GROWTH.md` item 1 asks for, with
  `Request::Transaction`'s exact guarantees and none of the liveness
  risk `ADR-0013` refused — bounded per connection, nothing held.
- `ADR-0022`'s rules get their first real exercise: the negotiated
  version is kept, rule 3 and rule 4 have a branch each, and the client
  library has a worked example of a version-gated API.
- Additive everywhere: no adapter, store, framing, or dependency change.

### Negative / tradeoffs

- **No read-your-writes.** A read between `Begin` and `Commit` sees
  committed state. A caller that needs to read then write reads first,
  then stages — the same two-step `ADR-0013` already prescribed.
- **No stage-time feedback.** A bad field or type is discovered at
  `Commit`, by index. Cheap to add later on top; not added now.
- **The version bump is real.** A version-2 client keeps working
  unchanged, but the table, the vectors, and the client gate are
  maintenance every future variant inherits — the cost `ADR-0022`
  chose to pay once and this is the first installment of.
- Still `UpdateField`-only, still one domain per connection, still no
  savepoints — `ADR-0013`'s list, unchanged.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0023`: no implementation, no test, no probe — every proposed
  addition is an appended variant or a per-connection field on shapes
  that already compile, and the commit path is the tested
  `apply_transaction`.
- Revisit if: read-your-writes inside a session becomes wanted — a
  per-connection overlay on the read paths, a real design with a cost
  on every read.
- Revisit if: stage-time validation becomes wanted — a `ConnectionStore`
  validation hook, additive.
- Revisit if: a second mutating operation kind appears — `TransactionOp`
  grows, and so does what a session can stage (`ADR-0013`'s third
  trigger, unchanged).
- Revisit if: `MAX_STAGED_OPS` is hit in practice — it is a constant,
  not a config, on purpose; the first real report decides whether it
  becomes one.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — a buffered
  session at protocol version 3, no lock across round trips, commit as
  one `Transaction`, read-your-writes and stage-time validation named
  as not included; **(b)** a lock-held interactive session with an idle
  timeout and forced abort — the literal reading of `ADR-0013`'s
  trigger, at the liveness cost it named; **(c)** close as not
  warranted — client-side batching through `Request::Transaction` is
  judged sufficient, and `ADR-0013`'s trigger stays armed.
- Outcome: pending the owner's decision.
