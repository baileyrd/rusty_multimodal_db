# Server Session Read-Your-Writes Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, `ADR-0027` option (a); `FilterEq`
  adjustment and closing declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit, after
  `ADR-0026`'s — see `ADR-0027`'s "Acceptance and implementation"
  section.
- Date: 2026-09-03
- Related: `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part A /
  `ADR-0024` (the buffered session, `SERVER-001` v0.14.0 / FR-024;
  `SESS-FR-007` — *reads inside a session see committed state, no
  read-your-writes, named* — and its first revisit trigger, *read-your-
  writes inside a session becomes wanted — a per-connection overlay on
  the read paths, a real design with a cost on every read*, are what
  this document answers), `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md`
  / `ADR-0022` (the append-only rules under which the new request is
  added and gated), `ADR-0013` (`Request::Transaction`, and the
  read-then-stage two-step it prescribed), `src/server/mod.rs`
  (`handle_connection`'s session intercept), `src/server/client.rs`
  (`Session`), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0.

## Purpose and scope

A session (`Begin` … `Commit`) stages writes in a per-connection buffer
and applies them as one batch at `Commit`. `SESS-FR-007` says, on
purpose, that a read made while the session is open — by this
connection or any other — sees committed state: a staged write is
invisible until `Commit` returns `Ok`. `ADR-0024` recorded the
consequence ("a caller that needs to read then write reads first, then
stages — the same two-step `ADR-0013` already prescribed") and named
the revisit: a per-connection overlay on the read paths, "a real design
with a cost on every read."

The owner picked that revisit as the second of four next directions.
This document is the design it asked for. Its scope is deliberately the
smallest slice that is still the real thing:

**In scope:**

- A session mode, chosen at `Begin` time, in which `GetById` on the
  session's own connection returns the committed record with the
  session's staged writes laid over it — last staged write per field
  wins.
- The wire addition that requests it: `Request::BeginWith { flags: u32 }`
  (index 14, protocol version 5), with `SESSION_READ_YOUR_WRITES = 1`.
- The client library's half: `SchemaDrivenClient::begin_read_your_writes()`
  and `Session::get(id)` (the latter for every session — today a
  `Session` borrows the client mutably, so no read at all is possible
  through the library while one is open).
- Exact semantics for the edge cases: a staged write to an id with no
  record, to a field the record does not carry, of a value whose kind
  does not match the field's.

**Out of scope (see "Non-goals")**: set reads (`ScanField`, `FilterEq`,
`Parent`/`Children`/`Neighbors`), stage-time validation, any change to
plain `Begin` sessions, any change to what other connections see.

## Non-goals

- **Set reads.** `ScanField` returns values by position, with no ids,
  so a staged write cannot be placed in it without an id-to-position
  map the store does not expose; `FilterEq` returns ids that match a
  value, and adjusting it under staged writes means an existence probe
  per staged operation on that field plus adding and removing ids from
  the store's answer — a second decision with a per-read cost that
  scales with the buffer. Both keep `SESS-FR-007`'s committed-state
  semantics in every session, and the design says so where it says
  what `GetById` does. `ADR-0027` offers `FilterEq` as option (b).
- **Relations.** `Parent`/`Children`/`Neighbors` are not updatable;
  nothing to overlay.
- **Stage-time validation.** `ADR-0024`'s second trigger, independent;
  a staged write that would fail at `Commit` is still discovered at
  `Commit`. The overlay is defined so that such a write does not
  produce a misleading read (below).
- **Plain sessions.** `Request::Begin` (index 11) is unchanged in every
  respect: a version-3 or -4 client, and a version-5 client that sends
  `Begin`, keeps `SESS-FR-007` exactly.
- **Cross-connection visibility.** Nothing here changes what any other
  connection sees; a staged write is still invisible to everyone else
  until `Commit`.

## Context and terminology

- **Session** (`SESS-FR-002`): `handle_connection`'s
  `session: Option<Vec<TransactionOp>>`; `Begin` sets `Some(vec![])`,
  an admitted `UpdateField` pushes and answers `Staged { index }`,
  `Commit` takes the buffer and runs `apply_transaction`, `Rollback`
  takes and drops it.
- **Point read**: `GetById { id }` → `Response::Record { id, fields:
  Vec<(FieldRef, ScanValue)> }` or `NotFound` — the one read whose
  answer is keyed by id and carries fields, so an overlay keyed by
  `(id, field)` can be applied to it exactly.
- **Set read**: `ScanField` (values by position), `FilterEq`
  (ids by value), the relation reads (ids).
- **Overlay**: for a `Record { id, fields }`, for each `(field, value)`
  in `fields`, if the buffer holds one or more `TransactionOp`s with
  this `id` and this `field`, replace `value` with the *last* such
  op's value — provided its `ScanValue` kind equals the committed
  value's kind. A pure function over `(id, &mut fields, &staged)`.
- **Flags**: a `u32` in `BeginWith`; bit 0 is read-your-writes; any
  other bit set is `Malformed` (the server does not know it, so it
  must not silently open a session without it).

### What the current code does, read from `main` `3987f9c`

`handle_connection` intercepts `Begin`/`Commit`/`Rollback`, and
`UpdateField`/`Transaction` while a session is open; every other
request — `GetById` included — falls through to `dispatch(store, req)`,
which reads the store. There is no per-connection state a read
consults. `SchemaDrivenClient::begin` sends `Request::Begin` and returns
a `Session<'_>` that borrows the client mutably; `Session` has
`update`/`commit`/`rollback` only, so through the library no read can
be issued while a session is open. The protocol is at version 4 with
`Request` indices 0–13; `tests/server_protocol_version.rs` uses index
14 as its "unknown request" probe.

## Requirements

- `RYW-FR-001` — **An opt-in mode, chosen at `Begin`.**
  `Request::BeginWith { flags: u32 }` is appended at `Request` index 14,
  introduced at protocol version 5; `pub const SESSION_READ_YOUR_WRITES:
  u32 = 1`. `BeginWith { flags: 0 }` is exactly `Begin`. Any bit outside
  the known set is answered `Response::Err { Malformed, .. }` and opens
  nothing. `SessionOpen` while one is open, as `Begin`. Answered by
  `handle_connection`, never through `dispatch` (which maps it to
  `Unsupported`, as it does `Begin`).
- `RYW-FR-002` — **The overlay on `GetById`.** While a read-your-writes
  session is open on a connection, its `GetById { id }` is served as
  today and then, if the answer is `Record`, overlaid: for each field
  in the record, the last staged `TransactionOp` with that `id` and
  `field` replaces the value **if the staged value's kind equals the
  committed value's kind**. Everything else is untouched: a `NotFound`
  stays `NotFound` (a staged write to a missing id creates nothing), a
  staged field the record does not carry is ignored, a kind-mismatched
  staged value is ignored — each of these would fail at `Commit`, and
  the read must not pretend otherwise.
- `RYW-FR-003` — **Set reads are unchanged** in every session:
  `ScanField`, `FilterEq`, `Parent`, `Children`, `Neighbors` see
  committed state (`SESS-FR-007` restated for them), documented on
  `BeginWith` and in the client library.
- `RYW-FR-004` — **Other connections are unchanged**: a staged write is
  visible to no other connection until `Commit` returns `Ok`. After
  `Rollback`, this connection's `GetById` returns committed state
  again; after `Commit`, every connection sees the batch.
- `RYW-FR-005` — **Cost.** The overlay runs only on a connection whose
  open session asked for it, only on `GetById`, and is linear in the
  buffer (at most `MAX_STAGED_OPS`). A plain session, a connection with
  no session, and every other request path are unchanged — no new
  branch is taken outside `handle_connection`'s existing session
  intercept.
- `RYW-FR-006` — **Versioning per `ADR-0022`.** `PROTOCOL_VERSION`
  becomes 5; the version table gains row 5 (`Request::BeginWith`, 14);
  a golden vector for `BeginWith { flags: 1 }` (`0e 00 00 00 01 00 00 00`);
  `BeginWith` on a connection negotiated below 5 is `Malformed` (rule
  3 — as `Begin` below 3); a client sends it only after negotiating ≥ 5
  (rule 4). No new `Response` variant or `ErrorCode`, so
  `downgrade_for_version` is unchanged. The unknown-index probe in
  `tests/server_protocol_version.rs` moves to 15.
- `RYW-FR-007` — **Client library.**
  `SchemaDrivenClient::begin_read_your_writes() -> Result<Session<'_>, _>`,
  `ClientError::Unsupported("read-your-writes session")` below 5 with
  no frame sent; `Session::get(id)` on every session (committed state
  on a plain one, overlaid on a read-your-writes one), delegating to
  the client's `get`; `Session::read_your_writes() -> bool`. No
  existing client method changes shape.
- `RYW-FR-008` — `SERVER-001` takes the next minor version and FR at
  implementation (v0.17.0 / FR-027 if this lands before
  `ADR-0026`'s implementation, else v0.18.0 / FR-028); `SESS-FR-007`
  and `ADR-0024`'s consequence resolved by pointer for read-your-writes
  sessions and restated for plain ones; `ADR-0024`'s first trigger
  taken.

## Considered options

**How a session asks.**

1. **`BeginWith { flags: u32 }` (proposed).** One appended variant, a
   flags word so a later session option (stage-time validation is the
   obvious one) is a bit, not a variant; unknown bits refused. The
   shape `ADR-0022`'s rules were written for.
2. **A unit variant `BeginReadYourWrites`.** Simpler to read; every
   future option is another variant and a combinatorial set if two
   ever compose. Rejected for that reason only; a fair alternative.
3. **Change `Begin`'s meaning** — every session, every version, gets
   the overlay. Rejected: `SESS-FR-007` is a numbered requirement a
   version-3/4 client may rely on, and `ADR-0022`'s rules version
   *shapes*, not meanings; a semantic change under an unchanged shape
   would be the first, with nothing to gate it on.
4. **Gate on the negotiated version alone** (`PROTOCOL_VERSION = 5`
   with no new variant; sessions on a version-5 connection overlay).
   Rejected: it is option 3 with a gate, still a meaning change on an
   existing index, and it takes the choice away from a version-5 client
   that wants committed-state reads.

**Which reads.**

1. **`GetById` only (proposed).** The one read keyed by id with fields;
   the overlay is exact and the cost is one linear pass over the
   buffer.
2. **`GetById` and `FilterEq`.** `FilterEq { field, value }` adjusted
   by the buffer: an id whose last staged value for `field` equals
   `value` is added (if the record exists — a `get` per such id), one
   whose last staged value differs is removed. Coherent, and offered
   as `ADR-0027`'s option (b); not proposed, because it is a second
   cost model (probes per staged op, a result set rewritten) for a
   read the session two-step already handles, and because it would
   answer in a different order than the store does.
3. **All reads, `ScanField` included.** Not possible without an id per
   position; rejected.

**Where the overlay lives.**

1. **`handle_connection`, a pure function in `mod.rs` (proposed).** The
   session buffer is per-connection state only `handle_connection`
   has; `dispatch` and `ConnectionStore` never see it, exactly as
   `ADR-0024` placed the session.
2. **On `ConnectionStore`** (a `get_with_overlay`). Rejected: every
   adapter would re-implement one pure function, and `ADR-0024`
   rejected session state on the trait for the same reason.

**Kind mismatch.**

1. **Ignore the staged value, show committed (proposed).** The write
   will fail at `Commit`; a read that showed it would mislead, and a
   client decoding by the schema's kind would break.
2. **Overlay anyway.** Rejected for those two reasons.
3. **Validate at stage time instead.** That is `ADR-0024`'s second
   trigger, a separate decision; the overlay must be correct whether
   or not it is ever taken.

## Proposed shape

```rust
// src/server/protocol.rs
pub const PROTOCOL_VERSION: u32 = 5;
pub const SESSION_READ_YOUR_WRITES: u32 = 1;
pub enum Request { /* 0–13 as today */ BeginWith { flags: u32 } /* 14, protocol 5 */ }

// src/server/mod.rs — handle_connection
let mut session: Option<Vec<TransactionOp>> = None;
let mut read_your_writes = false;
// intercepts, after the existing Begin/Rollback/Commit arms:
Request::BeginWith { .. } if negotiated < 5 => err_response(ErrorCode::Malformed),
Request::BeginWith { flags } => {
    if flags & !SESSION_READ_YOUR_WRITES != 0 { err_response(ErrorCode::Malformed) }
    else if session.is_some() { err_response(ErrorCode::SessionOpen) }
    else { session = Some(Vec::new()); read_your_writes = flags & SESSION_READ_YOUR_WRITES != 0; Response::Ok }
}
Request::GetById { id } if read_your_writes && session.is_some() => {
    match dispatch(store, Request::GetById { id }) {
        Response::Record { id, mut fields } => { overlay_staged(id, &mut fields, staged); Response::Record { id, fields } }
        other => other,
    }
}
// Commit/Rollback clear `read_your_writes` with the session.

/// RYW-FR-002: last staged write per (id, field) wins, kinds must match.
pub(crate) fn overlay_staged(id: RecordId, fields: &mut [(FieldRef, ScanValue)], staged: &[TransactionOp]) {
    for (field, value) in fields.iter_mut() {
        if let Some(op) = staged.iter().rev().find(|op| op.id == id && op.field == *field) {
            if ValueKind::of(&op.value) == ValueKind::of(value) { *value = op.value.clone(); }
        }
    }
}

// src/server/client.rs
impl SchemaDrivenClient {
    pub fn begin_read_your_writes(&mut self) -> Result<Session<'_>, ClientError>; // ≥ 5, else Unsupported, no frame
}
impl Session<'_> {
    pub fn get(&mut self, id: RecordId) -> Result<Option<Vec<(String, ScanValue)>>, ClientError>; // delegates
    pub fn read_your_writes(&self) -> bool;
}
```

`ValueKind::of` (or the equivalent match) may already exist for the
schema; if not, a private kind comparison in `mod.rs` is enough.

## Data/state and invariants

- `read_your_writes` is per-connection, set only by `BeginWith`,
  cleared with the session; a plain `Begin` leaves it `false`.
- The overlay never adds a field, never adds a record, never changes a
  value's kind, and never touches the store — it is a pure function of
  the committed answer and the buffer.
- Overlay order equals `Commit` order: the last staged write per
  `(id, field)` is what `apply_transaction` would leave, so a read
  after `Commit` returns what the overlaid read showed, for every field
  the overlay touched.
- A connection with no session, or a plain session, takes no new
  branch beyond the existing `if session.is_some()` guards.
- Lock discipline unchanged: the overlay runs after `dispatch` returned,
  outside any store lock, on the connection's own thread.

## Errors, failure, recovery, and observability

- `BeginWith` with unknown bits: `Malformed`, nothing opened. Below
  version 5: `Malformed` (rule 3). While a session is open:
  `SessionOpen`.
- A staged write that the overlay ignored (missing id, absent field,
  kind mismatch) surfaces at `Commit` as `TransactionFailed` by index,
  exactly as today; the design guarantees only that the read did not
  show it.
- No new `ErrorCode`, no new `Response`; nothing to downgrade.
- Not observable except through the reads themselves.

## Security, privacy, and compatibility

- A read-your-writes read shows a connection its own unapplied data —
  data it sent. No other connection, class, or token sees anything
  new. `ReadOnly` gating is unchanged: `BeginWith` is admitted as
  `Begin` is, and a `ReadOnly` session's `UpdateField`s are still
  refused before they could be staged.
- Version 1–4 clients are byte-for-byte unchanged (rules 1–3): every
  existing golden vector holds; `Begin`'s semantics hold.
- A version-5 client that sends plain `Begin` gets plain semantics; the
  library exposes both.

## Acceptance criteria

1. `Request::BeginWith` at index 14, protocol 5; `PROTOCOL_VERSION == 5`;
   the version table row; the golden vector; every version 1–4 vector
   unchanged; `BeginWith` below 5 and with unknown bits is `Malformed`;
   `dispatch` maps it to `Unsupported`.
2. In a read-your-writes session, `GetById` on a staged id returns the
   staged value for that field; a second stage of the same field
   returns the later value; every other field is the committed one.
3. A concurrent connection's `GetById` on the same id returns committed
   state throughout; after `Rollback` the session's own read returns
   committed state; after `Commit` both connections return the batch.
4. Edge cases: a staged write to a missing id leaves `NotFound`; to a
   field the record does not carry, no change; of a kind-mismatched
   value, no change — and each then fails at `Commit` by its index.
5. `ScanField` and `FilterEq` in a read-your-writes session return
   committed state; a plain `Begin` session's `GetById` returns
   committed state (`SESS-FR-007` holds).
6. `SchemaDrivenClient::begin_read_your_writes` is `Unsupported` with no
   frame below 5 (the gating test's pre-hello and `Hello { 4 }`
   shapes); `Session::get` works on both session kinds.
7. Every existing test, bench, and binary unchanged apart from the
   unknown-index probe moving to 15; no `Cargo.toml`, store, adapter,
   `ConnectionStore`, or `dispatch`-signature change.

## Verification plan

- `src/server/mod.rs`: unit tests for `overlay_staged` (last wins,
  kind mismatch ignored, absent field ignored, other id ignored) and
  `dispatch_never_routes_session_requests_to_a_store` extended to
  `BeginWith`.
- `src/server/protocol.rs`: the golden vector and the version-table
  pin.
- `tests/server_transaction_integration.rs`: criteria 2–5 on the dog
  fixture, raw protocol and via `SchemaDrivenClient`.
- `tests/server_protocol_version.rs`: criterion 1's gating and
  criterion 6, the probe index moved to 15.
- `tests/server_schema_driven_client.rs`: `Session::get` on every
  domain.

## Traceability

- → `SERVER-001` next minor / next FR (`RYW-FR-001`–`008`), `ADR-0027`;
  resolves `ADR-0024`'s first revisit trigger and `SESS-FR-007` by
  pointer for read-your-writes sessions; cites `ADR-0022` (rules 1–4,
  a version-5 variant).
- Roadmap: `SERVER-SESSION-READ-YOUR-WRITES-DESIGN` (this document),
  then `SERVER-SESSION-READ-YOUR-WRITES` as the implementation unit if
  accepted.

## Open questions

- Whether `Session::get` on a *plain* session should exist at all, or
  only on a read-your-writes one. Proposed yes: the mutable borrow
  makes it the only way to read during a session through the library,
  and committed-state reads during a session are what `ADR-0013`'s
  two-step already assumes a caller can do.
- Whether a future stage-time validation (`ADR-0024`'s second trigger)
  should be a second flag bit in `BeginWith` — the design reserves the
  word for it but decides nothing.
- `FilterEq` adjustment (option (b)) — a second decision if a caller
  ever needs it; the cost model is in "Considered options."

## Change history

- 2026-09-03: Initial proposal, in response to the owner selecting
  read-your-writes in sessions as the second of four next directions
  ("1, 2, 3, 4"). Offered as a bounded change when the options were
  listed; on reading `ADR-0024` ("a real design with a cost on every
  read") and `SESS-FR-007` (a numbered requirement whose meaning would
  change), written up as a design round instead, so the versioning and
  the opt-in are decided rather than assumed. (PR #147.)
- 2026-09-03: Accepted as proposed. No content change. Implementation
  after `ADR-0026`'s unit, as `SERVER-001`'s next minor / FR. (PR #153.)
- 2026-09-03: Implemented as `SERVER-001` v0.18.0 / FR-028 (this PR),
  per the verification plan: acceptance criteria 1–7 hold as written.
  One clarification: `RYW-FR-002`'s overlay also requires the field to
  be schema-updatable (read once at `BeginWith`) — a read-only field of
  the right kind, `Dog::breed`, would otherwise have been shown and then
  refused at `Commit`, which the requirement's own last sentence
  forbids. `ADR-0027`'s acceptance log carries the same note.
