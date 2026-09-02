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

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    }

    #[test]
    fn request_and_response_round_trip_through_bincode() {
        let req = Request::UpdateField {
            id: Uuid::from_u128(1),
            field: 1,
            value: ScanValue::U32(42),
        };
        let bytes = bincode::serialize(&req).unwrap();
        let decoded: Request = bincode::deserialize(&bytes).unwrap();
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
        let bytes = bincode::serialize(&resp).unwrap();
        let decoded: Response = bincode::deserialize(&bytes).unwrap();
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
        let bytes = bincode::serialize(&req).unwrap();
        let decoded: Request = bincode::deserialize(&bytes).unwrap();
        assert!(matches!(decoded, Request::Authenticate { token } if token == "s3cr3t"));

        for code in [ErrorCode::Unauthenticated, ErrorCode::Unauthorized] {
            let resp = Response::Err {
                code,
                message: "irrelevant".into(),
            };
            let bytes = bincode::serialize(&resp).unwrap();
            let decoded: Response = bincode::deserialize(&bytes).unwrap();
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
        let bytes = bincode::serialize(&req).unwrap();
        let decoded: Request = bincode::deserialize(&bytes).unwrap();
        assert!(matches!(
            decoded,
            Request::Transaction { updates } if updates.len() == 2
        ));

        let resp = Response::TransactionFailed {
            index: 1,
            code: ErrorCode::RecordNotFound,
            message: "irrelevant".into(),
        };
        let bytes = bincode::serialize(&resp).unwrap();
        let decoded: Response = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, resp);
    }
}
