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
//! | 5 | `SERVER-001` v0.18.0 | + `Request::BeginWith { flags }` (14) — a session opened with options; [`SESSION_READ_YOUR_WRITES`] makes the connection's own `GetById` see its staged writes (`RYW-FR-001`/`002`). `Malformed` below 5 (rule 3); sent only after negotiating ≥ 5 (rule 4). No new response shape or error code. ADR-0027 |
//! | 6 | `SERVER-001` v0.20.0 | No new variant: `BeginWith` learns a flag bit, [`SESSION_VALIDATE_ON_STAGE`] — every `UpdateField` staged in such a session is validated when staged, refused with the code `Commit` would have given, nothing staged (`STV-FR-001`/`002`). A flag bit is introduced at a version exactly as a variant is: unknown (`Malformed`) below 6 (rule 3), sent only after negotiating ≥ 6 (rule 4). `ADR-0024`'s second trigger |
//! | 7 | `SERVER-001` v0.26.0 | + `ErrorCode::Conflict` (10) — a snapshot-isolated session's read set was invalidated by a commit from another connection before its own `Commit` landed; carried by `Response::TransactionFailed { index: 0, .. }`, the same sentinel-index shape `ErrorCode::Journal` established, and downgraded to `Unsupported` on a connection negotiated below 7 (rule 3). `BeginWith` learns a third flag bit, [`SESSION_SNAPSHOT_ISOLATION`] — every session `GetById` records the committed value it returned into a read set re-checked atomically at `Commit` (`ISO-FR-001`–`003`). Unknown below 7 (rule 3), sent only after negotiating ≥ 7 (rule 4). ADR-0033 |
//! | 8 | `SERVER-001` v0.27.0 | + [`Request::Query`] (15) and [`Response::Rows`] (12) — a read-only `SELECT`-shaped query (`Selection`/`Predicate`/`CompareOp`), parsed client-side from real SQL text and compiled to a new unconditional full-scan-then-filter, never an index — see `src/server/sql.rs`/[`super::client::SchemaDrivenClient::query`]. `Malformed` below 8 is not reachable (the client never sends one below 8, `SQL-FR-010`); no new `ErrorCode` — `UnknownField`/`Malformed` cover every rejection, reused. Not overlaid by a read-your-writes session and never tracked into a snapshot-isolated session's read set — the same "only `GetById`" line `RYW-FR`/`ISO-FR-002` already draw (`SQL-FR-009`). ADR-0034 |
//! | 9 | `SERVER-001` v0.28.0 | + [`Request::Aggregate`] (16) and [`Response::Groups`] (13) — `GROUP BY`/`COUNT`/`SUM`/`AVG`/`MIN`/`MAX` on top of [`Request::Query`]'s own machinery: `group_by` buckets `ConnectionStore::scan_all`'s already-filtered rows (empty `group_by` is one implicit whole-table group), `aggregates` reduces each bucket (`AggregateFn`/`AggregateSpec`), `filter` reuses [`Predicate`]/[`CompareOp`] unchanged. [`ScanValue`] gains `F64(f64)` — [`AggregateFn::Avg`]'s result, the wire's first fractional value; never a stored field's `ValueKind`. No new `ErrorCode`. Not overlaid, not read-set-tracked — the identical line `SQL-FR-009` already draws. `Malformed` below 9 is not reachable (`AGG-FR-010`); `Request::Query`/`Response::Rows` and every byte at version 8 and below are unchanged — a plain `SELECT` still only needs version 8. ADR-0035 |
//! | 10 | `SERVER-001` v0.31.0 | + [`Request::NeighborsByRelation`] (17), [`Request::ListRelationKinds`] (18), and [`Response::RelationKinds`] (14) — `ENT2-FR-004`/`005`, ADR-0039: a one-hop neighbor lookup filtered to one named relation label (`NeighborsByRelation`, answered by the existing [`Response::RecordList`] unchanged), and relation-label discovery (`ListRelationKinds`/`RelationKinds`) for a domain with more than one named `SymmetricRelation` — [`crate::generic::entity::Entity`], the first. No field added to any pre-existing `Request`/`Response` variant or to `DomainSchema`/`RelationCapabilities` — `bincode`'s positional struct encoding makes that unsafe (rule 1); both are brand-new, appended variants instead. No new `ErrorCode` — an unknown relation label reuses `Malformed`. Gated entirely client-side, the same posture `Query`/`Aggregate` already established: `dispatch` itself performs no version check (as it never has for `Query`/`Aggregate` either), relying on `SchemaDrivenClient` never sending either request below version 10. Not overlaid, not read-set-tracked. ADR-0039 |
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
pub const PROTOCOL_VERSION: u32 = 10;

/// `Request::BeginWith` flag bit 0 (protocol 5, `RYW-FR-001`, ADR-0027):
/// the session's own point reads (`GetById`) see its staged writes —
/// see `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md`. Every
/// other bit is unknown to this build and makes the request `Malformed`.
pub const SESSION_READ_YOUR_WRITES: u32 = 1;

/// `Request::BeginWith` flag bit 1 (protocol 6, `STV-FR-001`, `ADR-0024`'s
/// second revisit trigger): every `UpdateField` staged in the session is
/// validated when it is staged — `ConnectionStore::validate_op` — and
/// refused, nothing staged, with the code `Commit` would have reported for
/// it. Unknown below protocol 6 (`Malformed`, as any unknown bit).
pub const SESSION_VALIDATE_ON_STAGE: u32 = 2;

/// `Request::BeginWith` flag bit 2 (protocol 7, `ISO-FR-001`, ADR-0033):
/// every session `GetById` records the raw, committed value it returned
/// (before any read-your-writes overlay) into a per-session read set; at
/// `Commit`, inside the same exclusive section the batch's own write
/// validation and apply already use, every tracked key is re-checked
/// against current state, refusing the whole commit atomically on any
/// mismatch (`ErrorCode::Conflict`) — see
/// `docs/design/SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN.md`. Unknown
/// below protocol 7 (`Malformed`, as any unknown bit).
pub const SESSION_SNAPSHOT_ISOLATION: u32 = 4;

/// The most `UpdateField`s one connection may stage between
/// [`Request::Begin`] and [`Request::Commit`] (`SESS-FR-004`, ADR-0024):
/// the `MAX_STAGED_OPS + 1`-th is answered `ErrorCode::SessionFull` and
/// not staged, the session staying open. A constant, not a config, on
/// purpose — the first real report of hitting it decides whether it
/// becomes one. Smaller than one `MAX_FRAME_BYTES` `Transaction` could
/// carry, so a session never exceeds what a single request already may.
pub const MAX_STAGED_OPS: usize = 4096;

/// The most distinct `(id, field)` keys a snapshot-isolated session's own
/// read set tracks (`ISO-FR-004`, ADR-0033): past the cap, a `GetById` for
/// a *new* key is simply not added — the request still succeeds, `Commit`
/// still runs, the session just loses the ability to detect a conflict on
/// whatever went untracked. A constant, not a config, matching
/// [`MAX_STAGED_OPS`]'s own precedent.
pub const MAX_TRACKED_READS: usize = 4096;

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
/// `U32`/`I64`/`Bool`. `F64` (protocol 9, `AGG-FR-005`, ADR-0035) is a
/// later, narrower addition: it never describes a stored field's real
/// type (no `ValueKind::F64` exists to match it) — it exists solely to
/// carry [`AggregateFn::Avg`]'s computed result on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScanValue {
    U32(u32),
    I64(i64),
    Bool(bool),
    Str(String),
    F64(f64),
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

/// [`Request::Query`]'s column list — `All` is SQL's bare `*`, `Fields`
/// a specific, ordered subset. Protocol 8, `SQL-FR-003`, ADR-0034. See
/// `docs/design/SERVER-SQL-SELECT-DESIGN.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Selection {
    All,
    Fields(Vec<FieldRef>),
}

/// A [`Request::Query`] predicate's comparator. `Eq`/`Ne` are valid
/// against every [`ScanValue`] kind; `Lt`/`Le`/`Gt`/`Ge` are valid only
/// against `U32`/`I64` — a `Str`/`Bool` field paired with one of these is
/// `ErrorCode::Malformed`, the same code a kind-mismatched literal
/// already uses (`SQL-FR-007`). Protocol 8, ADR-0034.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CompareOp {
    /// Whether this comparator only makes sense against an orderable
    /// value kind (`U32`/`I64`) — `Str`/`Bool` paired with one of these
    /// is `ErrorCode::Malformed` server-side (`SQL-FR-007`) and the
    /// identical, shared rule a client-side query resolution rejects
    /// before any frame is sent (`SQL-FR-010`).
    pub fn is_ordering(self) -> bool {
        matches!(
            self,
            CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge
        )
    }
}

/// One `WHERE`-clause condition within a [`Request::Query`]. Every
/// `Predicate` in a query's `filter` is `AND`-ed together — there is no
/// `OR`, no parentheses (`SQL-FR-001`'s own non-goal). Protocol 8,
/// ADR-0034.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    pub field: FieldRef,
    pub op: CompareOp,
    pub value: ScanValue,
}

/// One [`Request::Aggregate`] reduction function. `Count` is always
/// `COUNT(*)` — this schema has no `NULL` concept, so `COUNT(field)`
/// would be unconditionally identical to `COUNT(*)`; a deliberate
/// simplification (`AGG-FR-008`), not an oversight. `Sum`/`Avg`/`Min`/
/// `Max` each need an [`AggregateSpec::field`], the same "orderable
/// kind" rule [`CompareOp::is_ordering`] already established
/// (`U32`/`I64` only). Protocol 9, ADR-0035.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregateFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// One column of a [`Request::Aggregate`]'s `aggregates` list — `field`
/// is `None` only for [`AggregateFn::Count`] (`COUNT(*)`), `Some(_)`
/// for every other function. Protocol 9, ADR-0035.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateSpec {
    pub func: AggregateFn,
    pub field: Option<FieldRef>,
}

/// One row of a [`Response::Groups`] result: `key` echoes back each
/// `group_by` field's value for this group (empty when `group_by` was
/// empty — the implicit single-group case, `AGG-FR-007`); `values`
/// carries one result per `Request::Aggregate::aggregates` entry, same
/// order. Protocol 9, ADR-0035.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateGroup {
    pub key: Vec<(FieldRef, ScanValue)>,
    pub values: Vec<ScanValue>,
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
    /// Protocol 7. A snapshot-isolated session's read set (`ISO-FR-002`,
    /// ADR-0033) no longer matches current state at `Commit` — some
    /// tracked `GetById` changed under another connection's commit before
    /// this one landed. Nothing was applied. Only reachable via
    /// [`Response::TransactionFailed`] (index 0, the same sentinel shape
    /// [`ErrorCode::Journal`] uses); on a connection negotiated below 7
    /// the server sends [`ErrorCode::Unsupported`] in its place.
    Conflict,
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
    /// Protocol 5. [`Request::Begin`] with options (`RYW-FR-001`, ADR-0027,
    /// `docs/design/SERVER-SESSION-READ-YOUR-WRITES-DESIGN.md`): `flags: 0`
    /// is exactly `Begin`; [`SESSION_READ_YOUR_WRITES`] makes this
    /// connection's own [`Request::GetById`] answer with its staged writes
    /// laid over the committed record — last staged write per field wins,
    /// only for fields the record carries, only when the staged value's
    /// kind matches and the schema marks the field updatable, so a write
    /// that would fail at `Commit` never produces a misleading read
    /// (`RYW-FR-002`). Set reads (`ScanField`, `FilterEq`, the relation
    /// reads), plain `Begin` sessions, and every other connection keep
    /// committed-state reads (`RYW-FR-003`/`004`). Any other bit set is
    /// `Malformed` and opens nothing; `SessionOpen` while one is open;
    /// `Malformed` on a connection negotiated below 5. Answered by
    /// `handle_connection` itself, never through [`super::dispatch`].
    /// Protocol 6 adds [`SESSION_VALIDATE_ON_STAGE`]: each `UpdateField` is
    /// validated as it is staged (`STV-FR-001`/`002`) — a bit a connection
    /// below 6 does not know, so `Malformed` there.
    BeginWith {
        flags: u32,
    },
    /// Protocol 8 (`SQL-FR-003`, ADR-0034,
    /// `docs/design/SERVER-SQL-SELECT-DESIGN.md`): a read-only,
    /// `SELECT`-shaped query — the parsed, already-typed result of a real
    /// SQL string parsed client-side by `src/server/sql.rs`, never raw text on
    /// the wire. `select` names which fields come back (`Selection::All`
    /// for every described field); `filter` is an `AND`-only list of
    /// [`Predicate`]s, validated against this domain's schema before any
    /// scan runs (`ErrorCode::UnknownField`/`Malformed`, no new code);
    /// `limit` truncates the row count, nothing else. Unconditionally a
    /// full scan (`ConnectionStore::scan_all`) — no index is ever
    /// consulted, even for a field `FilterEq`/`GetById` could answer more
    /// cheaply (`SQL-FR-004`). Read-only, gated exactly like
    /// `GetById`/`FilterEq`/`ScanField`: authentication only, no
    /// `TokenClass::ReadWrite` requirement. Never overlaid by a
    /// read-your-writes session and never tracked into a
    /// snapshot-isolation read set — always committed state
    /// (`SQL-FR-009`). `Malformed` on a connection negotiated below 8;
    /// [`super::client::SchemaDrivenClient`] checks this locally and
    /// sends no frame below it (`SQL-FR-010`).
    Query {
        select: Selection,
        filter: Vec<Predicate>,
        limit: Option<usize>,
    },
    /// Protocol 9 (`AGG-FR-004`, ADR-0035,
    /// `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md`): `GROUP BY` and
    /// aggregate functions on top of [`Request::Query`]'s own machinery.
    /// `filter` is applied first, identically to `Query`'s own `filter`
    /// (reusing [`Predicate`]/[`CompareOp`] unchanged); the survivors are
    /// bucketed by `group_by`'s field values (`group_by` empty means one
    /// implicit whole-table bucket, the `SELECT COUNT(*) FROM t` case
    /// with no `GROUP BY`); each bucket is reduced through every
    /// `aggregates` entry in order. A bucket whose key matches zero rows
    /// never appears in the result — no null/zero-valued group is
    /// synthesized. `limit` truncates the number of *groups* returned,
    /// the same role `Query`'s own `limit` plays over rows. Validated
    /// against this domain's schema before any scan runs
    /// (`ErrorCode::UnknownField`/`Malformed`, no new code) —
    /// `AGG-FR-006`. Unconditionally a full scan
    /// (`ConnectionStore::scan_all`), same as `Query` — no index, no
    /// partial-aggregation pushdown. Read-only, gated exactly like
    /// `Query`: authentication only, never overlaid by read-your-writes,
    /// never tracked into a snapshot-isolation read set (`AGG-FR-009`).
    /// `Malformed` on a connection negotiated below 9;
    /// [`super::client::SchemaDrivenClient`] checks this locally and
    /// sends no frame below it (`AGG-FR-010`).
    Aggregate {
        group_by: Vec<FieldRef>,
        filter: Vec<Predicate>,
        aggregates: Vec<AggregateSpec>,
        limit: Option<usize>,
    },
    /// Protocol 10 (`ENT2-FR-004`, ADR-0039): [`Request::Neighbors`],
    /// filtered to one named relation label — for a domain with more
    /// than one `SymmetricRelation`. `relation` naming a label this
    /// domain doesn't have is `ErrorCode::Malformed`, the same code an
    /// unknown field already uses (no new code for a bounded slice, the
    /// `FR-037`/`FR-038` posture). Answered by the already-existing
    /// [`Response::RecordList`], unchanged — no new response shape
    /// needed for the result itself. Gated entirely client-side (see
    /// this module's own "Protocol versions" row 10) — never overlaid
    /// by a read-your-writes session, never read-set-tracked.
    NeighborsByRelation {
        id: RecordId,
        relation: String,
    },
    /// Protocol 10 (`ENT2-FR-005`, ADR-0039): every relation label this
    /// domain knows — `[]` for a domain with no symmetric relation at
    /// all, one label for a single-relation domain (`Dog`), more than
    /// one for `Entity`. Lets a client discover the real label set
    /// without hardcoding one. Answered by [`Response::RelationKinds`].
    ListRelationKinds,
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
    /// Protocol 8. Answers [`Request::Query`]: every matching record's id
    /// alongside its selected fields, in whatever unspecified order
    /// `ConnectionStore::scan_all` enumerates them in — the same
    /// "unspecified order" convention `ScanValues` already carries for a
    /// full-column read (`SQL-FR-006`). `limit` truncates this list, not
    /// a meaningful top-N (no `ORDER BY`).
    Rows {
        rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>,
    },
    /// Protocol 9. Answers [`Request::Aggregate`]: every group produced,
    /// in whatever unspecified order the grouping computation produces
    /// them in — the same "unspecified order" convention `Rows` already
    /// carries. `limit` truncates this list, not a meaningful top-N.
    Groups {
        groups: Vec<AggregateGroup>,
    },
    /// Protocol 10. Answers [`Request::ListRelationKinds`]: every
    /// relation label this domain knows, unspecified order.
    RelationKinds {
        kinds: Vec<String>,
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
        // Protocol 5 (`RYW-FR-006`): `BeginWith` at 14, its flags word LE.
        assert_golden(
            "BeginWith",
            &Request::BeginWith {
                flags: SESSION_READ_YOUR_WRITES,
            },
            &[0x0e, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00],
        );
        // Protocol 7 (`ISO-FR-001`): the third bit, and all three composed
        // (`flags: 7`) — still `BeginWith` at 14, just a different flags word.
        assert_golden(
            "BeginWith(snapshot isolation)",
            &Request::BeginWith {
                flags: SESSION_SNAPSHOT_ISOLATION,
            },
            &[0x0e, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00],
        );
        assert_golden(
            "BeginWith(all three bits)",
            &Request::BeginWith {
                flags: SESSION_READ_YOUR_WRITES
                    | SESSION_VALIDATE_ON_STAGE
                    | SESSION_SNAPSHOT_ISOLATION,
            },
            &[0x0e, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00],
        );
        // Protocol 8 (`SQL-FR-003`, ADR-0034): `Query` at 15 — the first
        // `Option` field on this wire; bincode encodes it as one byte
        // (`0x00`/`0x01`), not the four-byte variant index every other
        // enum here uses, confirmed empirically rather than assumed.
        assert_golden(
            "Query",
            &Request::Query {
                select: Selection::Fields(vec![1]),
                filter: vec![Predicate {
                    field: 1,
                    op: CompareOp::Gt,
                    value: ScanValue::U32(3),
                }],
                limit: Some(10),
            },
            &bytes(&[
                &[0x0f, 0x00, 0x00, 0x00], // Query
                &[0x01, 0x00, 0x00, 0x00], // Selection::Fields
                &LEN1,
                &[0x01, 0x00],                                     // field 1
                &LEN1,                                             // filter: one predicate
                &[0x01, 0x00],                                     // predicate.field
                &[0x04, 0x00, 0x00, 0x00],                         // CompareOp::Gt
                &[0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00], // ScanValue::U32(3)
                &[0x01],                                           // limit: Some
                &[0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 10usize
            ]),
        );
        // Protocol 9 (`AGG-FR-004`, ADR-0035): `Aggregate` at 16 —
        // exercises `group_by`, `filter` (reusing `Predicate`), an
        // `AggregateSpec` with `field: Some(_)`, and `limit: None`.
        assert_golden(
            "Aggregate",
            &Request::Aggregate {
                group_by: vec![1],
                filter: vec![Predicate {
                    field: 1,
                    op: CompareOp::Eq,
                    value: ScanValue::U32(5),
                }],
                aggregates: vec![AggregateSpec {
                    func: AggregateFn::Sum,
                    field: Some(2),
                }],
                limit: None,
            },
            &bytes(&[
                &[0x10, 0x00, 0x00, 0x00], // Aggregate
                &LEN1,                     // group_by: one field
                &[0x01, 0x00],
                &LEN1,                                             // filter: one predicate
                &[0x01, 0x00],                                     // predicate.field
                &[0x00, 0x00, 0x00, 0x00],                         // CompareOp::Eq
                &[0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00], // ScanValue::U32(5)
                &LEN1,                                             // aggregates: one spec
                &[0x01, 0x00, 0x00, 0x00],                         // AggregateFn::Sum
                &[0x01],                                           // field: Some
                &[0x02, 0x00],                                     // field 2
                &[0x00],                                           // limit: None
            ]),
        );
        // Protocol 10 (`ENT2-FR-004`/`005`, ADR-0039): `NeighborsByRelation`
        // at 17, `ListRelationKinds` at 18 (no fields, same shape as
        // `DescribeSchema`).
        assert_golden(
            "NeighborsByRelation",
            &Request::NeighborsByRelation {
                id,
                relation: "relates_to".into(),
            },
            &bytes(&[
                &[0x11, 0x00, 0x00, 0x00], // NeighborsByRelation
                &ID1,
                &[0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // relation len
                b"relates_to",
            ]),
        );
        assert_golden(
            "ListRelationKinds",
            &Request::ListRelationKinds,
            &[0x12, 0x00, 0x00, 0x00],
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
        // Protocol 8 (`SQL-FR-003`, ADR-0034): `Rows` at 12.
        assert_golden_eq(
            "Rows",
            &Response::Rows {
                rows: vec![(id, vec![(1, ScanValue::U32(3))])],
            },
            &bytes(&[
                &[0x0c, 0x00, 0x00, 0x00], // Rows
                &LEN1,                     // rows: one (id, fields) pair
                &ID1,
                &LEN1, // fields: one (tag, value) pair
                &[0x01, 0x00],
                &[0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00], // ScanValue::U32(3)
            ]),
        );
        // Protocol 9 (`AGG-FR-004`, ADR-0035): `Groups` at 13 — exercises
        // one group's `key` and two `values`, including the new
        // `ScanValue::F64` (`AGG-FR-005`).
        assert_golden_eq(
            "Groups",
            &Response::Groups {
                groups: vec![AggregateGroup {
                    key: vec![(1, ScanValue::U32(3))],
                    values: vec![ScanValue::I64(5), ScanValue::F64(2.5)],
                }],
            },
            &bytes(&[
                &[0x0d, 0x00, 0x00, 0x00], // Groups
                &LEN1,                     // groups: one AggregateGroup
                &LEN1,                     // key: one (tag, value) pair
                &[0x01, 0x00],
                &[0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00], // ScanValue::U32(3)
                &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // values: two entries
                &[
                    0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                ], // ScanValue::I64(5)
                &[
                    0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x40,
                ], // ScanValue::F64(2.5)
            ]),
        );
        // Protocol 10 (`ENT2-FR-005`, ADR-0039): `RelationKinds` at 14 —
        // answers `Request::ListRelationKinds`.
        assert_golden_eq(
            "RelationKinds",
            &Response::RelationKinds {
                kinds: vec!["relates_to".into()],
            },
            &bytes(&[
                &[0x0e, 0x00, 0x00, 0x00], // RelationKinds
                &LEN1,                     // kinds: one label
                &[0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                b"relates_to",
            ]),
        );
        for (code, index) in [
            (ErrorCode::NoSession, 0x06u8),
            (ErrorCode::SessionOpen, 0x07),
            (ErrorCode::SessionFull, 0x08),
            // Protocol 4 (`JRN-FR-008`): `Journal` at 9.
            (ErrorCode::Journal, 0x09),
            // Protocol 7 (`ISO-FR-003`): `Conflict` at 10.
            (ErrorCode::Conflict, 0x0a),
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
    /// module docs' table — version 9 is the one that added
    /// `Request::Aggregate`/`Response::Groups` (8 added `Request::Query`/
    /// `Response::Rows`, 7 `ErrorCode::Conflict` and
    /// `SESSION_SNAPSHOT_ISOLATION`, 6 `SESSION_VALIDATE_ON_STAGE`, 4
    /// `ErrorCode::Journal`, 3 the session variants, 2 `Hello`). A change
    /// that appends a variant bumps this by exactly one and extends the
    /// table; this test is the reminder.
    #[test]
    fn protocol_version_is_the_one_the_table_names() {
        assert_eq!(PROTOCOL_VERSION, 10);
    }

    /// `SESS-FR-001`: the session shapes round-trip through the codec like
    /// every existing variant.
    #[test]
    fn session_shapes_round_trip_through_the_codec() {
        for req in [
            Request::Begin,
            Request::Commit,
            Request::Rollback,
            Request::BeginWith { flags: 1 },
        ] {
            let bytes = crate::codec::encode(&req).unwrap();
            let decoded: Request = crate::codec::decode(&bytes).unwrap();
            assert!(matches!(
                (&req, &decoded),
                (Request::Begin, Request::Begin)
                    | (Request::Commit, Request::Commit)
                    | (Request::Rollback, Request::Rollback)
                    | (
                        Request::BeginWith { flags: 1 },
                        Request::BeginWith { flags: 1 }
                    )
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
