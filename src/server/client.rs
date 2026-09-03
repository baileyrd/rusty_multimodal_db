//! A schema-driven client: connects to a server/query-layer instance,
//! calls `Request::DescribeSchema` once at connect time, and drives every
//! subsequent request by field *name* rather than a compile-time-known
//! `FieldRef` constant — the client half of ADR-0011's own "genuinely
//! usable, not just descriptive" bar. `tests/server_dog_integration.rs`'s,
//! `tests/server_order_integration.rs`'s, and
//! `tests/server_employee_integration.rs`'s own schema-driven tests each
//! proved this is possible with test-local, one-off code; this module is
//! that logic promoted into real, reusable library API, so a future
//! caller (or those same tests) doesn't have to reimplement it by hand.
//!
//! # What "doesn't know a domain at compile time" means here
//!
//! [`SchemaDrivenClient`] never imports a `FIELD_*` constant from
//! [`super::dog`]/[`super::order`]/[`super::employee`] — every field is
//! addressed by the `&str` name [`Request::DescribeSchema`]'s response
//! names it. [`SchemaDrivenClient::get`] returns `(String, ScanValue)`
//! pairs, not `(FieldRef, ScanValue)`, for the same reason: a caller with
//! no compile-time schema knowledge has nothing useful to do with a bare
//! `u16` tag on its own.
//!
//! # Capability checks happen client-side first
//!
//! [`DomainSchema`]'s own `FieldCapabilities`/`RelationCapabilities` are
//! already trustworthy — every `ConnectionStore` implementor's
//! `describe()` is required to report its real, honest shape
//! (`SERVER-001-FR-010`). [`SchemaDrivenClient::filter_eq`]/`scan`/
//! `update`/`parent`/`children`/`neighbors` all check the relevant
//! capability locally before sending anything, returning
//! [`ClientError::Unsupported`] without paying a round trip for something
//! the schema already ruled out. This is an optimization, not a trust
//! boundary: the server's own `dispatch` (`src/server/mod.rs`) still
//! enforces the identical rules independently, so a client that skipped
//! this check (or a different, buggy client) would still get back a
//! typed `Response::Err`, never undefined behavior.
//!
//! # Protocol version (`Hello`), ADR-0022
//!
//! [`SchemaDrivenClient::connect`] says `Request::Hello` with this build's
//! [`PROTOCOL_VERSION`] before its `DescribeSchema` (`PROTO-FR-007`) and
//! keeps the negotiated version — `min(client, server)` — behind
//! [`SchemaDrivenClient::server_protocol_version`], and gates its own
//! version-3 API ([`SchemaDrivenClient::begin`]) on it (compatibility
//! rule 4 in [`super::protocol`]'s docs). A server that predates the hello
//! (any `SERVER-001` v0.9.1 or earlier build) does not answer it: it
//! closes the connection with no reply. Since `SERVER-001` FR-026 (the
//! reconnect-without-hello fallback ADR-0022 named as a revisit trigger,
//! taken at the owner's call as default-on) [`SchemaDrivenClient::connect_with`]
//! treats exactly that — the peer closing the connection under the
//! `Hello` frame, before any reply — as a pre-hello server: it opens a
//! second connection, sends no `Hello`, reports the version as 1, and
//! every version-gated API refuses from there. The heuristic fires once
//! per `connect_with`, only on an end-of-stream/reset/abort-class I/O
//! error under the `Hello`, never on a reply (a server that answers
//! anything is a versioned server, and `Malformed` still means what it
//! did). The cost is one extra connect when a server genuinely dies
//! under the first frame — the second attempt then fails the same way
//! and that error is the one returned. [`ConnectOptions::require_hello`]
//! turns the fallback off for a caller that would rather see the EOF.
//!
//! # Authentication (`AuthConfig`), `SERVER-001-FR-021`
//!
//! An `AuthConfig`-configured server rejects every request but
//! `Authenticate` — `DescribeSchema` included (`AUTH-FR-002`) — until a
//! recognized token is presented, so on such a server the schema fetch
//! [`SchemaDrivenClient::connect`] does cannot succeed and `connect` fails
//! with `ClientError::Server(ErrorCode::Unauthenticated, ..)`. That is why
//! the token goes into a *constructor*,
//! [`SchemaDrivenClient::connect_authenticated`], which sends
//! `Request::Authenticate` between the `Hello` and the `DescribeSchema`,
//! rather than only into a post-connect method. The method exists too —
//! [`SchemaDrivenClient::authenticate`] re-presents a token on an already
//! usable connection (changing a `ReadOnly` connection to `ReadWrite`,
//! for instance, or a no-op on a server with no tokens configured,
//! `AUTH-FR-007`). Neither learns the granted [`super::TokenClass`]: the
//! server answers `Authenticate` with a bare `Response::Ok`, and a wrong
//! token is `ErrorCode::Unauthenticated`, indistinguishable from never
//! having authenticated (`AUTH-FR-001`). Against a `TlsConfig`-configured
//! server the token has to travel inside TLS — see the next section; a
//! plaintext `connect_authenticated` there fails at the `Hello`, before
//! any token is written, so it never leaks the token.
//!
//! # Transaction sessions (`Begin`/`Commit`/`Rollback`), `SERVER-001-FR-024`
//!
//! [`SchemaDrivenClient::begin`] opens a server-side session (ADR-0024,
//! `docs/design/SERVER-TRANSACTION-SESSION-DESIGN.md` Part A) and hands
//! back a [`Session`]: [`Session::update`] *stages* a write — the server
//! answers with its index in the eventual batch and applies nothing —
//! and [`Session::commit`] applies every staged write as one batch,
//! exactly the all-or-nothing `Request::Transaction` gives, or fails as
//! [`ClientError::TransactionFailed`] naming that index with nothing
//! applied; [`Session::rollback`] discards, and dropping a `Session`
//! without either sends a best-effort `Rollback`. No lock is held on the
//! server between round trips, so a stalled session costs nobody else
//! anything. Reads made while a session is open — through the client
//! that owns it or any other — see committed state only; a write is
//! never visible before `commit` returns `Ok`. The session requests are
//! protocol version 3, so `begin` is the client's first version-gated
//! API (compatibility rule 4): against a server that negotiated below 3
//! it is [`ClientError::Unsupported`] with no frame sent.
//!
//! [`SchemaDrivenClient::begin_read_your_writes`] (`SERVER-001-FR-028`,
//! ADR-0027, protocol version 5) opens the one exception: on that
//! session, [`Session::get`] — the session's own point read — returns
//! the committed record with the session's staged writes laid over it
//! (last write per field wins; a write the server would refuse at commit
//! is not shown). Scans, filters, and every other connection still see
//! committed state. [`Session::get`] exists on every session, since a
//! `Session` borrows the client mutably and no other read is reachable
//! while one is open; on a plain session it reads committed state.
//!
//! [`SchemaDrivenClient::begin_with`] takes [`SessionOptions`] — the
//! same read-your-writes, and (`SERVER-001-FR-030`, protocol version 6)
//! `validate_on_stage`: each [`Session::update`] is validated by the
//! server as it is staged and refused with the code `commit` would have
//! reported, nothing staged, so a bad write is learned at its own round
//! trip rather than by index at `commit`.
//!
//! # Transport encryption (`TlsConfig`), `SERVER-001-FR-022`
//!
//! A `TlsConfig`-configured server completes a TLS handshake before it
//! reads a single frame (`TLS-FR-002`), so a plaintext client's `Hello`
//! is garbage to it and the connection is closed — `connect` fails with
//! `ClientError::Frame(..)`. [`SchemaDrivenClient::connect_with`] takes
//! [`ConnectOptions`], whose [`ConnectOptions::tls`] carries a
//! [`ClientTlsConfig`]: the server name the certificate must match (and
//! the SNI sent) plus a `rusty_tls` [`TrustPolicy`] — the OS trust store,
//! pinned anchors, or, for a throwaway self-signed certificate,
//! `DangerNoVerification` — exactly the explicit client-side trust
//! configuration ADR-0014's Consequences said a self-signed certificate
//! would need. The client-side half is `rusty_tls::TlsStream`, the same
//! ecosystem-wide `rustls` wrapper the server's `TlsConfig` uses, so this
//! crate still never touches `rustls` directly (ADR-0014's seam); the
//! `TrustPolicy` type is re-exported here so a caller does not have to
//! depend on `rusty_tls` to name one. `rusty_tls::TlsStream::new` performs
//! no I/O — the handshake runs lazily under the first frame — so a policy
//! or server-name the library rejects outright is
//! [`ClientError::Tls`] before anything is sent, while a certificate the
//! policy rejects, or a server that does not speak TLS at all, surfaces
//! under the `Hello` as `ClientError::Frame(FrameError::Io(..))`. Named,
//! not mitigated: a TLS client against a *plaintext* server can block
//! rather than fail, since the server reads the ClientHello as a
//! length-prefixed frame and waits for a payload the client never sends —
//! the same for any `rusty_tls` client, not specific to this one. Every
//! request after the handshake, `Authenticate` included, travels
//! encrypted, and nothing else about the client changes: the `Hello`,
//! the token, the schema fetch, and every method are transport-agnostic
//! (the private `Transport` enum is the only place the two differ).

use super::framing::{self, FrameError};
use super::protocol::{
    DomainSchema, ErrorCode, FieldDescriptor, ParentLookup, RecordId, Request, Response, ScanValue,
    PROTOCOL_VERSION, SESSION_READ_YOUR_WRITES, SESSION_VALIDATE_ON_STAGE,
};
use super::{pem, TlsConfigError};
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;

/// The client-side trust policy for [`ClientTlsConfig`], re-exported
/// from `rusty_tls` so a caller can name one without depending on
/// `rusty_tls` itself (`FR-022`).
pub use rusty_tls::TrustPolicy;

/// Everything that can go wrong driving a [`SchemaDrivenClient`]: framing/
/// I/O failure, a field name the discovered schema doesn't have, an
/// operation this domain's schema doesn't support (checked locally before
/// sending — see this module's own doc comment), the server's own typed
/// [`ErrorCode`], or a response shape that doesn't match what the request
/// kind should have produced (a `dispatch` bug, not something a correct
/// server should ever send).
#[derive(Debug)]
pub enum ClientError {
    Frame(FrameError),
    /// `rusty_tls` rejected the [`ClientTlsConfig`] before any I/O — an
    /// invalid server name, or a [`TrustPolicy`] that could not be built
    /// (no usable OS trust anchors, for instance). Handshake failures
    /// happen later, under the first frame, and are `Frame(Io(..))`; see
    /// this module's own "Transport encryption" section (`FR-022`).
    Tls(rusty_tls::Error),
    UnknownField(String),
    Unsupported(&'static str),
    Server(ErrorCode, String),
    /// A session's [`Session::commit`] was rejected: `index` names the
    /// first staged write that failed its precondition (the value
    /// [`Session::update`] returned for it) and nothing was applied
    /// (`SESS-FR-002`, `FR-024`).
    TransactionFailed {
        index: usize,
        code: ErrorCode,
        message: String,
    },
    UnexpectedResponse(&'static str),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Frame(e) => write!(f, "framing error: {e}"),
            ClientError::Tls(e) => write!(f, "tls configuration error: {e}"),
            ClientError::UnknownField(name) => {
                write!(f, "no field named {name:?} in this domain's schema")
            }
            ClientError::Unsupported(what) => {
                write!(f, "{what} is not supported by this domain's schema")
            }
            ClientError::Server(code, message) => write!(f, "server error {code:?}: {message}"),
            ClientError::TransactionFailed {
                index,
                code,
                message,
            } => write!(
                f,
                "transaction rejected at staged write {index}: {code:?}: {message}"
            ),
            ClientError::UnexpectedResponse(expected) => {
                write!(f, "expected a {expected} response, got a different shape")
            }
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Frame(e) => Some(e),
            ClientError::Tls(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        ClientError::Frame(e)
    }
}

/// How [`SchemaDrivenClient::connect_with`] should reach a server whose
/// `serve` was given a `TlsConfig` (`FR-022`; see this module's own
/// "Transport encryption" section): the name the server's certificate
/// must carry (also sent as SNI) and the [`TrustPolicy`] it is verified
/// under. A self-signed development certificate needs
/// `TrustPolicy::DangerNoVerification` or `PinnedAnchors`; a real one
/// works under `TrustPolicy::System`. Against a server built with
/// `TlsConfig::new_with_client_auth` (mutual TLS, `FR-023`, ADR-0023)
/// the client must also present an identity — see
/// [`ClientTlsConfig::with_identity`].
#[derive(Clone)]
pub struct ClientTlsConfig {
    server_name: String,
    trust: TrustPolicy,
    /// `MTLS-FR-003`: a DER certificate chain (leaf first) and DER private
    /// key presented during the handshake. Never printed — see the
    /// `Debug` impl.
    identity: Option<(Vec<Vec<u8>>, Vec<u8>)>,
}

/// Hand-written so the identity's private key never reaches a log:
/// prints whether an identity is present, not its bytes (the same
/// never-echo-a-secret rule `AuthConfig` follows for tokens).
impl fmt::Debug for ClientTlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientTlsConfig")
            .field("server_name", &self.server_name)
            .field("trust", &self.trust)
            .field("identity", &self.identity.is_some())
            .finish()
    }
}

impl ClientTlsConfig {
    pub fn new(server_name: impl Into<String>, trust: TrustPolicy) -> Self {
        Self {
            server_name: server_name.into(),
            trust,
            identity: None,
        }
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn trust(&self) -> &TrustPolicy {
        &self.trust
    }

    /// Present this DER-encoded certificate chain (leaf first) and DER
    /// private key (PKCS#8, PKCS#1, or SEC1) during the handshake
    /// (`MTLS-FR-003`, `FR-023`) — what a server built with
    /// `TlsConfig::new_with_client_auth` requires. Mechanism:
    /// `rusty_tls::TlsStream::new_with_client_identity`. A key that does
    /// not match the certificate, or malformed DER, is
    /// [`ClientError::Tls`] at connect time before any I/O; an identity
    /// the server's roots reject fails under the `Hello` as
    /// `ClientError::Frame(FrameError::Io(..))`, like any other handshake
    /// failure. Against a server that does not require a certificate the
    /// identity is simply never asked for.
    pub fn with_identity(mut self, cert_chain_der: Vec<Vec<u8>>, private_key_der: Vec<u8>) -> Self {
        self.identity = Some((cert_chain_der, private_key_der));
        self
    }

    /// [`ClientTlsConfig::with_identity`] from PEM files — the chain file
    /// may hold the leaf followed by intermediates, the key file exactly
    /// one block — decoded by the same `pem` module and reporting the same
    /// [`TlsConfigError`] shapes (`Io`, `Pem`) as `TlsConfig::from_pem_files`.
    pub fn with_identity_pem_files(
        self,
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
        Ok(self.with_identity(cert_chain_der, private_key_der))
    }

    /// Whether an identity will be presented — see [`ClientTlsConfig::with_identity`].
    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }
}

/// Everything [`SchemaDrivenClient::connect_with`] can be told beyond the
/// address: an `Authenticate` token (`FR-021`) and a [`ClientTlsConfig`]
/// (`FR-022`), each optional and independent. `ConnectOptions::new()`
/// (or `default()`) is exactly [`SchemaDrivenClient::connect`].
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    token: Option<String>,
    tls: Option<ClientTlsConfig>,
    /// `FR-026`: when set, a pre-hello server is an error, not a silent
    /// reconnect. Default off — the fallback is on.
    require_hello: bool,
}

impl ConnectOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Present `token` between the `Hello` and the `DescribeSchema`, as
    /// [`SchemaDrivenClient::connect_authenticated`] does.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Complete a TLS handshake (as `tls` describes) before the `Hello`.
    pub fn tls(mut self, tls: ClientTlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Disable the pre-hello fallback (`FR-026`; see this module's own
    /// "Protocol version" section): a server that closes the connection
    /// under the `Hello` is then reported as
    /// `ClientError::Frame(FrameError::Io(..))`, as it was before v0.16.0,
    /// instead of being reconnected to without a `Hello`.
    pub fn require_hello(mut self) -> Self {
        self.require_hello = true;
        self
    }
}

/// The one place plaintext and TLS differ. `rusty_tls::TlsStream` cannot
/// be split into independent read/write halves the way `TcpStream::try_clone`
/// allows (the same finding `src/server/mod.rs`'s `ReadHalf`/`WriteHalf`
/// resolved server-side), so the client keeps a single stream and writes
/// each frame through the `BufReader`'s `get_mut` — a request/response
/// protocol never reads and writes at once, and each frame is assembled
/// into one buffer first so the stream sees one `write_all` per request,
/// exactly what the pre-v0.12.0 `BufWriter` + `flush` produced.
enum Transport {
    Plain(TcpStream),
    Tls(Box<rusty_tls::TlsStream<TcpStream>>),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buf),
            Transport::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buf),
            Transport::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            Transport::Tls(s) => s.flush(),
        }
    }
}

/// An open transaction session on a [`SchemaDrivenClient`] (`FR-024`,
/// ADR-0024) — see this module's own "Transaction sessions" section.
/// Borrows the client for its lifetime; [`Session::commit`] and
/// [`Session::rollback`] consume it, and `Drop` rolls back a session
/// neither was called on (best effort: an I/O error there is ignored,
/// since the server discards the session on disconnect anyway).
pub struct Session<'a> {
    client: &'a mut SchemaDrivenClient,
    open: bool,
    read_your_writes: bool,
    validate_on_stage: bool,
}

/// How to open a session with [`SchemaDrivenClient::begin_with`]: each
/// option is a `Request::BeginWith` flag bit and is gated on the protocol
/// version that introduced it (compatibility rule 4) — `read_your_writes`
/// on 5 (`FR-028`), `validate_on_stage` on 6 (`FR-030`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionOptions {
    read_your_writes: bool,
    validate_on_stage: bool,
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The session's own point reads see its staged writes (`RYW-FR-002`).
    pub fn read_your_writes(mut self) -> Self {
        self.read_your_writes = true;
        self
    }

    /// Each write is validated as it is staged and refused with the code
    /// `commit` would have reported, nothing staged (`STV-FR-001`).
    pub fn validate_on_stage(mut self) -> Self {
        self.validate_on_stage = true;
        self
    }

    fn flags(self) -> u32 {
        (if self.read_your_writes {
            SESSION_READ_YOUR_WRITES
        } else {
            0
        }) | (if self.validate_on_stage {
            SESSION_VALIDATE_ON_STAGE
        } else {
            0
        })
    }

    /// The protocol version the chosen options need.
    fn required_version(self) -> u32 {
        if self.validate_on_stage {
            6
        } else if self.read_your_writes {
            5
        } else {
            3
        }
    }
}

impl Session<'_> {
    /// Stage a write (`SESS-FR-002`): the same client-side capability
    /// checks as [`SchemaDrivenClient::update`], then `Request::UpdateField`
    /// answered `Response::Staged` — the returned index is the write's
    /// position in the batch [`Session::commit`] will apply, and the one a
    /// `TransactionFailed` would name. Nothing is applied here; the
    /// server validates at commit. `ErrorCode::SessionFull` once
    /// `MAX_STAGED_OPS` are staged (the session stays open).
    pub fn update(
        &mut self,
        id: RecordId,
        field_name: &str,
        value: ScanValue,
    ) -> Result<u32, ClientError> {
        let field = self.client.field(field_name)?;
        if !field.capabilities.update {
            return Err(ClientError::Unsupported("update on this field"));
        }
        let tag = field.tag;
        match self.client.roundtrip(Request::UpdateField {
            id,
            field: tag,
            value,
        })? {
            Response::Staged { index } => Ok(index),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Staged")),
        }
    }

    /// Apply every staged write as one all-or-nothing batch and close the
    /// session. `Ok(())` means every write is now visible to every
    /// connection; [`ClientError::TransactionFailed`] means none is.
    pub fn commit(mut self) -> Result<(), ClientError> {
        self.open = false;
        match self.client.roundtrip(Request::Commit)? {
            Response::Ok => Ok(()),
            Response::TransactionFailed {
                index,
                code,
                message,
            } => Err(ClientError::TransactionFailed {
                index,
                code,
                message,
            }),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Ok or TransactionFailed")),
        }
    }

    /// Full-record read through this session (`RYW-FR-007`): on a
    /// session opened by [`SchemaDrivenClient::begin_read_your_writes`]
    /// the record carries this session's staged writes (`RYW-FR-002`);
    /// on a plain [`SchemaDrivenClient::begin`] session it is committed
    /// state, exactly [`SchemaDrivenClient::get`]. Fields come back named.
    pub fn get(&mut self, id: RecordId) -> Result<Option<Vec<(String, ScanValue)>>, ClientError> {
        self.client.get(id)
    }

    /// Whether this session was opened with read-your-writes.
    pub fn read_your_writes(&self) -> bool {
        self.read_your_writes
    }

    /// Whether this session validates each write as it is staged — on
    /// such a session [`Session::update`] returns [`ClientError::Server`]
    /// with the code `commit` would have reported, and stages nothing.
    pub fn validate_on_stage(&self) -> bool {
        self.validate_on_stage
    }

    /// Discard every staged write and close the session.
    pub fn rollback(mut self) -> Result<(), ClientError> {
        self.open = false;
        let resp = self.client.roundtrip(Request::Rollback)?;
        SchemaDrivenClient::expect_ok(resp)
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.client.roundtrip(Request::Rollback);
        }
    }
}

/// A real client to one server/query-layer domain, built entirely from
/// what [`Request::DescribeSchema`] reports at connect time — see this
/// module's own doc comment.
pub struct SchemaDrivenClient {
    stream: BufReader<Transport>,
    schema: DomainSchema,
    server_protocol_version: u32,
}

impl SchemaDrivenClient {
    /// Connects, disables Nagle's algorithm (`SERVER-001-FR-006` — see
    /// `src/server/mod.rs`'s own doc comment for why this isn't optional
    /// for this protocol's synchronous request/response shape), sends
    /// `Request::Hello` as the very first frame and keeps the negotiated
    /// protocol version (`PROTO-FR-007`; see this module's own doc
    /// comment for what a pre-hello server looks like from here), then
    /// immediately sends `Request::DescribeSchema` and keeps the result
    /// for every subsequent field-name lookup this client does.
    ///
    /// Against an `AuthConfig`-configured server this fails with
    /// `ClientError::Server(ErrorCode::Unauthenticated, ..)` — the schema
    /// fetch itself is gated (`AUTH-FR-002`); use
    /// [`SchemaDrivenClient::connect_authenticated`] there.
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self, ClientError> {
        Self::connect_with(addr, ConnectOptions::new())
    }

    /// [`SchemaDrivenClient::connect`] with `Request::Authenticate { token }`
    /// sent between the `Hello` and the `DescribeSchema` (`FR-021`), so the
    /// schema fetch runs on an authenticated connection. A token the
    /// server does not recognize fails here with
    /// `ClientError::Server(ErrorCode::Unauthenticated, ..)`; on a server
    /// with no tokens configured the `Authenticate` is a no-op
    /// (`AUTH-FR-007`) and this is exactly `connect`. See this module's
    /// own "Authentication" doc section.
    pub fn connect_authenticated<A: ToSocketAddrs>(
        addr: A,
        token: &str,
    ) -> Result<Self, ClientError> {
        Self::connect_with(addr, ConnectOptions::new().token(token))
    }

    /// The general constructor (`FR-022`): [`SchemaDrivenClient::connect`]
    /// and [`SchemaDrivenClient::connect_authenticated`] are this with
    /// `ConnectOptions::new()` and `ConnectOptions::new().token(..)`.
    /// With [`ConnectOptions::tls`] set, the TCP connection is wrapped in
    /// a `rusty_tls::TlsStream` first, so the `Hello`, any token, the
    /// schema fetch, and every later request travel encrypted; the
    /// handshake itself runs under the `Hello` (see this module's own
    /// "Transport encryption" section for how each failure surfaces).
    pub fn connect_with<A: ToSocketAddrs>(
        addr: A,
        options: ConnectOptions,
    ) -> Result<Self, ClientError> {
        let mut stream = Self::open_transport(&addr, &options)?;

        let server_protocol_version = match Self::exchange(
            &mut stream,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        ) {
            Ok(Response::Hello { protocol_version }) => protocol_version,
            Ok(_) => return Err(ClientError::UnexpectedResponse("Hello")),
            // `FR-026`: the peer closed the connection under the `Hello`
            // with no reply — what a pre-hello server does. Reconnect
            // once, say nothing, and speak version 1 (see this module's
            // own "Protocol version" section for the heuristic's bounds).
            Err(ClientError::Frame(FrameError::Io(e)))
                if !options.require_hello && Self::peer_closed(&e) =>
            {
                stream = Self::open_transport(&addr, &options)?;
                1
            }
            Err(e) => return Err(e),
        };

        if let Some(token) = options.token {
            Self::expect_ok(Self::exchange(
                &mut stream,
                &Request::Authenticate { token },
            )?)?;
        }

        let schema = match Self::exchange(&mut stream, &Request::DescribeSchema)? {
            Response::Schema(schema) => schema,
            Response::Err { code, message } => return Err(ClientError::Server(code, message)),
            _ => return Err(ClientError::UnexpectedResponse("Schema")),
        };

        Ok(Self {
            stream,
            schema,
            server_protocol_version,
        })
    }

    /// The TCP connection, `TCP_NODELAY`, and the TLS wrap `options`
    /// asks for — everything before the first frame. Called once, or
    /// twice when the `FR-026` fallback reconnects.
    fn open_transport<A: ToSocketAddrs>(
        addr: &A,
        options: &ConnectOptions,
    ) -> Result<BufReader<Transport>, ClientError> {
        let tcp = TcpStream::connect(addr).map_err(FrameError::from)?;
        tcp.set_nodelay(true).map_err(FrameError::from)?;
        let transport = match &options.tls {
            None => Transport::Plain(tcp),
            Some(tls) => {
                let stream = match &tls.identity {
                    None => rusty_tls::TlsStream::new(tcp, &tls.server_name, &tls.trust),
                    Some((chain, key)) => rusty_tls::TlsStream::new_with_client_identity(
                        tcp,
                        &tls.server_name,
                        &tls.trust,
                        chain.clone(),
                        key.clone(),
                    ),
                }
                .map_err(ClientError::Tls)?;
                Transport::Tls(Box::new(stream))
            }
        };
        Ok(BufReader::new(transport))
    }

    /// The I/O errors that mean "the peer closed on us" rather than
    /// "the network failed" — the only shape the `FR-026` fallback
    /// answers. Anything else under the `Hello` is returned as is.
    fn peer_closed(e: &io::Error) -> bool {
        matches!(
            e.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
        )
    }

    /// One request, one response: the frame is assembled in memory and
    /// written through the reader's underlying stream in a single
    /// `write_all` (see [`Transport`]), then the reply is read back.
    fn exchange(stream: &mut BufReader<Transport>, req: &Request) -> Result<Response, ClientError> {
        let mut frame = Vec::new();
        framing::write_message(&mut frame, req)?;
        let transport = stream.get_mut();
        transport.write_all(&frame).map_err(FrameError::from)?;
        transport.flush().map_err(FrameError::from)?;
        Ok(framing::read_message(stream)?)
    }

    /// The server's answer to `Request::Authenticate`, folded to the
    /// client's own error shape: `Ok` is success, a typed `Err` is the
    /// server's verdict (a wrong token is `Unauthenticated`), anything
    /// else is a `dispatch`-level surprise.
    fn expect_ok(resp: Response) -> Result<(), ClientError> {
        match resp {
            Response::Ok => Ok(()),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Ok")),
        }
    }

    /// Present `token` on this already-usable connection (`FR-021`). The
    /// server accepts `Authenticate` at any point, so this changes the
    /// connection's class for every request that follows — a `ReadOnly`
    /// connection becomes `ReadWrite` with a write token, and the reverse
    /// — and on a server with no tokens configured it is a no-op
    /// (`AUTH-FR-007`). A rejected token leaves the connection's class as
    /// it was and returns `ClientError::Server(ErrorCode::Unauthenticated,
    /// ..)`. This cannot make a fresh connection to an auth-configured
    /// server usable — [`SchemaDrivenClient::connect`] has already failed
    /// there; use [`SchemaDrivenClient::connect_authenticated`].
    pub fn authenticate(&mut self, token: &str) -> Result<(), ClientError> {
        let resp = self.roundtrip(Request::Authenticate {
            token: token.to_string(),
        })?;
        Self::expect_ok(resp)
    }

    /// The protocol version negotiated at connect time —
    /// `min(PROTOCOL_VERSION, the server's)`, so never above this build's
    /// own [`PROTOCOL_VERSION`] and never below 1. Equal to
    /// `PROTOCOL_VERSION` against a server from the same build.
    pub fn server_protocol_version(&self) -> u32 {
        self.server_protocol_version
    }

    /// Open a transaction session on this connection (`FR-024`; see this
    /// module's own "Transaction sessions" section). Requires a server
    /// that negotiated protocol version 3 or later —
    /// `ClientError::Unsupported("session")` otherwise, before any frame is
    /// sent (compatibility rule 4). The server refuses a second `Begin`
    /// while one is open (`ErrorCode::SessionOpen`); the returned
    /// [`Session`] borrows this client mutably, so that cannot happen from
    /// safe use of this API.
    pub fn begin(&mut self) -> Result<Session<'_>, ClientError> {
        if self.server_protocol_version < 3 {
            return Err(ClientError::Unsupported("session"));
        }
        let resp = self.roundtrip(Request::Begin)?;
        Self::expect_ok(resp)?;
        Ok(Session {
            client: self,
            open: true,
            read_your_writes: false,
            validate_on_stage: false,
        })
    }

    /// Open a transaction session with [`SessionOptions`] (`FR-028`,
    /// `FR-030`) — `Request::BeginWith { flags }`. Each option is gated on
    /// the protocol version that introduced it:
    /// `ClientError::Unsupported("session options")` with no frame sent
    /// when the server negotiated below it (compatibility rule 4). With no
    /// options this is [`SchemaDrivenClient::begin`].
    pub fn begin_with(&mut self, options: SessionOptions) -> Result<Session<'_>, ClientError> {
        if self.server_protocol_version < options.required_version() {
            return Err(ClientError::Unsupported("session options"));
        }
        let flags = options.flags();
        let resp = if flags == 0 {
            self.roundtrip(Request::Begin)?
        } else {
            self.roundtrip(Request::BeginWith { flags })?
        };
        Self::expect_ok(resp)?;
        Ok(Session {
            client: self,
            open: true,
            read_your_writes: options.read_your_writes,
            validate_on_stage: options.validate_on_stage,
        })
    }

    /// Open a transaction session whose own point reads see its staged
    /// writes (`FR-028`, ADR-0027; see this module's own "Transaction
    /// sessions" section) — `Request::BeginWith { SESSION_READ_YOUR_WRITES }`.
    /// Requires a server that negotiated protocol version 5 or later —
    /// `ClientError::Unsupported("read-your-writes session")` otherwise,
    /// before any frame is sent (compatibility rule 4). Everything else is
    /// [`SchemaDrivenClient::begin`].
    pub fn begin_read_your_writes(&mut self) -> Result<Session<'_>, ClientError> {
        if self.server_protocol_version < 5 {
            return Err(ClientError::Unsupported("read-your-writes session"));
        }
        let resp = self.roundtrip(Request::BeginWith {
            flags: SESSION_READ_YOUR_WRITES,
        })?;
        Self::expect_ok(resp)?;
        Ok(Session {
            client: self,
            open: true,
            read_your_writes: true,
            validate_on_stage: false,
        })
    }

    /// The schema discovered at connect time — every field's name, wire
    /// type, and per-operation capability, plus which relation kinds this
    /// domain supports.
    pub fn schema(&self) -> &DomainSchema {
        &self.schema
    }

    fn field(&self, name: &str) -> Result<&FieldDescriptor, ClientError> {
        self.schema
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| ClientError::UnknownField(name.to_string()))
    }

    fn roundtrip(&mut self, req: Request) -> Result<Response, ClientError> {
        Self::exchange(&mut self.stream, &req)
    }

    /// Full-record read, `None` if `id` has no record. Fields come back
    /// named, not tagged — `(name, value)` pairs, in whatever order the
    /// server returned them (fixed per adapter, not re-sorted here).
    pub fn get(&mut self, id: RecordId) -> Result<Option<Vec<(String, ScanValue)>>, ClientError> {
        match self.roundtrip(Request::GetById { id })? {
            Response::Record { fields, .. } => {
                let named = fields
                    .into_iter()
                    .map(|(tag, value)| {
                        let name = self
                            .schema
                            .fields
                            .iter()
                            .find(|f| f.tag == tag)
                            .map(|f| f.name.clone())
                            .unwrap_or_else(|| tag.to_string());
                        (name, value)
                    })
                    .collect();
                Ok(Some(named))
            }
            Response::NotFound => Ok(None),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Record or NotFound")),
        }
    }

    /// Equality filter by field name. Checked against the discovered
    /// schema first — `Err(ClientError::Unsupported)`, no round trip, if
    /// this field isn't `filter_eq`-capable.
    pub fn filter_eq(
        &mut self,
        field_name: &str,
        value: ScanValue,
    ) -> Result<Vec<RecordId>, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.filter_eq {
            return Err(ClientError::Unsupported("filter_eq on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::FilterEq { field: tag, value })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }

    /// Every record's value for a scannable field, by name.
    pub fn scan(&mut self, field_name: &str) -> Result<Vec<ScanValue>, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.scan {
            return Err(ClientError::Unsupported("scan on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::ScanField { field: tag })? {
            Response::ScanValues { values } => Ok(values),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("ScanValues")),
        }
    }

    /// `Ok(true)` if `id` was found and updated, `Ok(false)` if `id` has
    /// no record.
    pub fn update(
        &mut self,
        id: RecordId,
        field_name: &str,
        value: ScanValue,
    ) -> Result<bool, ClientError> {
        let field = self.field(field_name)?;
        if !field.capabilities.update {
            return Err(ClientError::Unsupported("update on this field"));
        }
        let tag = field.tag;
        match self.roundtrip(Request::UpdateField {
            id,
            field: tag,
            value,
        })? {
            Response::Ok => Ok(true),
            Response::NotFound => Ok(false),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Ok or NotFound")),
        }
    }

    /// The directed relation's "one hop up" — see [`ParentLookup`]'s own
    /// doc comment for the three-way not-found/no-parent/parent
    /// distinction this preserves. `Err(ClientError::Unsupported)`
    /// locally if this domain has no directed relation at all.
    pub fn parent(&mut self, id: RecordId) -> Result<ParentLookup, ClientError> {
        if !self.schema.relations.parent_children {
            return Err(ClientError::Unsupported("Parent on this domain"));
        }
        match self.roundtrip(Request::Parent { id })? {
            Response::Id { id } => Ok(ParentLookup::Parent(id)),
            Response::NoParent => Ok(ParentLookup::NoParent),
            Response::NotFound => Ok(ParentLookup::ChildNotFound),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("Id, NoParent, or NotFound")),
        }
    }

    /// The directed relation's "one hop down".
    pub fn children(&mut self, id: RecordId) -> Result<Vec<RecordId>, ClientError> {
        if !self.schema.relations.parent_children {
            return Err(ClientError::Unsupported("Children on this domain"));
        }
        match self.roundtrip(Request::Children { id })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }

    /// The symmetric relation.
    pub fn neighbors(&mut self, id: RecordId) -> Result<Vec<RecordId>, ClientError> {
        if !self.schema.relations.neighbors {
            return Err(ClientError::Unsupported("Neighbors on this domain"));
        }
        match self.roundtrip(Request::Neighbors { id })? {
            Response::RecordList { records } => Ok(records),
            Response::Err { code, message } => Err(ClientError::Server(code, message)),
            _ => Err(ClientError::UnexpectedResponse("RecordList")),
        }
    }
}
