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
    NotFound,
    NoParent,
    Ok,
    Err {
        code: ErrorCode,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
