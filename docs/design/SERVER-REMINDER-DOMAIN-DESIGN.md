# Server Reminder Domain Design (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-04 — the
  owner approved the design as proposed, `ADR-0036` option (a); (b)
  keeping `Reminder` behind `research` and (c) closing as not
  warranted both declined; no changes requested). Acceptance
  authorizes the design; implementation follows as its own unit — see
  `ADR-0036`'s "Acceptance and implementation" section.
- Date: 2026-09-04
- Related: `ADR-0009`/`docs/design/GENERIC-SCHEMA-DESIGN.md` (the generic
  schema library, `crate::generic`, this design's engine), `ADR-0018`/
  `docs/design/SYMMETRIC-EDGE-PORTABILITY-DESIGN.md` (the durability
  companion machinery `GenericMmapStore` already provides), `SERVER-001`
  (`docs/specifications/server/SERVER-001-query-layer.md` — `FR-004`
  `Dog`, `FR-005` `Order`/`Customer`, `FR-012` `Employee`: the three
  existing domain adapters this proposal follows the identical shape
  of), `ADR-0034`/`ADR-0035` (`Request::Query`/`Request::Aggregate` —
  already fully generic across domains, needing nothing new here).
- Supersedes/Superseded by: none. Adds one new domain (`Reminder`) and
  its `ConnectionStore` adapter; changes no existing `Request`/
  `Response` variant, no `PROTOCOL_VERSION`, no existing domain's
  behavior.

## Purpose and scope

This session's own owner asked, in conversation rather than from
`docs/FUTURE-GROWTH.md`, whether this crate could back a real external
tool — `rusty_remind_me`, a separate MCP-exposed reminder/memory
system. A short scoping discussion (not itself part of this document)
concluded that `rusty_remind_me`'s full surface (semantic search,
schema-less memory content, entity graphs, provenance/history) is a
genuinely different kind of database than this crate builds, and is
out of scope for one bounded round. What *does* fit cleanly, with zero
new storage-adjacent primitives: the reminders themselves — a small,
fixed-schema record (title, due time, status) that is exactly the
shape every existing domain in this crate already handles.

This document proposes exactly that: a fourth domain, `Reminder`,
built on the already-accepted generic schema library
(`crate::generic::production::GenericProductionStore`) and exposed
over the already-accepted server/query layer (`SERVER-001`) — reusing
`Request::GetById`/`UpdateField`/`FilterEq`/`ScanField`/`Query`/
`Aggregate` exactly as they exist today, with no wire-protocol change
at all. A real, standalone `reminder_server` binary (mirroring
`dog_server`) is also proposed, since a domain with no runnable server
would not actually be reachable by an external client — the stated
motivation for this round.

**This document does not build an integration with `rusty_remind_me`
itself.** `rusty_remind_me`'s own codebase was not attached to this
session and its real reminder schema was not read — the field
shape below is inferred from its MCP tool *names*
(`set_reminder`/`list_reminders`/`reminders_ics_url`) alone, named as
an explicit assumption in "Open questions," not a verified fact.
Wiring an actual bridge (an MCP tool or client calling
`SchemaDrivenClient` against a running `reminder_server`) is a
separate, follow-on unit, scoped only once that assumption is checked
against the real target.

## Non-goals

- Not `rusty_remind_me`'s full feature surface — no full-text or
  semantic search, no schema-less/arbitrary-shaped memory content, no
  entity graph, no provenance/history/revert, no multi-peer sync. Each
  is a real, separate, and substantially larger effort than this
  round (see this session's own scoping discussion); `docs/FUTURE-
  GROWTH.md` is not amended to name them, since they were never
  sourced from it — recorded here instead, as this proposal's own
  explicit boundary.
- Not an MCP bridge or any change to `rusty_remind_me`'s own
  repository — this crate gains a domain and a server binary a client
  *could* connect to; nothing here writes the client.
- Not a new storage-adjacent primitive, wire shape, `PROTOCOL_VERSION`,
  or `ErrorCode` — `Reminder` is deliberately the *simplest* domain
  shape this generic schema library supports (see "Proposed shape"):
  no relation of either kind, so — unlike every existing generic-
  library domain — no `Symmetric`/`Reversed` composition layer is
  needed at all, just `GenericMmapStore` directly.
- Not recurrence, snoozing logic, timezones, or calendar-feed
  generation (the real work behind `reminders_ics_url`) — `due_at` is
  a single stored instant (Unix milliseconds, the same representation
  `Order::created_at_unix_ms` already uses), full stop; any richer
  scheduling model is a separate, future decision, not assumed here.
- Not moving the generic schema library's example domains
  (`Order`/`Customer`, `Employee`) out from behind `research` — this
  proposal makes `Reminder` the *first* front-door (non-`research`)
  consumer of `crate::generic`, a deliberate first, not a retroactive
  change to the other two (see "Considered options").

## Context and terminology

- **`crate::generic`** (`GENERIC-SCHEMA-DESIGN`, `STORAGE-012`) is
  already front-door/unconditional — only the two domains built on it
  so far (`order_customer`, `generic_spike::employee_impl`) are
  `research`-gated, as reference/validation material for the library
  itself, not because the library is experimental. Nothing today
  demonstrates the generic library as real, deployable capability; this
  proposal is that demonstration.
- **`GenericMmapStore<R, IndexMarker, ScanMarker>`** (the durable core
  every generic-schema domain wraps) always takes exactly one
  `IndexedField` (equality-filterable, read-only over the wire in
  every existing domain) and exactly one `ScannableField` (durably
  mutable via `UpdateField`) — a structural constraint of the type
  itself, not a per-domain choice; `Order`/`Employee` both put an enum
  (`Status`/`Department`) in the index slot and a plain number
  (`Amount`/`SalaryCents`) in the scan slot. This proposal inverts
  that: `Reminder` puts its enum (`status`) in the *scan* slot instead
  — a new combination (see "Considered options" for why).
- **`Request::Query`/`Request::Aggregate`** (`ADR-0034`/`ADR-0035`)
  already filter/sort/aggregate on *any* schema-described field via a
  full scan, independent of that field's `filter_eq`/`scan`/`update`
  capability flags (`Dog::breed`, every flag `false`, is the existing
  proof). This matters here: `Reminder`'s `IndexedField` choice affects
  only the low-level `Request::FilterEq`, not whether a field is
  usable in SQL — lowering the stakes of that choice considerably
  relative to when `Order`/`Employee` were designed, before `Query`
  existed.

## Requirements

- `RMD-FR-001` — **The `Reminder` record**, `src/generic/reminder.rs`
  (new, front-door — not behind `research`): `id: Uuid`, `title:
  String`, `due_at_unix_ms: i64`, `status: ReminderStatus` where
  `ReminderStatus::{Pending, Done, Snoozed, Cancelled}`. `Record`,
  `SchemaTag` (`"reminder::Reminder"`), `Serialize`/`Deserialize`
  (`GenericMmapStore`'s companion record blob) implemented the same
  way every existing generic-schema record already is.
- `RMD-FR-002` — **`status` is the `ScannableField`** (marker
  `StatusField`), encoded as its `u32` discriminant
  (`Pending`=0/`Done`=1/`Snoozed`=2/`Cancelled`=3, the same fixed-
  discriminant-mapping shape `server::order`'s `status_to_u32`/
  `status_from_u32` already established) — durably mutable via
  `Request::UpdateField`/`Session::update`, so marking a reminder done
  needs no SQL round trip. A new combination for this library (every
  existing `ScannableField` has been a plain number); `type ScanValue
  = u32: Copy` satisfies the trait bound unchanged.
- `RMD-FR-003` — **`due_at_unix_ms` is the `IndexedField`** (marker
  `DueAtField`), `type IndexValue = i64: Eq + Hash + Clone` —
  equality-filterable via `Request::FilterEq` (an exact-timestamp
  lookup; narrow, but real — e.g. a scheduler polling "what's due at
  exactly this tick"), read-only over the wire otherwise. Range
  queries (`due_at < now`, the actually common case) go through
  `Request::Query`'s `WHERE due_at < ...`, unaffected by this field's
  `filter_eq`-only capability, per `RMD-FR-006` below.
- `RMD-FR-004` — **`title` is read-only over the wire** — present in
  every `GetById`/`Query` result, never independently `scan`/
  `update`/`filter_eq`-able (the identical shape `Order`'s
  `created_at_unix_ms`/`discount_cents` and `Employee`'s `name`
  already have: a real field with every capability flag `false`).
- `RMD-FR-005` — **No relation of either kind** —
  `RelationCapabilities { parent_children: false, neighbors: false }`,
  the one combination no existing adapter has (`Dog`: neighbors only;
  `Order`: parent/children only; `Employee`: both). `parent`/
  `children`/`neighbors` all report `ErrorCode::Unsupported`, the
  identical shape `Dog::parent`/`Dog::children` already use for their
  own missing half. `ReminderProductionStack` is therefore
  `GenericMmapStore<Reminder, DueAtField, StatusField>` directly — no
  `Indexed`/`Scanned`/`Symmetric`/`Reversed` composition layer at all,
  the simplest domain shape this library supports (every existing
  generic-schema domain needed at least one relation layer).
- `RMD-FR-006` — **`ReminderConnectionStore`**
  (`src/server/reminder.rs`, gated by `server` alone — *not*
  `server + research`, matching `Dog`'s precedent, not `Order`/
  `Employee`'s) implements `ConnectionStore` exactly as `OrderConnectionStore`/
  `EmployeeConnectionStore` already do: `get`/`scan_all` reconstruct
  every field per id; `filter_eq` supports `due_at_unix_ms` only
  (`Unsupported` for `status`/`title`, `UnknownField` otherwise);
  `scan_field`/`update_field` support `status` only, validating the
  incoming discriminant against the four known values (`Malformed` on
  an unrecognized one, the identical shape `status_from_u32`'s own
  `filter_eq` validation already establishes, now reused on the update
  path too — the one genuinely new validation code path this round
  needs, small and precedented); `describe` reports the schema above
  honestly. No new `ErrorCode`, no `Response` change, no `dispatch`
  change beyond a new match arm identical in shape to the other three.
- `RMD-FR-007` — **`journal`/`validate_op`/`apply_transaction`/read-set
  checking** on `ReminderConnectionStore` mirror `OrderConnectionStore`/
  `EmployeeConnectionStore` exactly (`with_journal`, the
  `validate_batch`-then-`apply_batch` shape, `check_read_set` for
  snapshot isolation) — `Request::Transaction`, sessions (read-your-
  writes, stage-time validation, snapshot isolation), and journaled
  crash-atomicity all work for `Reminder` with zero new code beyond
  the mechanical per-domain repetition every existing adapter already
  has. Nothing here is new capability; it's the fourth instance of an
  already-proven pattern.
- `RMD-FR-008` — **A real, runnable `reminder_server` binary**
  (`src/bin/reminder_server.rs`, `required-features = ["server"]`,
  mirroring `dog_server.rs`'s shape and env-var reading —
  `SERVER_TXN_JOURNAL_PATH`, `SERVER_AUTH_*`, `SERVER_TLS_*`,
  `SERVER_AUDIT_LOG`, `SERVER_ACCESS_LOG`, `SERVER_AUTH_RATE_LIMIT`,
  every operational knob `dog_server` already exposes, unchanged in
  shape) — seeded from a small hand-written sample dataset, not
  `generator` (which is `Dog`-specific and research-gated). Without a
  runnable binary this domain would be reachable only from an
  in-process test, the same gap that would make this round fail its
  own stated motivation.
- `RMD-FR-009` — **`tests/server_reminder_integration.rs`** (new,
  `required-features = ["server"]` only — no `research` needed, since
  nothing here depends on `order_customer`/`employee_impl`) covering:
  a real client round trip (`get`, `filter_eq` on `due_at_unix_ms`,
  `update_field`/`Session::update` marking a reminder `Done`, an
  unrecognized status discriminant rejected `Malformed`), `Query`/
  `Aggregate` against the domain (`SELECT * FROM reminder WHERE
  due_at_unix_ms < ... `, `SELECT status, COUNT(*) FROM reminder GROUP
  BY status`), `Transaction`, and the version-8/9 gates already proven
  domain-agnostic by `tests/server_sql_integration.rs`. Every relation
  request (`parent`/`children`/`neighbors`) confirmed `Unsupported`.

## Considered options

**Where the generic schema library's example domains have lived.**
Every domain built on `crate::generic::production::GenericProductionStore`
so far (`Order`/`Customer`, `Employee`) has been `research`-gated —
reference/validation material proving the library works, not a real
deployment target. Three options for `Reminder`:

1. **(Proposed) Front-door, `server`-gated only** — `src/generic/
   reminder.rs` with no `#[cfg(feature = "research")]`, `src/server/
   reminder.rs` gated by `server` alone. Makes the generic schema
   library's first real, deployable, non-`research` appearance —
   directly serving this round's own motivation (a domain an external
   tool could actually reach without building from `--all-features`).
   Cost: this crate has never shipped a `crate::generic`-backed domain
   outside `research` before, so this is genuinely new ground, not a
   mechanical repeat of `Order`/`Employee`'s own wiring — named
   plainly, not hidden.
2. **`research`-gated, matching `Order`/`Employee`'s precedent
   exactly.** Cheaper to justify (identical shape to two already-
   accepted rounds), but directly undercuts the stated purpose: a
   `research`-gated domain is not one an external consumer's default
   build would ever compile in.
3. **Hand-rolled, bespoke store** (mirroring `Dog`/`ProductionStore`
   instead of the generic library) — avoids relying on `crate::generic`
   at all. Rejected: `Dog`'s bespoke store predates the generic schema
   library and has never been revisited specifically *because* the
   generic library exists to replace that duplication going forward
   (`ADR-0009`'s own stated purpose); building a fourth domain the old
   way would be a real step backward, not a neutral choice.

Option 1 (front-door) proposed.

**Which field is `ScannableField` vs. `IndexedField`.** Every existing
domain put its enum in the index slot and a plain number in the scan
slot. For `Reminder`, the field that actually changes over a record's
lifecycle is `status` (pending → done/snoozed/cancelled), and the
field a caller most wants to search on structurally is `due_at`. Since
`Request::Query` already makes range/ordering search on `due_at`
possible regardless of its `filter_eq` capability (`RMD-FR-003`), there
is no real cost to inverting the usual assignment; the benefit — real
`UpdateField`-level "mark done" support, no SQL required — is
concrete. Considered and rejected: keeping the usual assignment
(`status` indexed, `due_at` scanned) would leave status changes only
reachable by rewriting the whole record via a hypothetical future
`Request::Delete`+`Insert` pair (this schema has no such pair and
none is proposed here) — a real, avoidable gap.

**A companion `reminder_server` binary vs. adapter-and-tests only**
(`Order`/`Employee`'s own precedent — neither has a standalone
binary). Proposed: build the binary. Rejected staying adapter-only:
this round's whole motivation is a domain an external tool can
actually connect to; without a runnable server that's still not true,
no matter how complete the adapter is.

## Proposed shape

```rust
// src/generic/reminder.rs (new, front-door)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReminderStatus { Pending, Done, Snoozed, Cancelled }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: Uuid,
    pub title: String,
    pub due_at_unix_ms: i64,
    pub status: ReminderStatus,
}

impl Record for Reminder { type Id = Uuid; fn id(&self) -> Uuid { self.id } }
impl SchemaTag for Reminder { const SCHEMA_TAG: &'static str = "reminder::Reminder"; }

pub struct DueAtField;
impl IndexedField<DueAtField> for Reminder {
    type IndexValue = i64;
    fn indexed_value(&self) -> &i64 { &self.due_at_unix_ms }
}

pub struct StatusField;
impl ScannableField<StatusField> for Reminder {
    type ScanValue = u32; // ReminderStatus's discriminant — RMD-FR-002
    fn scannable_value(&self) -> u32 { status_to_u32(self.status) }
    fn set_scannable_value(&mut self, value: u32) {
        // Only ever called with an already-validated discriminant —
        // the adapter's own validate_batch/status_from_u32 rejects
        // anything else before this runs, same shape filter_eq's own
        // status_from_u32 already established.
        self.status = status_from_u32(value).expect("validated by the adapter");
    }
}

// No Symmetric/Reversed layer — RMD-FR-005, no relation of either kind.
pub type ReminderProductionStack = GenericMmapStore<Reminder, DueAtField, StatusField>;

pub fn create_reminder_production_stack(
    reminders: Vec<Reminder>,
    path: &Path,
) -> Result<ReminderProductionStack, DurabilityError> {
    GenericMmapStore::<Reminder, DueAtField, StatusField>::create(reminders, path)
}
```

```rust
// src/server/reminder.rs (new, `server`-gated only — RMD-FR-006)

pub const FIELD_TITLE: FieldRef = 0;
pub const FIELD_DUE_AT: FieldRef = 1;
pub const FIELD_STATUS: FieldRef = 2;

pub struct ReminderConnectionStore {
    store: GenericProductionStore<ReminderProductionStack>,
    journal: Option<CommitGroup>, // RMD-FR-007, identical shape to Order/Employee
}

impl ConnectionStore for ReminderConnectionStore {
    // get/scan_all: reconstruct all three fields per id, the Order/Employee shape.
    // filter_eq: FIELD_DUE_AT only (Ok(self.store.filter_eq::<Reminder, DueAtField>(&ts))),
    //            FIELD_STATUS/FIELD_TITLE => Unsupported.
    // scan_field/update_field: FIELD_STATUS only, validating the discriminant
    //            (Malformed on an unrecognized value) — the one new
    //            validation path this round adds.
    // parent/children/neighbors: Unsupported, unconditionally — RMD-FR-005.
    // describe: the schema RMD-FR-001–005 name, honestly reported.
    // apply_transaction/validate_op: identical shape to
    //            OrderConnectionStore/EmployeeConnectionStore — RMD-FR-007.
}
```

`src/server/mod.rs`'s `dispatch` needs no change at all — it is already
generic over `S: ConnectionStore`; a fourth implementor is invisible to
it. No `Request`/`Response` variant, `PROTOCOL_VERSION`, or `ErrorCode`
changes anywhere in this proposal.

## Data/state and invariants

- No new persistent format — `ReminderProductionStack` is
  `GenericMmapStore<Reminder, DueAtField, StatusField>`, the same mmap
  + companion-record-blob shape (`STORAGE-015`) every generic-schema
  domain already uses; `Reminder`'s own `SchemaTag`
  (`"reminder::Reminder"`) is the only new on-disk-format value, and it
  is exactly the mechanism `Order`/`Employee` already established for
  this purpose.
- `status` durably lives in the mmap file's scan slot (one `u32` per
  record, the same storage `Amount`/`SalaryCents` already use); `title`
  lives only in the companion record blob (rewritten on `open`, same
  as `Order::created_at_unix_ms`/`discount_cents`).
- No relation state at all — no edge blob, no `Symmetric`/`Reversed`
  layer, nothing `STORAGE-016`'s edge-list portability work touches.

## Errors, failure, recovery, and observability

- No new `ErrorCode`. `UnknownField`/`Unsupported`/`Malformed`/
  `RecordNotFound` cover every rejection shape, identically to every
  existing domain.
- An `update_field`/`Transaction`/session write to `status` with a
  `u32` outside `0..=3` is `ErrorCode::Malformed`, nothing applied —
  the same "reject before any write" posture every existing domain's
  `validate_batch` already guarantees, now covering an update path for
  the first time (every prior enum-shaped rejection was `filter_eq`-
  only).
- `reminder_server`'s own startup/audit/access-log/rate-limit
  observability is identical to `dog_server`'s — no new environment
  variable, no new sink, no new log line shape.

## Security, privacy, and compatibility

- No wire-protocol change of any kind — a connection negotiated at any
  version behaves identically whether or not it ever talks to a
  `Reminder`-wrapping server; `PROTOCOL_VERSION` is untouched.
- `reminder_server` reuses every existing `ServeOptions` mechanism
  (tokens, mTLS, certificate classing, rate limiting, audit/access
  logs) unchanged — a deployment gets the identical security posture
  `dog_server` already offers, opt-in the same way.
- A reminder's `title` is free-text and travels in plain `ScanValue::Str`
  the same way `Dog::breed`/`Employee::name` already do — no new PII
  handling is introduced or claimed; an operator choosing to store
  sensitive reminder text inherits exactly the transport/auth posture
  `TlsConfig`/`ServeOptions` already provide, not a new guarantee.

## Acceptance criteria

1. `Reminder`/`ReminderStatus`/`DueAtField`/`StatusField`/
   `ReminderProductionStack` exist exactly as specified, front-door
   (not behind `research`); `ReminderConnectionStore` exists behind
   `server` alone.
2. `GetById`/`Query`/`Aggregate` against a `reminder_server` return
   every field correctly, including a `GROUP BY status` count matching
   a hand-computed tally.
3. `FilterEq` on `due_at_unix_ms` returns exactly the matching ids;
   `FilterEq` on `status`/`title` is `Unsupported`.
4. `UpdateField`/`Session::update` on `status` with a valid
   discriminant (0–3) succeeds and is immediately visible to a
   subsequent `GetById`; an invalid discriminant is `Malformed` with
   nothing applied. `UpdateField` on `due_at_unix_ms`/`title` is
   `Unsupported`.
5. `parent`/`children`/`neighbors` are `Unsupported` unconditionally.
6. `Request::Transaction`, every session kind (read-your-writes,
   stage-time validation, snapshot isolation), and journaled crash-
   atomicity all work against `Reminder` with the same acceptance
   shape every existing domain's own tests already establish.
7. `reminder_server` builds and runs under `cargo run --bin
   reminder_server --features server` alone — no `research` feature
   required.
8. Every existing test in `tests/server_*.rs` — including every
   `Order`/`Employee` test — is unchanged; adding `Reminder` costs
   nothing to a caller that never uses it.

## Verification plan

- `src/generic/reminder.rs` unit tests: `IndexedField`/`ScannableField`
  round trips, the `status_to_u32`/`status_from_u32` mapping (every
  variant, plus an out-of-range value rejected), `create`/`open`
  round-tripping a small fixture set.
- `src/server/reminder.rs` unit tests (the `OrderConnectionStore`/
  `EmployeeConnectionStore` precedent): `get`, `filter_eq` by
  `due_at_unix_ms`, `scan_field`/`update_field` on `status` including
  the invalid-discriminant rejection, `describe`'s reported
  capabilities, every relation method's `Unsupported`.
- `tests/server_reminder_integration.rs` (new, `required-features =
  ["server"]`): a real client round trip covering acceptance criteria
  2–7 above over a real socket, plus the version-8/9 SQL gates already
  proven domain-agnostic.
- A manual `cargo run --bin reminder_server --features server` smoke
  check — confirms acceptance criterion 7 directly, not just by
  `required-features` inspection.

## Traceability

- → `SERVER-001` next minor / FR (`RMD-FR-001`–`009`), a new ADR — the
  identical "domain adapter, same spec" shape `FR-004`/`FR-005`/
  `FR-012` already established for `Dog`/`Order`/`Employee`.
- Not sourced from `docs/FUTURE-GROWTH.md` — recorded here, in this
  document's own "Purpose and scope," as this round's real origin.

## Open questions

- Whether `rusty_remind_me`'s real `set_reminder`/`list_reminders`
  shape actually matches the three-field guess this document makes —
  unverified, since that repository was not read this session. The
  follow-on integration unit (out of scope here) should check this
  directly against `rusty_remind_me`'s own source before assuming the
  schema above is sufficient.
- Whether `Order`/`Employee` should ever move out from behind
  `research` now that `Reminder` proves the generic library works
  front-door — named, not decided; a separate, later call, and not
  required for this proposal to stand on its own.
- Whether recurrence/snoozing/timezone handling is ever wanted as a
  richer `due_at` model — explicitly out of scope (see "Non-goals"),
  revisited only if a real need is named.

## Change history

- 2026-09-04: Initial proposal, in response to the owner's own
  question ("could we use this DB for `rusty_remind_me`?") rather than
  a `docs/FUTURE-GROWTH.md` item — scoped down, after discussion, to
  the one slice that fits this crate's existing architecture without
  new engine capability: a `Reminder` domain on the already-accepted
  generic schema library, made front-door for the first time.
- 2026-09-04: Accepted as proposed. No content change. Implementation
  follows as `SERVER-001`'s next minor / FR.
- 2026-09-04: Implemented as `SERVER-001` v0.29.0 / FR-039, exactly
  this document's "Proposed shape" with no deviation. `Reminder`/
  `ReminderStatus`/`DueAtField`/`StatusField`/`ReminderProductionStack`
  landed front-door in `src/generic/reminder.rs`, not behind
  `research`; `ReminderConnectionStore` landed in `src/server/
  reminder.rs`, gated by `server` alone; `reminder_server` mirrors
  `dog_server`'s shape exactly, confirmed listening by a real smoke
  run. `CheckpointFlush` needed one small addition beyond the design's
  own worked examples, not a deviation from them: `Reminder`'s
  crash-atomic journal path (`RMD-FR-007`) requires
  `ReminderProductionStack` to implement `CheckpointFlush`
  (`src/server/journal.rs`), the same trait `OrderProductionStack`/
  `EmployeeProductionStack` already implement — added directly,
  delegating to `GenericMmapStore`'s already-generic `Flush`, since
  `Reminder` needs no wrapping `Symmetric`/`Reversed` layer to forward
  through. Every acceptance criterion 1–8 holds.
