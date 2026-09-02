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
//! unless both `AuthConfig` and `TlsConfig` (below) are configured** —
//! see ADR-0010's Consequences; ADR-0012 closed the authentication/
//! authorization half of that gap, ADR-0014 closes the transport-
//! encryption half, but neither alone is the whole story (a
//! `TlsConfig`-only server still lets anyone who can connect do
//! anything; an `AuthConfig`-only server still puts tokens and every
//! record value in plaintext on the wire). ADR-0010's third named gap,
//! "no transaction semantics," is now partly closed too — see "Atomic
//! transactions" below for exactly which slice.
//!
//! # Authentication/authorization (`AuthConfig`), ADR-0012
//!
//! `docs/design/SERVER-AUTH-DESIGN.md` closes the "no authentication, no
//! authorization" gap ADR-0010 originally left open: [`serve`] takes an
//! [`AuthConfig`] naming which token(s) (if any) a server instance
//! accepts and the [`TokenClass`] (`ReadOnly`/`ReadWrite`) each grants.
//! `Request::Authenticate` establishes a connection's class; every other
//! request kind is rejected with `ErrorCode::Unauthenticated` until it
//! does, and `ReadOnly` is further rejected from `Request::UpdateField`/
//! `Request::Transaction` with `ErrorCode::Unauthorized`.
//! `AuthConfig::default()` (no tokens configured) reproduces exactly the
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
//! full account.
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
//! identity remains exactly [`AuthConfig`]'s existing shared-secret token
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
//! existing client, test, or bench changed (`PROTO-FR-002`). No
//! negotiated-version state is stored at version 2: there is no
//! version-gated variant yet to consult it (rule 3 in
//! [`crate::server::protocol`] names where that state goes when the first
//! one arrives).

pub mod client;
pub mod dog;
#[cfg(feature = "research")]
pub mod employee;
pub mod framing;
#[cfg(feature = "research")]
pub mod order;
mod pem;
pub mod protocol;

use protocol::{
    DomainSchema, ErrorCode, FieldRef, ParentLookup, RecordId, Request, Response, ScanValue,
    TransactionOp, PROTOCOL_VERSION,
};
use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

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

    /// A symmetric relation (e.g. `Dog`'s `littermate_of`).
    /// `Err(ErrorCode::Unsupported)` for a domain with no symmetric
    /// relation at all (e.g. `Order`/`Customer`).
    fn neighbors(&self, id: RecordId) -> Result<Vec<RecordId>, ErrorCode>;

    /// This domain's schema, for a client that doesn't know it at compile
    /// time — ADR-0011. Infallible: every `ConnectionStore` implementor
    /// knows its own field/relation shape unconditionally, no store access
    /// needed.
    fn describe(&self) -> DomainSchema;

    /// Apply every operation in `updates` atomically: every precondition
    /// (id exists, field known and updatable, value type matches) is
    /// checked before any write is applied; either every write in
    /// `updates` is applied, or none are. `Err((index, code))` names the
    /// first operation that failed its precondition check — see
    /// `docs/design/SERVER-TRANSACTION-DESIGN.md`, ADR-0013,
    /// `TXN-FR-002`/`TXN-FR-003`.
    fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)>;
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
    }
}

fn err_response(code: ErrorCode) -> Response {
    Response::Err {
        code,
        message: error_message(code).to_string(),
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
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    read_only_token: Option<String>,
    read_write_token: Option<String>,
}

impl AuthConfig {
    /// Build directly from already-known tokens. Use this from tests and
    /// from any caller that already has its tokens from its own config
    /// source — see [`AuthConfig::from_env`]'s own doc comment for why
    /// tests specifically must not use real environment variables.
    pub fn new(read_only_token: Option<String>, read_write_token: Option<String>) -> Self {
        Self {
            read_only_token,
            read_write_token,
        }
    }

    /// Build from `SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`
    /// at process startup (`AUTH-FR-005`). Reserved for a real server
    /// binary's own one-time startup — `cargo test` runs many tests in
    /// parallel within one process, so reading real process-wide
    /// environment variables from a test would race with every other test
    /// doing the same; tests use [`AuthConfig::new`] instead.
    pub fn from_env() -> Self {
        Self {
            read_only_token: std::env::var("SERVER_AUTH_READ_ONLY_TOKEN").ok(),
            read_write_token: std::env::var("SERVER_AUTH_READ_WRITE_TOKEN").ok(),
        }
    }

    /// No tokens configured at all — `AUTH-FR-007`: every connection
    /// behaves exactly as it did before this feature existed, and
    /// `Authenticate` becomes a no-op success.
    pub fn is_configured(&self) -> bool {
        self.read_only_token.is_some() || self.read_write_token.is_some()
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
}

/// Native TLS configuration for [`serve`] (ADR-0014,
/// `docs/design/SERVER-TLS-DESIGN.md`). Wraps a `rusty_tls::TlsAcceptor`
/// — this owner's own ecosystem-wide `rustls` wrapper, not a direct
/// `rustls` dependency, see that design's own "Ecosystem check" for why —
/// built once at server startup and shared across every connection
/// thread [`serve`] spawns, the same lifecycle [`AuthConfig`] already
/// uses for its own configured tokens. `serve` with `tls: None` behaves
/// exactly as it did before this feature existed (`TLS-FR-008`) — this is
/// purely opt-in.
pub struct TlsConfig {
    acceptor: rusty_tls::TlsAcceptor,
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
        Ok(Self { acceptor })
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

    /// Build from `SERVER_TLS_CERT_CHAIN_PATH`/`SERVER_TLS_PRIVATE_KEY_PATH`
    /// at process startup, mirroring [`AuthConfig::from_env`]'s own
    /// pattern — `None` (rather than an error) if either variable is
    /// unset, so a caller can treat "TLS not configured" and "TLS
    /// misconfigured" differently: the former is `serve(..., None)`'s
    /// ordinary opt-out, the latter is a real startup error a caller
    /// should surface (`Some(Err(..))`).
    pub fn from_env() -> Option<Result<Self, TlsConfigError>> {
        let cert_chain_path = std::env::var("SERVER_TLS_CERT_CHAIN_PATH").ok()?;
        let private_key_path = std::env::var("SERVER_TLS_PRIVATE_KEY_PATH").ok()?;
        Some(Self::from_pem_files(cert_chain_path, private_key_path))
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
/// sufficient; no `Mutex`/`Arc` needed for this, unlike `AuthConfig`/
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
        Request::DescribeSchema => Response::Schema(store.describe()),
        // `Authenticate` is intercepted directly by `handle_connection`,
        // which has the per-connection state (and `AuthConfig`) this
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
        Request::Transaction { updates } => match store.apply_transaction(&updates) {
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
fn handle_connection<S: ConnectionStore + ?Sized>(
    stream: TcpStream,
    store: &S,
    auth: &AuthConfig,
    tls: &Option<TlsConfig>,
) {
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
            let tls_stream = match tls.acceptor.accept(stream) {
                Ok(s) => s,
                Err(_) => return, // config/setup error building the connection object — drop cleanly, no panic
            };
            let shared = Rc::new(RefCell::new(tls_stream));
            (
                BufReader::new(ReadHalf::Tls(Rc::clone(&shared))),
                BufWriter::new(WriteHalf::Tls(shared)),
            )
        }
    };

    let mut authenticated: Option<TokenClass> = if auth.is_configured() {
        None
    } else {
        Some(TokenClass::ReadWrite)
    };

    // `PROTO-FR-004`: only the very first frame may be a `Hello`. This is
    // the one piece of per-connection version state at protocol version 2
    // — the negotiated version itself is not kept, because nothing yet
    // consults it (see `protocol`'s compatibility rule 3).
    let mut first_frame = true;

    loop {
        let req: Request = match framing::read_message(&mut reader) {
            Ok(req) => req,
            Err(_) => return, // client disconnected, or a framing/decode error — end the connection
        };

        if let Request::Hello { protocol_version } = &req {
            let resp = if !first_frame || *protocol_version == 0 {
                err_response(ErrorCode::Malformed)
            } else {
                Response::Hello {
                    protocol_version: (*protocol_version).min(PROTOCOL_VERSION),
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
            let resp = if !auth.is_configured() {
                Response::Ok
            } else {
                match auth.check(token) {
                    Some(class) => {
                        authenticated = Some(class);
                        Response::Ok
                    }
                    None => err_response(ErrorCode::Unauthenticated),
                }
            };
            if !send_response(&mut writer, &resp) {
                return;
            }
            continue;
        }

        let class = match authenticated {
            Some(class) => class,
            None => {
                if !send_response(&mut writer, &err_response(ErrorCode::Unauthenticated)) {
                    return;
                }
                continue;
            }
        };

        if class == TokenClass::ReadOnly
            && matches!(
                req,
                Request::UpdateField { .. } | Request::Transaction { .. }
            )
        {
            if !send_response(&mut writer, &err_response(ErrorCode::Unauthorized)) {
                return;
            }
            continue;
        }

        let resp = dispatch(store, req);
        if !send_response(&mut writer, &resp) {
            return;
        }
    }
}

/// Accept connections on `listener` and serve each one on its own OS
/// thread against the same shared `store` — the thread-per-connection
/// model ADR-0010 chose over an async runtime. Every connection thread
/// takes only `&S`; all coordination is whatever locking `store` already
/// does internally (see this module's own doc comment). `auth` and `tls`
/// are each shared (`Arc`) across every connection thread the same way
/// `store` is — see this module's own `handle_connection` for the gating/
/// handshake each performs. `tls: None` reproduces plaintext behavior
/// exactly (`TLS-FR-008`); `tls: Some(..)` requires every connection to
/// complete a TLS handshake before any request is served. Runs until
/// `listener` itself errors (e.g. the socket is closed) or forever
/// otherwise — a real deployment's shutdown/drain story is an explicit
/// non-goal of the accepted design, not solved here.
pub fn serve<S: ConnectionStore + 'static>(
    listener: TcpListener,
    store: Arc<S>,
    auth: AuthConfig,
    tls: Option<TlsConfig>,
) {
    let auth = Arc::new(auth);
    let tls = Arc::new(tls);
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(_) => continue, // one bad accept doesn't take down the server
        };
        let store = Arc::clone(&store);
        let auth = Arc::clone(&auth);
        let tls = Arc::clone(&tls);
        thread::spawn(move || {
            handle_connection(stream, store.as_ref(), auth.as_ref(), tls.as_ref())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fn apply_transaction(&self, updates: &[TransactionOp]) -> Result<(), (usize, ErrorCode)> {
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
        let auth = AuthConfig::new(Some("ro".into()), Some("rw".into()));
        thread::spawn(move || serve(listener, Arc::new(FixtureStore), auth, None));
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
        assert!(!AuthConfig::default().is_configured());
        assert_eq!(AuthConfig::default().check("anything"), None);
    }

    #[test]
    fn auth_config_check_maps_each_token_to_its_own_class() {
        let auth = AuthConfig::new(Some("ro-secret".into()), Some("rw-secret".into()));
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
        let read_only_only = AuthConfig::new(Some("ro-secret".into()), None);
        assert_eq!(
            read_only_only.check("ro-secret"),
            Some(TokenClass::ReadOnly)
        );
        assert_eq!(read_only_only.check("rw-secret"), None);

        let read_write_only = AuthConfig::new(None, Some("rw-secret".into()));
        assert_eq!(
            read_write_only.check("rw-secret"),
            Some(TokenClass::ReadWrite)
        );
        assert_eq!(read_write_only.check("ro-secret"), None);
    }

    /// `AUTH-FR-006`'s empirical half: a wrong token that differs from the
    /// configured one at the very first byte must not check measurably
    /// faster than one that differs only at the very last byte — the
    /// classic signature of an early-exit (non-constant-time) comparison.
    /// Measured directly against `AuthConfig::check` (not over a real TCP
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
        let auth = AuthConfig::new(None, Some(configured.clone()));

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
