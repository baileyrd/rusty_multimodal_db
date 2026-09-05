# SERVER-002 — Wire Format for Foreign Clients

- Version: 0.1.0 (protocol version 12 — `SERVER-001` v0.35.0; `ECO-FR-004`,
  ADR-0043)
- Status: Accepted / Implemented / Verified
- Owner: baileyrd
- Depends on: `SERVER-001` (the protocol this document describes),
  `STORAGE-018` (the codec it derives from)
- Conformance fixture: `tests/fixtures/wire-vectors.txt` (§9)
- Reference implementation: `clients/python/` (Python 3, standard library
  only)

## 1. Scope and relationship to `SERVER-001`/`STORAGE-018`

This document specifies, byte for byte and in language-neutral terms,
the network protocol `rusty_multimodal_db`'s server speaks, so that a
client can be written in any language from this document alone. It is
**derivative**: every rule here restates one already made in
`SERVER-001` (the protocol's requirements and semantics) or
`STORAGE-018` (the `bincode` configuration every byte is encoded with).
Where this document and the golden vectors in `src/server/protocol.rs`
(mirrored in the fixture of §9) disagree, the vectors are authoritative
and this document has a bug.

Every `SERVER-001` minor that changes the wire — a new request, response,
value variant, or flag bit — bumps this document's minor in the same
change and adds a row to §8's table. This document never moves alone.

Notation: `u8`/`u16`/`u32`/`u64`/`i64` are unsigned/signed integers of
that width; `LE` is little-endian; `len` is a `u64` element count.
Hex bytes are written space-separated in wire order.

## 2. Transport

- **TCP.** One connection, one client. Requests and responses strictly
  alternate: the client sends one frame, the server answers with exactly
  one frame, then the next request may be sent. There is no pipelining
  and no server-initiated message.
- **TLS (optional, server-configured).** Standard TLS via the server's
  `rustls`-based stack. A client must send SNI (the server's configured
  name). If the server was configured for mutual TLS, the client must
  present a certificate chaining to the server's configured client CA
  or the handshake fails. Nothing in the transport is proprietary; any
  language's standard TLS library connects.
- **Disable Nagle's algorithm** (`TCP_NODELAY`). The protocol is
  synchronous request/response with small frames; with Nagle on, every
  round trip pays delayed-ACK latency (`SERVER-001-FR-006`).

## 3. Framing

Every message — request or response — is one frame:

```text
+----------------------+------------------------+
| length: u32 LE       | payload: length bytes  |
+----------------------+------------------------+
```

- `length` is the payload's byte count. It must be at most
  **16 777 216** (16 MiB). A server refuses a larger frame *before*
  reading it and closes the connection with no reply; a client must do
  the same.
- The payload is one `Request` (client → server) or one `Response`
  (server → client), encoded as §4 and §5 say.

## 4. Primitive encodings (`STORAGE-018`)

The payload is `bincode` 1.x with **fixed-width integers**,
**little-endian byte order**, and **no trailing bytes** (a payload with
bytes left over after the value is fully decoded is an error). There
are no type tags, field names, alignment, or varints.

| Rust type | Wire | Example |
|---|---|---|
| `u8` | 1 byte | `01` |
| `u16` | 2 bytes LE | `03 00` |
| `u32` | 4 bytes LE | `0c 00 00 00` (12) |
| `u64`, `usize` | 8 bytes LE | `02 00 00 00 00 00 00 00` |
| `i64` | 8 bytes LE, two's complement | `fb ff ff ff ff ff ff ff` (-5) |
| `f64` | 8 bytes IEEE 754 binary64, LE | `00 00 00 00 00 00 04 40` (2.5) |
| `bool` | 1 byte, `00` false / `01` true | `01` |
| `String` | `len: u64`, then that many UTF-8 bytes | `03 00 00 00 00 00 00 00 41 64 61` ("Ada") |
| `Vec<T>` | `len: u64`, then `len` encoded `T`s | `00 00 00 00 00 00 00 00` (empty) |
| `Option<T>` | `00` for `None`; `01` then the encoded `T` for `Some` | `01 02 00 00 00 00 00 00 00` (`Some(2usize)`) |
| struct, tuple | each field in declaration order; no count, no names | see §5 |
| enum | the variant index as `u32` LE, then the variant's fields | `07 00 00 00` (`Response::Ok`) |
| `Uuid` | `len: u64` = 16, then the 16 bytes big-endian (RFC 4122 order) — 24 bytes | `10 00 00 00 00 00 00 00` + 16 bytes |

Two worked examples a client must reproduce exactly (both are in §9's
fixture and are asserted by the reference client's tests):

- `Request::Hello { protocol_version: 12 }`, framed:
  `08 00 00 00` · `0a 00 00 00` (variant 10) · `0c 00 00 00` (12).
- `Request::GetById { id: 00000000-0000-0000-0000-000000000001 }`,
  framed: `1c 00 00 00` (28) · `00 00 00 00` (variant 0) ·
  `10 00 00 00 00 00 00 00` (16) · fifteen `00` · `01`.

## 5. Types at protocol version 12

Enum indices are declaration order and **append-only** (§8, rule 1).
"Since" is the protocol version that introduced the item; everything
unmarked is version 1.

### 5.1 Scalars and aliases

- `FieldRef` = `u16`. A field's tag within one domain; meaningful only
  on the connection that reported it (§6.4).
- `RecordId` = `Uuid`.

### 5.2 Fieldless enums (a `u32` index)

| Enum | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `ValueKind` | `U32` | `I64` | `Bool` | `Str` | `StrList` (since 11) | | | | | | |
| `CompareOp` (since 8) | `Eq` | `Ne` | `Lt` | `Le` | `Gt` | `Ge` | | | | | |
| `AggregateFn` (since 9) | `Count` | `Sum` | `Avg` | `Min` | `Max` | | | | | | |
| `ErrorCode` | `UnknownField` | `Unsupported` | `Malformed` | `Unauthenticated` | `Unauthorized` | `RecordNotFound` | `NoSession` (3) | `SessionOpen` (3) | `SessionFull` (3) | `Journal` (4) | `Conflict` (7) |

### 5.3 `ScanValue` — a field's value

| Index | Variant | Payload |
|---|---|---|
| 0 | `U32` | `u32` |
| 1 | `I64` | `i64` |
| 2 | `Bool` | `bool` |
| 3 | `Str` | `String` |
| 4 | `F64` (since 9) | `f64` — only ever `AggregateFn::Avg`'s result; never a stored field's kind |
| 5 | `StrList` (since 11) | `Vec<String>` — a stored list-of-strings field, read-only over the wire |

### 5.4 `Selection` (since 8), `JoinRelation` (since 12)

| Enum | 0 | 1 | 2 |
|---|---|---|---|
| `Selection` | `All` | `Fields(Vec<FieldRef>)` | |
| `JoinRelation` | `Neighbors(Option<String>)` — every symmetric relation, or one named label | `Parent` | `Children` |

### 5.5 Structs (fields in order)

| Struct | Fields |
|---|---|
| `TransactionOp` | `id: RecordId`, `field: FieldRef`, `value: ScanValue` |
| `Predicate` (8) | `field: FieldRef`, `op: CompareOp`, `value: ScanValue` |
| `AggregateSpec` (9) | `func: AggregateFn`, `field: Option<FieldRef>` (`None` only for `Count`) |
| `AggregateGroup` (9) | `key: Vec<(FieldRef, ScanValue)>`, `values: Vec<ScanValue>` |
| `FieldCapabilities` | `filter_eq: bool`, `scan: bool`, `update: bool` |
| `FieldDescriptor` | `tag: FieldRef`, `name: String`, `value_kind: ValueKind`, `capabilities: FieldCapabilities` |
| `RelationCapabilities` | `parent_children: bool`, `neighbors: bool` |
| `DomainSchema` | `fields: Vec<FieldDescriptor>`, `relations: RelationCapabilities` |
| `JoinSpec` (12) | `relation: JoinRelation`, `right_table: Option<String>` (always `None` at 12; `Some` is `Malformed`), `left: Selection`, `right: Selection`, `left_filter: Vec<Predicate>`, `right_filter: Vec<Predicate>`, `limit: Option<usize>` |
| `JoinedRow` (12) | `left_id: RecordId`, `left: Vec<(FieldRef, ScanValue)>`, `right_id: RecordId`, `right: Vec<(FieldRef, ScanValue)>` |
| `RelationDescriptor` (12) | `name: String`, `kind: JoinRelation`, `target_table: Option<String>` (`None` = this table's own rows) |

A tuple `(FieldRef, ScanValue)` is its two fields in order, no count.

### 5.6 `Request` (client → server)

| Index | Variant | Fields | Since | Answered by |
|---|---|---|---|---|
| 0 | `GetById` | `id: RecordId` | 1 | `Record` / `NotFound` |
| 1 | `FilterEq` | `field: FieldRef`, `value: ScanValue` | 1 | `RecordList` |
| 2 | `ScanField` | `field: FieldRef` | 1 | `ScanValues` |
| 3 | `UpdateField` | `id: RecordId`, `field: FieldRef`, `value: ScanValue` | 1 | `Ok` / `NotFound` |
| 4 | `Parent` | `id: RecordId` | 1 | `Id` / `NoParent` / `NotFound` |
| 5 | `Children` | `id: RecordId` | 1 | `RecordList` |
| 6 | `Neighbors` | `id: RecordId` | 1 | `RecordList` |
| 7 | `DescribeSchema` | — | 1 | `Schema` |
| 8 | `Authenticate` | `token: String` | 1 | `Ok` |
| 9 | `Transaction` | `updates: Vec<TransactionOp>` | 1 | `Ok` / `TransactionFailed` |
| 10 | `Hello` | `protocol_version: u32` | 2 | `Hello` |
| 11 | `Begin` | — | 3 | `Ok` |
| 12 | `Commit` | — | 3 | `Ok` / `TransactionFailed` |
| 13 | `Rollback` | — | 3 | `Ok` |
| 14 | `BeginWith` | `flags: u32` (§7.6) | 5 | `Ok` |
| 15 | `Query` | `select: Selection`, `filter: Vec<Predicate>`, `limit: Option<usize>` | 8 | `Rows` |
| 16 | `Aggregate` | `group_by: Vec<FieldRef>`, `filter: Vec<Predicate>`, `aggregates: Vec<AggregateSpec>`, `limit: Option<usize>` | 9 | `Groups` |
| 17 | `NeighborsByRelation` | `id: RecordId`, `relation: String` | 10 | `RecordList` |
| 18 | `ListRelationKinds` | — | 10 | `RelationKinds` |
| 19 | `Join` | `JoinSpec` (a newtype: the struct's fields directly) | 12 | `JoinedRows` |
| 20 | `DescribeRelations` | — | 12 | `Relations` |

Any request may instead be answered by `Err`. A request index the
server does not know closes the connection with no reply (§6.3).

### 5.7 `Response` (server → client)

| Index | Variant | Fields | Since |
|---|---|---|---|
| 0 | `Record` | `id: RecordId`, `fields: Vec<(FieldRef, ScanValue)>` | 1 |
| 1 | `RecordList` | `records: Vec<RecordId>` | 1 |
| 2 | `ScanValues` | `values: Vec<ScanValue>` | 1 |
| 3 | `Id` | `id: RecordId` | 1 |
| 4 | `Schema` | `DomainSchema` (a newtype: the struct's fields directly) | 1 |
| 5 | `NotFound` | — | 1 |
| 6 | `NoParent` | — | 1 |
| 7 | `Ok` | — | 1 |
| 8 | `Err` | `code: ErrorCode`, `message: String` | 1 |
| 9 | `TransactionFailed` | `index: usize`, `code: ErrorCode`, `message: String` | 1 |
| 10 | `Hello` | `protocol_version: u32` | 2 |
| 11 | `Staged` | `index: u32` | 3 |
| 12 | `Rows` | `rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>` | 8 |
| 13 | `Groups` | `groups: Vec<AggregateGroup>` | 9 |
| 14 | `RelationKinds` | `kinds: Vec<String>` | 10 |
| 15 | `JoinedRows` | `rows: Vec<JoinedRow>` | 12 |
| 16 | `Relations` | `relations: Vec<RelationDescriptor>` | 12 |

## 6. Connection lifecycle

### 6.1 `Hello` — optional, first, or never

A client *may* send `Hello { protocol_version }` as the **first** frame
on a connection. The server answers `Hello { min(client, server) }`;
that value is the connection's **negotiated version** for its lifetime.
A `Hello` with version 0, or a `Hello` that is not the first frame, is
answered `Err { Malformed }` and changes nothing (the connection stays
open). A client that never sends `Hello` is served at **version 1** —
it sees the `SERVER-001` v0.9.1 shape: no sessions, no `Query`, no
`StrList` field, no joins (§8, rule 3). Recommended: always send `Hello`
with the version this client implements.

Servers older than protocol 2 do not answer `Hello`; they close the
connection. A client that wants to talk to such a server reconnects
without a `Hello` and treats the version as 1.

### 6.2 `Authenticate` — before anything else, on an authenticated server

A server may be configured with tokens. On such a server every request
except `Hello` and `Authenticate` — `DescribeSchema` included — is
answered `Err { Unauthenticated }` until `Authenticate { token }` has
been answered `Ok`. A wrong token is `Err { Unauthenticated }`,
indistinguishable from never having authenticated. A token grants a
class (read-only or read-write); a write with a read-only token is
`Err { Unauthorized }`. Repeated failures from one peer are rate limited
and may lock the peer out for a period (`SERVER-001-FR-032`). A server
with no tokens configured answers `Authenticate` with `Ok` and gates
nothing. **Send the token only inside TLS**: on a TLS server the order is
TLS handshake → `Hello` → `Authenticate` → everything else.

### 6.3 Close-without-reply

The server closes the connection with no response frame in exactly two
cases: a frame whose length exceeds 16 MiB (§3), and a payload whose
request index it does not know (a client speaking a newer protocol than
the server). Any other malformed payload — a bad variant index inside a
value, a truncated field — is also treated as undecodable and closed. A
client must treat an EOF after sending a frame as one of these, not as
an empty response.

### 6.4 `DescribeSchema` — every field by name

One connection serves **one domain**. `DescribeSchema` returns that
domain's `DomainSchema`: every field's `tag` (the `FieldRef` every other
request uses), `name`, `value_kind`, and three capability flags — whether
`FilterEq`, `ScanField`, and `UpdateField` are supported for it — plus
whether the domain has a directed relation (`parent_children`) and a
symmetric one (`neighbors`). A client should fetch the schema once at
connect and address fields by name from then on; tags are stable for a
server build but are not promised across builds. Capability flags are
honest: a request against a field whose flag is `false` is
`Err { Unsupported }`. Checking client-side first saves a round trip
but is not a trust boundary — the server enforces the same rules.

Since protocol 12, `DescribeRelations` returns what a `Join`'s `ON` may
name (§7.9).

### 6.5 Sessions (protocol 3+) — specified, per-connection state

`Begin` (or `BeginWith { flags }`, protocol 5+) opens a transaction
session on the connection. While one is open, `UpdateField` **stages**
the write and answers `Staged { index }` instead of applying it; `Commit`
applies every staged write atomically (answered `Ok`, or
`TransactionFailed { index, .. }` with nothing applied); `Rollback`
discards. At most 4096 writes may be staged (`Err { SessionFull }`).
`Begin` while open is `Err { SessionOpen }`; `Commit`/`Rollback` with
none open is `Err { NoSession }`; a one-shot `Transaction` while a
session is open is `Err { SessionOpen }`. Closing the connection
discards an open session. `Query`/`Aggregate`/`Join` are never affected
by a session. The reference client does not implement sessions; a
client that does needs nothing beyond this section.

## 7. Semantics per request

Each item names the `SERVER-001` requirement that owns it.

1. **`GetById`** — the full record: every field of the domain as
   `(tag, value)` pairs in the adapter's fixed order, or `NotFound`.
   Since 11 an `Entity` row includes `aliases` as a `StrList`; a
   connection below 11 never sees that pair (§8, rule 3). (`FR-001`)
2. **`FilterEq`** — ids of every record whose field equals `value`;
   only for fields with `filter_eq: true`; a value of the wrong kind is
   `Malformed`. For `Entity`'s `label` the match is case- and
   whitespace-insensitive and includes aliases (`FR-042`/`043`).
   (`FR-002`)
3. **`ScanField`** — every record's value for a `scan: true` field, in
   unspecified order. (`FR-003`)
4. **`UpdateField`** — sets one `update: true` field; `Ok` if the record
   exists, `NotFound` if not; `Malformed` for the wrong kind;
   `Unsupported` for a read-only field. Inside a session: `Staged`.
   (`FR-004`)
5. **`Parent`/`Children`** — the directed relation, if the domain has
   one (`parent_children: true`); otherwise `Unsupported`. `Parent`
   distinguishes "no such record" (`NotFound`) from "exists, has no
   parent" (`NoParent`). (`FR-008`)
6. **`Neighbors`** — the union of every symmetric relation's neighbors;
   `NeighborsByRelation { relation }` one named label (an unknown label
   is `Malformed`); `ListRelationKinds` the labels. `Unsupported` for a
   domain with `neighbors: false`. (`FR-008`, `FR-041`)
7. **`Transaction`** — a batch of `UpdateField`-shaped writes applied
   all-or-nothing; every precondition is checked before any write;
   `TransactionFailed { index, code, .. }` names the first failing
   operation and nothing was applied. (`FR-017`)
8. **`Query`** — a read-only full scan: `filter` predicates are `AND`ed
   (`Lt`/`Le`/`Gt`/`Ge` only on `U32`/`I64` fields; a `StrList` field
   cannot be filtered), `select` projects, `limit` truncates the row
   count. Unknown tag `UnknownField`; kind mismatch `Malformed`. There
   is no ordering. **`Aggregate`** — `group_by` buckets the filtered
   rows (an empty `group_by` is one implicit group), `aggregates` reduce
   each bucket: `Count` takes no field; `Sum`/`Avg`/`Min`/`Max` need a
   `U32`/`I64` field; `Avg` returns `F64`; a `StrList` field may not be
   a group key. The server never sees SQL text: the SQL front end in the
   Rust client compiles to these two requests, and a foreign client
   without one builds them directly. (`FR-037`, `FR-038`, `FR-044`)
9. **`Join`** (12) — an inner join of the connection's one table with
   itself over one declared relation: for each left row passing
   `left_filter`, the rows its relation points to (`Neighbors(None)`,
   `Neighbors(Some(label))`, `Parent`, `Children`), each passing
   `right_filter`, both sides projected; `limit` truncates the pair
   count. A symmetric relation yields both orientations of an edge. The
   relation must be one `DescribeRelations` lists (`Malformed`
   otherwise); one listed with a `target_table` is `Unsupported`;
   `right_table: Some` is `Malformed` at 12. **`DescribeRelations`**
   (12) — the list; a domain may list nothing. (`FR-045`)

### 7.6 `BeginWith` flags

| Bit | Name | Since | Meaning |
|---|---|---|---|
| `1` | `SESSION_READ_YOUR_WRITES` | 5 | the connection's own `GetById` sees its staged writes |
| `2` | `SESSION_VALIDATE_ON_STAGE` | 6 | each staged write is validated when staged |
| `4` | `SESSION_SNAPSHOT_ISOLATION` | 7 | session `GetById`s are tracked and re-checked at `Commit` (`Conflict` on mismatch) |

An unknown bit for the negotiated version is `Malformed`.

## 8. Protocol versions and compatibility rules

| Version | `SERVER-001` | Added |
|---|---|---|
| 1 | v0.1.0–v0.9.1 | `Request` 0–9, `Response` 0–9, `ScanValue` 0–3, `ValueKind` 0–3, `ErrorCode` 0–5 |
| 2 | v0.10.0 | `Request::Hello` (10), `Response::Hello` (10) |
| 3 | v0.14.0 | `Begin`/`Commit`/`Rollback` (11–13), `Staged` (11), `ErrorCode` 6–8 |
| 4 | v0.15.0 | `ErrorCode::Journal` (9) |
| 5 | v0.18.0 | `BeginWith` (14), flag bit 1 |
| 6 | v0.20.0 | flag bit 2 |
| 7 | v0.26.0 | `ErrorCode::Conflict` (10), flag bit 4 |
| 8 | v0.27.0 | `Query` (15), `Rows` (12), `Selection`, `Predicate`, `CompareOp` |
| 9 | v0.28.0 | `Aggregate` (16), `Groups` (13), `ScanValue::F64` (4), `AggregateFn`, `AggregateSpec`, `AggregateGroup` |
| 10 | v0.31.0 | `NeighborsByRelation` (17), `ListRelationKinds` (18), `RelationKinds` (14) |
| 11 | v0.34.0 | `ScanValue::StrList` (5), `ValueKind::StrList` (4) — stripped from `Record`/`Rows`/`Schema` for connections below 11 |
| 12 | v0.35.0 | `Join` (19), `DescribeRelations` (20), `JoinedRows` (15), `Relations` (16), `JoinRelation`, `JoinSpec`, `JoinedRow`, `RelationDescriptor` |

Four rules (`SERVER-001-FR-020`, ADR-0022), restated for an implementer:

1. **Append-only.** New variants are appended; existing indices, fields,
   and struct layouts never change. Every vector in §9 for version *N*
   is byte-identical at every later version.
2. **One version per change.** `PROTOCOL_VERSION` rises by exactly one
   per wire change and the table above gains a row.
3. **The server answers in the nearest older shape.** A connection
   negotiated at *N* never receives a variant introduced after *N*: a
   request it could not send is `Err { Malformed }` (sessions below 3,
   `Join`/`DescribeRelations` below 12); an error code introduced later
   is reported as `Unsupported`; and — the one *content* rewrite — a
   `StrList` field is removed from `Record`/`Rows`/`Schema` below 11,
   so an older client sees exactly the record shape it knew.
4. **The client never sends above the negotiated version.** A conformant
   client implements a version, says so in `Hello`, and never sends a
   request, flag bit, or value variant introduced after `min(its
   version, the server's)`.

## 9. Conformance

`tests/fixtures/wire-vectors.txt` holds one line per pinned vector:

```text
<name>\t<introduced-at-version>\t<hex payload bytes>
```

`name` is `Request/<Variant>` or `Response/<Variant>` (with a
parenthesized qualifier where one variant has several vectors, e.g.
`Response/Err(Conflict)`, `Response/Record(StrList)`). The hex is the
**payload** (§3's length prefix excluded). The file is generated from the
Rust golden-vector tests and enforced by them: a `cargo test` fails if
any line differs from the pins, so the file cannot drift.

**A client implementing protocol *N* is conformant when it encodes every
`Request/*` line and decodes every `Response/*` line with version ≤ *N*
byte-for-byte**, and re-encodes what it decoded to the same bytes.
`clients/python/tests/test_vectors.py` is that check for the reference
client; it needs only `python3`. The live half —
`tests/server_python_client.rs` — drives the reference client against a
real server at 12 and at a hand-negotiated 10.

## 10. Change history

- 0.1.0 (`SERVER-001` v0.35.1 patch entry, ADR-0043, `ECO-FR-004`–`006`):
  initial specification at protocol version 12, transcribed from
  `src/server/protocol.rs`, `src/server/framing.rs`, `src/codec.rs`, and
  `SERVER-001`'s requirements; fixture `tests/fixtures/wire-vectors.txt`
  (48 vectors) generated and enforced; reference client `clients/python/`
  verified against both.
