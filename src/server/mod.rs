//! A network server/query layer in front of [`crate::production::ProductionStore`]/
//! [`crate::generic::production::GenericProductionStore`] — accepted design,
//! `docs/design/SERVER-QUERY-LAYER-DESIGN.md`, ADR-0010. Off by default
//! behind the `server` Cargo feature: this module adds a real,
//! network-listening binary capability, distinct from the `research`
//! feature's benchmarked-alternative/historical-spike bucket, so it's
//! opted into deliberately rather than pulled in incidentally by
//! `--all-features`.
//!
//! # This is a thin translation layer, not a new storage engine
//!
//! [`dispatch`] and [`serve`] never touch a record's bytes directly —
//! every operation goes through the existing, already-validated
//! [`ConnectionStore`] adapter around a real store type
//! ([`dog::DogConnectionStore`] wraps [`crate::production::ProductionStore`];
//! [`order::OrderConnectionStore`]/[`employee::EmployeeConnectionStore`] wrap
//! [`crate::generic::production::GenericProductionStore`]). Concurrency
//! across client connections adds no new lock: it collapses onto whatever
//! `RwLock` the wrapped store already manages internally, per
//! `docs/FUTURE-GROWTH.md`'s own "Path to a server / query layer" section.
//!
//! # What this does not provide
//!
//! No query language beyond fixed field-tag addressing — an explicit
//! non-goal of the accepted design(s). **Do not expose a server built
//! from this module beyond a trusted, localhost/development network
//! unless both `ServeOptions` and `TlsConfig` (below) are configured** —
//! see ADR-0010's Consequences; ADR-0012 closed the authentication/
//! authorization half of that gap, ADR-0014 closes the transport-
//! encryption half, but neither alone is the whole story (a
//! `TlsConfig`-only server still lets anyone who can connect do
//! anything; a `ServeOptions`-only server still puts tokens and every
//! record value in plaintext on the wire). ADR-0010's third named gap,
//! "no transaction semantics," is now partly closed too — see "Atomic
//! transactions" below for exactly which slice.
//!
//! # Authentication/authorization (`ServeOptions`), ADR-0012
//!
//! `docs/design/SERVER-AUTH-DESIGN.md` closes the "no authentication, no
//! authorization" gap ADR-0010 originally left open: [`serve`] takes an
//! [`ServeOptions`] naming which token(s) (if any) a server instance
//! accepts and the [`TokenClass`] (`ReadOnly`/`ReadWrite`) each grants.
//! `Request::Authenticate` establishes a connection's class; every other
//! request kind is rejected with `ErrorCode::Unauthenticated` until it
//! does, and `ReadOnly` is further rejected from `Request::UpdateField`/
//! `Request::Transaction` with `ErrorCode::Unauthorized`.
//! `ServeOptions::default()` (no tokens configured) reproduces exactly the
//! pre-ADR-0012 unauthenticated behavior (`AUTH-FR-007`) — this is purely
//! opt-in. See this module's own `handle_connection` for the full gating
//! logic.
//!
//! # Atomic transactions, ADR-0013
//!
//! `docs/design/SERVER-TRANSACTION-DESIGN.md` closes one bounded slice of
//! ADR-0010's "no transaction semantics" gap: `Request::Transaction { updates }`
//! batches several `UpdateField`-shaped writes and applies them
//! all-or-nothing — every operation's precondition is checked before any
//! write happens, isolated from concurrent connections by holding the
//! wrapped store's existing lock for the whole batch (no new lock; see
//! `crate::production::TransactionalStore`/
//! `crate::generic::production::GenericProductionStore::with_exclusive`).
//! **Deliberately not a multi-round-trip interactive session** (no
//! `Begin`/`Commit`, transaction state held open across several
//! requests — a real, unaccepted-elsewhere liveness risk) **and not
//! crash-atomic** (a process crash mid-batch can leave a partial batch
//! durably applied) — see that design document's own "Non-goals" for the
//! full account. Both halves were later taken by
//! `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md`: a *buffered*
//! session (`SERVER-001` FR-024, ADR-0024) that stages writes
//! per connection and commits them as one batch, holding no lock across
//! round trips; and an opt-in redo journal (`SERVER-001` FR-025,
//! ADR-0025, [`journal`]) that an adapter built with `with_journal`
//! appends and `fsync`s before a batch's first write — since `SERVER-001`
//! FR-027 (ADR-0026) as a leader/follower group commit that runs outside
//! the store's exclusive section and applies in journal order — and replays on the
//! next open, so a batch answered `Ok` survives a crash whole.
//!
//! # A real, schema-driven client
//!
//! [`client::SchemaDrivenClient`] is the client half of ADR-0011's schema
//! discovery: a real, reusable client that never imports a domain's own
//! `FIELD_*` constants, driving every request purely from what
//! `Request::DescribeSchema` reports at connect time. Unconditional under
//! `server` (not `research`-gated) — it has no domain-specific code at
//! all, only `Request`/`Response`/framing.
//!
//! # Native transport encryption (`TlsConfig`), ADR-0014
//!
//! `docs/design/SERVER-TLS-DESIGN.md` closes the transport-encryption half
//! of ADR-0010's gap that ADR-0012 explicitly left open: [`serve`] takes
//! an optional [`TlsConfig`] (`None` reproduces today's plaintext behavior
//! exactly, `TLS-FR-008`). When configured, every accepted connection
//! performs a TLS server handshake — via
//! [`rusty_tls::TlsAcceptor`](https://github.com/Rusty-Mill/rusty_mill/tree/main/crates/rusty_tls),
//! this owner's own ecosystem-wide `rustls` wrapper, not a direct
//! `rustls` dependency (see that design's own "Ecosystem check" for why)
//! — before any framed `Request`/`Response` traffic, including
//! `Authenticate`, is ever read or written. `dispatch`/`ConnectionStore`
//! remain completely unaware transport encryption exists, and
//! `src/server/framing.rs` needed zero changes, since its functions were
//! already generic over `Read`/`Write`. Explicitly not mTLS — client
//! identity remains exactly [`ServeOptions`]'s existing shared-secret token
//! scheme, now traveling encrypted rather than plaintext.
//!
//! # Protocol version (`Hello`), ADR-0022
//!
//! `docs/design/SERVER-PROTOCOL-VERSION-DESIGN.md` gives the wire *shape*
//! (the set of `Request`/`Response` variants) a version:
//! [`crate::server::protocol::PROTOCOL_VERSION`], the "Protocol versions"
//! table and the append-only compatibility rules in
//! [`crate::server::protocol`]'s own docs. A client may send
//! `Request::Hello { protocol_version }` as its **first** frame;
//! `handle_connection` answers it itself — before the `Authenticate`
//! intercept and the authentication gate, on plain and TLS connections
//! alike — with `Response::Hello { min(client, PROTOCOL_VERSION) }`
//! (`PROTO-FR-003`). Version 0, or a `Hello` that is not the first frame,
//! is `ErrorCode::Malformed` and the connection stays open
//! (`PROTO-FR-004`). A client that never says `Hello` speaks version 1
//! (the `SERVER-001` v0.9.1 shape) and is served exactly as before — no
//! existing client, test, or bench changed (`PROTO-FR-002`). Since
//! protocol version 3 (`SERVER-001` FR-024, ADR-0024) the negotiated
//! version *is* kept per connection, because the first gated variants —
//! the transaction-session requests — consult it: rule 3 in
//! [`crate::server::protocol`] got its first branch exactly where
//! ADR-0022 said it would (see `handle_connection`'s own "Transaction
//! sessions" section).

pub mod access;
pub mod audit;
pub mod client;
pub mod dog;
#[cfg(feature = "research")]
pub mod employee;
/// `Entity`'s adapter — `server`-gated alone, not `server` +
/// `research` (`ENT-FR-006`, ADR-0037): matching `Reminder`'s own
/// front-door precedent, not `order`/`employee`'s.
pub mod entity;
pub mod framing;
pub mod journal;
#[cfg(feature = "research")]
pub mod order;
mod pem;
pub mod protocol;
/// `Reminder`'s adapter — `server`-gated alone, not `server` +
/// `research` (`RMD-FR-006`, ADR-0036): unlike `order`/`employee`,
/// `Reminder` is real, deployable capability, not reference material.
pub mod reminder;
mod sql;

use protocol::{
    AggregateFn, AggregateGroup, AggregateSpec, CompareOp, DomainSchema, ErrorCode, FieldRef,
    ParentLookup, Predicate, RecordId, Request, Response, ScanValue, Selection, TransactionOp,
    MAX_STAGED_OPS, MAX_TRACKED_READS, PROTOCOL_VERSION, SESSION_READ_YOUR_WRITES,
    SESSION_SNAPSHOT_ISOLATION, SESSION_VALIDATE_ON_STAGE,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// One shared trait the dispatch loop is generic over, implemented by a
/// thin per-domain adapter — the dispatch loop itself never depends on
/// which concrete store it's serving. See [`dog::DogConnectionStore`]
/// (`Neighbors` only), [`order::OrderConnectionStore`] (`Parent`/`Children`
/// only), and [`employee::EmployeeConnectionStore`] (both — the first
/// domain to combine them) for the three domains this crate validates
/// against, matching this project's own "validate against a second,
/// structurally different domain" discipline
/// (`docs/decisions/ADR-0009-generic-schema-design-proposal.md`).
pub trait ConnectionStore: Send + Sync {
    /// Full-record read. `None` if `id` has no record — an ordinary
    /// outcome, not an error, matching [`crate::store::DogStore::get`]'s
    /// own convention.
    fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>>;

    /// Equality filter on an indexed field. `Err(ErrorCode::UnknownField)`
    /// for a tag this adapter doesn't recognize at all;
    /// `Err(ErrorCode::Unsupported)` for a recognized field with no
    /// equality-index in-process; `Err(ErrorCode::Malformed)` if `value`'s
    /// variant doesn't match the field's real type.
    fn filter_eq(&self, field: FieldRef, value: &ScanValue) -> Result<Vec<RecordId>, ErrorCode>;

    /// Every record's value for a scannable field, unspecified order —
    /// generalizes `scan_ages`/`ScanField::scan`.
    fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode>;

    /// `Ok(true)` if `id` was found and updated, `Ok(false)` if `id` has no
    /// record (an ordinary outcome, matching `update_age`'s own
    /// `NotFound` case at this layer) — `Err` only for a field/value
    /// problem, not a missing record.
    fn update_field(
        &self,
        id: RecordId,
        field: FieldRef,
        value: ScanValue,
    ) -> Result<bool, ErrorCode>;

    /// The "one hop up" side of a directed relation. See
    /// [`ParentLookup`]'s own doc comment for why this preserves the
    /// not-found/no-parent distinction rather than collapsing it.
    /// `Err(ErrorCode::Unsupported)` for a domain with no directed
    /// relation at all (e.g. `Dog`).
    fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode>;

    /// The "one hop down" side of a directed relation.
    /// `Err(ErrorCode::Unsupported)` for a domain with no directed
    /// relation at all.
    fn children(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode>;

    /// A symmetric relation (e.g. `Dog`'s `littermate_of`). Returns the
    /// union of every named relation this domain has, if it has more
    /// than one — the same "no relation-type discriminant" shape
    /// [`Request::Neighbors`] has always had. `Err(ErrorCode::Unsupported)`
    /// for a domain with no symmetric relation at all (e.g.
    /// `Order`/`Customer`).
    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode>;

    /// `ENT2-FR-004` (ADR-0039): a symmetric relation filtered to one
    /// named label — for a domain with more than one `SymmetricRelation`
    /// (`Entity`, the first). `Err(ErrorCode::Malformed)` for a label
    /// this domain doesn't have at all; `Err(ErrorCode::Unsupported)`
    /// for a domain with no symmetric relation, matching `neighbors`'s
    /// own convention. Every adapter with at most one relation label
    /// implements this as `Err(ErrorCode::Unsupported)` unconditionally
    /// — the same "not yet needed" shape `parent`/`children` had before
    /// `Employee` needed them for real.
    fn neighbors_by_relation(
        &self,
        id: RecordId,
        relation: &str,
    ) -> Result<Vec<RecordId>, ErrorCode>;

    /// `ENT2-FR-005` (ADR-0039): every relation label this domain knows
    /// — `[]` for a domain with no symmetric relation at all, one label
    /// for a single-relation domain, more than one for `Entity`.
    /// Infallible, the same shape `describe` already has.
    fn list_relation_kinds(&self) -> Vec<String>;

    /// This domain's schema, for a client that doesn't know it at compile
    /// time — ADR-0011. Infallible: every `ConnectionStore` implementor
    /// knows its own field/relation shape unconditionally, no store access
    /// needed.
    fn describe(&self) -> DomainSchema;

    /// `STV-FR-002` (`ADR-0024`'s second trigger): the precondition check
    /// [`ConnectionStore::apply_transaction`] would run for `op` alone —
    /// id exists, field known and updatable, value type matches — with no
    /// write, so a stage-time validating session can refuse the write
    /// now with the code `Commit` would have reported. Existence may
    /// change before `Commit` only in the direction this crate never
    /// takes (no runtime deletion), so `Ok` here is `Ok` at `Commit`.
    fn validate_op(&self, op: &TransactionOp) -> Result<(), ErrorCode>;

    /// Apply every operation in `updates` atomically: every precondition
    /// (id exists, field known and updatable, value type matches) is
    /// checked before any write is applied; either every write in
    /// `updates` is applied, or none are. `Err((index, code))` names the
    /// first operation that failed its precondition check — see
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013,
    /// `TXN-FR-002`/`TXN-FR-003`.
    ///
    /// `read_set` (`ISO-FR-006`, ADR-0033) is a snapshot-isolated session's
    /// tracked `(id, field) -> value` reads, empty when the session has
    /// snapshot isolation off (`SESSION_SNAPSHOT_ISOLATION` unset) — every
    /// entry is re-checked against current state inside the same exclusive
    /// section this method already applies writes under, atomically with
    /// that apply. Any mismatch fails the whole call with
    /// `(0, ErrorCode::Conflict)` before any write happens, the same
    /// sentinel-index shape a precondition failure from `updates` itself
    /// uses. See `docs/design/SERVER-SESSION-SNAPSHOT-ISOLATION-DESIGN.md`,
    /// `ISO-FR-002`.
    /// Every record's id and full field set, unspecified order — the
    /// `ScanField`-style "no meaningful order" convention, generalized
    /// from one field to all of them. The one new primitive
    /// `Request::Query` needs (`SQL-FR-004`, ADR-0034): unconditionally a
    /// full scan, no index — `dispatch`'s `Query` arm filters, projects,
    /// and limits the result centrally, the same way for every domain, so
    /// this method itself does neither. See
    /// `docs/design/SERVER-SQL-SELECT-DESIGN.md`.
    fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>;

    fn apply_transaction(
        &self,
        updates: &[TransactionOp],
        read_set: &[(RecordId, FieldRef, ScanValue)],
    ) -> Result<(), (usize, ErrorCode)>;
}

/// One message per [`ErrorCode`] variant — shared by [`err_response`] (a
/// single request's failure) and `dispatch`'s `Request::Transaction` arm
/// (a batch operation's failure, `Response::TransactionFailed`), so both
/// paths report the same wording for the same code.
fn error_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::UnknownField => "unrecognized field tag for this domain",
        ErrorCode::Unsupported => "this operation is not available for this field/domain",
        ErrorCode::Malformed => "the supplied value does not match this field's type",
        ErrorCode::Unauthenticated => "this connection has not presented a recognized token",
        ErrorCode::Unauthorized => "this connection's token does not permit this operation",
        ErrorCode::RecordNotFound => "this operation's id has no record",
        ErrorCode::NoSession => "no transaction session is open on this connection",
        ErrorCode::SessionOpen => "a transaction session is already open on this connection",
        ErrorCode::SessionFull => "this session already holds the maximum number of staged writes",
        ErrorCode::Journal => {
            "the batch could not be journaled before applying it; nothing was applied"
        }
        ErrorCode::Conflict => {
            "this session's read set no longer matches current state; nothing was applied"
        }
    }
}

/// `RYW-FR-002` (ADR-0027): lay a read-your-writes session's staged
/// writes over a committed `Record`'s fields. For each field the record
/// carries, the *last* staged operation with this `id` and `field`
/// replaces the value — provided the staged value's kind equals the
/// committed one's and the field is one of `updatable` (the schema's
/// `update`-capable tags, read once at `BeginWith`). Everything else is
/// untouched: a missing id never gains a record (the caller only reaches
/// here with a `Record`), an absent field, a kind mismatch, or a
/// read-only field is ignored — each would fail at `Commit`, and the read
/// must not pretend otherwise. A pure function; linear in the buffer.
pub(crate) fn overlay_staged(
    id: RecordId,
    fields: &mut [(FieldRef, ScanValue)],
    staged: &[TransactionOp],
    updatable: &[FieldRef],
) {
    for (field, value) in fields.iter_mut() {
        if !updatable.contains(field) {
            continue;
        }
        if let Some(op) = staged
            .iter()
            .rev()
            .find(|op| op.id == id && op.field == *field)
        {
            if same_kind(&op.value, value) {
                *value = op.value.clone();
            }
        }
    }
}

fn same_kind(a: &ScanValue, b: &ScanValue) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// `ISO-FR-002`/`ISO-FR-004`/`ISO-FR-005` (ADR-0033): fold `id`'s raw,
/// committed `fields` — the exact result `dispatch` returned, before any
/// read-your-writes overlay — into a snapshot-isolated session's read
/// set. Each `(id, field)` key holds the most recently read value; past
/// `MAX_TRACKED_READS` distinct keys a *new* key is simply not added,
/// while an already-tracked key keeps updating on re-read — the read
/// never fails, and `Commit` still runs on whatever *was* tracked. A
/// pure function over the map, the same shape `overlay_staged` is over
/// `fields`.
pub(crate) fn record_read_set(
    reads: &mut HashMap<(RecordId, FieldRef), ScanValue>,
    id: RecordId,
    fields: &[(FieldRef, ScanValue)],
) {
    for (field, value) in fields {
        let key = (id, *field);
        if reads.contains_key(&key) || reads.len() < MAX_TRACKED_READS {
            reads.insert(key, value.clone());
        }
    }
}

/// `SQL-FR-007` (ADR-0034): every rejection `Request::Query` can produce,
/// checked against `schema` alone — before `ConnectionStore::scan_all`
/// ever runs, so a malformed query costs nothing beyond this. An unknown
/// tag in `select` or `filter` is `ErrorCode::UnknownField`; a predicate
/// whose value kind doesn't match its field's real type, or whose
/// comparator is an ordering one (`Lt`/`Le`/`Gt`/`Ge`) against a
/// `Str`/`Bool` field, is `ErrorCode::Malformed` — both existing codes,
/// no new wire addition.
fn validate_query(
    schema: &DomainSchema,
    select: &Selection,
    filter: &[Predicate],
) -> Result<(), ErrorCode> {
    let kind_of = |tag: FieldRef| {
        schema
            .fields
            .iter()
            .find(|f| f.tag == tag)
            .map(|f| f.value_kind)
    };
    if let Selection::Fields(fields) = select {
        for &tag in fields {
            if kind_of(tag).is_none() {
                return Err(ErrorCode::UnknownField);
            }
        }
    }
    for predicate in filter {
        let kind = kind_of(predicate.field).ok_or(ErrorCode::UnknownField)?;
        if !value_matches_kind(kind, &predicate.value) {
            return Err(ErrorCode::Malformed);
        }
        let orderable_kind = matches!(kind, protocol::ValueKind::U32 | protocol::ValueKind::I64);
        if predicate.op.is_ordering() && !orderable_kind {
            return Err(ErrorCode::Malformed);
        }
    }
    Ok(())
}

/// Deliberately no `StrList` arm (`ENT4-FR-002`, ADR-0041): a predicate
/// against a list-kinded field, whatever value it carries, is `Malformed`
/// — `aliases` is read-only over the wire; resolving *by* alias is
/// `FilterEq` on `label`.
fn value_matches_kind(kind: protocol::ValueKind, value: &ScanValue) -> bool {
    matches!(
        (kind, value),
        (protocol::ValueKind::U32, ScanValue::U32(_))
            | (protocol::ValueKind::I64, ScanValue::I64(_))
            | (protocol::ValueKind::Bool, ScanValue::Bool(_))
            | (protocol::ValueKind::Str, ScanValue::Str(_))
    )
}

/// `SQL-FR-006` (ADR-0034): filter, project, and limit `rows` — the one
/// place this logic is written, shared by every domain's
/// `Request::Query` (`ConnectionStore::scan_all` itself returns every
/// record's every field, unfiltered and unprojected). `filter`'s
/// predicates are `AND`-ed; `limit` truncates whatever order the input
/// arrived in, not a meaningful top-N (no `ORDER BY`).
fn evaluate_query(
    rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>,
    select: &Selection,
    filter: &[Predicate],
    limit: Option<usize>,
) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
    let mut matched: Vec<_> = rows
        .into_iter()
        .filter(|(_, fields)| filter.iter().all(|p| predicate_matches(fields, p)))
        .map(|(id, fields)| (id, select_fields(fields, select)))
        .collect();
    if let Some(limit) = limit {
        matched.truncate(limit);
    }
    matched
}

fn predicate_matches(fields: &[(FieldRef, ScanValue)], predicate: &Predicate) -> bool {
    fields
        .iter()
        .find(|(field, _)| *field == predicate.field)
        .is_some_and(|(_, value)| compare(value, predicate.op, &predicate.value))
}

/// `validate_query` already refused an ordering comparator against a
/// `Str`/`Bool` field before this ever runs, so the `_ => false` arm
/// below is unreachable through `dispatch` — kept as a safe default
/// rather than a `match` that could panic if that invariant ever broke.
fn compare(actual: &ScanValue, op: CompareOp, expected: &ScanValue) -> bool {
    match op {
        CompareOp::Eq => actual == expected,
        CompareOp::Ne => actual != expected,
        CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge => match (actual, expected) {
            (ScanValue::U32(a), ScanValue::U32(b)) => ordering_matches(a.cmp(b), op),
            (ScanValue::I64(a), ScanValue::I64(b)) => ordering_matches(a.cmp(b), op),
            _ => false,
        },
    }
}

fn ordering_matches(ordering: std::cmp::Ordering, op: CompareOp) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    matches!(
        (op, ordering),
        (CompareOp::Lt, Less)
            | (CompareOp::Gt, Greater)
            | (CompareOp::Le, Less | Equal)
            | (CompareOp::Ge, Greater | Equal)
    )
}

fn select_fields(
    fields: Vec<(FieldRef, ScanValue)>,
    select: &Selection,
) -> Vec<(FieldRef, ScanValue)> {
    match select {
        Selection::All => fields,
        Selection::Fields(wanted) => fields
            .into_iter()
            .filter(|(field, _)| wanted.contains(field))
            .collect(),
    }
}

/// `AGG-FR-006` (ADR-0035): every rejection `Request::Aggregate` can
/// produce, checked against `schema` alone — before
/// `ConnectionStore::scan_all` ever runs. `filter` is validated exactly
/// like `Request::Query`'s own filter (`validate_query`'s identical
/// checks). An unknown tag in `group_by` or an `AggregateSpec::field` is
/// `ErrorCode::UnknownField`; a `StrList`-kinded tag in `group_by` is
/// `ErrorCode::Malformed` (`ENT4-FR-003`, ADR-0041 — a list is never a
/// group key, so `Response::Groups` never carries one); a `Count` whose `field` is `Some(_)` (only
/// `COUNT(*)` is supported — this schema has no `NULL` concept, so
/// `COUNT(field)` would be unconditionally identical), or a
/// `Sum`/`Avg`/`Min`/`Max` with no field or a non-`U32`/`I64` field (the
/// identical "orderable kind" rule `CompareOp::is_ordering` already
/// established), is `ErrorCode::Malformed`.
fn validate_aggregate(
    schema: &DomainSchema,
    group_by: &[FieldRef],
    filter: &[Predicate],
    aggregates: &[AggregateSpec],
) -> Result<(), ErrorCode> {
    let kind_of = |tag: FieldRef| {
        schema
            .fields
            .iter()
            .find(|f| f.tag == tag)
            .map(|f| f.value_kind)
    };
    for &tag in group_by {
        match kind_of(tag) {
            None => return Err(ErrorCode::UnknownField),
            // `ENT4-FR-003` (ADR-0041): a `StrList` field is not groupable
            // — the one way a `StrList` could otherwise reach
            // `Response::Groups`, which `downgrade_for_version` cannot
            // strip. `Malformed`, the code every other kind-shaped
            // aggregate rejection already uses.
            Some(protocol::ValueKind::StrList) => return Err(ErrorCode::Malformed),
            Some(_) => {}
        }
    }
    for predicate in filter {
        let kind = kind_of(predicate.field).ok_or(ErrorCode::UnknownField)?;
        if !value_matches_kind(kind, &predicate.value) {
            return Err(ErrorCode::Malformed);
        }
        let orderable_kind = matches!(kind, protocol::ValueKind::U32 | protocol::ValueKind::I64);
        if predicate.op.is_ordering() && !orderable_kind {
            return Err(ErrorCode::Malformed);
        }
    }
    for spec in aggregates {
        match (spec.func, spec.field) {
            (AggregateFn::Count, None) => {}
            (AggregateFn::Count, Some(_)) => return Err(ErrorCode::Malformed),
            (_, None) => return Err(ErrorCode::Malformed),
            (_, Some(tag)) => {
                let kind = kind_of(tag).ok_or(ErrorCode::UnknownField)?;
                if !matches!(kind, protocol::ValueKind::U32 | protocol::ValueKind::I64) {
                    return Err(ErrorCode::Malformed);
                }
            }
        }
    }
    Ok(())
}

/// `AGG-FR-007`/`AGG-FR-008` (ADR-0035): filter, bucket, and reduce
/// `rows` — the one place this logic is written, shared by every
/// domain's `Request::Aggregate` (`scan_all`'s result is unfiltered and
/// unbucketed). `filter` reuses `predicate_matches` unchanged; `group_by`
/// empty means exactly one implicit bucket over every filtered row (the
/// `SELECT COUNT(*) FROM t` case with no `GROUP BY`) — that bucket always
/// exists, even holding zero rows (`SELECT COUNT(*) FROM t WHERE false`-
/// equivalent still returns one group whose `Count` is `0`, acceptance
/// criterion 4); a *keyed* bucket (`group_by` non-empty) whose key
/// matches zero rows never appears, since there is no key to have not
/// matched. `limit` truncates the *group* count, applied after the full
/// reduction — the same "bounds the response, not the work" shape
/// `evaluate_query`'s own `limit` already has. `schema` resolves each
/// `Sum`/`Avg`/`Min`/`Max` field's `ValueKind` for the one case a real
/// observed value can't: the implicit whole-table bucket with zero rows.
fn evaluate_aggregate(
    rows: Vec<(RecordId, Vec<(FieldRef, ScanValue)>)>,
    group_by: &[FieldRef],
    filter: &[Predicate],
    aggregates: &[AggregateSpec],
    limit: Option<usize>,
    schema: &DomainSchema,
) -> Vec<AggregateGroup> {
    type RowFields = Vec<(FieldRef, ScanValue)>;
    let mut buckets: Vec<(RowFields, Vec<RowFields>)> = if group_by.is_empty() {
        vec![(Vec::new(), Vec::new())]
    } else {
        Vec::new()
    };
    for (_, fields) in rows
        .into_iter()
        .filter(|(_, fields)| filter.iter().all(|p| predicate_matches(fields, p)))
    {
        let key: Vec<(FieldRef, ScanValue)> = group_by
            .iter()
            .filter_map(|&tag| fields.iter().find(|(f, _)| *f == tag).cloned())
            .collect();
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group_rows)) => group_rows.push(fields),
            None => buckets.push((key, vec![fields])),
        }
    }
    let mut groups: Vec<AggregateGroup> = buckets
        .into_iter()
        .map(|(key, group_rows)| AggregateGroup {
            key,
            values: aggregates
                .iter()
                .map(|spec| reduce(spec, &group_rows, schema))
                .collect(),
        })
        .collect();
    if let Some(limit) = limit {
        groups.truncate(limit);
    }
    groups
}

/// One `AggregateSpec`'s result over one bucket's rows — `AGG-FR-008`.
/// `validate_aggregate` already guarantees `Sum`/`Avg`/`Min`/`Max` carry
/// a field of `U32`/`I64` kind, and `Count` carries none. `rows` is empty
/// only for the implicit whole-table bucket when no row matched `filter`
/// (`evaluate_aggregate`'s own doc comment) — `Sum` and `Avg` are already
/// well-defined there (an empty sum is `0`, an empty average is defined
/// as `0.0` by this schema's own choice, since it has no `NULL` to return
/// instead); `Min`/`Max` have no real observed value to pass through, so
/// they fall back to `schema`-typed zero rather than panicking.
fn reduce(
    spec: &AggregateSpec,
    rows: &[Vec<(FieldRef, ScanValue)>],
    schema: &DomainSchema,
) -> ScanValue {
    const FIELD_REQUIRED: &str = "validate_aggregate requires a field for Sum/Avg/Min/Max";
    match spec.func {
        AggregateFn::Count => ScanValue::I64(rows.len() as i64),
        AggregateFn::Sum => {
            let field = spec.field.expect(FIELD_REQUIRED);
            ScanValue::I64(field_values(field, rows).map(|v| numeric_i64(&v)).sum())
        }
        AggregateFn::Avg => {
            let field = spec.field.expect(FIELD_REQUIRED);
            let values: Vec<i64> = field_values(field, rows).map(|v| numeric_i64(&v)).collect();
            if values.is_empty() {
                ScanValue::F64(0.0)
            } else {
                let sum: i64 = values.iter().sum();
                ScanValue::F64(sum as f64 / values.len() as f64)
            }
        }
        AggregateFn::Min => reduce_extreme(spec.field.expect(FIELD_REQUIRED), rows, false, schema),
        AggregateFn::Max => reduce_extreme(spec.field.expect(FIELD_REQUIRED), rows, true, schema),
    }
}

fn field_values<'a>(
    field: FieldRef,
    rows: &'a [Vec<(FieldRef, ScanValue)>],
) -> impl Iterator<Item = ScanValue> + 'a {
    rows.iter()
        .filter_map(move |fields| fields.iter().find(|(f, _)| *f == field))
        .map(|(_, v)| v.clone())
}

fn numeric_i64(value: &ScanValue) -> i64 {
    match value {
        ScanValue::U32(n) => i64::from(*n),
        ScanValue::I64(n) => *n,
        other => unreachable!(
            "validate_aggregate only allows U32/I64 fields for Sum/Avg/Min/Max, got {other:?}"
        ),
    }
}

/// The actual observed `ScanValue` with the largest (`want_max`) or
/// smallest numeric value in `rows` for `field` — a passthrough of a
/// real stored value, never promoted or converted, unlike `Sum`/`Avg`.
/// `rows` is empty only for the implicit whole-table bucket with no
/// matching row (`reduce`'s own doc comment): there is no real value to
/// pass through, so this falls back to a `schema`-typed zero
/// (`ScanValue::U32(0)`/`ScanValue::I64(0)` matching `field`'s declared
/// kind) rather than panicking on an empty iterator.
fn reduce_extreme(
    field: FieldRef,
    rows: &[Vec<(FieldRef, ScanValue)>],
    want_max: bool,
    schema: &DomainSchema,
) -> ScanValue {
    let mut values = field_values(field, rows);
    let mut best = match values.next() {
        Some(first) => first,
        None => {
            let kind = schema
                .fields
                .iter()
                .find(|f| f.tag == field)
                .map(|f| f.value_kind)
                .expect("validate_aggregate already confirmed this field exists in schema");
            return match kind {
                protocol::ValueKind::U32 => ScanValue::U32(0),
                _ => ScanValue::I64(0),
            };
        }
    };
    for v in values {
        let replace = if want_max {
            numeric_i64(&v) > numeric_i64(&best)
        } else {
            numeric_i64(&v) < numeric_i64(&best)
        };
        if replace {
            best = v;
        }
    }
    best
}

/// Compatibility rule 3's "nearest older shape" (`protocol.rs`): a
/// response carrying a variant introduced after the connection's
/// negotiated version is rewritten before it is sent. Three cases exist.
/// Two remap an error code — `ErrorCode::Journal` (version 4, ADR-0025)
/// and `ErrorCode::Conflict` (version 7, ADR-0033), both inside
/// `TransactionFailed`, each seen as `Unsupported` by a connection below
/// the version that introduced it. The third rewrites *content* —
/// `ENT4-FR-003` (version 11, ADR-0041): `ScanValue::StrList` is the
/// first appended value variant that reaches an ungated response
/// (`GetById`/`Query`/`DescribeSchema` are protocol-1/8/1 requests a
/// silent client can send), so below 11 every `StrList` pair is stripped
/// from `Record` and from each `Rows` row, and every `StrList`
/// descriptor from `Schema` — the field did not exist for that client at
/// `FR-042` and does not now. `ScanValues`/`Groups` never carry one (the
/// adapter refuses `ScanField` on such a field; `validate_aggregate`
/// refuses it in `group_by`/aggregates), pinned by a debug assertion.
/// The session shapes (version 3) and `Request::Query`/`Response::Rows`
/// themselves (version 8) never need this: neither can arise on a
/// connection that could not send the gated request that produces it.
fn downgrade_for_version(resp: Response, negotiated: u32) -> Response {
    fn is_str_list(pair: &(FieldRef, ScanValue)) -> bool {
        matches!(pair.1, ScanValue::StrList(_))
    }
    match resp {
        Response::Record { id, fields } if negotiated < 11 => Response::Record {
            id,
            fields: fields.into_iter().filter(|p| !is_str_list(p)).collect(),
        },
        Response::Rows { rows } if negotiated < 11 => Response::Rows {
            rows: rows
                .into_iter()
                .map(|(id, fields)| (id, fields.into_iter().filter(|p| !is_str_list(p)).collect()))
                .collect(),
        },
        Response::Schema(mut schema) if negotiated < 11 => {
            schema
                .fields
                .retain(|f| f.value_kind != protocol::ValueKind::StrList);
            Response::Schema(schema)
        }
        Response::ScanValues { ref values } => {
            debug_assert!(
                !values.iter().any(|v| matches!(v, ScanValue::StrList(_))),
                "a StrList field is never scannable (ENT4-FR-003)"
            );
            resp
        }
        Response::Groups { ref groups } => {
            debug_assert!(
                !groups.iter().any(|g| g.key.iter().any(is_str_list)
                    || g.values.iter().any(|v| matches!(v, ScanValue::StrList(_)))),
                "a StrList field is never groupable or aggregatable (ENT4-FR-003)"
            );
            resp
        }
        Response::TransactionFailed {
            index,
            code: ErrorCode::Journal,
            ..
        } if negotiated < 4 => Response::TransactionFailed {
            index,
            code: ErrorCode::Unsupported,
            message: error_message(ErrorCode::Unsupported).to_string(),
        },
        Response::TransactionFailed {
            index,
            code: ErrorCode::Conflict,
            ..
        } if negotiated < 7 => Response::TransactionFailed {
            index,
            code: ErrorCode::Unsupported,
            message: error_message(ErrorCode::Unsupported).to_string(),
        },
        other => other,
    }
}

fn err_response(code: ErrorCode) -> Response {
    Response::Err {
        code,
        message: error_message(code).to_string(),
    }
}

/// `ACC-FR-001`: a dispatched request's outcome *shape*, for the access
/// log — exhaustive over every `Response` variant, never its content.
/// `NotFound`/`NoParent` are `Ok` (a normal outcome, per this crate's own
/// convention); `Err`/`TransactionFailed` are `Err(code)`, the code alone.
fn outcome_of(resp: &Response) -> access::Outcome {
    match resp {
        Response::Err { code, .. } | Response::TransactionFailed { code, .. } => {
            access::Outcome::Err(*code)
        }
        Response::Record { .. }
        | Response::RecordList { .. }
        | Response::ScanValues { .. }
        | Response::Id { .. }
        | Response::Schema(_)
        | Response::NotFound
        | Response::NoParent
        | Response::Ok
        | Response::Hello { .. }
        | Response::Staged { .. }
        | Response::Rows { .. }
        | Response::Groups { .. }
        | Response::RelationKinds { .. } => access::Outcome::Ok,
    }
}

/// The two static permission classes a configured token can grant — see
/// `docs/design/SERVER-AUTH-DESIGN.md`, ADR-0012. Deliberately coarse:
/// `ReadOnly` is blocked only from [`Request::UpdateField`] and
/// [`Request::Transaction`] (`TXN-FR-004` extends `AUTH-FR-003`'s rule to
/// the latter); both classes can do everything else, including
/// `DescribeSchema` (`AUTH-FR-003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    ReadOnly,
    ReadWrite,
}

/// Which tokens (if any) this server instance accepts, and what
/// [`TokenClass`] each grants (`AUTH-FR-005`). Built once at server
/// startup and shared (`Arc`) across every connection thread [`serve`]
/// spawns.
///
/// Tokens are never logged or echoed back on any path, including error
/// messages (`AUTH-FR-005`).
#[derive(Default)]
pub struct ServeOptions {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
    /// `AUD-FR-003` (ADR-0029): where admission, authentication, and
    /// authorization decisions are recorded; `None` is [`audit::NoAudit`].
    audit: Option<Arc<dyn audit::AuditSink>>,
    /// `CLS-FR-003` (ADR-0028): a presented leaf certificate's exact DER
    /// bytes mapped to the class it grants. Matched by `==` on byte
    /// slices — no parsing, no constant-time requirement (certificates
    /// are public material).
    certificate_classes: Vec<(Vec<u8>, TokenClass)>,
    /// `RL-FR-002`/`RL-FR-003` (ADR-0030): the opt-in per-peer failed-
    /// `Authenticate` budget; `None` is no budget. Shared across every
    /// connection thread through the `Arc`, same lifecycle as `audit`.
    rate_limit: Option<Arc<FailureTable>>,
    /// `ACC-FR-003` (ADR-0031): where per-request access events are
    /// recorded, independent of `audit`; `None` is [`access::NoAccessLog`].
    access_log: Option<Arc<dyn access::AccessSink>>,
    /// `SRV-FR-001` (ADR-0032): native TLS, folded in from the former
    /// second `serve` parameter — `None` is plaintext, exactly [`serve`]'s
    /// behavior before this field existed (`TLS-FR-008`).
    tls: Option<TlsConfig>,
}

impl std::fmt::Debug for ServeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_only_certs = self
            .certificate_classes
            .iter()
            .filter(|(_, class)| *class == TokenClass::ReadOnly)
            .count();
        let read_write_certs = self.certificate_classes.len() - read_only_certs;
        f.debug_struct("ServeOptions")
            .field("read_only_token", &self.read_only_token)
            .field("read_write_token", &self.read_write_token)
            // `CLS-FR-006`: counts only, never a configured leaf's bytes.
            .field("read_only_certificates", &read_only_certs)
            .field("read_write_certificates", &read_write_certs)
            .field(
                "audit",
                &if self.audit.is_some() {
                    "configured"
                } else {
                    "none"
                },
            )
            // `RL-FR-002`: the budget's numbers, never the tracked peers.
            .field("rate_limit", &self.rate_limit())
            .field(
                "access_log",
                &if self.access_log.is_some() {
                    "configured"
                } else {
                    "none"
                },
            )
            .field(
                "tls",
                &if self.tls.is_some() {
                    "configured"
                } else {
                    "none"
                },
            )
            .finish()
    }
}

static NO_AUDIT: audit::NoAudit = audit::NoAudit;
static NO_ACCESS_LOG: access::NoAccessLog = access::NoAccessLog;

impl ServeOptions {
    /// Build directly from already-known tokens. Use this from tests and
    /// from any caller that already has its tokens from its own config
    /// source — see [`ServeOptions::from_env`]'s own doc comment for why
    /// tests specifically must not use real environment variables.
    pub fn new(read_only_token: Option<String>, read_write_token: Option<String>) -> Self {
        Self {
            read_only_token,
            read_write_token,
            audit: None,
            certificate_classes: Vec::new(),
            rate_limit: None,
            access_log: None,
            tls: None,
        }
    }

    /// `CLS-FR-003` (ADR-0028): a client presenting a certificate whose
    /// leaf's DER bytes exactly equal `leaf_der` starts the connection at
    /// `class`, with no `Authenticate` needed (`CLS-FR-004`) — see
    /// `handle_connection`'s TLS arm. Repeatable; builds the map one
    /// certificate at a time. Only takes effect when [`with_tls`][Self::with_tls]
    /// configures client auth (`SERVER_TLS_CLIENT_CA_PATH`) — this crate
    /// does not refuse the combination on its own; see `dog_server`'s
    /// startup check (`CLS-FR-005`).
    pub fn with_certificate_class(mut self, leaf_der: Vec<u8>, class: TokenClass) -> Self {
        self.certificate_classes.push((leaf_der, class));
        self
    }

    /// [`ServeOptions::with_certificate_class`] for every `CERTIFICATE`
    /// block in a PEM file — a leaf per class-holding certificate,
    /// classed identically (`CLS-FR-003`).
    pub fn with_certificate_class_pem_file(
        mut self,
        path: impl AsRef<Path>,
        class: TokenClass,
    ) -> Result<Self, TlsConfigError> {
        let pem_text = std::fs::read_to_string(path).map_err(TlsConfigError::Io)?;
        let leaves = pem::decode_blocks(&pem_text).map_err(TlsConfigError::Pem)?;
        for leaf_der in leaves {
            self.certificate_classes.push((leaf_der, class));
        }
        Ok(self)
    }

    /// `CLS-FR-003`: the class a presented leaf's DER bytes match, by
    /// exact byte equality against every configured certificate — `None`
    /// if `leaf_der` matches none of them.
    pub(crate) fn class_for_certificate(&self, leaf_der: &[u8]) -> Option<TokenClass> {
        self.certificate_classes
            .iter()
            .find(|(configured, _)| configured.as_slice() == leaf_der)
            .map(|(_, class)| *class)
    }

    /// `AUD-FR-003` (ADR-0029): record every admission, authentication,
    /// and authorization decision on `sink` — see [`audit`]. Off unless
    /// called; the sink is shared by every connection thread and is
    /// called after each decision, before the response, outside every
    /// lock. A sink that fails never fails a connection (`AUD-FR-006`).
    pub fn with_audit(mut self, sink: Arc<dyn audit::AuditSink>) -> Self {
        self.audit = Some(sink);
        self
    }

    /// The configured sink, or [`audit::NoAudit`].
    pub fn audit(&self) -> &dyn audit::AuditSink {
        match &self.audit {
            Some(sink) => sink.as_ref(),
            None => &NO_AUDIT,
        }
    }

    /// Build from `SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`
    /// at process startup (`AUTH-FR-005`). Reserved for a real server
    /// binary's own one-time startup — `cargo test` runs many tests in
    /// parallel within one process, so reading real process-wide
    /// environment variables from a test would race with every other test
    /// doing the same; tests use [`ServeOptions::new`] instead.
    pub fn from_env() -> Self {
        Self {
            read_only_token: std::env::var("SERVER_AUTH_READ_ONLY_TOKEN").ok(),
            read_write_token: std::env::var("SERVER_AUTH_READ_WRITE_TOKEN").ok(),
            audit: None,
            certificate_classes: Vec::new(),
            rate_limit: None,
            access_log: None,
            tls: None,
        }
    }

    /// `SRV-FR-001`/`SRV-FR-003` (ADR-0032): native TLS — the former
    /// second `serve` parameter, now one more opt-in field. Kept a
    /// separate, still-fallible construction step deliberately
    /// (`TlsConfig::new`/`from_env` can fail; nothing about `ServeOptions`
    /// itself can) — a caller builds a `TlsConfig` and handles its
    /// `Result` first, then folds it in here, exactly the two-step
    /// pattern every other conditionally-configured piece already uses
    /// (`with_audit`, `with_rate_limit`, `with_access_log`).
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// The configured `TlsConfig`, or `None` for plaintext.
    pub fn tls(&self) -> Option<&TlsConfig> {
        self.tls.as_ref()
    }

    /// No tokens *and* no certificate classes configured — `AUTH-FR-007`:
    /// every connection behaves exactly as it did before this feature
    /// existed, and `Authenticate` becomes a no-op success. Since
    /// `CLS-FR-003` (ADR-0028) a certificates-only deployment (classes,
    /// no tokens) is also "configured": an admitted certificate not in
    /// the map starts unauthenticated rather than falling back to
    /// `ReadWrite` — the safe direction, see `SERVER-MTLS-CLASS-DESIGN.md`.
    pub fn is_configured(&self) -> bool {
        self.read_only_token.is_some()
            || self.read_write_token.is_some()
            || !self.certificate_classes.is_empty()
    }

    /// Check `token` against every configured token in constant time
    /// (`AUTH-FR-006`, via the `subtle` crate rather than a hand-rolled
    /// comparison — see this crate's `Cargo.toml` for why). Both slots are
    /// always checked, never short-circuited on the first match, so
    /// neither which slot (if either) matched nor how many slots are
    /// configured is observable from timing.
    fn check(&self, token: &str) -> Option<TokenClass> {
        use subtle::ConstantTimeEq;
        let mut result: Option<TokenClass> = None;
        if let Some(read_write) = &self.read_write_token {
            if bool::from(read_write.as_bytes().ct_eq(token.as_bytes())) {
                result = Some(TokenClass::ReadWrite);
            }
        }
        if let Some(read_only) = &self.read_only_token {
            if bool::from(read_only.as_bytes().ct_eq(token.as_bytes())) {
                result = Some(TokenClass::ReadOnly);
            }
        }
        result
    }

    /// `RL-FR-002` (ADR-0030): opt-in — count failed `Authenticate`s per
    /// peer IP over `limit.window`; once a peer is at or over
    /// `limit.failures` in its current window, every further
    /// `Authenticate` from that address is refused before any comparison,
    /// audited as `Throttled` — see `handle_connection`. Off unless
    /// called; bounded by `MAX_TRACKED_PEERS` regardless of how many
    /// addresses fail (`RL-FR-003`).
    pub fn with_rate_limit(mut self, limit: RateLimit) -> Self {
        self.rate_limit = Some(Arc::new(FailureTable::new(limit)));
        self
    }

    /// The configured budget, if any.
    pub fn rate_limit(&self) -> Option<RateLimit> {
        self.rate_limit.as_ref().map(|table| table.limit)
    }

    /// `RL-FR-002`: whether `peer` is currently over its configured
    /// budget — `false` with no budget configured or no peer address (a
    /// `peer_addr` failure never throttles; it still locks out per
    /// connection).
    fn is_throttled(&self, peer: Option<IpAddr>) -> bool {
        match (&self.rate_limit, peer) {
            (Some(table), Some(peer)) => table.is_throttled(peer),
            _ => false,
        }
    }

    /// `RL-FR-002`: record one failed `Authenticate` from `peer` against
    /// the configured budget — a no-op with no budget configured or no
    /// peer address. Returns whether `peer` is now over budget.
    fn note_failure(&self, peer: Option<IpAddr>) -> bool {
        match (&self.rate_limit, peer) {
            (Some(table), Some(peer)) => table.note_failure(peer),
            _ => false,
        }
    }

    /// `ACC-FR-003` (ADR-0031): record one [`access::AccessEvent`] per
    /// dispatched request on `sink` — see [`access`]. Off unless called,
    /// independent of `with_audit`: an operator's choice to turn on one
    /// never implies the other's cost. Called after the response is
    /// decided, outside every lock, in `handle_connection`.
    pub fn with_access_log(mut self, sink: Arc<dyn access::AccessSink>) -> Self {
        self.access_log = Some(sink);
        self
    }

    /// The configured sink, or [`access::NoAccessLog`].
    pub fn access_log(&self) -> &dyn access::AccessSink {
        match &self.access_log {
            Some(sink) => sink.as_ref(),
            None => &NO_ACCESS_LOG,
        }
    }
}

/// `RL-FR-001` (ADR-0030): a connection's fifth failed `Authenticate` is
/// answered `Unauthenticated` as any wrong token is, then the server
/// records `LockedOut` and closes the connection. On by default and not
/// configurable — the `MAX_STAGED_OPS` precedent: a knob nobody has asked
/// for yet, not one designed in speculatively.
pub const MAX_AUTH_FAILURES: u32 = 5;

/// `RL-FR-003` (ADR-0030): the per-peer failure table never grows past
/// this many tracked addresses — bounded memory under an address flood.
/// On insert, expired entries are dropped first, then the oldest window
/// start if the table is still full: the budget degrades toward "no
/// budget" under a flood, never toward "no service."
pub const MAX_TRACKED_PEERS: usize = 4096;

/// `RL-FR-002` (ADR-0030): an opt-in per-peer failed-`Authenticate`
/// budget — at most `failures` failures per `window`, per peer IP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub failures: u32,
    pub window: Duration,
}

/// `RateLimit::parse` failed — `SERVER_AUTH_RATE_LIMIT` was set but not
/// `"<failures>/<seconds>"`, or one half was zero.
#[derive(Debug)]
pub struct RateLimitParseError(String);

impl std::fmt::Display for RateLimitParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid rate limit {:?}: expected \"<failures>/<seconds>\", both nonzero",
            self.0
        )
    }
}

impl std::error::Error for RateLimitParseError {}

impl RateLimit {
    /// `RL-FR-006`: parse `"<failures>/<seconds>"` (e.g. `"10/60"`) —
    /// both halves nonzero integers, or an error naming the whole input.
    pub fn parse(s: &str) -> Result<Self, RateLimitParseError> {
        let invalid = || RateLimitParseError(s.to_string());
        let (failures, seconds) = s.split_once('/').ok_or_else(invalid)?;
        let failures: u32 = failures.parse().map_err(|_| invalid())?;
        let seconds: u64 = seconds.parse().map_err(|_| invalid())?;
        if failures == 0 || seconds == 0 {
            return Err(invalid());
        }
        Ok(Self {
            failures,
            window: Duration::from_secs(seconds),
        })
    }
}

/// One peer's current window: when it started (monotonic — never
/// wall-clock, so a clock step cannot shorten or lengthen it) and how
/// many failures have landed in it.
#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    failures: u32,
}

/// `RL-FR-002`/`RL-FR-003`: the shared per-peer failure table backing
/// [`ServeOptions::with_rate_limit`]. One mutex, touched only on the
/// `Authenticate` path of a server with a budget configured, for one
/// lookup or insert (`RL-FR-007`).
#[derive(Debug)]
struct FailureTable {
    limit: RateLimit,
    peers: Mutex<HashMap<IpAddr, Window>>,
}

impl FailureTable {
    fn new(limit: RateLimit) -> Self {
        Self {
            limit,
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// A poisoned mutex fails open — "not throttled" — trading a
    /// vanishingly unlikely availability gap (another thread panicked
    /// mid-update) for never wedging every connection's `Authenticate`
    /// path; the per-connection lockout still holds regardless.
    fn is_throttled(&self, peer: IpAddr) -> bool {
        let Ok(peers) = self.peers.lock() else {
            return false;
        };
        match peers.get(&peer) {
            Some(window) => {
                Instant::now().duration_since(window.started) < self.limit.window
                    && window.failures >= self.limit.failures
            }
            None => false,
        }
    }

    /// Record one failure for `peer`: a fresh window if none is tracked
    /// or the current one expired, otherwise one more failure in it. If
    /// tracking a new peer would exceed `MAX_TRACKED_PEERS`, expired
    /// entries are purged first, then the oldest window start is evicted
    /// if the table is still full (`RL-FR-003`). Returns whether `peer`
    /// is now over budget.
    fn note_failure(&self, peer: IpAddr) -> bool {
        let Ok(mut peers) = self.peers.lock() else {
            return false;
        };
        let now = Instant::now();
        if let Some(window) = peers.get_mut(&peer) {
            if now.duration_since(window.started) >= self.limit.window {
                *window = Window {
                    started: now,
                    failures: 1,
                };
            } else {
                window.failures += 1;
            }
        } else {
            if peers.len() >= MAX_TRACKED_PEERS {
                let window = self.limit.window;
                peers.retain(|_, w| now.duration_since(w.started) < window);
                if peers.len() >= MAX_TRACKED_PEERS {
                    if let Some(oldest) = peers
                        .iter()
                        .min_by_key(|(_, w)| w.started)
                        .map(|(addr, _)| *addr)
                    {
                        peers.remove(&oldest);
                    }
                }
            }
            peers.insert(
                peer,
                Window {
                    started: now,
                    failures: 1,
                },
            );
        }
        peers
            .get(&peer)
            .is_some_and(|w| w.failures >= self.limit.failures)
    }
}

/// Native TLS configuration for [`serve`] (ADR-0014,
/// `docs/design/SERVER-TLS-DESIGN.md`). Wraps a `rusty_tls::TlsAcceptor`
/// — this owner's own ecosystem-wide `rustls` wrapper, not a direct
/// `rustls` dependency, see that design's own "Ecosystem check" for why —
/// built once at server startup and shared across every connection
/// thread [`serve`] spawns, the same lifecycle [`ServeOptions`] already
/// uses for its own configured tokens. `serve` with `tls: None` behaves
/// exactly as it did before this feature existed (`TLS-FR-008`) — this is
/// purely opt-in.
pub struct TlsConfig {
    acceptor: rusty_tls::TlsAcceptor,
    /// `MTLS-FR-001` (ADR-0023): whether `acceptor` was built with client
    /// CA roots, so every connection must present a certificate chaining
    /// to one of them or fail the handshake.
    requires_client_certificate: bool,
}

/// Everything that can go wrong building a [`TlsConfig`].
#[derive(Debug)]
pub enum TlsConfigError {
    /// Reading a certificate/key file failed (missing file, permission
    /// denied, ...).
    Io(io::Error),
    /// A certificate/key file's contents weren't valid PEM.
    Pem(pem::PemError),
    /// `rusty_tls` rejected the decoded certificate chain or private key.
    Tls(rusty_tls::Error),
}

impl std::fmt::Display for TlsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsConfigError::Io(e) => write!(f, "reading TLS certificate/key file: {e}"),
            TlsConfigError::Pem(e) => write!(f, "parsing TLS certificate/key PEM: {e}"),
            TlsConfigError::Tls(e) => write!(f, "building TLS config: {e}"),
        }
    }
}

impl std::error::Error for TlsConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsConfigError::Io(e) => Some(e),
            TlsConfigError::Pem(e) => Some(e),
            TlsConfigError::Tls(e) => Some(e),
        }
    }
}

impl TlsConfig {
    /// Build directly from DER-encoded certificate chain + private key —
    /// `cert_chain_der` is the leaf certificate followed by any
    /// intermediates, each DER-encoded, leaf first; `private_key_der` is
    /// the leaf's private key, DER-encoded (PKCS#8, PKCS#1, or SEC1,
    /// auto-detected — see `rusty_tls::TlsAcceptor::new`'s own doc
    /// comment). Use this directly when the caller already has DER bytes;
    /// see [`TlsConfig::from_pem_files`] for the common PEM-file case.
    pub fn new(
        cert_chain_der: Vec<Vec<u8>>,
        private_key_der: Vec<u8>,
    ) -> Result<Self, TlsConfigError> {
        let acceptor = rusty_tls::TlsAcceptor::new(cert_chain_der, private_key_der)
            .map_err(TlsConfigError::Tls)?;
        Ok(Self {
            acceptor,
            requires_client_certificate: false,
        })
    }

    /// [`TlsConfig::new`] plus the DER-encoded CA certificates a client
    /// certificate must chain to (`MTLS-FR-001`, ADR-0023,
    /// `docs/design/SERVER-MTLS-DESIGN.md`) — mutual TLS as an *admission*
    /// gate: a connection that presents no certificate, or one that does
    /// not chain to any of `client_ca_roots_der`, fails the TLS handshake
    /// and is dropped before any framed message (`Authenticate` included)
    /// is read, on the same `TLS-FR-003` path as any other handshake
    /// failure. Admission is all the certificate decides: an admitted
    /// connection still starts exactly where [`ServeOptions`] says it does
    /// (`MTLS-FR-002`), and nothing in this crate ever reads the admitted
    /// certificate's contents (`MTLS-FR-005`). An empty root set is
    /// `TlsConfigError::Tls` (`rusty_tls::Error::InvalidClientCaRoots`) —
    /// a server never starts with mTLS silently off. `handle_connection`
    /// has no mTLS branch: the acceptor carries the policy.
    pub fn new_with_client_auth(
        cert_chain_der: Vec<Vec<u8>>,
        private_key_der: Vec<u8>,
        client_ca_roots_der: Vec<Vec<u8>>,
    ) -> Result<Self, TlsConfigError> {
        let acceptor = rusty_tls::TlsAcceptor::new_with_client_auth(
            cert_chain_der,
            private_key_der,
            client_ca_roots_der,
        )
        .map_err(TlsConfigError::Tls)?;
        Ok(Self {
            acceptor,
            requires_client_certificate: true,
        })
    }

    /// Whether every connection must present a client certificate —
    /// `true` only for a config built by [`TlsConfig::new_with_client_auth`]
    /// or its PEM/environment equivalents.
    pub fn requires_client_certificate(&self) -> bool {
        self.requires_client_certificate
    }

    /// Build from PEM-encoded certificate chain + private key files
    /// (`TLS-FR-006`) — the common operator-facing format (`openssl`,
    /// `mkcert`, a CA). `rusty_tls::TlsAcceptor::new` itself takes DER
    /// bytes directly (`rusty_tls` deliberately keeps its own public seam
    /// narrow and doesn't re-expose a PEM parser — see
    /// `docs/design/SERVER-TLS-DESIGN.md`'s "Ecosystem check"), so this
    /// decodes PEM into DER first (see the `pem` module — a small,
    /// hand-written decoder, not a new dependency). `cert_chain_path` may
    /// contain more than one `-----BEGIN CERTIFICATE-----` block (the
    /// leaf followed by any intermediates, leaf first); `private_key_path`
    /// must contain exactly one block.
    pub fn from_pem_files(
        cert_chain_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> Result<Self, TlsConfigError> {
        let cert_chain_pem =
            std::fs::read_to_string(cert_chain_path).map_err(TlsConfigError::Io)?;
        let private_key_pem =
            std::fs::read_to_string(private_key_path).map_err(TlsConfigError::Io)?;
        let cert_chain_der = pem::decode_blocks(&cert_chain_pem).map_err(TlsConfigError::Pem)?;
        let private_key_blocks =
            pem::decode_blocks(&private_key_pem).map_err(TlsConfigError::Pem)?;
        let [private_key_der] = <[Vec<u8>; 1]>::try_from(private_key_blocks)
            .map_err(|_| TlsConfigError::Pem(pem::PemError::UnterminatedBlock))?;
        Self::new(cert_chain_der, private_key_der)
    }

    /// [`TlsConfig::from_pem_files`] plus a PEM file holding one or more
    /// `CERTIFICATE` blocks — the client CA roots for
    /// [`TlsConfig::new_with_client_auth`] (`MTLS-FR-004`).
    pub fn from_pem_files_with_client_ca(
        cert_chain_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
        client_ca_path: impl AsRef<Path>,
    ) -> Result<Self, TlsConfigError> {
        let (cert_chain_der, private_key_der) =
            Self::read_pem_chain_and_key(cert_chain_path, private_key_path)?;
        let client_ca_pem = std::fs::read_to_string(client_ca_path).map_err(TlsConfigError::Io)?;
        let client_ca_roots_der =
            pem::decode_blocks(&client_ca_pem).map_err(TlsConfigError::Pem)?;
        Self::new_with_client_auth(cert_chain_der, private_key_der, client_ca_roots_der)
    }

    /// The shared PEM→DER step of both `from_pem_files*` constructors.
    fn read_pem_chain_and_key(
        cert_chain_path: impl AsRef<Path>,
        private_key_path: impl AsRef<Path>,
    ) -> Result<(Vec<Vec<u8>>, Vec<u8>), TlsConfigError> {
        let cert_chain_pem =
            std::fs::read_to_string(cert_chain_path).map_err(TlsConfigError::Io)?;
        let private_key_pem =
            std::fs::read_to_string(private_key_path).map_err(TlsConfigError::Io)?;
        let cert_chain_der = pem::decode_blocks(&cert_chain_pem).map_err(TlsConfigError::Pem)?;
        let private_key_blocks =
            pem::decode_blocks(&private_key_pem).map_err(TlsConfigError::Pem)?;
        let [private_key_der] = <[Vec<u8>; 1]>::try_from(private_key_blocks)
            .map_err(|_| TlsConfigError::Pem(pem::PemError::UnterminatedBlock))?;
        Ok((cert_chain_der, private_key_der))
    }

    /// Build from `SERVER_TLS_CERT_CHAIN_PATH`/`SERVER_TLS_PRIVATE_KEY_PATH`
    /// at process startup, mirroring [`ServeOptions::from_env`]'s own
    /// pattern — `None` (rather than an error) if either variable is
    /// unset, so a caller can treat "TLS not configured" and "TLS
    /// misconfigured" differently: the former is `serve(..., None)`'s
    /// ordinary opt-out, the latter is a real startup error a caller
    /// should surface (`Some(Err(..))`). Since v0.13.0 (`MTLS-FR-004`)
    /// an optional third variable, `SERVER_TLS_CLIENT_CA_PATH`, selects
    /// [`TlsConfig::from_pem_files_with_client_ca`]; set while the chain/key
    /// pair is not, it is `Some(Err(TlsConfigError::Io(NotFound, ..)))`
    /// naming the missing variables — never a silent plaintext or
    /// no-mTLS server. (The chain/key pair itself is all-or-nothing as it
    /// always was: one of the two set alone is still `None`.)
    pub fn from_env() -> Option<Result<Self, TlsConfigError>> {
        Self::from_env_values(
            std::env::var("SERVER_TLS_CERT_CHAIN_PATH").ok(),
            std::env::var("SERVER_TLS_PRIVATE_KEY_PATH").ok(),
            std::env::var("SERVER_TLS_CLIENT_CA_PATH").ok(),
        )
    }

    /// [`TlsConfig::from_env`]'s decision table, factored so a test can
    /// drive it without touching the real process environment (the same
    /// constraint [`ServeOptions::from_env`]'s docs impose).
    fn from_env_values(
        cert_chain_path: Option<String>,
        private_key_path: Option<String>,
        client_ca_path: Option<String>,
    ) -> Option<Result<Self, TlsConfigError>> {
        match (cert_chain_path, private_key_path, client_ca_path) {
            (Some(chain), Some(key), None) => Some(Self::from_pem_files(chain, key)),
            (Some(chain), Some(key), Some(ca)) => {
                Some(Self::from_pem_files_with_client_ca(chain, key, ca))
            }
            (_, _, Some(_)) => Some(Err(TlsConfigError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "SERVER_TLS_CLIENT_CA_PATH is set but SERVER_TLS_CERT_CHAIN_PATH and \
                 SERVER_TLS_PRIVATE_KEY_PATH are not both set",
            )))),
            _ => None,
        }
    }
}

/// Which raw stream a connection is speaking — a plain, unencrypted
/// `TcpStream`, or one wrapped in TLS (ADR-0014,
/// `docs/design/SERVER-TLS-DESIGN.md`). `dispatch`/`ConnectionStore`
/// never see this distinction; it's resolved once per connection in
/// [`handle_connection`]. `framing::read_message`/`write_message` work
/// unchanged either way, since both are already generic over
/// `Read`/`Write`.
///
/// Read and write are split into two owned halves the same way the plain
/// path already did (`TcpStream::try_clone`, two independent socket
/// handles) — but a `rusty_tls::TlsServerStream` can't be split that way:
/// its `rustls::ServerConnection` state is shared, single-owner data that
/// both a read and a write need to reach through the same object.
/// `Rc<RefCell<_>>` gives the TLS path the same two-owned-halves shape.
/// Single-threaded is enough — each connection is served by exactly one
/// OS thread (see [`serve`]), so a `RefCell`'s runtime borrow check is
/// sufficient; no `Mutex`/`Arc` needed for this, unlike `ServeOptions`/
/// `TlsConfig` themselves, which really are shared *across* connection
/// threads.
enum ReadHalf {
    Plain(TcpStream),
    Tls(Rc<RefCell<rusty_tls::TlsServerStream<TcpStream>>>),
}

enum WriteHalf {
    Plain(TcpStream),
    Tls(Rc<RefCell<rusty_tls::TlsServerStream<TcpStream>>>),
}

impl Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            ReadHalf::Plain(s) => s.read(buf),
            ReadHalf::Tls(s) => s.borrow_mut().read(buf),
        }
    }
}

impl Write for WriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            WriteHalf::Plain(s) => s.write(buf),
            WriteHalf::Tls(s) => s.borrow_mut().write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            WriteHalf::Plain(s) => s.flush(),
            WriteHalf::Tls(s) => s.borrow_mut().flush(),
        }
    }
}

/// Translate one [`Request`] into a [`Response`] against `store` — the
/// entire request-handling logic, independent of framing or sockets, kept
/// separate so it can be tested (see this module's tests) without a real
/// TCP connection.
pub fn dispatch<S: ConnectionStore + ?Sized>(store: &S, req: Request) -> Response {
    match req {
        Request::GetById { id } => match store.get(id) {
            Some(fields) => Response::Record { id, fields },
            None => Response::NotFound,
        },
        Request::FilterEq { field, value } => match store.filter_eq(field, &value) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        Request::ScanField { field } => match store.scan_field(field) {
            Ok(values) => Response::ScanValues { values },
            Err(code) => err_response(code),
        },
        // `SQL-FR-004`/`SQL-FR-007` (ADR-0034): validated against the
        // schema alone before `scan_all` ever runs; a read, gated like
        // `GetById`/`FilterEq`/`ScanField` above, never overlaid or
        // read-set-tracked by a session (`SQL-FR-009`) — the intercepts
        // for those live in `handle_connection`, keyed on `GetById`
        // alone, so `Query` reaching `dispatch` at all already means
        // neither applies.
        Request::Query {
            select,
            filter,
            limit,
        } => match validate_query(&store.describe(), &select, &filter) {
            Ok(()) => Response::Rows {
                rows: evaluate_query(store.scan_all(), &select, &filter, limit),
            },
            Err(code) => err_response(code),
        },
        // `AGG-FR-006`/`AGG-FR-009` (ADR-0035): the same validate-then-scan
        // shape `Query` above uses, and the same "never overlaid, never
        // read-set-tracked" posture — `Aggregate` never reaches the
        // `GetById`-keyed session intercepts in `handle_connection` either.
        Request::Aggregate {
            group_by,
            filter,
            aggregates,
            limit,
        } => {
            let schema = store.describe();
            match validate_aggregate(&schema, &group_by, &filter, &aggregates) {
                Ok(()) => Response::Groups {
                    groups: evaluate_aggregate(
                        store.scan_all(),
                        &group_by,
                        &filter,
                        &aggregates,
                        limit,
                        &schema,
                    ),
                },
                Err(code) => err_response(code),
            }
        }
        Request::UpdateField { id, field, value } => match store.update_field(id, field, value) {
            Ok(true) => Response::Ok,
            Ok(false) => Response::NotFound,
            Err(code) => err_response(code),
        },
        Request::Parent { id } => match store.parent(id) {
            Ok(ParentLookup::Parent(parent_id)) => Response::Id { id: parent_id },
            Ok(ParentLookup::NoParent) => Response::NoParent,
            Ok(ParentLookup::ChildNotFound) => Response::NotFound,
            Err(code) => err_response(code),
        },
        Request::Children { id } => match store.children(id) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        Request::Neighbors { id } => match store.neighbors(id) {
            Ok(records) => Response::RecordList { records },
            Err(code) => err_response(code),
        },
        // `ENT2-FR-004`/`005` (ADR-0039): gated entirely client-side,
        // the same posture `Query`/`Aggregate` above already have — no
        // negotiated-version check here.
        Request::NeighborsByRelation { id, relation } => {
            match store.neighbors_by_relation(id, &relation) {
                Ok(records) => Response::RecordList { records },
                Err(code) => err_response(code),
            }
        }
        Request::ListRelationKinds => Response::RelationKinds {
            kinds: store.list_relation_kinds(),
        },
        Request::DescribeSchema => Response::Schema(store.describe()),
        // `Authenticate` is intercepted directly by `handle_connection`,
        // which has the per-connection state (and `ServeOptions`) this
        // function has no way to reach — a store has no notion of "this
        // connection". Reaching this arm at all means `handle_connection`
        // let an `Authenticate` request fall through, which never happens
        // in the real dispatch loop; kept exhaustive rather than `_ =>`
        // so a future `Request` variant can't silently skip this decision.
        Request::Authenticate { .. } => err_response(ErrorCode::Unsupported),
        // Same story as `Authenticate`: `Hello` is a per-connection
        // negotiation `handle_connection` answers itself (`PROTO-FR-003`),
        // and a store has nothing to say about it.
        Request::Hello { .. } => err_response(ErrorCode::Unsupported),
        // Protocol 3, `SESS-FR-006`: a session is per-connection state
        // `handle_connection` owns; a store has nothing to say about it.
        Request::Begin | Request::BeginWith { .. } | Request::Commit | Request::Rollback => {
            err_response(ErrorCode::Unsupported)
        }
        // A one-shot `Request::Transaction` has no session, so no
        // snapshot-isolation read set to re-check — `ISO-FR-001` is
        // exclusively a session (`BeginWith`) feature.
        Request::Transaction { updates } => match store.apply_transaction(&updates, &[]) {
            Ok(()) => Response::Ok,
            Err((index, code)) => Response::TransactionFailed {
                index,
                code,
                message: error_message(code).to_string(),
            },
        },
    }
}

/// Write `resp` and flush, reporting whether the connection is still
/// usable — folds the write-then-flush-then-check-both boilerplate every
/// response path in [`handle_connection`] needs (there are now several,
/// since auth gating adds early-return response paths that don't go
/// through [`dispatch`]).
fn send_response(writer: &mut BufWriter<WriteHalf>, resp: &Response) -> bool {
    if framing::write_message(writer, resp).is_err() {
        return false;
    }
    writer.flush().is_ok()
}

/// Serve one already-accepted connection until the client disconnects or a
/// framing error occurs. Never panics on a bad client: a malformed or
/// oversized frame ends the connection after (when possible) one
/// [`Response::Err`], never the process — `SERVER-FR-004`.
///
/// # Transport encryption (`TLS-FR-002`/`TLS-FR-003`), ADR-0014
///
/// When `tls` is configured, the raw `stream` is wrapped in a TLS server
/// connection (`rusty_tls::TlsAcceptor::accept`) before anything else
/// happens — `dispatch`/`ConnectionStore` never see this. `accept` itself
/// performs no I/O; the handshake runs lazily, driven by the very first
/// `framing::read_message` call below, so a connection that fails the
/// handshake surfaces there as an ordinary I/O error and ends the
/// connection cleanly — the same "return on the first framing error, no
/// panic" path a malformed plaintext frame already takes, satisfying
/// `TLS-FR-003` with no special-casing needed.
///
/// # Authentication gating (`AUTH-FR-001`/`AUTH-FR-002`/`AUTH-FR-003`/`AUTH-FR-007`)
///
/// `auth` is checked once per connection, not per request, to decide the
/// starting state: if no tokens are configured at all, the connection
/// starts already authenticated at [`TokenClass::ReadWrite`] and every
/// request is allowed exactly as before this feature existed
/// (`AUTH-FR-007`) — `Request::Authenticate` still round-trips
/// successfully in that case, but as a no-op. Otherwise the connection
/// starts unauthenticated: every request except `Authenticate` is
/// rejected with `ErrorCode::Unauthenticated` (including `DescribeSchema`
/// — `AUTH-FR-002`) until a recognized token is presented, after which its
/// [`TokenClass`] gates `Request::UpdateField` and `Request::Transaction`
/// (`AUTH-FR-003`, `TXN-FR-004`). With `tls` also configured,
/// `Authenticate`'s token now travels over the encrypted channel rather
/// than plaintext (`TLS-FR-007`) — the handshake above always completes
/// before this loop ever reads a frame, so there's no ordering hazard.
///
/// # Protocol version negotiation (`PROTO-FR-003`/`PROTO-FR-004`), ADR-0022
///
/// `Request::Hello` is intercepted ahead of even the `Authenticate`
/// intercept — a client learns the server's protocol version before it
/// presents a token, and an unauthenticated connection can say `Hello`
/// and nothing else. Only the first frame may be a `Hello`, and its
/// version must be at least 1: the reply is `Response::Hello` carrying
/// `min(client, PROTOCOL_VERSION)`; otherwise `ErrorCode::Malformed`,
/// with the connection left open. A connection whose first frame is not
/// a `Hello` is served at version 1 with no other change (`PROTO-FR-002`).
///
/// # Transaction sessions (`SESS-FR-002`–`SESS-FR-006`), ADR-0024
///
/// Protocol 3 adds the first per-connection state after `authenticated`:
/// the *negotiated version* (kept at last — `ADR-0022` deferred it until
/// a gated variant existed) and an optional *session*, a buffer of
/// staged `TransactionOp`s. `Begin` opens it; while it is open every
/// `UpdateField` the gates admit is pushed and answered `Staged { index }`
/// — nothing applied, no lock taken, no validation (commit validates);
/// `Commit` hands the buffer to `ConnectionStore::apply_transaction`
/// exactly as a `Request::Transaction` would and closes the session
/// either way; `Rollback` or a disconnect discards it. No lock is ever
/// held between round trips — the only lock a session takes is
/// `apply_transaction`'s own, at `Commit`, for the same interval a
/// `Transaction` of the same batch holds it (`SESS-FR-003`). The three
/// requests sit *after* the auth and `ReadOnly` gates (`Commit` joins
/// `UpdateField`/`Transaction` in the latter) and are `Malformed` on a
/// connection negotiated below 3 — a silent client included — so no
/// version-3 response shape is ever sent on an older connection
/// (compatibility rule 3). Misuse (`NoSession`, `SessionOpen`,
/// `SessionFull`) is a typed error with the connection open.
///
/// # Audit (`AUD-FR-004`–`006`), ADR-0029
///
/// Every decision the gates below take is recorded on
/// [`ServeOptions::audit`] after the decision and before the response —
/// `Admitted` (after an *eager* TLS handshake, so a refused admission is
/// `HandshakeFailed` with the TLS error's text; `classed_by_certificate`
/// records whether `initial_class` came from a matched certificate —
/// `ADR-0029`'s fourth revisit trigger, taken once `ADR-0028` landed),
/// `Authenticated` / `AuthenticationFailed`, `Refused` at the
/// unauthenticated and `ReadOnly` gates, and exactly one `Disconnected`
/// on the way out. No successful request is recorded; no token,
/// certificate, id, or value ever is. With the default
/// [`audit::NoAudit`] every call is a no-op.
///
/// # Stage-time validation (`STV-FR-001`–`003`), `ADR-0024`'s second trigger
///
/// Protocol 6 adds a second `BeginWith` bit, `SESSION_VALIDATE_ON_STAGE`:
/// each `UpdateField` is passed to `ConnectionStore::validate_op` as it
/// is staged and refused — nothing staged — with the code `Commit`
/// would have reported, so a client learns about a bad write at the
/// round trip that sent it rather than by index at `Commit`. Commit
/// still validates the whole batch (the store's rule, not this one's).
/// Below version 6 the bit is unknown and `BeginWith` is `Malformed`.
///
/// # Read-your-writes sessions (`RYW-FR-001`–`005`), ADR-0027
///
/// Protocol 5 adds `BeginWith { flags }`: with `SESSION_READ_YOUR_WRITES`
/// the session also remembers the schema's updatable field tags, and this
/// connection's own `GetById` — and only that read — is served as usual
/// and then passed through [`overlay_staged`] before it is sent. Set
/// reads, plain `Begin` sessions, and every other connection see
/// committed state exactly as before; a connection with no session takes
/// no new branch. Unknown flag bits are `Malformed`; below version 5 the
/// request is `Malformed` (rule 3), like `Begin` below 3.
/// Records `Disconnected` when a connection's `handle_connection` frame
/// returns by any path (`AUD-FR-004`) — created only after `Admitted`.
struct DisconnectAudit<'a> {
    sink: &'a dyn audit::AuditSink,
    peer: Option<std::net::SocketAddr>,
}

impl Drop for DisconnectAudit<'_> {
    fn drop(&mut self) {
        self.sink.record(&audit::AuditEvent::now(
            self.peer,
            audit::AuditKind::Disconnected,
        ));
    }
}

fn handle_connection<S: ConnectionStore + ?Sized>(
    stream: TcpStream,
    store: &S,
    options: &ServeOptions,
) {
    // `SRV-FR-004` (ADR-0032): `tls` was `serve`'s own second parameter;
    // now it is read off the one consolidated `options` value.
    let tls = options.tls();
    // This is a synchronous request/response protocol: each side writes a
    // small frame, then blocks reading the other side's small frame back.
    // Left at its default, Nagle's algorithm delays a small write hoping
    // to coalesce it with more data, which collides with the peer's own
    // delayed-ACK timer — the textbook interaction that turns every
    // round trip into a ~40ms stall. Disabling it is the correct fix for
    // this protocol shape, not just a benchmark convenience: confirmed
    // directly (a concurrent-client integration test went from ~36s to
    // well under a second after this one call).
    let _ = stream.set_nodelay(true);
    // `AUD-FR-001`: the one identifying datum an audit event carries.
    let peer = stream.peer_addr().ok();
    let sink = options.audit();

    let transport = match tls {
        None => audit::Transport::Plain,
        Some(tls) if tls.requires_client_certificate() => audit::Transport::MutualTls,
        Some(_) => audit::Transport::Tls,
    };
    // `CLS-FR-004`: the class a presented, configured certificate grants,
    // if the TLS arm below finds one — `None` on a plain connection or an
    // admitted leaf that matches no configured class.
    let mut certificate_class: Option<TokenClass> = None;
    let (mut reader, mut writer): (BufReader<ReadHalf>, BufWriter<WriteHalf>) = match tls {
        None => {
            let peer_stream = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return,
            };
            (
                BufReader::new(ReadHalf::Plain(stream)),
                BufWriter::new(WriteHalf::Plain(peer_stream)),
            )
        }
        Some(tls) => {
            let mut tls_stream = match tls.acceptor.accept(stream) {
                Ok(s) => s,
                Err(_) => return, // config/setup error building the connection object — drop cleanly, no panic
            };
            // `AUD-FR-005` (and `CLS-FR-002`): complete the handshake before
            // the frame loop, so a refused admission has a typed reason to
            // record. Client-visible behavior is unchanged — the connection
            // ends with no response either way (`TLS-FR-003`).
            if let Err(e) = tls_stream.complete_handshake() {
                sink.record(&audit::AuditEvent::now(
                    peer,
                    audit::AuditKind::HandshakeFailed {
                        reason: e.to_string(),
                    },
                ));
                return;
            }
            // `CLS-FR-003`/`CLS-FR-004`: the class a presented leaf grants,
            // if its exact DER bytes are configured — `None` on a plain
            // acceptor (no client auth) or a leaf not in the map.
            certificate_class = tls_stream
                .peer_certificate_der()
                .and_then(|der| options.class_for_certificate(der));
            let shared = Rc::new(RefCell::new(tls_stream));
            (
                BufReader::new(ReadHalf::Tls(Rc::clone(&shared))),
                BufWriter::new(WriteHalf::Tls(shared)),
            )
        }
    };

    // `CLS-FR-004`: a certificate-classed connection starts at that class
    // with no `Authenticate` needed; a later `Authenticate` with a valid
    // token still replaces it (unchanged below). Otherwise exactly
    // today's rule: unauthenticated if anything is configured, `ReadWrite`
    // if nothing is.
    let mut authenticated: Option<TokenClass> = match (certificate_class, options.is_configured()) {
        (Some(class), _) => Some(class),
        (None, false) => Some(TokenClass::ReadWrite),
        (None, true) => None,
    };
    // `AUD-FR-004`: admitted — and exactly one `Disconnected` when this
    // function returns by any path.
    sink.record(&audit::AuditEvent::now(
        peer,
        audit::AuditKind::Admitted {
            transport,
            initial_class: authenticated,
            classed_by_certificate: certificate_class.is_some(),
        },
    ));
    let _disconnect = DisconnectAudit { sink, peer };

    // `PROTO-FR-004`: only the very first frame may be a `Hello`. Since
    // protocol 3 the negotiated version is kept too (`SESS-FR-006`): the
    // session requests consult it, exactly the moment `ADR-0022` said the
    // state would appear. A silent client is version 1.
    let mut first_frame = true;
    let mut negotiated: u32 = 1;
    // `SESS-FR-002`: the staged writes of an open session, if any.
    let mut session: Option<Vec<TransactionOp>> = None;
    // `RYW-FR-001`: `Some(updatable tags)` while a read-your-writes
    // session is open; cleared with the session.
    let mut read_your_writes: Option<Vec<FieldRef>> = None;
    // `STV-FR-001`: whether the open session validates each write as it
    // is staged; cleared with the session.
    let mut validate_on_stage = false;
    // `ISO-FR-002` (ADR-0033): `Some(read set)` while a snapshot-isolated
    // session is open — every `GetById`'s raw, pre-overlay result is
    // recorded here, keyed by `(id, field)`; cleared with the session.
    let mut snapshot_reads: Option<HashMap<(RecordId, FieldRef), ScanValue>> = None;
    // `RL-FR-001`: this connection's own failed-`Authenticate` count,
    // never reset by a success — the fifth failure locks it out
    // regardless of how many succeeded around it.
    let mut failures: u32 = 0;
    let peer_ip = peer.map(|addr| addr.ip());

    loop {
        let req: Request = match framing::read_message(&mut reader) {
            Ok(req) => req,
            Err(_) => return, // client disconnected, or a framing/decode error — end the connection
        };

        if let Request::Hello { protocol_version } = &req {
            let resp = if !first_frame || *protocol_version == 0 {
                err_response(ErrorCode::Malformed)
            } else {
                negotiated = (*protocol_version).min(PROTOCOL_VERSION);
                Response::Hello {
                    protocol_version: negotiated,
                }
            };
            first_frame = false;
            if !send_response(&mut writer, &resp) {
                return;
            }
            continue;
        }
        first_frame = false;

        if let Request::Authenticate { token } = &req {
            let resp = if !options.is_configured() {
                sink.record(&audit::AuditEvent::now(
                    peer,
                    audit::AuditKind::Authenticated {
                        class: TokenClass::ReadWrite,
                    },
                ));
                Response::Ok
            } else if options.is_throttled(peer_ip) {
                // `RL-FR-002`: over budget — refused before any
                // comparison, and it still counts toward the
                // per-connection lockout below.
                failures += 1;
                sink.record(&audit::AuditEvent::now(
                    peer,
                    audit::AuditKind::Throttled { failures },
                ));
                err_response(ErrorCode::Unauthenticated)
            } else {
                match options.check(token) {
                    Some(class) => {
                        authenticated = Some(class);
                        sink.record(&audit::AuditEvent::now(
                            peer,
                            audit::AuditKind::Authenticated { class },
                        ));
                        Response::Ok
                    }
                    None => {
                        failures += 1;
                        options.note_failure(peer_ip);
                        sink.record(&audit::AuditEvent::now(
                            peer,
                            audit::AuditKind::AuthenticationFailed,
                        ));
                        err_response(ErrorCode::Unauthenticated)
                    }
                }
            };
            if !send_response(&mut writer, &resp) {
                return;
            }
            // `RL-FR-001`: the response above is sent either way; only
            // *when the connection closes* changes.
            if failures >= MAX_AUTH_FAILURES {
                sink.record(&audit::AuditEvent::now(
                    peer,
                    audit::AuditKind::LockedOut { failures },
                ));
                return;
            }
            continue;
        }

        let class = match authenticated {
            Some(class) => class,
            None => {
                sink.record(&audit::AuditEvent::now(
                    peer,
                    audit::AuditKind::Refused {
                        class: None,
                        request: audit::RequestKind::of(&req),
                        code: ErrorCode::Unauthenticated,
                    },
                ));
                if !send_response(&mut writer, &err_response(ErrorCode::Unauthenticated)) {
                    return;
                }
                continue;
            }
        };

        if class == TokenClass::ReadOnly
            && matches!(
                req,
                Request::UpdateField { .. } | Request::Transaction { .. } | Request::Commit
            )
        {
            sink.record(&audit::AuditEvent::now(
                peer,
                audit::AuditKind::Refused {
                    class: Some(class),
                    request: audit::RequestKind::of(&req),
                    code: ErrorCode::Unauthorized,
                },
            ));
            if !send_response(&mut writer, &err_response(ErrorCode::Unauthorized)) {
                return;
            }
            continue;
        }

        // `ACC-FR-004`: everything from here on is a dispatched request —
        // past `Hello`/`Authenticate` (handled above) and the
        // unauthenticated/`ReadOnly` gates (also above, each its own
        // `continue`) — so `request_kind` is captured now, before the
        // match below consumes `req`.
        let request_kind = audit::RequestKind::of(&req);

        // `SESS-FR-002`/`SESS-FR-004`/`SESS-FR-006`: the session intercepts.
        let resp = match req {
            Request::Begin | Request::Commit | Request::Rollback if negotiated < 3 => {
                err_response(ErrorCode::Malformed)
            }
            Request::Begin => {
                if session.is_some() {
                    err_response(ErrorCode::SessionOpen)
                } else {
                    session = Some(Vec::new());
                    read_your_writes = None;
                    validate_on_stage = false;
                    snapshot_reads = None;
                    Response::Ok
                }
            }
            Request::BeginWith { .. } if negotiated < 5 => err_response(ErrorCode::Malformed),
            Request::BeginWith { flags } => {
                // A flag bit is introduced at a version like a variant
                // (`STV-FR-003`): below 6 the validate bit is unknown,
                // below 7 the snapshot-isolation bit is unknown
                // (`ISO-FR-001`).
                let known = SESSION_READ_YOUR_WRITES
                    | if negotiated >= 6 {
                        SESSION_VALIDATE_ON_STAGE
                    } else {
                        0
                    }
                    | if negotiated >= 7 {
                        SESSION_SNAPSHOT_ISOLATION
                    } else {
                        0
                    };
                if flags & !known != 0 {
                    err_response(ErrorCode::Malformed)
                } else if session.is_some() {
                    err_response(ErrorCode::SessionOpen)
                } else {
                    session = Some(Vec::new());
                    read_your_writes = (flags & SESSION_READ_YOUR_WRITES != 0).then(|| {
                        store
                            .describe()
                            .fields
                            .iter()
                            .filter(|f| f.capabilities.update)
                            .map(|f| f.tag)
                            .collect()
                    });
                    validate_on_stage = flags & SESSION_VALIDATE_ON_STAGE != 0;
                    snapshot_reads = (flags & SESSION_SNAPSHOT_ISOLATION != 0).then(HashMap::new);
                    Response::Ok
                }
            }
            Request::Rollback => {
                read_your_writes = None;
                validate_on_stage = false;
                snapshot_reads = None;
                if session.take().is_some() {
                    Response::Ok
                } else {
                    err_response(ErrorCode::NoSession)
                }
            }
            Request::GetById { id }
                if (read_your_writes.is_some() || snapshot_reads.is_some())
                    && session.is_some() =>
            {
                match dispatch(store, Request::GetById { id }) {
                    Response::Record { id, mut fields } => {
                        // `ISO-FR-002`/`ISO-FR-005`: record the raw,
                        // committed values — before any read-your-writes
                        // overlay — into the read set; only a found
                        // record is tracked at all (`dispatch` returned
                        // `Response::Record` here, so it was found).
                        if let Some(reads) = snapshot_reads.as_mut() {
                            record_read_set(reads, id, &fields);
                        }
                        if let (Some(staged), Some(updatable)) =
                            (session.as_ref(), read_your_writes.as_ref())
                        {
                            overlay_staged(id, &mut fields, staged, updatable);
                        }
                        Response::Record { id, fields }
                    }
                    other => other,
                }
            }
            Request::Commit => {
                read_your_writes = None;
                validate_on_stage = false;
                // `ISO-FR-003`: whatever this session tracked is handed
                // to `apply_transaction` alongside the staged batch, to
                // be re-checked atomically with the apply — empty when
                // snapshot isolation was never turned on.
                let read_set: Vec<(RecordId, FieldRef, ScanValue)> = snapshot_reads
                    .take()
                    .map(|reads| reads.into_iter().map(|((id, f), v)| (id, f, v)).collect())
                    .unwrap_or_default();
                match session.take() {
                    None => err_response(ErrorCode::NoSession),
                    Some(batch) => match store.apply_transaction(&batch, &read_set) {
                        Ok(()) => Response::Ok,
                        Err((index, code)) => Response::TransactionFailed {
                            index,
                            code,
                            message: error_message(code).to_string(),
                        },
                    },
                }
            }
            Request::UpdateField { id, field, value } if session.is_some() => {
                let op = TransactionOp { id, field, value };
                // `STV-FR-001`: a validating session refuses a bad write
                // now, with the code `Commit` would have given; nothing
                // is staged.
                let refused = if validate_on_stage {
                    store.validate_op(&op).err()
                } else {
                    None
                };
                match (refused, session.as_mut()) {
                    (Some(code), _) => err_response(code),
                    (None, Some(staged)) if staged.len() < MAX_STAGED_OPS => {
                        staged.push(op);
                        Response::Staged {
                            index: (staged.len() - 1) as u32,
                        }
                    }
                    _ => err_response(ErrorCode::SessionFull),
                }
            }
            Request::Transaction { .. } if session.is_some() => {
                err_response(ErrorCode::SessionOpen)
            }
            other => dispatch(store, other),
        };
        let resp = downgrade_for_version(resp, negotiated);
        // `ACC-FR-004`: after the audit log's own recording for this path
        // (if any — the gates above already returned), one access event
        // per dispatched request, before the response is sent.
        options.access_log().record(&access::AccessEvent::now(
            peer,
            Some(class),
            request_kind,
            outcome_of(&resp),
        ));
        if !send_response(&mut writer, &resp) {
            return;
        }
    }
}

/// Accept connections on `listener` and serve each one on its own OS
/// thread against the same shared `store` — the thread-per-connection
/// model ADR-0010 chose over an async runtime. Every connection thread
/// takes only `&S`; all coordination is whatever locking `store` already
/// does internally (see this module's own doc comment). `options` is
/// shared (`Arc`) across every connection thread the same way `store`
/// is — see this module's own `handle_connection` for the gating/
/// handshake it performs. `options.tls()`'s `None` reproduces plaintext
/// behavior exactly (`TLS-FR-008`); configured, it requires every
/// connection to complete a TLS handshake before any request is served.
/// Runs until `listener` itself errors (e.g. the socket is closed) or
/// forever otherwise — a real deployment's shutdown/drain story is an
/// explicit non-goal of the accepted design, not solved here.
///
/// `options` consolidates every cross-cutting server concern —
/// tokens, certificate classes, the audit/access-log sinks, the
/// rate-limit budget, and (since `ADR-0032`) native TLS — into `serve`'s
/// one configuration parameter (`SRV-FR-001`/`SRV-FR-004`); before this
/// it was `ServeOptions` (then named `AuthConfig`) plus a separate
/// `Option<TlsConfig>` parameter.
pub fn serve<S: ConnectionStore + 'static>(
    listener: TcpListener,
    store: Arc<S>,
    options: ServeOptions,
) {
    let options = Arc::new(options);
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(_) => continue, // one bad accept doesn't take down the server
        };
        let store = Arc::clone(&store);
        let options = Arc::clone(&options);
        thread::spawn(move || handle_connection(stream, store.as_ref(), options.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MTLS-FR-004`: `TlsConfig::from_env`'s decision table, driven
    /// through the factored `from_env_values` so no real environment
    /// variable is read (the constraint `ServeOptions::from_env`'s docs
    /// impose on tests). Chain/key unset → `None`; the pair set → the
    /// PEM path (here an `Io` error, since the files do not exist);
    /// client CA set without the pair → `Some(Err(Io(NotFound)))`, never
    /// `None`.
    #[test]
    fn tls_from_env_values_treats_a_client_ca_without_a_chain_and_key_as_an_error() {
        let missing = |name: &str| Some(format!("/nonexistent/{name}"));
        assert!(TlsConfig::from_env_values(None, None, None).is_none());
        assert!(TlsConfig::from_env_values(missing("chain"), None, None).is_none());
        assert!(matches!(
            TlsConfig::from_env_values(missing("chain"), missing("key"), None),
            Some(Err(TlsConfigError::Io(_)))
        ));
        assert!(matches!(
            TlsConfig::from_env_values(missing("chain"), missing("key"), missing("ca")),
            Some(Err(TlsConfigError::Io(_)))
        ));
        match TlsConfig::from_env_values(None, None, missing("ca")).map(|r| r.map(|_| ())) {
            Some(Err(TlsConfigError::Io(e))) => {
                assert_eq!(e.kind(), io::ErrorKind::NotFound);
                assert!(e.to_string().contains("SERVER_TLS_CLIENT_CA_PATH"));
            }
            other => panic!("expected a NotFound Io error naming the variable, got {other:?}"),
        }
        match TlsConfig::from_env_values(missing("chain"), None, missing("ca"))
            .map(|r| r.map(|_| ()))
        {
            Some(Err(TlsConfigError::Io(e))) => assert_eq!(e.kind(), io::ErrorKind::NotFound),
            other => panic!("expected a NotFound Io error, got {other:?}"),
        }
    }

    /// A minimal in-memory `ConnectionStore` fixture, independent of any
    /// real domain adapter — exercises `dispatch`'s own logic (response
    /// shape per request kind, error-code mapping) without needing
    /// `ProductionStore`/`GenericProductionStore` at all.
    struct FixtureStore;

    const FIELD_A: FieldRef = 0;

    impl ConnectionStore for FixtureStore {
        fn get(&self, id: RecordId) -> Option<Vec<(FieldRef, ScanValue)>> {
            if id == RecordId::from_u128(1) {
                Some(vec![(FIELD_A, ScanValue::U32(7))])
            } else {
                None
            }
        }
        fn scan_all(&self) -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
            vec![(RecordId::from_u128(1), vec![(FIELD_A, ScanValue::U32(7))])]
        }
        fn filter_eq(
            &self,
            field: FieldRef,
            _value: &ScanValue,
        ) -> Result<Vec<RecordId>, ErrorCode> {
            if field == FIELD_A {
                Ok(vec![RecordId::from_u128(1)])
            } else {
                Err(ErrorCode::UnknownField)
            }
        }
        fn scan_field(&self, field: FieldRef) -> Result<Vec<ScanValue>, ErrorCode> {
            if field == FIELD_A {
                Ok(vec![ScanValue::U32(7)])
            } else {
                Err(ErrorCode::UnknownField)
            }
        }
        fn update_field(
            &self,
            id: RecordId,
            field: FieldRef,
            value: ScanValue,
        ) -> Result<bool, ErrorCode> {
            match (field, &value) {
                (FIELD_A, ScanValue::U32(_)) => Ok(id == RecordId::from_u128(1)),
                (FIELD_A, _) => Err(ErrorCode::Malformed),
                _ => Err(ErrorCode::UnknownField),
            }
        }
        fn parent(&self, id: RecordId) -> Result<ParentLookup, ErrorCode> {
            if id == RecordId::from_u128(1) {
                Ok(ParentLookup::Parent(RecordId::from_u128(100)))
            } else if id == RecordId::from_u128(2) {
                Ok(ParentLookup::NoParent)
            } else {
                Ok(ParentLookup::ChildNotFound)
            }
        }
        fn children(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
            Ok(vec![RecordId::from_u128(1)])
        }
        fn neighbors(&self, _id: RecordId) -> Result<Vec<RecordId>, ErrorCode> {
            Err(ErrorCode::Unsupported)
        }
        fn neighbors_by_relation(
            &self,
            _id: RecordId,
            _relation: &str,
        ) -> Result<Vec<RecordId>, ErrorCode> {
            Err(ErrorCode::Unsupported)
        }
        fn list_relation_kinds(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_op(&self, _op: &TransactionOp) -> Result<(), ErrorCode> {
            Ok(())
        }
        fn describe(&self) -> DomainSchema {
            use protocol::{FieldCapabilities, FieldDescriptor, RelationCapabilities, ValueKind};
            DomainSchema {
                fields: vec![FieldDescriptor {
                    tag: FIELD_A,
                    name: "a".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: true,
                        scan: true,
                        update: true,
                    },
                }],
                relations: RelationCapabilities {
                    parent_children: true,
                    neighbors: false,
                },
            }
        }
        fn apply_transaction(
            &self,
            updates: &[TransactionOp],
            read_set: &[(RecordId, FieldRef, ScanValue)],
        ) -> Result<(), (usize, ErrorCode)> {
            // Same validate-then-apply shape a real adapter uses, against
            // this fixture's own single-record, non-mutating "store" —
            // exercises dispatch's Request::Transaction arm without
            // needing a real domain adapter.
            for (i, op) in updates.iter().enumerate() {
                match (op.field, &op.value) {
                    (FIELD_A, ScanValue::U32(_)) => {
                        if op.id != RecordId::from_u128(1) {
                            return Err((i, ErrorCode::RecordNotFound));
                        }
                    }
                    (FIELD_A, _) => return Err((i, ErrorCode::Malformed)),
                    _ => return Err((i, ErrorCode::UnknownField)),
                }
            }
            // `ISO-FR-002`/`ISO-FR-006`: re-check every tracked read
            // against this fixture's own fixed state (id 1, FIELD_A = 7)
            // the same way a real adapter re-checks against its store.
            for (id, field, value) in read_set {
                let current = if *id == RecordId::from_u128(1) && *field == FIELD_A {
                    Some(ScanValue::U32(7))
                } else {
                    None
                };
                if current.as_ref() != Some(value) {
                    return Err((0, ErrorCode::Conflict));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn get_by_id_found_and_not_found() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::GetById {
                    id: RecordId::from_u128(1)
                }
            ),
            Response::Record {
                id: RecordId::from_u128(1),
                fields: vec![(FIELD_A, ScanValue::U32(7))],
            }
        );
        assert_eq!(
            dispatch(
                &store,
                Request::GetById {
                    id: RecordId::from_u128(99)
                }
            ),
            Response::NotFound
        );
    }

    #[test]
    fn update_field_maps_found_missing_and_malformed() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(1),
                    field: FIELD_A,
                    value: ScanValue::U32(9)
                }
            ),
            Response::Ok
        );
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(99),
                    field: FIELD_A,
                    value: ScanValue::U32(9)
                }
            ),
            Response::NotFound
        );
        assert_eq!(
            dispatch(
                &store,
                Request::UpdateField {
                    id: RecordId::from_u128(1),
                    field: FIELD_A,
                    value: ScanValue::Bool(true)
                }
            ),
            err_response(ErrorCode::Malformed)
        );
    }

    #[test]
    fn parent_preserves_the_not_found_versus_no_parent_distinction() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(1)
                }
            ),
            Response::Id {
                id: RecordId::from_u128(100)
            }
        );
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(2)
                }
            ),
            Response::NoParent
        );
        assert_eq!(
            dispatch(
                &store,
                Request::Parent {
                    id: RecordId::from_u128(3)
                }
            ),
            Response::NotFound
        );
    }

    #[test]
    fn describe_schema_returns_the_fixture_store_own_shape() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(&store, Request::DescribeSchema),
            Response::Schema(store.describe())
        );
    }

    /// `SQL-FR-004` end to end through `dispatch`: a valid `Query`
    /// against `FixtureStore` answers `Response::Rows`; an unknown field
    /// answers the typed error, never a panic.
    #[test]
    fn dispatch_answers_query_with_rows_or_a_typed_error() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Query {
                    select: Selection::All,
                    filter: vec![Predicate {
                        field: FIELD_A,
                        op: CompareOp::Eq,
                        value: ScanValue::U32(7),
                    }],
                    limit: None,
                }
            ),
            Response::Rows {
                rows: vec![(RecordId::from_u128(1), vec![(FIELD_A, ScanValue::U32(7))])]
            }
        );
        assert_eq!(
            dispatch(
                &store,
                Request::Query {
                    select: Selection::Fields(vec![99]),
                    filter: vec![],
                    limit: None,
                }
            ),
            err_response(ErrorCode::UnknownField)
        );
    }

    #[test]
    fn unsupported_operation_reports_a_typed_error_not_a_panic() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Neighbors {
                    id: RecordId::from_u128(1)
                }
            ),
            err_response(ErrorCode::Unsupported)
        );
    }

    #[test]
    fn transaction_all_pass_reports_ok() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Transaction {
                    updates: vec![
                        TransactionOp {
                            id: RecordId::from_u128(1),
                            field: FIELD_A,
                            value: ScanValue::U32(9),
                        },
                        TransactionOp {
                            id: RecordId::from_u128(1),
                            field: FIELD_A,
                            value: ScanValue::U32(10),
                        },
                    ]
                }
            ),
            Response::Ok
        );
    }

    #[test]
    fn transaction_reports_the_first_failing_operations_index_and_code() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Transaction {
                    updates: vec![
                        TransactionOp {
                            id: RecordId::from_u128(1),
                            field: FIELD_A,
                            value: ScanValue::U32(9),
                        },
                        TransactionOp {
                            id: RecordId::from_u128(99),
                            field: FIELD_A,
                            value: ScanValue::U32(9),
                        },
                        TransactionOp {
                            id: RecordId::from_u128(1),
                            field: FIELD_A,
                            value: ScanValue::U32(11),
                        },
                    ]
                }
            ),
            Response::TransactionFailed {
                index: 1,
                code: ErrorCode::RecordNotFound,
                message: error_message(ErrorCode::RecordNotFound).to_string(),
            }
        );
    }

    #[test]
    fn dispatch_never_routes_authenticate_to_a_store() {
        // handle_connection intercepts Authenticate before dispatch is ever
        // called with it — this only documents that dispatch itself stays
        // exhaustive and safe if that invariant were ever violated.
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Authenticate {
                    token: "irrelevant".into()
                }
            ),
            err_response(ErrorCode::Unsupported)
        );
    }

    /// `PROTO-FR-003`'s dispatch half (design criterion 5): like
    /// `Authenticate`, `Hello` is answered by `handle_connection`, never
    /// by a store.
    #[test]
    fn dispatch_never_routes_hello_to_a_store() {
        let store = FixtureStore;
        assert_eq!(
            dispatch(
                &store,
                Request::Hello {
                    protocol_version: PROTOCOL_VERSION
                }
            ),
            err_response(ErrorCode::Unsupported)
        );
    }

    /// `JRN-FR-008` / compatibility rule 3: `ErrorCode::Journal` is
    /// version 4, so a connection negotiated below 4 sees `Unsupported`
    /// in its place; nothing else is rewritten.
    #[test]
    fn journal_error_code_is_downgraded_below_version_4() {
        let failed = Response::TransactionFailed {
            index: 0,
            code: ErrorCode::Journal,
            message: error_message(ErrorCode::Journal).to_string(),
        };
        for older in [1, 2, 3] {
            assert_eq!(
                downgrade_for_version(failed.clone(), older),
                Response::TransactionFailed {
                    index: 0,
                    code: ErrorCode::Unsupported,
                    message: error_message(ErrorCode::Unsupported).to_string(),
                }
            );
        }
        assert_eq!(downgrade_for_version(failed.clone(), 4), failed);
        let untouched = err_response(ErrorCode::SessionFull);
        assert_eq!(downgrade_for_version(untouched.clone(), 1), untouched);
    }

    /// `ENT4-FR-003`/`ENT4-FR-006` (ADR-0041), acceptance criterion 5:
    /// the protocol's first content-rewriting downgrade. Below 11 every
    /// `StrList` pair is stripped from `Record` and from each `Rows` row
    /// and every `StrList` descriptor from `Schema`, other fields kept in
    /// order; at 11 all three are untouched; a `Record` with no `StrList`
    /// is untouched at 1 (every other domain's regression); the two
    /// `ErrorCode` cases are unchanged.
    #[test]
    fn str_list_content_is_stripped_below_version_11() {
        let id = uuid::Uuid::from_u128(1);
        let aliases = || (3u16, ScanValue::StrList(vec!["Ada".into()]));
        let label = || (0u16, ScanValue::Str("Ada Lovelace".into()));
        let count = || (2u16, ScanValue::I64(3));

        let record = Response::Record {
            id,
            fields: vec![label(), aliases(), count()],
        };
        let stripped_record = Response::Record {
            id,
            fields: vec![label(), count()],
        };
        let rows = Response::Rows {
            rows: vec![(id, vec![aliases(), label()]), (id, vec![aliases()])],
        };
        let stripped_rows = Response::Rows {
            rows: vec![(id, vec![label()]), (id, vec![])],
        };
        let descriptor = |name: &str, value_kind: protocol::ValueKind| protocol::FieldDescriptor {
            tag: 0,
            name: name.into(),
            value_kind,
            capabilities: protocol::FieldCapabilities {
                filter_eq: false,
                scan: false,
                update: false,
            },
        };
        let schema = Response::Schema(DomainSchema {
            fields: vec![
                descriptor("label", protocol::ValueKind::Str),
                descriptor("aliases", protocol::ValueKind::StrList),
            ],
            relations: protocol::RelationCapabilities {
                parent_children: false,
                neighbors: true,
            },
        });
        let stripped_schema = Response::Schema(DomainSchema {
            fields: vec![descriptor("label", protocol::ValueKind::Str)],
            relations: protocol::RelationCapabilities {
                parent_children: false,
                neighbors: true,
            },
        });

        for older in [1, 10] {
            assert_eq!(
                downgrade_for_version(record.clone(), older),
                stripped_record
            );
            assert_eq!(downgrade_for_version(rows.clone(), older), stripped_rows);
            assert_eq!(
                downgrade_for_version(schema.clone(), older),
                stripped_schema
            );
        }
        for current in [11, PROTOCOL_VERSION] {
            assert_eq!(downgrade_for_version(record.clone(), current), record);
            assert_eq!(downgrade_for_version(rows.clone(), current), rows);
            assert_eq!(downgrade_for_version(schema.clone(), current), schema);
        }
        // Every other domain: no `StrList` anywhere, nothing rewritten.
        assert_eq!(
            downgrade_for_version(stripped_record.clone(), 1),
            stripped_record
        );
        assert_eq!(
            downgrade_for_version(stripped_schema.clone(), 1),
            stripped_schema
        );
        // The two pre-existing cases hold, above and below their versions.
        let conflict = Response::TransactionFailed {
            index: 0,
            code: ErrorCode::Conflict,
            message: error_message(ErrorCode::Conflict).to_string(),
        };
        assert_eq!(downgrade_for_version(conflict.clone(), 11), conflict);
        assert_eq!(
            downgrade_for_version(conflict, 6),
            Response::TransactionFailed {
                index: 0,
                code: ErrorCode::Unsupported,
                message: error_message(ErrorCode::Unsupported).to_string(),
            }
        );
    }

    /// `SESS-FR-006`'s dispatch half: the session requests are
    /// per-connection, like `Authenticate` and `Hello`.
    #[test]
    fn dispatch_never_routes_session_requests_to_a_store() {
        let store = FixtureStore;
        for req in [
            Request::Begin,
            Request::Commit,
            Request::Rollback,
            Request::BeginWith { flags: 1 },
        ] {
            assert_eq!(dispatch(&store, req), err_response(ErrorCode::Unsupported));
        }
    }

    /// `RYW-FR-002`: the overlay is exact where it applies and inert
    /// everywhere else — last staged write per field wins; another id,
    /// an absent field, a kind mismatch, and a read-only field are each
    /// ignored.
    #[test]
    fn overlay_staged_replaces_only_matching_updatable_fields() {
        let id = RecordId::from_u128(1);
        let other = RecordId::from_u128(2);
        let staged = vec![
            TransactionOp {
                id,
                field: 1,
                value: ScanValue::U32(10),
            },
            TransactionOp {
                id: other,
                field: 1,
                value: ScanValue::U32(99),
            },
            TransactionOp {
                id,
                field: 1,
                value: ScanValue::U32(11),
            },
            TransactionOp {
                id,
                field: 2,
                value: ScanValue::U32(5),
            },
            TransactionOp {
                id,
                field: 0,
                value: ScanValue::Str("poodle".into()),
            },
            TransactionOp {
                id,
                field: 3,
                value: ScanValue::I64(7),
            },
        ];
        let mut fields = vec![
            (0, ScanValue::Str("labrador".into())),
            (1, ScanValue::U32(3)),
            (2, ScanValue::I64(4)),
        ];
        overlay_staged(id, &mut fields, &staged, &[1, 2]);
        assert_eq!(
            fields,
            vec![
                (0, ScanValue::Str("labrador".into())), // read-only: untouched
                (1, ScanValue::U32(11)),                // last staged write wins
                (2, ScanValue::I64(4)),                 // kind mismatch: untouched
            ]
        );
        overlay_staged(other, &mut fields, &staged, &[1, 2]);
        assert_eq!(fields[1], (1, ScanValue::U32(99)));
    }

    /// `ISO-FR-002`: a second read of the same `(id, field)` replaces the
    /// earlier entry — the read set always holds the most recently seen
    /// value, not a stale first-read snapshot.
    #[test]
    fn record_read_set_replaces_a_repeated_key_with_the_latest_value() {
        let id = RecordId::from_u128(1);
        let mut reads = HashMap::new();
        record_read_set(&mut reads, id, &[(1, ScanValue::U32(3))]);
        assert_eq!(reads.get(&(id, 1)), Some(&ScanValue::U32(3)));
        record_read_set(&mut reads, id, &[(1, ScanValue::U32(9))]);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads.get(&(id, 1)), Some(&ScanValue::U32(9)));
    }

    /// `ISO-FR-002`: distinct fields on the same id, and the same field
    /// across distinct ids, are independent keys.
    #[test]
    fn record_read_set_keys_by_both_id_and_field() {
        let id = RecordId::from_u128(1);
        let other = RecordId::from_u128(2);
        let mut reads = HashMap::new();
        record_read_set(
            &mut reads,
            id,
            &[
                (0, ScanValue::Str("labrador".into())),
                (1, ScanValue::U32(3)),
            ],
        );
        record_read_set(&mut reads, other, &[(1, ScanValue::U32(5))]);
        assert_eq!(reads.len(), 3);
        assert_eq!(
            reads.get(&(id, 0)),
            Some(&ScanValue::Str("labrador".into()))
        );
        assert_eq!(reads.get(&(id, 1)), Some(&ScanValue::U32(3)));
        assert_eq!(reads.get(&(other, 1)), Some(&ScanValue::U32(5)));
    }

    /// `ISO-FR-004`: past `MAX_TRACKED_READS` distinct keys, a *new* key
    /// is simply not added — the call never panics or truncates existing
    /// entries, and an already-tracked key keeps updating even once the
    /// map is at the cap.
    #[test]
    fn record_read_set_stops_adding_new_keys_past_the_cap_but_keeps_updating_old_ones() {
        let mut reads = HashMap::new();
        for i in 0..MAX_TRACKED_READS {
            record_read_set(
                &mut reads,
                RecordId::from_u128(i as u128),
                &[(0, ScanValue::U32(0))],
            );
        }
        assert_eq!(reads.len(), MAX_TRACKED_READS);

        // A new key past the cap is not added.
        record_read_set(
            &mut reads,
            RecordId::from_u128(MAX_TRACKED_READS as u128),
            &[(0, ScanValue::U32(0))],
        );
        assert_eq!(reads.len(), MAX_TRACKED_READS);
        assert!(!reads.contains_key(&(RecordId::from_u128(MAX_TRACKED_READS as u128), 0)));

        // An already-tracked key still updates at the cap.
        record_read_set(
            &mut reads,
            RecordId::from_u128(0),
            &[(0, ScanValue::U32(7))],
        );
        assert_eq!(reads.len(), MAX_TRACKED_READS);
        assert_eq!(
            reads.get(&(RecordId::from_u128(0), 0)),
            Some(&ScanValue::U32(7))
        );
    }

    /// `SQL-FR-007` (ADR-0034): a two-field schema — one numeric
    /// (`age`, `U32`), one string (`breed`, `Str`), the latter with
    /// every capability flag `false` — for `validate_query`/
    /// `evaluate_query` tests independent of any real domain adapter.
    fn sql_test_schema() -> DomainSchema {
        use protocol::{FieldCapabilities, FieldDescriptor, RelationCapabilities, ValueKind};
        DomainSchema {
            fields: vec![
                FieldDescriptor {
                    tag: 1,
                    name: "age".into(),
                    value_kind: ValueKind::U32,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: true,
                        update: true,
                    },
                },
                FieldDescriptor {
                    tag: 2,
                    name: "breed".into(),
                    value_kind: ValueKind::Str,
                    capabilities: FieldCapabilities {
                        filter_eq: false,
                        scan: false,
                        update: false,
                    },
                },
            ],
            relations: RelationCapabilities {
                parent_children: false,
                neighbors: false,
            },
        }
    }

    /// `SQL-FR-007`: an unknown field in `select` or `filter` is
    /// `UnknownField`; an ordering comparator against `breed` (`Str`) is
    /// `Malformed`; a kind-mismatched literal against `age` (`U32`) is
    /// `Malformed`; a fully valid query is `Ok`, including one that
    /// selects/filters `breed` — every capability flag `false` there
    /// doesn't stop `Query` from reaching it (`SQL-FR-008`).
    #[test]
    fn validate_query_reports_unknown_fields_and_kind_mismatches() {
        let schema = sql_test_schema();
        assert_eq!(
            validate_query(&schema, &Selection::Fields(vec![99]), &[]),
            Err(ErrorCode::UnknownField)
        );
        assert_eq!(
            validate_query(
                &schema,
                &Selection::All,
                &[Predicate {
                    field: 99,
                    op: CompareOp::Eq,
                    value: ScanValue::U32(1)
                }]
            ),
            Err(ErrorCode::UnknownField)
        );
        assert_eq!(
            validate_query(
                &schema,
                &Selection::All,
                &[Predicate {
                    field: 2,
                    op: CompareOp::Gt,
                    value: ScanValue::Str("a".into())
                }]
            ),
            Err(ErrorCode::Malformed),
            "an ordering comparator against a Str field is Malformed"
        );
        assert_eq!(
            validate_query(
                &schema,
                &Selection::All,
                &[Predicate {
                    field: 1,
                    op: CompareOp::Eq,
                    value: ScanValue::Str("3".into())
                }]
            ),
            Err(ErrorCode::Malformed),
            "a Str literal against a U32 field is Malformed"
        );
        assert_eq!(
            validate_query(
                &schema,
                &Selection::Fields(vec![2]),
                &[Predicate {
                    field: 2,
                    op: CompareOp::Eq,
                    value: ScanValue::Str("labrador".into())
                }]
            ),
            Ok(()),
            "breed has every capability flag false but is still queryable"
        );
    }

    fn sql_test_rows() -> Vec<(RecordId, Vec<(FieldRef, ScanValue)>)> {
        vec![
            (
                RecordId::from_u128(1),
                vec![
                    (1, ScanValue::U32(3)),
                    (2, ScanValue::Str("labrador".into())),
                ],
            ),
            (
                RecordId::from_u128(2),
                vec![(1, ScanValue::U32(5)), (2, ScanValue::Str("poodle".into()))],
            ),
            (
                RecordId::from_u128(3),
                vec![
                    (1, ScanValue::U32(9)),
                    (2, ScanValue::Str("labrador".into())),
                ],
            ),
        ]
    }

    /// `SQL-FR-006`: an empty filter matches every row; a filter matching
    /// nothing returns an empty `Vec`, never an error.
    #[test]
    fn evaluate_query_empty_filter_matches_everything_and_a_filter_matching_nothing_is_empty() {
        let all = evaluate_query(sql_test_rows(), &Selection::All, &[], None);
        assert_eq!(all.len(), 3);
        let none = evaluate_query(
            sql_test_rows(),
            &Selection::All,
            &[Predicate {
                field: 1,
                op: CompareOp::Gt,
                value: ScanValue::U32(100),
            }],
            None,
        );
        assert!(none.is_empty());
    }

    /// `SQL-FR-006`: `AND`-ed predicates across every comparator kind —
    /// only the row(s) matching every predicate come back.
    #[test]
    fn evaluate_query_ands_every_predicate_across_every_comparator() {
        let result = evaluate_query(
            sql_test_rows(),
            &Selection::All,
            &[
                Predicate {
                    field: 1,
                    op: CompareOp::Ge,
                    value: ScanValue::U32(5),
                },
                Predicate {
                    field: 2,
                    op: CompareOp::Eq,
                    value: ScanValue::Str("labrador".into()),
                },
            ],
            None,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, RecordId::from_u128(3));

        for (op, expected_ids) in [
            (CompareOp::Eq, vec![2]),
            (CompareOp::Ne, vec![1, 3]),
            (CompareOp::Lt, vec![1]),
            (CompareOp::Le, vec![1, 2]),
            (CompareOp::Gt, vec![3]),
            (CompareOp::Ge, vec![2, 3]),
        ] {
            let rows = evaluate_query(
                sql_test_rows(),
                &Selection::All,
                &[Predicate {
                    field: 1,
                    op,
                    value: ScanValue::U32(5),
                }],
                None,
            );
            let mut ids: Vec<u128> = rows.iter().map(|(id, _)| id.as_u128() % 1000).collect();
            ids.sort();
            assert_eq!(ids, expected_ids, "comparator {op:?}");
        }
    }

    /// `SQL-FR-003`: `Selection::All` returns every field;
    /// `Selection::Fields` returns only the named subset.
    #[test]
    fn evaluate_query_selection_all_vs_named_fields() {
        let all = evaluate_query(sql_test_rows(), &Selection::All, &[], None);
        assert_eq!(all[0].1.len(), 2);
        let named = evaluate_query(sql_test_rows(), &Selection::Fields(vec![2]), &[], None);
        assert_eq!(named[0].1, vec![(2, ScanValue::Str("labrador".into()))]);
    }

    /// `SQL-FR-001`: `limit` truncates the row count and nothing else —
    /// shorter than the match count truncates, longer (or `None`) leaves
    /// every match.
    #[test]
    fn evaluate_query_limit_truncates_the_row_count_only() {
        let limited = evaluate_query(sql_test_rows(), &Selection::All, &[], Some(2));
        assert_eq!(limited.len(), 2);
        let unbounded = evaluate_query(sql_test_rows(), &Selection::All, &[], Some(100));
        assert_eq!(unbounded.len(), 3);
        let zero = evaluate_query(sql_test_rows(), &Selection::All, &[], Some(0));
        assert!(zero.is_empty());
    }

    /// `ENT4-FR-003` (ADR-0041): a `StrList`-kinded field in `group_by`
    /// is `Malformed` — the one path a list could otherwise take into
    /// `Response::Groups`, which the rule-3 strip does not cover. A
    /// `StrList` predicate is `Malformed` too (`value_matches_kind` has
    /// no list arm), and `Sum`/`Avg`/`Min`/`Max` over it fall under the
    /// existing orderable-kind rule.
    #[test]
    fn validate_aggregate_refuses_a_str_list_group_key_and_predicate() {
        let mut schema = sql_test_schema();
        schema.fields.push(protocol::FieldDescriptor {
            tag: 7,
            name: "aliases".into(),
            value_kind: protocol::ValueKind::StrList,
            capabilities: protocol::FieldCapabilities {
                filter_eq: false,
                scan: false,
                update: false,
            },
        });
        assert_eq!(
            validate_aggregate(&schema, &[7], &[], &[]),
            Err(ErrorCode::Malformed),
            "StrList in group_by"
        );
        assert_eq!(
            validate_aggregate(
                &schema,
                &[],
                &[Predicate {
                    field: 7,
                    op: CompareOp::Eq,
                    value: ScanValue::StrList(vec!["Ada".into()]),
                }],
                &[]
            ),
            Err(ErrorCode::Malformed),
            "StrList predicate"
        );
        assert_eq!(
            validate_aggregate(
                &schema,
                &[],
                &[],
                &[AggregateSpec {
                    func: AggregateFn::Max,
                    field: Some(7),
                }]
            ),
            Err(ErrorCode::Malformed),
            "MAX over a StrList"
        );
    }

    /// `AGG-FR-006`: an unknown field in `group_by` or an `AggregateSpec`
    /// is `UnknownField`; `Count` with an explicit field, `Sum`/`Avg`/
    /// `Min`/`Max` with no field, and `Sum`/`Avg`/`Min`/`Max` against
    /// `breed` (`Str`, non-orderable) are each `Malformed`; a fully valid
    /// spec is `Ok`.
    #[test]
    fn validate_aggregate_reports_unknown_fields_and_malformed_specs() {
        let schema = sql_test_schema();
        assert_eq!(
            validate_aggregate(&schema, &[99], &[], &[]),
            Err(ErrorCode::UnknownField),
            "unknown field in group_by"
        );
        assert_eq!(
            validate_aggregate(
                &schema,
                &[],
                &[],
                &[AggregateSpec {
                    func: AggregateFn::Sum,
                    field: Some(99),
                }]
            ),
            Err(ErrorCode::UnknownField),
            "unknown field in an AggregateSpec"
        );
        assert_eq!(
            validate_aggregate(
                &schema,
                &[],
                &[],
                &[AggregateSpec {
                    func: AggregateFn::Count,
                    field: Some(1),
                }]
            ),
            Err(ErrorCode::Malformed),
            "COUNT with an explicit field"
        );
        for func in [
            AggregateFn::Sum,
            AggregateFn::Avg,
            AggregateFn::Min,
            AggregateFn::Max,
        ] {
            assert_eq!(
                validate_aggregate(&schema, &[], &[], &[AggregateSpec { func, field: None }]),
                Err(ErrorCode::Malformed),
                "{func:?} with no field"
            );
            assert_eq!(
                validate_aggregate(
                    &schema,
                    &[],
                    &[],
                    &[AggregateSpec {
                        func,
                        field: Some(2),
                    }]
                ),
                Err(ErrorCode::Malformed),
                "{func:?} against a Str field"
            );
        }
        assert_eq!(
            validate_aggregate(
                &schema,
                &[2],
                &[],
                &[
                    AggregateSpec {
                        func: AggregateFn::Count,
                        field: None,
                    },
                    AggregateSpec {
                        func: AggregateFn::Sum,
                        field: Some(1),
                    }
                ]
            ),
            Ok(()),
            "GROUP BY breed, COUNT(*), SUM(age) is valid"
        );
    }

    /// `AGG-FR-007`: `group_by` empty means exactly one implicit bucket
    /// over every filtered row; a filter matching nothing produces zero
    /// groups when `group_by` is non-empty, or one group whose `Count` is
    /// `0` when `group_by` is empty.
    #[test]
    fn evaluate_aggregate_group_by_empty_vs_grouped_and_a_filter_matching_nothing() {
        let whole_table = evaluate_aggregate(
            sql_test_rows(),
            &[],
            &[],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            None,
            &sql_test_schema(),
        );
        assert_eq!(whole_table.len(), 1);
        assert!(whole_table[0].key.is_empty());
        assert_eq!(whole_table[0].values, vec![ScanValue::I64(3)]);

        let grouped = evaluate_aggregate(
            sql_test_rows(),
            &[2],
            &[],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            None,
            &sql_test_schema(),
        );
        assert_eq!(grouped.len(), 2, "labrador and poodle");

        let no_group_by_no_match = evaluate_aggregate(
            sql_test_rows(),
            &[],
            &[Predicate {
                field: 1,
                op: CompareOp::Gt,
                value: ScanValue::U32(100),
            }],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            None,
            &sql_test_schema(),
        );
        assert_eq!(no_group_by_no_match.len(), 1);
        assert!(no_group_by_no_match[0].key.is_empty());
        assert_eq!(no_group_by_no_match[0].values, vec![ScanValue::I64(0)]);

        let grouped_no_match = evaluate_aggregate(
            sql_test_rows(),
            &[2],
            &[Predicate {
                field: 1,
                op: CompareOp::Gt,
                value: ScanValue::U32(100),
            }],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            None,
            &sql_test_schema(),
        );
        assert!(grouped_no_match.is_empty());
    }

    /// The implicit whole-table bucket's `Min`/`Max`/`Avg` on zero
    /// matching rows fall back to a `schema`-typed zero rather than
    /// panicking — the one case `AGG-FR-008`'s "a group with zero rows
    /// never appears" guarantee does not cover, since this group is the
    /// deliberate exception (acceptance criterion 4).
    #[test]
    fn evaluate_aggregate_min_max_avg_on_an_empty_implicit_bucket_do_not_panic() {
        let groups = evaluate_aggregate(
            sql_test_rows(),
            &[],
            &[Predicate {
                field: 1,
                op: CompareOp::Gt,
                value: ScanValue::U32(100),
            }],
            &[
                AggregateSpec {
                    func: AggregateFn::Sum,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Avg,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Min,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Max,
                    field: Some(1),
                },
            ],
            None,
            &sql_test_schema(),
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].values[0], ScanValue::I64(0), "SUM");
        assert_eq!(groups[0].values[1], ScanValue::F64(0.0), "AVG");
        assert_eq!(groups[0].values[2], ScanValue::U32(0), "MIN, age is U32");
        assert_eq!(groups[0].values[3], ScanValue::U32(0), "MAX, age is U32");
    }

    /// `AGG-FR-008`: every aggregate function's reduction, including the
    /// `Sum`/`Count`-vs-`Avg` cross-check acceptance criterion 6 names —
    /// `AVG` equals that same group's `SUM` divided by its `COUNT`.
    #[test]
    fn evaluate_aggregate_every_function_reduces_correctly() {
        // Grouped by breed: labrador = {age 3, age 9}, poodle = {age 5}.
        let groups = evaluate_aggregate(
            sql_test_rows(),
            &[2],
            &[],
            &[
                AggregateSpec {
                    func: AggregateFn::Count,
                    field: None,
                },
                AggregateSpec {
                    func: AggregateFn::Sum,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Avg,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Min,
                    field: Some(1),
                },
                AggregateSpec {
                    func: AggregateFn::Max,
                    field: Some(1),
                },
            ],
            None,
            &sql_test_schema(),
        );
        assert_eq!(groups.len(), 2);
        let by_breed = |breed: &str| {
            groups
                .iter()
                .find(|g| g.key == vec![(2, ScanValue::Str(breed.into()))])
                .unwrap_or_else(|| panic!("no group for {breed}"))
        };
        let labrador = by_breed("labrador");
        assert_eq!(labrador.values[0], ScanValue::I64(2), "COUNT");
        assert_eq!(labrador.values[1], ScanValue::I64(12), "SUM(3+9)");
        assert_eq!(labrador.values[2], ScanValue::F64(6.0), "AVG(12/2)");
        assert_eq!(labrador.values[3], ScanValue::U32(3), "MIN");
        assert_eq!(labrador.values[4], ScanValue::U32(9), "MAX");
        let poodle = by_breed("poodle");
        assert_eq!(poodle.values[0], ScanValue::I64(1), "COUNT");
        assert_eq!(poodle.values[1], ScanValue::I64(5), "SUM");
        assert_eq!(poodle.values[2], ScanValue::F64(5.0), "AVG(5/1)");
        assert_eq!(poodle.values[3], ScanValue::U32(5), "MIN");
        assert_eq!(poodle.values[4], ScanValue::U32(5), "MAX");
    }

    /// `AGG-FR-007`: `limit` truncates the *group* count, applied after
    /// the full reduction.
    #[test]
    fn evaluate_aggregate_limit_truncates_the_group_count_only() {
        let limited = evaluate_aggregate(
            sql_test_rows(),
            &[2],
            &[],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            Some(1),
            &sql_test_schema(),
        );
        assert_eq!(limited.len(), 1);
        let unbounded = evaluate_aggregate(
            sql_test_rows(),
            &[2],
            &[],
            &[AggregateSpec {
                func: AggregateFn::Count,
                field: None,
            }],
            Some(100),
            &sql_test_schema(),
        );
        assert_eq!(unbounded.len(), 2);
    }

    /// Spin up `serve` over `FixtureStore` on a loopback port with
    /// authentication configured, so the `Hello` intercept is exercised
    /// exactly where it sits: ahead of the auth gate. Returns the address
    /// and a connected, framed client stream.
    fn hello_fixture() -> (BufReader<TcpStream>, BufWriter<TcpStream>) {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => panic!("bind loopback listener: {e}"),
        };
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(e) => panic!("listener address: {e}"),
        };
        let auth = ServeOptions::new(Some("ro".into()), Some("rw".into()));
        thread::spawn(move || serve(listener, Arc::new(FixtureStore), auth));
        let stream = match TcpStream::connect(addr) {
            Ok(s) => s,
            Err(e) => panic!("connect to fixture server: {e}"),
        };
        let peer = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => panic!("clone client stream: {e}"),
        };
        (BufReader::new(stream), BufWriter::new(peer))
    }

    fn roundtrip(
        reader: &mut BufReader<TcpStream>,
        writer: &mut BufWriter<TcpStream>,
        req: &Request,
    ) -> Response {
        if let Err(e) = framing::write_message(writer, req) {
            panic!("write request: {e}");
        }
        if let Err(e) = writer.flush() {
            panic!("flush request: {e}");
        }
        match framing::read_message(reader) {
            Ok(resp) => resp,
            Err(e) => panic!("read response: {e}"),
        }
    }

    /// Design criteria 3 and 4 on one connection each: a newer client is
    /// answered with this build's own version, an older one with its own;
    /// `Hello` is answered on an auth-configured server *before* any
    /// token is presented, and the gate behind it is intact — the next
    /// non-`Hello` request is still `Unauthenticated`.
    #[test]
    fn hello_is_answered_unauthenticated_with_the_min_version() {
        let (mut reader, mut writer) = hello_fixture();
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: PROTOCOL_VERSION + 3
                }
            ),
            Response::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        );
        assert_eq!(
            roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
            err_response(ErrorCode::Unauthenticated)
        );

        let (mut reader, mut writer) = hello_fixture();
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: 1
                }
            ),
            Response::Hello {
                protocol_version: 1
            }
        );
        assert_eq!(
            roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
            err_response(ErrorCode::Unauthenticated)
        );
    }

    /// `PROTO-FR-004`: version 0 and a second `Hello` are each `Malformed`,
    /// and neither ends the connection — the client can carry on (here,
    /// into the auth gate, which is still in place).
    #[test]
    fn hello_version_zero_and_a_second_hello_are_malformed_but_not_fatal() {
        let (mut reader, mut writer) = hello_fixture();
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: 0
                }
            ),
            err_response(ErrorCode::Malformed)
        );
        // The rejected frame was still the first frame; a `Hello` after it
        // is no longer first.
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: 1
                }
            ),
            err_response(ErrorCode::Malformed)
        );
        assert_eq!(
            roundtrip(&mut reader, &mut writer, &Request::DescribeSchema),
            err_response(ErrorCode::Unauthenticated)
        );

        // A valid first `Hello`, then a second valid one: the second is
        // `Malformed` too.
        let (mut reader, mut writer) = hello_fixture();
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: PROTOCOL_VERSION
                }
            ),
            Response::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        );
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: PROTOCOL_VERSION
                }
            ),
            err_response(ErrorCode::Malformed)
        );
        // And a `Hello` that is not the first frame at all — after an
        // `Authenticate` — is `Malformed` regardless of the auth outcome.
        let (mut reader, mut writer) = hello_fixture();
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Authenticate { token: "rw".into() }
            ),
            Response::Ok
        );
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::Hello {
                    protocol_version: PROTOCOL_VERSION
                }
            ),
            err_response(ErrorCode::Malformed)
        );
        // The connection is still authenticated and serving.
        assert_eq!(
            roundtrip(
                &mut reader,
                &mut writer,
                &Request::GetById {
                    id: RecordId::from_u128(1)
                }
            ),
            Response::Record {
                id: RecordId::from_u128(1),
                fields: vec![(FIELD_A, ScanValue::U32(7))]
            }
        );
    }

    #[test]
    fn auth_config_default_is_unconfigured() {
        assert!(!ServeOptions::default().is_configured());
        assert_eq!(ServeOptions::default().check("anything"), None);
    }

    #[test]
    fn auth_config_check_maps_each_token_to_its_own_class() {
        let auth = ServeOptions::new(Some("ro-secret".into()), Some("rw-secret".into()));
        assert!(auth.is_configured());
        assert_eq!(auth.check("ro-secret"), Some(TokenClass::ReadOnly));
        assert_eq!(auth.check("rw-secret"), Some(TokenClass::ReadWrite));
        assert_eq!(auth.check("wrong"), None);
        // A prefix or superstring of a real token must not match — rules
        // out an accidental substring/prefix comparison bug.
        assert_eq!(auth.check("ro-secret-extra"), None);
        assert_eq!(auth.check("ro-secre"), None);
    }

    #[test]
    fn auth_config_works_with_only_one_class_configured() {
        let read_only_only = ServeOptions::new(Some("ro-secret".into()), None);
        assert_eq!(
            read_only_only.check("ro-secret"),
            Some(TokenClass::ReadOnly)
        );
        assert_eq!(read_only_only.check("rw-secret"), None);

        let read_write_only = ServeOptions::new(None, Some("rw-secret".into()));
        assert_eq!(
            read_write_only.check("rw-secret"),
            Some(TokenClass::ReadWrite)
        );
        assert_eq!(read_write_only.check("ro-secret"), None);
    }

    /// `CLS-FR-003`: exact byte equality, and only exact byte equality —
    /// a prefix/superstring must not match, matching `check`'s own
    /// substring-safety test above.
    #[test]
    fn class_for_certificate_matches_by_exact_der_bytes_only() {
        let auth = ServeOptions::default()
            .with_certificate_class(vec![1, 2, 3], TokenClass::ReadOnly)
            .with_certificate_class(vec![4, 5, 6], TokenClass::ReadWrite);
        assert_eq!(
            auth.class_for_certificate(&[1, 2, 3]),
            Some(TokenClass::ReadOnly)
        );
        assert_eq!(
            auth.class_for_certificate(&[4, 5, 6]),
            Some(TokenClass::ReadWrite)
        );
        assert_eq!(auth.class_for_certificate(&[1, 2, 3, 4]), None);
        assert_eq!(auth.class_for_certificate(&[1, 2]), None);
        assert_eq!(auth.class_for_certificate(&[9, 9, 9]), None);
    }

    /// `CLS-FR-003`: a certificates-only `ServeOptions` (no tokens) is
    /// `is_configured()` — the safe direction `AUTH-FR-007` requires of a
    /// configured server (`SERVER-MTLS-CLASS-DESIGN.md`'s "Security,
    /// privacy, and compatibility").
    #[test]
    fn is_configured_is_true_with_only_a_certificate_class() {
        let auth = ServeOptions::default().with_certificate_class(vec![1], TokenClass::ReadOnly);
        assert!(auth.is_configured());
    }

    /// `CLS-FR-006`: `Debug` prints counts per class, never a configured
    /// leaf's bytes.
    #[test]
    fn auth_config_debug_prints_certificate_counts_not_bytes() {
        let auth = ServeOptions::default()
            .with_certificate_class(vec![0xAB, 0xCD], TokenClass::ReadOnly)
            .with_certificate_class(vec![0xEF], TokenClass::ReadWrite)
            .with_certificate_class(vec![0x12], TokenClass::ReadWrite);
        let printed = format!("{auth:?}");
        assert!(printed.contains("read_only_certificates: 1"));
        assert!(printed.contains("read_write_certificates: 2"));
        assert!(!printed.contains("171")); // 0xAB as decimal — no raw byte ever printed
        assert!(!printed.contains("[171, 205]"));
    }

    /// `RL-FR-006`, acceptance criterion 6: valid input parses, every
    /// documented malformed shape is rejected.
    #[test]
    fn rate_limit_parse_accepts_valid_and_rejects_malformed_input() {
        assert_eq!(
            RateLimit::parse("10/60").unwrap(),
            RateLimit {
                failures: 10,
                window: Duration::from_secs(60),
            }
        );
        for bad in ["10", "0/60", "10/0", "a/b", "10/", "/60", "", "10/60/1"] {
            assert!(
                RateLimit::parse(bad).is_err(),
                "{bad:?} should have been rejected"
            );
        }
    }

    /// Acceptance criterion 3: two peers are tracked independently.
    #[test]
    fn failure_table_tracks_each_peer_independently() {
        let table = FailureTable::new(RateLimit {
            failures: 3,
            window: Duration::from_secs(60),
        });
        let a: IpAddr = "127.0.0.1".parse().unwrap();
        let b: IpAddr = "127.0.0.2".parse().unwrap();
        for _ in 0..3 {
            table.note_failure(a);
        }
        assert!(table.is_throttled(a));
        assert!(!table.is_throttled(b));
    }

    /// Acceptance criterion 4: after the window elapses, a peer that was
    /// over budget is under budget again.
    #[test]
    fn failure_table_forgets_a_peer_once_its_window_elapses() {
        let table = FailureTable::new(RateLimit {
            failures: 1,
            window: Duration::from_millis(20),
        });
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        table.note_failure(peer);
        assert!(table.is_throttled(peer));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!table.is_throttled(peer));
    }

    /// Acceptance criterion 5: inserting more addresses than
    /// `MAX_TRACKED_PEERS` evicts (here, since nothing has expired) the
    /// oldest entries first — the table never exceeds the cap, and the
    /// earliest-tracked peer is the one that falls out.
    #[test]
    fn failure_table_never_exceeds_max_tracked_peers() {
        let table = FailureTable::new(RateLimit {
            failures: 1,
            window: Duration::from_secs(3600),
        });
        let first = IpAddr::V4(std::net::Ipv4Addr::from(0u32));
        for i in 0..(MAX_TRACKED_PEERS as u32 + 10) {
            let peer = IpAddr::V4(std::net::Ipv4Addr::from(i));
            table.note_failure(peer);
            assert!(table.peers.lock().unwrap().len() <= MAX_TRACKED_PEERS);
        }
        assert!(!table.peers.lock().unwrap().contains_key(&first));
    }

    /// `AUTH-FR-006`'s empirical half: a wrong token that differs from the
    /// configured one at the very first byte must not check measurably
    /// faster than one that differs only at the very last byte — the
    /// classic signature of an early-exit (non-constant-time) comparison.
    /// Measured directly against `ServeOptions::check` (not over a real TCP
    /// round trip, unlike the rest of this crate's server tests): a
    /// network hop's own jitter (microseconds to milliseconds) would
    /// completely swamp the signal this specific claim is about — a
    /// difference on the order of one byte comparison in a same-length
    /// byte string. Every configured token is checked unconditionally
    /// regardless of position (see `check`'s own doc comment), so this is
    /// expected to hold structurally, not just empirically; the timing
    /// measurement is still real evidence per `SERVER-AUTH-DESIGN.md`'s
    /// own "not just a read-through" verification plan.
    #[test]
    fn token_comparison_time_does_not_depend_on_where_the_mismatch_is() {
        use std::time::Instant;

        let configured = "a".repeat(64);
        let auth = ServeOptions::new(None, Some(configured.clone()));

        let mut differs_at_start = "b".to_string();
        differs_at_start.push_str(&"a".repeat(63));
        let mut differs_at_end = "a".repeat(63);
        differs_at_end.push('b');
        assert_eq!(differs_at_start.len(), configured.len());
        assert_eq!(differs_at_end.len(), configured.len());

        const ITERATIONS: u32 = 20_000;

        // Warm up (first-touch page faults, branch predictor, etc.) before
        // the real measurement, same discipline this crate's own
        // benchmarks already use.
        for _ in 0..1_000 {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_start)));
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_end)));
        }

        let start_timer = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_start)));
        }
        let start_elapsed = start_timer.elapsed();

        let end_timer = Instant::now();
        for _ in 0..ITERATIONS {
            std::hint::black_box(auth.check(std::hint::black_box(&differs_at_end)));
        }
        let end_elapsed = end_timer.elapsed();

        let ratio = start_elapsed.as_secs_f64() / end_elapsed.as_secs_f64().max(1e-12);
        assert!(
            (0.2..5.0).contains(&ratio),
            "mismatch-position timing ratio {ratio} is well outside a noise-only \
             range (first-byte-diff: {start_elapsed:?}, last-byte-diff: {end_elapsed:?}) \
             — investigate for an early-exit comparison"
        );
    }
}
