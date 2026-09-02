//! The server's wire protocol: request/response shapes and the field-tag
//! addressing scheme they use, per
//! `docs/design/SERVER-QUERY-LAYER-DESIGN.md` (Accepted, ADR-0010).
//! `bincode`-encoded (already a dependency, from the durability work) over
//! the length-prefixed framing in [`super::framing`].
//!
//! # Differences from the accepted design's "Proposed shape"
//!
//! The design document's own shapes were a proof that the protocol
//! *compiles*, not a final API — implementing it against the real
//! `ProductionStore`/`GenericProductionStore` surfaced three small,
//! necessary completions, none of which reopen any decision ADR-0010
//! recorded (protocol/framing choice, concurrency model, field-addressing
//! scheme all unchanged):
//!
//! - [`ScanValue`] gains a `Str(String)` variant. `Dog::breed` isn't a
//!   `ScannableField` anywhere in this crate (only `age` is), so the
//!   design's numeric/bool-only `ScanValue` never needed to represent it —
//!   but a full-record `GetById` response does need to carry it.
//! - [`Response`] gains an `Id` variant, carrying a single [`RecordId`] —
//!   needed for `Parent`'s "found, has a parent" case, which the design's
//!   original `Response` enum had no shape for.
//! - [`Response`] gains a `ScanValues` variant for `ScanField`'s result —
//!   the design's dispatch sketch only worked out `GetById`/`FilterEq`/
//!   `UpdateField` in detail.
//! - [`ErrorCode`] replaces the design's bare `code: u8` with a small,
//!   named enum — clearer for the two error shapes this implementation
//!   actually produces (`UnknownField`, `Unsupported`), plus `Malformed`
//!   for a value that doesn't match its field's real type.
//!
//! # Schema discovery (`DescribeSchema`/`Response::Schema`), ADR-0011
//!
//! The original design deferred string field names/schema discovery,
//! choosing compile-time-fixed integer tags for v1. ADR-0011 revisits
//! that: `Request::DescribeSchema` (no arguments — one server instance
//! serves one domain) returns a [`DomainSchema`] naming every field this
//! domain adapter exposes, its wire value type, and which operations it
//! supports, plus whether the domain has a directed (`Parent`/`Children`)
//! or symmetric (`Neighbors`) relation. Field *tags* are unchanged and
//! still required for `FilterEq`/`ScanField`/`UpdateField` — this adds a
//! runtime way to discover what a compile-time client already knows, not
//! a new addressing scheme.
//!
//! # Protocol versions, ADR-0022
//!
//! `STORAGE-018` pins every byte *inside* a frame; this section names the
//! *shape* — which variants exist, at which indices — so that two builds
//! can tell each other which shape they speak
//! (`docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md`, `SERVER-001`
//! FR-020). [`PROTOCOL_VERSION`] is the running build's; a client says
//! its own in an optional first-frame [`Request::Hello`] and the server
//! answers [`Response::Hello`] with `min(client, server)` — the
//! *negotiated version* for that connection. A connection that never
//! says hello is served at version 1.
//!
//! | Version | Introduced | Shape |
//! |---|---|---|
//! | 1 | `SERVER-001` v0.1.0 – v0.9.1 | `Request` 0–9 (`GetById` … `Transaction`), `Response` 0–9 (`Record` … `TransactionFailed`), every `ScanValue`/`ValueKind`/`ErrorCode`/`ParentLookup` variant and every struct as of v0.9.1 |
//! | 2 | `SERVER-001` v0.10.0 | + `Request::Hello` (index 10), `Response::Hello` (index 10) |
//! | 4 | `SERVER-001` v0.15.0 | + `ErrorCode::Journal` (9) — a journaled adapter could not journal a batch (nothing applied); carried by `Response::TransactionFailed`, and downgraded to `Unsupported` on a connection negotiated below 4 (rule 3's "nearest older shape"). ADR-0025 |
//! | 3 | `SERVER-001` v0.14.0 | + `Request::Begin`/`Commit`/`Rollback` (11–13), `Response::Staged` (11), `ErrorCode::NoSession`/`SessionOpen`/`SessionFull` (6–8) — the first *gated* variants: a server keeps the negotiated version per connection and answers the three requests `Malformed` below 3 (rule 3); a client sends them only after negotiating ≥ 3 (rule 4). ADR-0024 |
//!
//! ## Compatibility rules (`PROTO-FR-005`)
//!
//! These specialize `STORAGE-018`'s evolution rules to the wire shape:
//!
//! 1. **Append-only.** No variant or field of any type in this module is
//!    reordered, inserted, removed, retyped, or resized once shipped. A
//!    change that needs one of those is a new variant, or a new major
//!    protocol (not designed; nothing here anticipates it beyond the
//!    hello's field being a `u32`).
//! 2. **Every variant records the version that introduced it** in the
//!    table above, and `PROTOCOL_VERSION` is bumped by exactly one in the
//!    same change. The golden vectors below are the check: a change that
//!    breaks an existing vector is a rule-1 violation, not a bump.
//! 3. **The server sends a variant introduced at *N* only on a
//!    connection negotiated at ≥ *N***, otherwise the nearest older
//!    shape — a new `ErrorCode` → `Unsupported`; a new `Response` kind →
//!    `Response::Err { Unsupported, .. }`; a new `ValueKind`/`ScanValue`
//!    in a schema → that field omitted from `DomainSchema` for that
//!    connection. At version 2 there is no such variant to gate, so
//!    `handle_connection` stores no negotiated version yet; the first
//!    version-3 variant adds that per-connection state and the branch.
//! 4. **A client sends a request introduced at *N* only after
//!    negotiating ≥ *N***, else it reports the gap locally without a
//!    round trip — the posture [`super::client::SchemaDrivenClient`]
//!    already takes for capability checks.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The wire shape this build speaks — see this module's own "Protocol
/// versions" table. Bumped by exactly one in any change that appends a
/// variant (rule 2). Version 1 is retroactively the `SERVER-001` v0.9.1
/// shape: what a client that never sends [`Request::Hello`] speaks.
pub const PROTOCOL_VERSION: u32 = 4;

/// The most `UpdateField`s one connection may stage between
/// [`Request::Begin`] and [`Request::Commit`] (`SESS-FR-004`, ADR-0024):
/// the `MAX_STAGED_OPS + 1`-th is answered `ErrorCode::SessionFull` and
/// not staged, the session staying open. A constant, not a config, on
/// purpose — the first real report of hitting it decides whether it
/// becomes one. Smaller than one `MAX_FRAME_BYTES` `Transaction` could
/// carry, so a session never exceeds what a single request already may.
pub const MAX_STAGED_OPS: usize = 4096;

/// A record's id — every domain this crate has ever used is `Uuid`-keyed.
pub type RecordId = Uuid;

/// A field is addressed by a small, server-assigned integer tag, fixed per
/// domain at server start, rather than a string — avoids a schema-
/// description sub-protocol for v1 (design doc's "Field addressing"
/// considered options). Each domain adapter documents its own tag
/// assignment — see [`super::dog`]/[`super::order`].
pub type FieldRef = u16;

/// The scan/filter/update/field value type. See this module's own doc
/// comment for why `Str` was added beyond the design document's original
/// `U32`/`I64`/`Bool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScanValue {
    U32(u32),
    I64(i64),
    Bool(bool),
    Str(String),
}

/// One write within a [`Request::Transaction`] batch — the same three
/// fields `Request::UpdateField` already carries. See
/// `docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013, `TXN-FR-001`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionOp {
    pub id: RecordId,
    pub field: FieldRef,
    pub value: ScanValue,
}

/// The outcome of a `Parent` lookup — kept as a three-way enum, not a
/// nested `Option<Option<_>>` or a collapsed `Option<_>`, specifically to
/// preserve the not-found/no-parent distinction this project's own PR #21
/// (`docs/PROJECT-STATUS.md`'s `Parent::parent` fix) restored in-process
/// after finding it genuinely mattered. Losing that distinction again at
/// the wire-protocol boundary would silently regress a bug this project
/// already paid to fix once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentLookup {
    /// `id` is a real record and has a parent.
    Parent(RecordId),
    /// `id` is a real record with no parent.
    NoParent,
    /// `id` is not a real record at all.
    ChildNotFound,
}

/// A field's wire value type — mirrors [`ScanValue`]'s variants without
/// carrying a value, for [`FieldDescriptor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueKind {
    U32,
    I64,
    Bool,
    Str,
}

/// Which operations a field supports over this protocol — not every
/// `ScannableField`/`IndexedField` in-process is necessarily reachable
/// the same way over the wire (e.g. `Order::created_at_unix_ms` is
/// `ScannableField` in-memory but was never part of the durable
/// production stack this server wraps, so it's read-only here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldCapabilities {
    pub filter_eq: bool,
    pub scan: bool,
    pub update: bool,
}

/// One field a domain adapter exposes, named and typed for a client that
/// doesn't know the domain at compile time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDescriptor {
    pub tag: FieldRef,
    pub name: String,
    pub value_kind: ValueKind,
    pub capabilities: FieldCapabilities,
}

/// Which relation kinds a domain supports — at most one of the two is
/// ever true for either domain this crate serves today (see
/// `dog.rs`/`order.rs`'s own module docs on why each has the relation
/// kind it does, not both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationCapabilities {
    pub parent_children: bool,
    pub neighbors: bool,
}

/// The full answer to `Request::DescribeSchema` — everything a client
/// needs to drive `GetById`/`FilterEq`/`ScanField`/`UpdateField`/
/// `Parent`/`Children`/`Neighbors` against this domain without having
/// compiled against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainSchema {
    pub fields: Vec<FieldDescriptor>,
    pub relations: RelationCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// The field tag isn't one this domain's adapter recognizes at all.
    UnknownField,
    /// The field tag is recognized, but this operation isn't available for
    /// it (e.g. `Dog::breed` has no `ScanField`/`UpdateField` in-process
    /// either; `Order`'s `Neighbors` doesn't exist — it has no symmetric
    /// relation).
    Unsupported,
    /// The field tag is recognized and the operation is supported, but the
    /// supplied [`ScanValue`] variant doesn't match the field's real type.
    Malformed,
    /// The connection has not presented a recognized token yet — every
    /// request kind except [`Request::Authenticate`] is rejected this way
    /// until it does. Deliberately indistinguishable from "presented a
    /// wrong token": `AUTH-FR-001`/`AUTH-FR-002`,
    /// `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012.
    Unauthenticated,
    /// The connection authenticated, but its token's class doesn't permit
    /// this request kind (`ReadOnly` vs. `UpdateField`/`Transaction` —
    /// `AUTH-FR-003`, `TXN-FR-004`).
    Unauthorized,
    /// One [`TransactionOp`] within a [`Request::Transaction`] batch named
    /// an `id` with no record — only reachable via
    /// [`Response::TransactionFailed`]. A single, non-transactional
    /// `Request::UpdateField` keeps using [`Response::NotFound`] for the
    /// same case, unchanged — `TXN-FR-005`,
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013.
    RecordNotFound,
    /// Protocol 3. [`Request::Commit`] or [`Request::Rollback`] on a
    /// connection with no session open (`SESS-FR-004`, ADR-0024).
    NoSession,
    /// Protocol 3. [`Request::Begin`] while a session is already open, or
    /// a [`Request::Transaction`] inside one — one batch at a time per
    /// connection (`SESS-FR-004`).
    SessionOpen,
    /// Protocol 3. The session already holds [`MAX_STAGED_OPS`] staged
    /// writes; this one was not staged and the session stays open
    /// (`SESS-FR-004`).
    SessionFull,
    /// Protocol 4. A journaled adapter (`JRN-FR-001`, ADR-0025) could not
    /// append the batch to its journal before applying it — nothing was
    /// applied. Only reachable via [`Response::TransactionFailed`] (index
    /// 0); on a connection negotiated below 4 the server sends
    /// [`ErrorCode::Unsupported`] in its place.
    Journal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    GetById {
        id: RecordId,
    },
    FilterEq {
        field: FieldRef,
        value: ScanValue,
    },
    ScanField {
        field: FieldRef,
    },
    UpdateField {
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    },
    /// `id` is a child record's id (an `Order`, for `Order`/`Customer`) —
    /// see [`super::order`]'s own doc comment for why `Parent`/`Children`
    /// take differently-typed ids, matching the underlying directed
    /// relation's real shape rather than pretending both sides are
    /// interchangeable.
    Parent {
        id: RecordId,
    },
    /// `id` is a parent record's id (a `Customer`, for `Order`/`Customer`).
    Children {
        id: RecordId,
    },
    Neighbors {
        id: RecordId,
    },
    /// Discover this server's domain schema at runtime — see this
    /// module's own "Schema discovery" doc section, ADR-0011.
    DescribeSchema,
    /// Authenticate this connection against a configured token — see
    /// `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012, `AUTH-FR-001`. Only
    /// meaningful before any other request on a connection; the server's
    /// own `handle_connection` intercepts it directly rather than routing
    /// it through [`super::dispatch`] (which has no store-level notion of
    /// "this connection", so it can't record the outcome).
    Authenticate {
        token: String,
    },
    /// A batch of `UpdateField`-shaped writes, applied all-or-nothing —
    /// see `docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013,
    /// `TXN-FR-001`/`TXN-FR-002`/`TXN-FR-003`. Every operation's
    /// precondition is checked before any write is applied; either every
    /// write in `updates` is applied, or none are.
    Transaction {
        updates: Vec<TransactionOp>,
    },
    /// Protocol 2. Optional first frame on a connection: the client's own
    /// [`PROTOCOL_VERSION`]. Answered by the server's `handle_connection`
    /// itself — before authentication, like [`Request::Authenticate`],
    /// and never through [`super::dispatch`] — with [`Response::Hello`]
    /// carrying `min(client, server)`. A `protocol_version` of 0, or a
    /// `Hello` that is not the connection's first frame, is answered
    /// `Response::Err { code: Malformed, .. }` and changes nothing. See
    /// this module's own "Protocol versions" section, ADR-0022.
    Hello {
        protocol_version: u32,
    },
    /// Protocol 3. Open a transaction session on this connection
    /// (`SESS-FR-002`, ADR-0024, `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`
    /// Part A): until [`Request::Commit`]/[`Request::Rollback`], every
    /// admitted [`Request::UpdateField`] is *staged* in a per-connection
    /// buffer — answered [`Response::Staged`], nothing applied, no lock
    /// held — and `Commit` applies the buffer as one batch exactly as
    /// [`Request::Transaction`] would. Reads inside a session see committed
    /// state only. Answered by `handle_connection` itself, never through
    /// [`super::dispatch`]; `Malformed` on a connection negotiated below 3.
    Begin,
    /// Protocol 3. Apply the session's staged writes as one batch —
    /// `Response::Ok`, or [`Response::TransactionFailed`] naming the staged
    /// index — and close the session either way. Requires
    /// `TokenClass::ReadWrite`, as `Transaction` does. `NoSession` without
    /// one open.
    Commit,
    /// Protocol 3. Discard the session's staged writes and close it.
    /// `NoSession` without one open.
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Record {
        id: RecordId,
        fields: Vec<(FieldRef, ScanValue)>,
    },
    RecordList {
        records: Vec<RecordId>,
    },
    ScanValues {
        values: Vec<ScanValue>,
    },
    /// A single id — `Parent`'s "found, has a parent" case.
    Id {
        id: RecordId,
    },
    /// Answers `Request::DescribeSchema`.
    Schema(DomainSchema),
    NotFound,
    NoParent,
    Ok,
    Err {
        code: ErrorCode,
        message: String,
    },
    /// A [`Request::Transaction`] batch was rejected: `index` names the
    /// first operation (into `updates`) that failed its precondition
    /// check — no write in the batch was applied. `TXN-FR-001`.
    TransactionFailed {
        index: usize,
        code: ErrorCode,
        message: String,
    },
    /// Protocol 2. Answers [`Request::Hello`] with the version negotiated
    /// for this connection: `min(client's Hello, PROTOCOL_VERSION)`. The
    /// server's own version is never sent on its own — the minimum is
    /// all either side needs (rules 3 and 4 above).
    Hello {
        protocol_version: u32,
    },
    /// Protocol 3. Answers a [`Request::UpdateField`] staged inside a
    /// session: `index` is its position in the batch [`Request::Commit`]
    /// will apply — the index a later `TransactionFailed` would name.
    /// Nothing has been applied yet (`SESS-FR-002`).
    Staged {
        index: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assert_golden, assert_golden_eq};

    /// `Uuid::from_u128(1)` on the wire: a `u64` length of 16, then the
    /// 16 big-endian bytes. Every id-carrying variant below pays this.
    const ID1: [u8; 24] = [
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ];

    /// `Uuid::from_u128(2)`, likewise.
    const ID2: [u8; 24] = [
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
    ];

    /// A `u64` sequence length of 1 — what every one-element `Vec`/
    /// one-byte `String` below is prefixed with.
    const LEN1: [u8; 8] = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    /// Concatenate the pinned pieces of one vector, so each variant's
    /// literal reads as "variant index, then its fields" without
    /// spelling the 24-byte id out every time.
    fn bytes(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// `BINENC-FR-004`: every `Request` variant's wire bytes, pinned. A
    /// variant is its `u32` index (declaration order) then its fields;
    /// appending a variant keeps every line below true, and any other
    /// change to `Request`/`ScanValue`/`TransactionOp` breaks one — that
    /// is a wire-format change, per `STORAGE-018`'s evolution rules.
    #[test]
    fn every_request_variant_encodes_to_its_pinned_bytes() {
        let id = Uuid::from_u128(1);
        assert_golden(
            "GetById",
            &Request::GetById { id },
            &bytes(&[&[0x00, 0x00, 0x00, 0x00], &ID1]),
        );
        assert_golden(
            "FilterEq",
            &Request::FilterEq {
                field: 1,
                value: ScanValue::U32(42),
            },
            &[
                0x01, 0x00, 0x00, 0x00, // FilterEq
                0x01, 0x00, // field
                0x00, 0x00, 0x00, 0x00, // ScanValue::U32
                0x2a, 0x00, 0x00, 0x00,
            ],
        );
        assert_golden(
            "ScanField",
            &Request::ScanField { field: 2 },
            &[0x02, 0x00, 0x00, 0x00, 0x02, 0x00],
        );
        assert_golden(
            "UpdateField",
            &Request::UpdateField {
                id,
                field: 1,
                value: ScanValue::I64(-1),
            },
            &bytes(&[
                &[0x03, 0x00, 0x00, 0x00],
                &ID1,
                &[0x01, 0x00],
                &[0x01, 0x00, 0x00, 0x00], // ScanValue::I64
                &[0xff; 8],
            ]),
        );
        assert_golden(
            "Parent",
            &Request::Parent { id },
            &bytes(&[&[0x04, 0x00, 0x00, 0x00], &ID1]),
        );
        assert_golden(
            "Children",
            &Request::Children { id },
            &bytes(&[&[0x05, 0x00, 0x00, 0x00], &ID1]),
        );
        assert_golden(
            "Neighbors",
            &Request::Neighbors { id },
            &bytes(&[&[0x06, 0x00, 0x00, 0x00], &ID1]),
        );
        assert_golden(
            "DescribeSchema",
            &Request::DescribeSchema,
            &[0x07, 0x00, 0x00, 0x00],
        );
        assert_golden(
            "Authenticate",
            &Request::Authenticate {
                token: "s3cr3t".into(),
            },
            &[
                0x08, 0x00, 0x00, 0x00, // Authenticate
                0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // token len
                b's', b'3', b'c', b'r', b'3', b't',
            ],
        );
        assert_golden(
            "Transaction",
            &Request::Transaction {
                updates: vec![TransactionOp {
                    id: Uuid::from_u128(2),
                    field: 1,
                    value: ScanValue::Bool(true),
                }],
            },
            &bytes(&[
                &[0x09, 0x00, 0x00, 0x00],
                &LEN1,
                &ID2,
                &[0x01, 0x00],
                &[0x02, 0x00, 0x00, 0x00], // ScanValue::Bool
                &[0x01],
            ]),
        );
        // Protocol 2 (`PROTO-FR-002`): appended at index 10, so every
        // line above is exactly as it was at v0.9.1.
        assert_golden(
            "Hello",
            &Request::Hello {
                protocol_version: 2,
            },
            &[0x0a, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00],
        );
        // Protocol 3 (`SESS-FR-001`): appended at 11–13.
        assert_golden("Begin", &Request::Begin, &[0x0b, 0x00, 0x00, 0x00]);
        assert_golden("Commit", &Request::Commit, &[0x0c, 0x00, 0x00, 0x00]);
        assert_golden("Rollback", &Request::Rollback, &[0x0d, 0x00, 0x00, 0x00]);
    }

    /// `BINENC-FR-004`: every `Response` variant's wire bytes, pinned —
    /// same reading as the `Request` test.
    #[test]
    fn every_response_variant_encodes_to_its_pinned_bytes() {
        let id = Uuid::from_u128(1);
        assert_golden_eq(
            "Record",
            &Response::Record {
                id,
                fields: vec![(0, ScanValue::Str("labrador".into()))],
            },
            &bytes(&[
                &[0x00, 0x00, 0x00, 0x00],
                &ID1,
                &LEN1,
                &[0x00, 0x00],             // field tag
                &[0x03, 0x00, 0x00, 0x00], // ScanValue::Str
                &[0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                b"labrador",
            ]),
        );
        assert_golden_eq(
            "RecordList",
            &Response::RecordList {
                records: vec![id, Uuid::from_u128(2)],
            },
            &bytes(&[
                &[0x01, 0x00, 0x00, 0x00],
                &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                &ID1,
                &ID2,
            ]),
        );
        assert_golden_eq(
            "ScanValues",
            &Response::ScanValues {
                values: vec![ScanValue::U32(3)],
            },
            &bytes(&[
                &[0x02, 0x00, 0x00, 0x00],
                &LEN1,
                &[0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00],
            ]),
        );
        assert_golden_eq(
            "Id",
            &Response::Id {
                id: Uuid::from_u128(2),
            },
            &bytes(&[&[0x03, 0x00, 0x00, 0x00], &ID2]),
        );
        assert_golden_eq(
            "Schema",
            &Response::Schema(DomainSchema {
                fields: vec![FieldDescriptor {
                    tag: 1,
                    name: "age".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: true,
                        update: true,
                    },
                }],
                relations: RelationCapabilities {
                    parent_children: false,
                    neighbors: true,
                },
            }),
            &bytes(&[
                &[0x04, 0x00, 0x00, 0x00],
                &LEN1,
                &[0x01, 0x00], // tag
                &[0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                b"age",
                &[0x00, 0x00, 0x00, 0x00], // ValueKind::U32
                &[0x01, 0x01, 0x01],       // capabilities
                &[0x00, 0x01],             // relations
            ]),
        );
        assert_golden_eq("NotFound", &Response::NotFound, &[0x05, 0x00, 0x00, 0x00]);
        assert_golden_eq("NoParent", &Response::NoParent, &[0x06, 0x00, 0x00, 0x00]);
        assert_golden_eq("Ok", &Response::Ok, &[0x07, 0x00, 0x00, 0x00]);
        assert_golden_eq(
            "Err",
            &Response::Err {
                code: ErrorCode::UnknownField,
                message: "x".into(),
            },
            &bytes(&[
                &[0x08, 0x00, 0x00, 0x00],
                &[0x00, 0x00, 0x00, 0x00], // ErrorCode::UnknownField
                &LEN1,
                b"x",
            ]),
        );
        assert_golden_eq(
            "TransactionFailed",
            &Response::TransactionFailed {
                index: 1,
                code: ErrorCode::RecordNotFound,
                message: "x".into(),
            },
            &bytes(&[
                &[0x09, 0x00, 0x00, 0x00],
                &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // usize: 8 bytes
                &[0x05, 0x00, 0x00, 0x00],                         // ErrorCode::RecordNotFound
                &LEN1,
                b"x",
            ]),
        );
        // Protocol 2 (`PROTO-FR-002`): appended at index 10.
        assert_golden_eq(
            "Hello",
            &Response::Hello {
                protocol_version: 2,
            },
            &[0x0a, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00],
        );
        // Protocol 3 (`SESS-FR-001`): `Staged` at 11; the three new error
        // codes at 6–8, pinned through `Err`.
        assert_golden_eq(
            "Staged",
            &Response::Staged { index: 2 },
            &[0x0b, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00],
        );
        for (code, index) in [
            (ErrorCode::NoSession, 0x06u8),
            (ErrorCode::SessionOpen, 0x07),
            (ErrorCode::SessionFull, 0x08),
            // Protocol 4 (`JRN-FR-008`): `Journal` at 9.
            (ErrorCode::Journal, 0x09),
        ] {
            assert_golden_eq(
                "Err(session code)",
                &Response::Err {
                    code,
                    message: "x".into(),
                },
                &bytes(&[
                    &[0x08, 0x00, 0x00, 0x00],
                    &[index, 0x00, 0x00, 0x00],
                    &LEN1,
                    b"x",
                ]),
            );
        }
    }

    /// `PROTO-FR-001`/`PROTO-FR-005` rule 2: the constant matches the
    /// module docs' table — version 4 is the one that added
    /// `ErrorCode::Journal` (3 added the session variants, 2 `Hello`). A
    /// change that appends a variant bumps this by exactly one and
    /// extends the table; this test is the reminder.
    #[test]
    fn protocol_version_is_the_one_the_table_names() {
        assert_eq!(PROTOCOL_VERSION, 4);
    }

    /// `SESS-FR-001`: the session shapes round-trip through the codec like
    /// every existing variant.
    #[test]
    fn session_shapes_round_trip_through_the_codec() {
        for req in [Request::Begin, Request::Commit, Request::Rollback] {
            let bytes = crate::codec::encode(&req).unwrap();
            let decoded: Request = crate::codec::decode(&bytes).unwrap();
            assert!(matches!(
                (&req, &decoded),
                (Request::Begin, Request::Begin)
                    | (Request::Commit, Request::Commit)
                    | (Request::Rollback, Request::Rollback)
            ));
        }
        let resp = Response::Staged { index: 7 };
        let bytes = crate::codec::encode(&resp).unwrap();
        let decoded: Response = crate::codec::decode(&bytes).unwrap();
        assert_eq!(decoded, resp);
        for code in [
            ErrorCode::NoSession,
            ErrorCode::SessionOpen,
            ErrorCode::SessionFull,
            ErrorCode::Journal,
        ] {
            let resp = Response::Err {
                code,
                message: "irrelevant".into(),
            };
            let bytes = crate::codec::encode(&resp).unwrap();
            let decoded: Response = crate::codec::decode(&bytes).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    #[test]
    fn request_and_response_round_trip_through_bincode() {
        let req = Request::UpdateField {
            id: Uuid::from_u128(1),
            field: 1,
            value: ScanValue::U32(42),
        };
        let bytes = crate::codec::encode(&req).unwrap();
        let decoded: Request = crate::codec::decode(&bytes).unwrap();
        assert!(matches!(
            decoded,
            Request::UpdateField {
                field: 1,
                value: ScanValue::U32(42),
                ..
            }
        ));

        let resp = Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (0, ScanValue::Str("labrador".into())),
                (1, ScanValue::U32(3)),
            ],
        };
        let bytes = crate::codec::encode(&resp).unwrap();
        let decoded: Response = crate::codec::decode(&bytes).unwrap();
        assert_eq!(decoded, resp);
    }

    /// `AUTH-FR-001`/`AUTH-FR-004`: the new `Authenticate` request and the
    /// two new `ErrorCode` variants round-trip through `bincode` the same
    /// way every existing variant already does.
    #[test]
    fn authenticate_request_and_new_error_codes_round_trip_through_bincode() {
        let req = Request::Authenticate {
            token: "s3cr3t".into(),
        };
        let bytes = crate::codec::encode(&req).unwrap();
        let decoded: Request = crate::codec::decode(&bytes).unwrap();
        assert!(matches!(decoded, Request::Authenticate { token } if token == "s3cr3t"));

        for code in [ErrorCode::Unauthenticated, ErrorCode::Unauthorized] {
            let resp = Response::Err {
                code,
                message: "irrelevant".into(),
            };
            let bytes = crate::codec::encode(&resp).unwrap();
            let decoded: Response = crate::codec::decode(&bytes).unwrap();
            assert_eq!(decoded, resp);
        }
    }

    /// `TXN-FR-001`/`TXN-FR-005`: the new `Transaction` request,
    /// `TransactionFailed` response, and `RecordNotFound` error code
    /// round-trip through `bincode` the same way every existing variant
    /// already does.
    #[test]
    fn transaction_request_and_new_shapes_round_trip_through_bincode() {
        let req = Request::Transaction {
            updates: vec![
                TransactionOp {
                    id: Uuid::from_u128(1),
                    field: 1,
                    value: ScanValue::U32(9),
                },
                TransactionOp {
                    id: Uuid::from_u128(2),
                    field: 1,
                    value: ScanValue::U32(10),
                },
            ],
        };
        let bytes = crate::codec::encode(&req).unwrap();
        let decoded: Request = crate::codec::decode(&bytes).unwrap();
        assert!(matches!(
            decoded,
            Request::Transaction { updates } if updates.len() == 2
        ));

        let resp = Response::TransactionFailed {
            index: 1,
            code: ErrorCode::RecordNotFound,
            message: "irrelevant".into(),
        };
        let bytes = crate::codec::encode(&resp).unwrap();
        let decoded: Response = crate::codec::decode(&bytes).unwrap();
        assert_eq!(decoded, resp);
    }
}
