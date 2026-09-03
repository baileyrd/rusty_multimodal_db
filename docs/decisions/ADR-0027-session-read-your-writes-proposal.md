# ADR-0027: Read-your-writes in a transaction session — opt-in at `Begin`, `GetById` only, protocol version 5

- Status: **Accepted** (promoted from Proposed on 2026-09-03 — the owner
  approved the design as proposed, option (a): opt-in via
  `BeginWith { flags }` at protocol 5, `GetById` overlaid, set reads and
  plain sessions unchanged, `Session::get` on every session; (b)
  `FilterEq` adjusted too and (c) close as not warranted declined; no
  changes requested). Acceptance authorizes the design; implementation
  follows as its own unit, after `ADR-0026`'s — see "Acceptance and
  implementation" below.
- Date: 2026-09-03
- Deciders: baileyrd
- Related: `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md` (the
  full design this ADR summarizes), `ADR-0024` /
  `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part A (the
  buffered session, `SERVER-001` v0.14.0 / FR-024; `SESS-FR-007`'s
  committed-state reads and the first revisit trigger — *read-your-
  writes inside a session becomes wanted — a per-connection overlay on
  the read paths, a real design with a cost on every read* — are what
  this answers), `ADR-0022` / `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md`
  (rules 1–4; this is a version-5 variant, cited as that design
  requires), `ADR-0013` (the read-then-stage two-step this relaxes for
  one read), `docs/specifications/server/SERVER-001-query-layer.md`
  v0.16.0.
- Supersedes/Superseded by: none. Adds a second way to open a session;
  changes nothing `Begin` does.

## Context

A session stages writes and applies them at `Commit`; `SESS-FR-007`
says a read in between — on this connection or any other — sees
committed state. `ADR-0024` chose that deliberately (the overlay was
"a real design with a cost on every read") and told a caller who needs
to read then write to read first, then stage. The owner picked the
revisit.

Two facts shape the answer. First, `SESS-FR-007` is a numbered
requirement a version-3/4 client may rely on, and `ADR-0022`'s rules
version *shapes*, not meanings — so read-your-writes cannot simply be
switched on for every session; it needs a shape a client asks with.
Second, only one read is keyed by id and carries fields — `GetById` —
so only one read can be overlaid exactly, at a cost linear in the
buffer; the set reads (`ScanField` by position, `FilterEq` by value)
cannot, or only with a second cost model.

A third fact, found while reading the client: a `Session` borrows the
`SchemaDrivenClient` mutably, so today *no* read can be issued through
the library while a session is open. Any read-your-writes design must
also add `Session::get`, or the feature would be unreachable from the
library.

The owner selected this as the second of four directions. This ADR
proposes a design and authorizes no implementation — the posture
`ADR-0016` through `ADR-0026` took.

## Decision drivers

- Let a session read its own staged writes without changing what any
  plain session, any older client, or any other connection sees.
- Add exactly one shape, under `ADR-0022`'s rules, extensible without
  a variant per option.
- Make the overlay exact where it applies and honest where it cannot
  (a staged write that will fail at `Commit` must not produce a read
  that says otherwise).
- Pay the cost only on the sessions and the reads that asked.

## Considered options

1. **Opt-in `BeginWith { flags: u32 }` at protocol 5, `GetById`
   overlaid, last staged write per field wins, kinds must match** —
   proposed. A pure function in `handle_connection`, where the buffer
   lives; `Session::get` and `begin_read_your_writes` in the library.
2. **The same, plus `FilterEq`** adjusted by the buffer (ids added when
   a staged value matches, if the record exists; removed when it
   differs). Coherent; offered as option (b). Not proposed: a second
   cost model — an existence probe per staged op on that field, a
   result set rewritten — for a read the two-step already covers.
3. **Change `Begin`'s meaning** (every session overlays). Rejected: a
   meaning change on an existing index, with nothing to gate it on.
4. **Gate on negotiated version alone.** Rejected: option 3 with a
   gate; takes the choice from a version-5 client that wants committed
   reads.
5. **A unit variant `BeginReadYourWrites`.** A fair alternative; the
   flags word is chosen so the next session option (stage-time
   validation, `ADR-0024`'s second trigger) is a bit, not a variant.
6. **On `ConnectionStore`.** Rejected: every adapter re-implementing
   one pure function; `ADR-0024` kept session state off the trait for
   the same reason.
7. **Close** — reads stay committed-state; the two-step stands. Offered
   as option (c).

## Decision

Proposed: option 1. Concretely, at implementation:

- `src/server/protocol.rs`: `Request::BeginWith { flags: u32 }` (index
  14), `SESSION_READ_YOUR_WRITES = 1`, `PROTOCOL_VERSION = 5`, table
  row 5, golden vector.
- `src/server/mod.rs`: the `BeginWith` intercept (unknown bits →
  `Malformed`; below 5 → `Malformed`; `SessionOpen` as `Begin`), a
  per-connection `read_your_writes` flag cleared with the session, the
  `GetById` overlay arm, `overlay_staged` as a pure `pub(crate)`
  function; `dispatch` maps `BeginWith` to `Unsupported`.
- `src/server/client.rs`: `begin_read_your_writes()` gated on ≥ 5,
  `Session::get`, `Session::read_your_writes()`.
- `tests/server_protocol_version.rs`: the unknown-index probe moves to
  15.
- `SERVER-001`'s next minor / FR (`RYW-FR-001`–`008`); `SESS-FR-007` and
  `ADR-0024`'s consequence resolved by pointer for read-your-writes
  sessions and restated for plain ones; `SPEC-REGISTRY`,
  `TRACEABILITY`, `ROADMAP` (`SERVER-SESSION-READ-YOUR-WRITES`),
  `PROJECT-STATUS`.
- Tests per the design's verification plan: overlay unit tests, the
  golden vector, the two-connection visibility test, the edge cases,
  the set-read and plain-session controls, the client gate.
- No `Cargo.toml`, store, adapter, `ConnectionStore`, `dispatch`-
  signature, or `Response`/`ErrorCode` change.

## Consequences

### Positive

- A session can read what it staged, on the one read where that is
  exact, at a cost paid only by sessions that asked for it.
- Every older client, every plain session, every other connection, and
  every set read keep `SESS-FR-007` unchanged — the meaning of nothing
  that shipped moves.
- The library finally has a read inside a session (`Session::get`),
  useful even without the overlay.
- The flags word gives the next session option a home without a
  variant.

### Negative / tradeoffs

- **Partial by design.** `GetById` sees staged writes; `ScanField` and
  `FilterEq` do not. A caller must know which — documented on
  `BeginWith`, on `Session::get`, and in the spec, but it is a seam.
- **Another version bump** (5) for one variant — the cost `ADR-0022`
  chose to pay per variant; the table, a vector, a client gate, and
  the probe index moving are the installment.
- **The overlay hides nothing and fixes nothing**: a staged write that
  will fail at `Commit` is invisible to the read (by design) and still
  fails at `Commit`. Stage-time validation is the other trigger, not
  this one.
- **Linear in the buffer per read**, up to `MAX_STAGED_OPS` (4096)
  comparisons per `GetById` in a full session — microseconds, on the
  connection's own thread, outside any lock; named.

## Validation and revisit triggers

- **Design-only at proposal time**, matching `ADR-0013` through
  `ADR-0026`. Every claim about the current code (the session
  intercept, `dispatch`'s read arms, `Session`'s mutable borrow,
  indices 0–13, the probe at 14) was read from `main` rather than
  recalled; the mechanism is a pure function and needs no probe.
- Revisit if: `FilterEq` under staged writes becomes wanted — option 2,
  a second flag bit or the same one, with its cost model.
- Revisit if: stage-time validation is taken (`ADR-0024`'s second
  trigger) — a second bit in `BeginWith`; the kind-mismatch rule here
  then never fires, and stays as belt-and-braces.
- Revisit if: `MAX_STAGED_OPS` grows enough that a linear overlay is
  measurable — an index over the buffer, built at stage time.

## Acceptance and implementation

- Options offered at proposal: **(a)** accept as proposed — opt-in via
  `BeginWith { flags }` at protocol 5, `GetById` overlaid, set reads
  unchanged, `Session::get` on every session; **(b)** accept with
  `FilterEq` adjusted too — the same, plus ids added and removed by
  the buffer's last staged value per id, with an existence probe per
  staged op on that field; **(c)** close as not warranted — reads stay
  committed-state, `ADR-0013`'s two-step stands, `ADR-0024`'s trigger
  stays armed. Proposed in PR #147.
- 2026-09-03: accepted as proposed (option (a); (b) and (c) declined).
  Implemented after `ADR-0026`'s unit, as `SERVER-001`'s next minor /
  FR, per `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md`.
  (PR #153.)
