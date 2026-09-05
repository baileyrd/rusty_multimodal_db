# Server Entity Aliases on the Wire (Accepted)

- Status: **Accepted** (promoted from Proposed on 2026-09-05 — the
  owner approved the design as proposed, `ADR-0041` option (a):
  `ScanValue::StrList`/`ValueKind::StrList` at protocol 11, `aliases`
  readable via `GetById`/`Query` with every capability flag `false`,
  rule-3 content stripping in `downgrade_for_version` for every client
  below 11; (b) the separator-joined `Str` fallback and (c) closing as
  not warranted both declined). Acceptance authorizes the design;
  implementation follows as its own unit — see `ADR-0041`'s
  "Acceptance and implementation" section.
- Date: 2026-09-05
- Related: `ADR-0040`/`docs/design/SERVER-ENTITY-ALIASES-DESIGN.md`
  (named `aliases`' own wire-readability as the one deferred piece of
  that round — this round closes it), `SERVER-001-FR-042` (the merged
  shape this builds on: `aliases: Vec<String>` durable, no `FieldRef`,
  no wire representation), `ADR-0035`/`SERVER-001-FR-038` (the `F64`
  precedent — the last time `ScanValue` gained a variant, and the
  reasoning this round has to argue *against* on one point),
  `ADR-0022` (the append-only rules; rule 3's "nearest older shape" is
  the load-bearing clause here), `ADR-0025`/`ADR-0033` (the two
  existing `downgrade_for_version` cases this round extends).
- Supersedes: none. Additive — `Entity`'s fields, relations, indices,
  and every existing `Request`/`Response` variant's bytes are unchanged.

## Purpose and scope

`ADR-0040` shipped `aliases` as a durable field a client can *resolve
by* (`FilterEq` on `label` matches any alias) but cannot *read back*:
`ScanValue` has no list variant, so `aliases` was given no `FieldRef`
and appears in no `GetById`/`Query` response — named there as a
genuinely new "durable but not wire-representable" category, and
deferred with two candidate mechanisms (a new `ScanValue` variant vs.
remodeling aliases as edges).

This document picks the first mechanism and works out the one part
that is not a repeat of any prior variant addition: **backward
compatibility for clients that predate the variant.** Every earlier
appended variant either was itself a new request (gated by rule 4 —
the client never sends it below the version) or rode only inside a
gated response (`F64` inside `Response::Groups`, protocol 9). A list
value inside `Response::Record` is neither: `GetById` is a protocol-1
request a silent client can send, and its answer would now carry a
variant that client cannot decode. Rule 3 says what to do — answer in
the nearest older shape — and `downgrade_for_version` (`src/server/
mod.rs:716`) is where that already happens; this round is the first
time it must rewrite a response's *content* rather than remap an
`ErrorCode`.

## Non-goals

- **Not making `aliases` filterable, scannable, updatable, orderable,
  or aggregatable over the wire.** Every capability flag stays `false`.
  Resolving *by* alias is `FilterEq` on `label` (`FR-042`), unchanged.
  A `WHERE aliases = ..` predicate has no SQL literal shape and is a
  client-side `ClientError::Sql` from `resolve_literal`'s existing
  fallthrough; server-side `value_matches_kind` is a `matches!` with no
  list arm, so a hand-built predicate is `Malformed`. Writing `aliases`
  is `Unsupported`, the same as `label`/`kind`. This is a **read-only
  projection** of a field that already exists, nothing more.
- **Not a general list type.** `StrList(Vec<String>)` only — no
  `I64List`, no nested values, no heterogenous lists. One real field
  needs one real shape; `docs/FUTURE-GROWTH.md`'s "dynamic schema" is
  not this round.
- **Not changing `NameIndex`, normalization, or lookup semantics.**
  The value returned is the raw, un-normalized `Vec<String>` exactly as
  stored — what the caller wrote, not the lowercased/trimmed keys the
  index holds.
- **Not remodeling aliases as edges to string-keyed nodes.** Examined
  and rejected in `ADR-0040`'s own "Considered options"; nothing here
  changes that reasoning (relations connect `Record`-typed values; a
  bare `String` is not a `Record`).
- **Not touching the on-disk format.** `Entity`'s struct is unchanged;
  this round is wire-only.

## Context and terminology

The real merged shape (`SERVER-001-FR-042`), read directly:

- `ScanValue` (`src/server/protocol.rs:162`) has five variants —
  `U32`/`I64`/`Bool`/`Str`/`F64` — and derives `Debug, Clone,
  PartialEq, Serialize, Deserialize`. `Vec<String>` satisfies every one
  of those; a sixth variant appended at index 5 changes no existing
  variant's bytes (rule 1).
- `ValueKind` (`protocol.rs:285`) has four — `U32`/`I64`/`Bool`/`Str`,
  deliberately **no `F64`**: ADR-0035 reasoned that `F64` "never
  describes a stored field's real type... it exists solely to carry
  `Avg`'s computed result." `aliases` *is* a stored field with a
  `FieldDescriptor`, so `ValueKind` needs a `StrList` too — the one
  place this round must diverge from the `F64` precedent, and the reason
  it touches more sites than `F64` did.
- `Response::Record { id, fields: Vec<(FieldRef, ScanValue)> }`,
  `Response::Rows { rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> }`,
  and `Response::Schema(DomainSchema { fields: Vec<FieldDescriptor>, ..
  })` are the three response shapes a `StrList` or a `ValueKind::
  StrList` can appear in. `ScanValues` and `Groups` cannot — `aliases`
  is not scannable or aggregatable.
- `downgrade_for_version(resp: Response, negotiated: u32) -> Response`
  (`mod.rs:716`) rewrites two `ErrorCode`s (`Journal` below 4, ADR-0025;
  `Conflict` below 7, ADR-0033) to `Unsupported`; it is called once, on
  every dispatched response, at `mod.rs:2109`, *after* `dispatch` and
  *before* the access log and the send. `DescribeSchema` goes through
  `dispatch`, so `Response::Schema` passes through it too. This is the
  single funnel, already in place — no new plumbing.
- Every exhaustive-looking match over `ScanValue`/`ValueKind` already
  has a safe fallthrough, checked site by site: `resolve_literal`
  (`client.rs:670`, `_ => Err(Sql)`), `value_matches_kind` (`mod.rs:
  415`, a `matches!` — unlisted pairs are `false`), the aggregate
  zero-value (`mod.rs:688`, `_ => I64(0)`, unreachable for a list since
  `validate_aggregate` and the client both gate to `U32 | I64`),
  `overlay_staged` (discriminant compare, only over `updatable` fields
  — `aliases` is not one), and the adapters' `check_read_set`
  (`ScanValue` equality — a list compares fine, and never changes).
- Three tests pin the current version literally and will need bumping,
  exactly as Unit 42 found: `protocol.rs:1101`, `tests/server_protocol_
  version.rs:260`, `tests/server_schema_driven_client.rs:323`.

## Requirements

- `ENT4-FR-001` — **`ScanValue::StrList(Vec<String>)`** appended (index
  5) and **`ValueKind::StrList`** appended (index 4). `PROTOCOL_VERSION`
  moves to 11; the version table gains row 11. Both are pure appends —
  every existing golden vector is byte-for-byte unchanged.
- `ENT4-FR-002` — **`aliases` gains `FIELD_ALIASES: FieldRef = 3`** in
  `src/server/entity.rs`, a `FieldDescriptor { name: "aliases",
  value_kind: StrList, capabilities: all false }`, and appears in
  `get`/`scan_all` as `(FIELD_ALIASES, ScanValue::StrList(entity.
  aliases))`; `check_read_set` gains the matching arm; `filter_eq`/
  `scan_field`/`update_field`/`validate_batch` answer `Unsupported` for
  it explicitly (today the tag is `UnknownField` — it must become a
  known, unsupported field).
- `ENT4-FR-003` — **Rule-3 downgrade for content**: `downgrade_for_
  version` gains, for `negotiated < 11`: strip every `(_, ScanValue::
  StrList(_))` pair from `Response::Record.fields` and from each row of
  `Response::Rows.rows`; strip every `FieldDescriptor` whose
  `value_kind == ValueKind::StrList` from `Response::Schema`. A client
  below 11 therefore sees exactly the schema and records it saw at
  `FR-042` — the field did not exist for it then and does not now. No
  other response shape can carry a `StrList` (Non-goals), so nothing
  else is touched; a debug assertion pins that `ScanValues`/`Groups`
  never do.
- `ENT4-FR-004` — **No new client API.** `SchemaDrivenClient::get`/
  `query` already return `Vec<(String, ScanValue)>` rows; a version-11
  client receives the `StrList` pair with no code change. `SELECT
  aliases FROM entity` and `SELECT *` project it; `WHERE aliases = ..`,
  `filter_eq("aliases", ..)`, `update("aliases", ..)`, `ORDER`/
  aggregates over it are all already refused client-side by the
  existing kind/capability checks. A version-11 client against a
  version-10 server never sees the field (the server doesn't have it).
- `ENT4-FR-005` — **No new `ErrorCode`, no new `Request`.** Reading is
  free; every write/filter path reuses `Unsupported`/`Malformed`/
  `UnknownField` exactly as today.
- `ENT4-FR-006` — **Golden vectors**: one `Response::Record` carrying a
  `StrList` pair, one `Response::Schema` carrying a `StrList`
  descriptor, one `Response::Rows` row carrying a `StrList`; plus unit
  tests for `downgrade_for_version` — each of the three shapes stripped
  at 10 and 1, untouched at 11; a `Record` with no `StrList` untouched
  at 1 (regression for every other domain).

## Considered options

**How a sub-11 client is protected from a variant it cannot decode.**
**(a) (proposed) rule-3 strip in `downgrade_for_version`** — the field
is removed from `Record`/`Rows`/`Schema` for `negotiated < 11`. The
client sees exactly what it saw before this round; one function, one
funnel, already the established place for version-shaped rewrites
(ADR-0025, ADR-0033). **(b) gate `GetById`/`Query`/`DescribeSchema`
themselves below 11** — rejected outright: those are protocol-1/8/1
requests; refusing them would break every existing client of every
domain, the opposite of append-only. **(c) do nothing; document that
a sub-11 client must not call `GetById` on `Entity`** — rejected: a
silent (version-1) client cannot know which domain it connected to
before `DescribeSchema`, and `DescribeSchema` itself would carry the
undecodable `ValueKind`. Not a real option.

Option (a) proposed.

**Whether `ValueKind` gains `StrList`, following `F64`'s precedent of
not having one.** **(a) (proposed) yes** — `aliases` is a stored field
with a `FieldDescriptor`; a descriptor must name its kind, and
`ValueKind` has no other way to say "list of strings." `F64` skipped
this because it never described a stored field; `StrList` always does.
**(b) describe `aliases` as `ValueKind::Str` and carry the list anyway**
— rejected: a client reading `Str` from the schema would expect
`ScanValue::Str` in the record and get `StrList`; `resolve_literal`
would happily build a `WHERE aliases = 'x'` predicate the server then
calls `Malformed`. A schema that lies about a field's kind is worse
than no schema. **(c) no `FieldDescriptor` at all — carry the pair in
`Record` but leave it out of `Schema`** — rejected: `SchemaDrivenClient`
resolves field names to tags via the schema; a pair whose tag the
schema doesn't list has no name and cannot be projected by `SELECT`.

Option (a) proposed.

**Whether to build this at all versus the alternatives `ADR-0040`
named.** **(a) (proposed) `ScanValue::StrList`, protocol 11** — the
smallest mechanism that lets a client read the list back, with a
contained compatibility story. **(b) join the aliases into one
`ScanValue::Str` with a separator** — no protocol bump, but a lossy,
escaping-fraught encoding (an alias containing the separator is
ambiguous) of exactly the kind ADR-0035 rejected as a "false economy"
when it declined to truncate `AVG` to an integer. Offered as the
no-bump fallback the owner may still prefer; not proposed. **(c) close
as not warranted** — lookup already works; no consumer of the list
itself has been demonstrated. Real, and the honest fallback if the
owner judges read-back not worth a protocol bump.

Option (a) proposed.

## Proposed shape

```rust
// src/server/protocol.rs
pub enum ScanValue {
    U32(u32), I64(i64), Bool(bool), Str(String), F64(f64),
    /// Protocol 11, `ENT4-FR-001` (ADR-0041): a stored list-of-strings
    /// field — `Entity::aliases`, the first. Unlike `F64`, this *does*
    /// describe a stored field's real type, so `ValueKind::StrList`
    /// exists to match it. Stripped from every response by
    /// `downgrade_for_version` on a connection negotiated below 11.
    StrList(Vec<String>),
}
pub enum ValueKind { U32, I64, Bool, Str, StrList }
pub const PROTOCOL_VERSION: u32 = 11;
```

```rust
// src/server/mod.rs — downgrade_for_version, new arms
Response::Record { id, fields } if negotiated < 11 => Response::Record {
    id,
    fields: fields.into_iter().filter(|(_, v)| !matches!(v, ScanValue::StrList(_))).collect(),
},
Response::Rows { rows } if negotiated < 11 => Response::Rows {
    rows: rows.into_iter().map(|(id, fields)| (id, fields.into_iter()
        .filter(|(_, v)| !matches!(v, ScanValue::StrList(_))).collect())).collect(),
},
Response::Schema(mut schema) if negotiated < 11 => {
    schema.fields.retain(|f| f.value_kind != ValueKind::StrList);
    Response::Schema(schema)
}
```

```rust
// src/server/entity.rs
pub const FIELD_ALIASES: FieldRef = 3;
// get(): + (FIELD_ALIASES, ScanValue::StrList(entity.aliases))
// describe(): + FieldDescriptor { tag: FIELD_ALIASES, name: "aliases",
//   value_kind: ValueKind::StrList, capabilities: all false }
// filter_eq / scan_field / update_field / validate_batch:
//   (FIELD_ALIASES, _) => Err(ErrorCode::Unsupported)
```

## Data/state and invariants

- No on-disk change of any kind: `Entity`'s struct, `SchemaTag`, the
  record blob, the edge blobs, and `NameIndex` are all untouched. This
  round is wire-only.
- Invariant, pinned by a debug assertion in `downgrade_for_version` and
  by the adapter's own `scan_field`/aggregate refusals: a `StrList`
  never appears inside `Response::ScanValues` or `Response::Groups`.
- Invariant: the `StrList` a client reads is the raw stored
  `Vec<String>`, order preserved, un-normalized — `NameIndex`'s keys
  are derived from it, never the other way round.

## Errors, failure, recovery, and observability

- No new `ErrorCode`. Every write/filter/scan path on `aliases` is
  `Unsupported`; a hand-built `StrList` predicate is `Malformed`; both
  already exist.
- A sub-11 client experiences no error at all — it simply never sees
  the field, exactly as at `FR-042`.

## Security, privacy, and compatibility

- `PROTOCOL_VERSION` 10 → 11; version table row 11. Rule 1: both
  appends change no existing bytes — every prior golden vector holds.
  Rule 3: the strip above is the "nearest older shape." Rule 4 does not
  apply — the client never *sends* a `StrList` on its own initiative
  (a version-11 client that hand-builds one in `UpdateField` gets
  `Unsupported` from the adapter, not a decode failure).
- **This is the first time rule 3 rewrites content rather than an
  `ErrorCode`**, and the first appended value variant that can reach a
  client which never negotiated for it. Named plainly: it is a real
  extension of what `downgrade_for_version` is for, not a repeat of the
  two existing cases. The single-funnel structure (`mod.rs:2109`) is
  what keeps it contained to one function.
- Read-only, gated exactly like `GetById` (authentication only). A
  snapshot-isolated session's `GetById` on `Entity` will now record a
  `StrList` pair into its read set; harmless (the field is immutable
  over the wire, so it can never cause a `Conflict`) and covered by the
  existing `PartialEq`.

## Acceptance criteria

1. `ScanValue::StrList`/`ValueKind::StrList` exist at indices 5/4;
   `PROTOCOL_VERSION == 11`; version-table row 11; every pre-existing
   golden vector byte-for-byte unchanged.
2. `GetById` on `entity_server` over a version-11 connection returns
   four fields including `("aliases", StrList([..]))` with the raw
   stored strings in stored order; `SELECT aliases FROM entity` and
   `SELECT *` project it.
3. The same `GetById`, `SELECT *`, and `DescribeSchema` over a
   connection negotiated at 10, and over a silent (version-1)
   connection, return exactly the three-field shape `FR-042` returned —
   no `aliases` pair, no `aliases` descriptor — proven against a real
   server with a hand-negotiated `Hello { 10 }` and with no `Hello`.
4. `filter_eq`/`update`/`Session::update` on `"aliases"` are client-side
   `Unsupported`; `WHERE aliases = 'x'` is a client-side `Sql` error; a
   raw `Request::UpdateField` carrying a `StrList` is server-side
   `Unsupported`; a raw `Request::Query` predicate carrying a `StrList`
   is server-side `Malformed`.
5. `downgrade_for_version` unit tests: each of `Record`/`Rows`/`Schema`
   stripped at 10 and 1, untouched at 11; a `Record` with no `StrList`
   untouched at 1; the two existing `ErrorCode` cases unchanged.
6. Every other domain's schema and every other domain's test is
   unchanged; `Dog`/`Order`/`Employee`/`Reminder` never emit a
   `StrList`.
7. The three hardcoded version pins updated to 11; nothing else in
   `tests/server_protocol_version.rs` changes.

## Verification plan

- `src/server/protocol.rs`: three new golden vectors (`ENT4-FR-006`),
  the version pin, every existing vector re-run.
- `src/server/mod.rs`: `downgrade_for_version` unit tests per criterion
  5; the debug-assertion invariant exercised.
- `src/server/entity.rs`: `get` returns four fields; `describe` lists
  four with `aliases` all-false; the four `Unsupported` arms.
- `tests/server_entity_integration.rs` (extended): criteria 2–4 over a
  real socket, including the hand-negotiated `Hello { 10 }` and the
  silent-client cases via raw framing (the same raw-framing precedent
  `Request::Transaction`'s test in that file already uses).
- `tests/server_protocol_version.rs`: the pin, plus one assertion that
  a `Hello { 10 }` client's `DescribeSchema` against `entity_server`
  lacks `aliases` — the cross-domain version suite is the natural home
  for the rule-3 proof.

## Traceability

- → `SERVER-001` next minor / FR (`ENT4-FR-001`–`006`), a new ADR
  (`ADR-0041`) — closes the one piece `ADR-0040` deferred.
- Not sourced from `docs/FUTURE-GROWTH.md` — the seventh round in the
  `rusty_remind_me`-motivated line `ADR-0036` started.

## Open questions

- Whether any other domain ever wants a list-valued field — `StrList`
  is general, but this round proves it on exactly one field of one
  domain, matching every prior primitive's single-instance precedent.
- Whether content-stripping downgrades should be centralized further
  (a per-`ValueKind` "introduced at version N" table driving the strip
  generically) if a second such variant ever lands — one instance is
  not enough to justify the abstraction; named, not built.
- Whether a version-11 client should be told, via the schema, that a
  field was stripped for it — it cannot be (that is the point of the
  strip), and no client below 11 can ask; named as inherent, not open.

## Change history

- 2026-09-05: Initial proposal, the seventh round in the
  `rusty_remind_me`-motivated line `ADR-0036` started — `ScanValue::
  StrList`/`ValueKind::StrList` at protocol 11 so `Entity::aliases` can
  be read back, with rule-3 content stripping in `downgrade_for_version`
  for every client negotiated below 11.
- 2026-09-05: Accepted as proposed, `ADR-0041` option (a); (b) and (c)
  declined.
