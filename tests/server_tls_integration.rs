//! Real end-to-end coverage of `TlsConfig` (`docs/design/SERVER-TLS-DESIGN.md`,
//! ADR-0014, Accepted) against the `Dog` domain — a real `TcpListener`, a
//! real TLS handshake (via `rusty_tls::TlsStream`, the client-side half of
//! the same ecosystem-wide `rustls` wrapper `TlsConfig` uses server-side —
//! not a hand-rolled test client), real `bincode` framing over the
//! encrypted channel. This suite's own job is proving `rusty_multimodal_db`'s
//! wiring is correct, not re-proving TLS itself: `rusty_tls` already
//! carries its own hermetic rejection-path suite (wrong hostname, expired
//! cert, untrusted root, a real OS-trust-anchor corpus test) — see
//! `docs/design/SERVER-TLS-DESIGN.md`'s own "Verification plan" for why
//! this suite doesn't duplicate that coverage.
//!
//! The test certificate is a throwaway, self-signed leaf generated fresh
//! per test via `rcgen` (a dev-only dependency, never shipped — see
//! `Cargo.toml`'s own comment on why this matches `rusty_tls`'s own
//! precedent rather than a committed certificate or shelling out to
//! `openssl`). The test client trusts it via `TrustPolicy::DangerNoVerification`
//! — the same policy `rusty_tls`'s own self-signed-leaf tests use — not
//! `TrustPolicy::PinnedAnchors`, since constructing a `rustls::pki_types::CertificateDer`
//! for that variant would mean this crate's own test code depending on
//! `rustls` directly, exactly the "no consumer rolls its own TLS" seam
//! ADR-0014 exists to preserve.

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::audit::{AuditEvent, AuditKind, AuditSink, Transport};
use rusty_multimodal_db::server::client::{
    ClientError, ClientTlsConfig, ConnectOptions, SchemaDrivenClient,
};
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    ErrorCode, Request, Response, ScanValue, PROTOCOL_VERSION,
};
use rusty_multimodal_db::server::{serve, ServeOptions, TlsConfig, TokenClass};
use rusty_multimodal_db::ProductionStore;
use rusty_tls::{TlsStream, TrustPolicy};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s own `unique_dir` for why this
/// needs both the process id and a monotonic counter, not just one.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

/// A throwaway, self-signed leaf certificate for `localhost` — DER-encoded
/// certificate + DER-encoded private key, exactly the shape
/// `TlsConfig::new` takes. Generated fresh per call; nothing here is
/// committed to the repository.
fn self_signed_leaf() -> (Vec<u8>, Vec<u8>) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (cert.der().to_vec(), key_pair.serialize_der())
}

fn start_server(auth: ServeOptions, tls: Option<TlsConfig>) -> std::net::SocketAddr {
    let dir = unique_dir("server_tls_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let records = vec![DogRecord::new(Uuid::from_u128(1), "labrador", 3)];
    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let options = match tls {
        Some(tls) => auth.with_tls(tls),
        None => auth,
    };
    thread::spawn(move || serve(listener, connection_store, options));
    addr
}

fn connect_tls(addr: std::net::SocketAddr) -> TlsStream<TcpStream> {
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_nodelay(true).unwrap();
    TlsStream::new(tcp, "localhost", &TrustPolicy::DangerNoVerification).unwrap()
}

fn roundtrip<S: Read + Write>(stream: &mut S, req: Request) -> Response {
    write_message(stream, &req).unwrap();
    read_message(stream).unwrap()
}

/// `TLS-FR-002`: a real client, trusting the server's self-signed cert,
/// completes a full request/response round trip over TLS identically to
/// today's plaintext behavior — `dispatch`/`ConnectionStore` genuinely
/// don't need to know transport encryption exists.
#[test]
fn a_real_client_completes_a_request_response_round_trip_over_tls() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let addr = start_server(ServeOptions::default(), Some(tls));

    let mut client = connect_tls(addr);
    assert_eq!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record {
            id: Uuid::from_u128(1),
            fields: vec![
                (
                    rusty_multimodal_db::server::dog::FIELD_BREED,
                    ScanValue::Str("labrador".into())
                ),
                (FIELD_AGE, ScanValue::U32(3)),
            ],
        }
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(9),
            }
        ),
        Response::Ok
    );
    assert!(matches!(
        roundtrip(&mut client, Request::DescribeSchema),
        Response::Schema(_)
    ));
}

/// `TLS-FR-007`: composed with `ServeOptions`, `Authenticate`'s token now
/// travels over the encrypted channel — the handshake always completes
/// before any framed `Request` is ever read, so authenticating over TLS
/// works exactly like authenticating in plaintext.
#[test]
fn authentication_composes_with_tls() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let auth = ServeOptions::new(None, Some("write-token".into()));
    let addr = start_server(auth, Some(tls));

    let mut client = connect_tls(addr);
    assert_eq!(
        roundtrip(
            &mut client,
            Request::Authenticate {
                token: "write-token".into()
            }
        ),
        Response::Ok
    );
    assert_eq!(
        roundtrip(
            &mut client,
            Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_AGE,
                value: ScanValue::U32(9),
            }
        ),
        Response::Ok
    );
}

/// A `TcpStream` wrapper that records every byte actually written to the
/// underlying socket — used below to capture the real bytes a TLS client
/// puts on the wire, so the token-secrecy test has direct evidence rather
/// than trusting "we used `rusty_tls` so it must be fine."
struct RecordingStream {
    inner: TcpStream,
    sent: Arc<Mutex<Vec<u8>>>,
}

impl Read for RecordingStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for RecordingStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.sent.lock().unwrap().extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The actual evidence this design's whole purpose depends on
/// (`SERVER-TLS-DESIGN.md`'s own "Acceptance criteria"): `Request::Authenticate`'s
/// token, captured directly off the wire (the real bytes written to the
/// TCP socket, post-TLS-encryption), is not present anywhere in that
/// capture — not "we used `rusty_tls` so it must be fine."
#[test]
fn the_authenticate_token_is_not_observable_in_the_bytes_sent_on_the_wire() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let auth = ServeOptions::new(None, Some("super-secret-token-xyz123".into()));
    let addr = start_server(auth, Some(tls));

    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_nodelay(true).unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let recording = RecordingStream {
        inner: tcp,
        sent: Arc::clone(&sent),
    };
    let mut client =
        TlsStream::new(recording, "localhost", &TrustPolicy::DangerNoVerification).unwrap();

    assert_eq!(
        roundtrip(
            &mut client,
            Request::Authenticate {
                token: "super-secret-token-xyz123".into()
            }
        ),
        Response::Ok
    );

    let captured = sent.lock().unwrap();
    assert!(
        !captured
            .windows(b"super-secret-token-xyz123".len())
            .any(|w| w == b"super-secret-token-xyz123"),
        "the plaintext token was found in the raw bytes sent on the wire — TLS did not encrypt it"
    );
}

/// `TLS-FR-002`/`TLS-FR-003`: a real client connecting to a `TlsConfig`-configured
/// server over a *plain* (non-TLS) socket never gets a valid response —
/// TLS is genuinely enforced, not merely offered. Sending a raw `bincode`
/// frame looks like garbage handshake bytes to the server's TLS layer, so
/// the connection ends (a framing/decode error), not a `Response`.
#[test]
fn a_plain_connection_to_a_tls_configured_server_never_gets_a_valid_response() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let addr = start_server(ServeOptions::default(), Some(tls));

    let mut plain = TcpStream::connect(addr).unwrap();
    plain.set_nodelay(true).unwrap();
    write_message(
        &mut plain,
        &Request::GetById {
            id: Uuid::from_u128(1),
        },
    )
    .unwrap();
    let result: Result<Response, _> = read_message(&mut plain);
    assert!(
        result.is_err(),
        "a plaintext request against a TLS-configured server unexpectedly decoded as a real Response: {result:?}"
    );
}

/// `TLS-FR-008`: a server started with no `TlsConfig` behaves identically
/// to every other integration test in this suite — plaintext, no
/// handshake required. Already exercised broadly by every other
/// `tests/server_*_integration.rs` file (all pass `None`); this is the one
/// direct side-by-side check in this file specifically.
#[test]
fn no_tls_config_reproduces_plaintext_behavior() {
    let addr = start_server(ServeOptions::default(), None);
    let mut client = TcpStream::connect(addr).unwrap();
    client.set_nodelay(true).unwrap();
    assert!(matches!(
        roundtrip(
            &mut client,
            Request::GetById {
                id: Uuid::from_u128(1)
            }
        ),
        Response::Record { .. }
    ));
}

fn dev_tls() -> ClientTlsConfig {
    ClientTlsConfig::new("localhost", TrustPolicy::DangerNoVerification)
}

/// `SERVER-001-FR-022`: `SchemaDrivenClient` reaches a `TlsConfig`-configured
/// server through `ConnectOptions::tls` — the `Hello` negotiates the same
/// `PROTOCOL_VERSION`, the schema is fetched, and reads, writes, and
/// scans all work over the encrypted channel exactly as they do in
/// plaintext (`tests/server_schema_driven_client.rs`).
#[test]
fn schema_driven_client_connects_over_tls() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let addr = start_server(ServeOptions::default(), Some(tls));

    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(dev_tls())).unwrap();
    assert_eq!(client.server_protocol_version(), PROTOCOL_VERSION);
    assert!(client.schema().fields.iter().any(|f| f.name == "age"));

    let record = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(record.contains(&("age".to_string(), ScanValue::U32(3))));
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(9)]);
}

/// `SERVER-001-FR-022` composed with `FR-021` (`TLS-FR-007`): the token
/// travels inside TLS. A plaintext `connect_authenticated` against the
/// TLS server fails at the `Hello`, before the token is written; a TLS
/// connection with no token is the server's own `Unauthenticated`; a TLS
/// connection with a read token reads but cannot write; `authenticate`
/// promotes it mid-connection exactly as in plaintext.
#[test]
fn schema_driven_client_authenticates_over_tls() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let auth = ServeOptions::new(Some("read-token".into()), Some("write-token".into()));
    let addr = start_server(auth, Some(tls));

    match SchemaDrivenClient::connect_authenticated(addr, "write-token").map(|_| ()) {
        Err(ClientError::Frame(_)) => {}
        other => panic!("expected the plaintext Hello to fail the TLS handshake, got {other:?}"),
    }
    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(dev_tls())).map(|_| ()) {
        Err(ClientError::Server(ErrorCode::Unauthenticated, _)) => {}
        other => panic!("expected Unauthenticated over TLS without a token, got {other:?}"),
    }

    let mut client = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(dev_tls()).token("read-token"),
    )
    .unwrap();
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    match client.update(Uuid::from_u128(1), "age", ScanValue::U32(9)) {
        Err(ClientError::Server(ErrorCode::Unauthorized, _)) => {}
        other => panic!("expected Unauthorized for a read-only TLS connection, got {other:?}"),
    }
    client.authenticate("write-token").unwrap();
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(9)]);
}

/// `SERVER-001-FR-022`'s failure shapes, none of them a hang: a plain
/// `connect` against a TLS server fails under the `Hello`
/// (`ClientError::Frame`); a server name `rusty_tls` cannot parse is
/// `ClientError::Tls` before any I/O; and a self-signed certificate
/// under `TrustPolicy::System` is refused by the client at the handshake
/// (`Frame(Io(..))` from the rejected certificate, or `Tls` if this host
/// has no usable trust anchors at all) — the client never proceeds to
/// send a frame to a server it could not verify.
#[test]
fn schema_driven_client_tls_failures_are_errors_not_hangs() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let addr = start_server(ServeOptions::default(), Some(tls));

    match SchemaDrivenClient::connect(addr).map(|_| ()) {
        Err(ClientError::Frame(_)) => {}
        other => panic!("expected a plaintext connect to fail at the Hello, got {other:?}"),
    }

    let bad_name = ClientTlsConfig::new("not a server name", TrustPolicy::DangerNoVerification);
    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(bad_name)).map(|_| ()) {
        Err(ClientError::Tls(_)) => {}
        other => panic!("expected ClientError::Tls for an unparseable server name, got {other:?}"),
    }

    let system = ClientTlsConfig::new("localhost", TrustPolicy::System);
    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(system)).map(|_| ()) {
        Err(ClientError::Frame(_)) | Err(ClientError::Tls(_)) => {}
        other => panic!("expected the self-signed certificate to be refused, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Mutual TLS (`SERVER-001-FR-023`, ADR-0023, `docs/design/SERVER-MTLS-DESIGN.md`)
// ---------------------------------------------------------------------------

/// A throwaway CA (DER certificate + its key pair), generated fresh per
/// test, never committed — the operator's own CA the design assumes.
fn throwaway_ca() -> (rcgen::Certificate, rcgen::KeyPair) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    (params.self_signed(&key).unwrap(), key)
}

/// A client leaf (`ClientAuth` EKU) signed by `ca` — DER chain (just the
/// leaf) + DER key, exactly the shape `ClientTlsConfig::with_identity`
/// takes.
fn client_identity(ca: &rcgen::Certificate, ca_key: &rcgen::KeyPair) -> (Vec<Vec<u8>>, Vec<u8>) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["client".to_string()]).unwrap();
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let cert = params.signed_by(&key, ca, ca_key).unwrap();
    (vec![cert.der().to_vec()], key.serialize_der())
}

/// A server requiring a certificate chaining to `ca`.
fn mtls_server_config(ca: &rcgen::Certificate) -> TlsConfig {
    let (cert_der, key_der) = self_signed_leaf();
    let tls =
        TlsConfig::new_with_client_auth(vec![cert_der], key_der, vec![ca.der().to_vec()]).unwrap();
    assert!(tls.requires_client_certificate());
    tls
}

fn identity_client(chain: Vec<Vec<u8>>, key: Vec<u8>) -> ClientTlsConfig {
    dev_tls().with_identity(chain, key)
}

/// `MTLS-FR-001`/`MTLS-FR-003`: a client presenting a certificate signed by
/// the configured CA is admitted, and `SchemaDrivenClient` over that
/// identity reads, writes, and scans exactly as it does over plain TLS
/// (`schema_driven_client_connects_over_tls`). A raw `rusty_tls` client
/// with the same identity completes a round trip too, so the admission
/// is the server's, not something the library client adds.
#[test]
fn mtls_admits_a_client_whose_certificate_chains_to_the_configured_ca() {
    let (ca, ca_key) = throwaway_ca();
    let addr = start_server(ServeOptions::default(), Some(mtls_server_config(&ca)));

    let (chain, key) = client_identity(&ca, &ca_key);
    let mut raw = TlsStream::new_with_client_identity(
        TcpStream::connect(addr).unwrap(),
        "localhost",
        &TrustPolicy::DangerNoVerification,
        chain.clone(),
        key.clone(),
    )
    .unwrap();
    assert!(matches!(
        roundtrip(&mut raw, Request::DescribeSchema),
        Response::Schema(_)
    ));

    let tls = identity_client(chain, key);
    assert!(tls.has_identity());
    assert!(
        !format!("{tls:?}").contains("PRIVATE") && format!("{tls:?}").contains("identity: true"),
        "Debug must report the identity's presence, never its bytes: {tls:?}"
    );
    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(tls)).unwrap();
    assert_eq!(client.server_protocol_version(), PROTOCOL_VERSION);
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(9)]);
}

/// `MTLS-FR-001`/`MTLS-FR-008`: a client with no certificate, and one whose
/// certificate chains to a *different* CA, each fail the handshake — an
/// error at the client, never a valid `Response`, never a hang — and the
/// server keeps serving admitted clients afterwards (it neither panicked
/// nor wedged).
#[test]
fn mtls_rejects_a_client_without_a_certificate_or_with_one_from_another_ca() {
    let (ca, ca_key) = throwaway_ca();
    let addr = start_server(ServeOptions::default(), Some(mtls_server_config(&ca)));

    // No identity: raw and library clients alike.
    let mut raw = connect_tls(addr);
    let no_identity: Result<Response, _> = write_message(&mut raw, &Request::DescribeSchema)
        .map_err(|e| e.to_string())
        .and_then(|()| read_message(&mut raw).map_err(|e| e.to_string()));
    assert!(
        no_identity.is_err(),
        "no certificate, yet a Response decoded: {no_identity:?}"
    );
    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(dev_tls())).map(|_| ()) {
        Err(ClientError::Frame(_)) => {}
        other => panic!("expected the handshake to fail without an identity, got {other:?}"),
    }

    // An identity from a CA the server does not trust.
    let (other_ca, other_key) = throwaway_ca();
    let (chain, key) = client_identity(&other_ca, &other_key);
    match SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(chain, key)),
    )
    .map(|_| ())
    {
        Err(ClientError::Frame(_)) => {}
        other => panic!("expected the handshake to fail for a foreign CA, got {other:?}"),
    }

    // The server is still healthy for an admitted client.
    let (chain, key) = client_identity(&ca, &ca_key);
    let mut ok = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(chain, key)),
    )
    .unwrap();
    assert_eq!(ok.scan("age").unwrap(), vec![ScanValue::U32(3)]);
}

/// `MTLS-FR-002`: admission and class are layered, not merged. With
/// tokens configured an admitted connection still starts unauthenticated
/// (`Unauthenticated` without a token, `Unauthorized` for a write on a
/// read token, a write on the write token); with no tokens an admitted
/// connection writes immediately (`AUTH-FR-007` behind the gate).
#[test]
fn mtls_composes_with_auth_config_as_admission_then_class() {
    let (ca, ca_key) = throwaway_ca();
    let auth = ServeOptions::new(Some("read-token".into()), Some("write-token".into()));
    let addr = start_server(auth, Some(mtls_server_config(&ca)));
    let (chain, key) = client_identity(&ca, &ca_key);
    let identity = || identity_client(chain.clone(), key.clone());

    match SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(identity())).map(|_| ())
    {
        Err(ClientError::Server(ErrorCode::Unauthenticated, _)) => {}
        other => panic!("an admitted connection must still need a token, got {other:?}"),
    }
    let mut client = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity()).token("read-token"),
    )
    .unwrap();
    match client.update(Uuid::from_u128(1), "age", ScanValue::U32(9)) {
        Err(ClientError::Server(ErrorCode::Unauthorized, _)) => {}
        other => panic!("read token behind the gate must still be read-only, got {other:?}"),
    }
    client.authenticate("write-token").unwrap();
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());

    // Certificates only: no tokens configured.
    let addr = start_server(ServeOptions::default(), Some(mtls_server_config(&ca)));
    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(identity())).unwrap();
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(4))
        .unwrap());
}

/// `MTLS-FR-004`: the PEM path on both ends — a CA file with more than
/// one `CERTIFICATE` block, a client identity from PEM files — and the
/// construction-time errors: an empty root set is `TlsConfigError::Tls`,
/// a missing file is `Io`. Files live in a per-test directory, never in
/// the repository.
#[test]
fn mtls_pem_files_and_construction_errors() {
    use rusty_multimodal_db::server::TlsConfigError;
    let dir = unique_dir("server_tls_integration_mtls_pem");
    std::fs::create_dir_all(&dir).unwrap();

    let (ca, ca_key) = throwaway_ca();
    let (unrelated_ca, _) = throwaway_ca();
    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&server_key)
        .unwrap();
    let client_key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["client".to_string()]).unwrap();
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = params.signed_by(&client_key, &ca, &ca_key).unwrap();

    let chain_path = dir.join("server.pem");
    let key_path = dir.join("server.key");
    let ca_path = dir.join("client-ca.pem");
    let client_chain_path = dir.join("client.pem");
    let client_key_path = dir.join("client.key");
    std::fs::write(&chain_path, server_cert.pem()).unwrap();
    std::fs::write(&key_path, server_key.serialize_pem()).unwrap();
    std::fs::write(&ca_path, format!("{}{}", unrelated_ca.pem(), ca.pem())).unwrap();
    std::fs::write(&client_chain_path, client_cert.pem()).unwrap();
    std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

    let tls = TlsConfig::from_pem_files_with_client_ca(&chain_path, &key_path, &ca_path).unwrap();
    assert!(tls.requires_client_certificate());
    let addr = start_server(ServeOptions::default(), Some(tls));
    let identity = dev_tls()
        .with_identity_pem_files(&client_chain_path, &client_key_path)
        .unwrap();
    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(identity)).unwrap();
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);

    match TlsConfig::from_pem_files_with_client_ca(&chain_path, &key_path, dir.join("missing.pem"))
        .map(|_| ())
    {
        Err(TlsConfigError::Io(_)) => {}
        other => panic!("expected Io for a missing CA file, got {other:?}"),
    }
    let (cert_der, key_der) = self_signed_leaf();
    match TlsConfig::new_with_client_auth(vec![cert_der], key_der, Vec::new()).map(|_| ()) {
        Err(TlsConfigError::Tls(_)) => {}
        other => panic!("expected Tls for an empty root set, got {other:?}"),
    }
    match dev_tls()
        .with_identity_pem_files(dir.join("missing.pem"), &client_key_path)
        .map(|_| ())
    {
        Err(TlsConfigError::Io(_)) => {}
        other => panic!("expected Io for a missing identity file, got {other:?}"),
    }
}

/// `AUD-FR-001`/`AUD-FR-005` (design criterion 4): with the eager
/// handshake, a client without a certificate on an mTLS server is a
/// `HandshakeFailed` with a reason and no `Admitted`; an admitted mTLS
/// client is `Admitted { MutualTls, .. }`.
#[test]
fn handshake_outcomes_are_audited_with_a_reason() {
    #[derive(Default)]
    struct Collecting(Mutex<Vec<AuditEvent>>);
    impl AuditSink for Collecting {
        fn record(&self, event: &AuditEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }
    let (ca, ca_key) = throwaway_ca();
    let sink = Arc::new(Collecting::default());
    let addr = start_server(
        ServeOptions::default().with_audit(sink.clone()),
        Some(mtls_server_config(&ca)),
    );

    // No certificate: refused at the handshake.
    let mut bare = connect_tls(addr);
    let _ = write_message(&mut bare, &Request::DescribeSchema);
    let reply: Result<Response, _> = read_message(&mut bare);
    assert!(reply.is_err());
    drop(bare);
    let failed = loop {
        let events = sink.0.lock().unwrap().clone();
        if let Some(e) = events
            .iter()
            .find(|e| matches!(e.kind, AuditKind::HandshakeFailed { .. }))
        {
            break e.clone();
        }
        thread::sleep(std::time::Duration::from_millis(1));
    };
    match &failed.kind {
        AuditKind::HandshakeFailed { reason } => assert!(!reason.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(failed.peer.is_some());
    assert!(sink
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|e| !matches!(e.kind, AuditKind::Admitted { .. })));

    // A CA-signed client: admitted as mutual TLS at `ReadWrite` (no tokens).
    let (chain, key) = client_identity(&ca, &ca_key);
    let mut client = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(chain, key)),
    )
    .unwrap();
    assert!(client.get(Uuid::from_u128(1)).unwrap().is_some());
    drop(client);
    for _ in 0..5_000 {
        if sink.0.lock().unwrap().iter().any(|e| {
            e.kind
                == AuditKind::Admitted {
                    transport: Transport::MutualTls,
                    initial_class: Some(rusty_multimodal_db::server::TokenClass::ReadWrite),
                    classed_by_certificate: false,
                }
        }) {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!(
        "no mutual-TLS admission recorded: {:?}",
        sink.0.lock().unwrap()
    );
}

/// `ADR-0029`'s fourth revisit trigger, taken once `ADR-0028` landed:
/// `Admitted.classed_by_certificate` is `true` only for the connection
/// whose presented leaf matched a configured class, `false` for an
/// admitted leaf outside the map (unauthenticated here, on a
/// certificates-only server).
#[test]
fn admitted_records_classed_by_certificate_for_a_matched_leaf_only() {
    #[derive(Default)]
    struct Collecting(Mutex<Vec<AuditEvent>>);
    impl AuditSink for Collecting {
        fn record(&self, event: &AuditEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }
    let (ca, ca_key) = throwaway_ca();
    let (classed_chain, classed_key) = client_identity(&ca, &ca_key);
    let (unclassed_chain, unclassed_key) = client_identity(&ca, &ca_key);
    let sink = Arc::new(Collecting::default());
    let auth = ServeOptions::default()
        .with_certificate_class(classed_chain[0].clone(), TokenClass::ReadOnly)
        .with_audit(sink.clone());
    let addr = start_server(auth, Some(mtls_server_config(&ca)));

    let mut classed = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(classed_chain, classed_key)),
    )
    .unwrap();
    assert_eq!(classed.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    drop(classed);

    let _ = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(unclassed_chain, unclassed_key)),
    )
    .map(|_| ());

    for _ in 0..5_000 {
        let events = sink.0.lock().unwrap();
        let classed_seen = events.iter().any(|e| {
            matches!(
                e.kind,
                AuditKind::Admitted {
                    classed_by_certificate: true,
                    initial_class: Some(TokenClass::ReadOnly),
                    ..
                }
            )
        });
        let unclassed_seen = events.iter().any(|e| {
            matches!(
                e.kind,
                AuditKind::Admitted {
                    classed_by_certificate: false,
                    initial_class: None,
                    ..
                }
            )
        });
        if classed_seen && unclassed_seen {
            return;
        }
        drop(events);
        thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!(
        "expected one classed and one unclassed admission: {:?}",
        sink.0.lock().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Class from certificate (`SERVER-001` next minor / `CLS-FR-003`–`008`,
// ADR-0028, `docs/design/SERVER-MTLS-CLASS-DESIGN.md`)
// ---------------------------------------------------------------------------

/// Acceptance criterion 3: a leaf configured `ReadOnly` reads and is
/// refused writes with `Unauthorized` without any `Authenticate`; one
/// configured `ReadWrite` writes.
#[test]
fn a_classed_leaf_starts_at_its_class_with_no_authenticate() {
    let (ca, ca_key) = throwaway_ca();
    let (chain, key) = client_identity(&ca, &ca_key);
    let leaf_der = chain[0].clone();

    let read_only_auth =
        ServeOptions::default().with_certificate_class(leaf_der.clone(), TokenClass::ReadOnly);
    let addr = start_server(read_only_auth, Some(mtls_server_config(&ca)));
    let mut client = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(chain.clone(), key.clone())),
    )
    .unwrap();
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    match client.update(Uuid::from_u128(1), "age", ScanValue::U32(9)) {
        Err(ClientError::Server(ErrorCode::Unauthorized, _)) => {}
        other => panic!("a ReadOnly-classed leaf must not write, got {other:?}"),
    }

    let read_write_auth =
        ServeOptions::default().with_certificate_class(leaf_der, TokenClass::ReadWrite);
    let addr = start_server(read_write_auth, Some(mtls_server_config(&ca)));
    let mut client = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(chain, key)),
    )
    .unwrap();
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());
}

/// Acceptance criterion 4: a `ReadOnly`-classed connection that
/// `Authenticate`s with the `ReadWrite` token writes afterwards; with a
/// wrong token it is `Unauthenticated` and still reads at its
/// certificate class, unchanged.
#[test]
fn a_valid_token_replaces_the_certificate_class_an_invalid_one_does_not() {
    let (ca, ca_key) = throwaway_ca();
    let (chain, key) = client_identity(&ca, &ca_key);
    let leaf_der = chain[0].clone();
    let auth = ServeOptions::new(Some("read-token".into()), Some("write-token".into()))
        .with_certificate_class(leaf_der, TokenClass::ReadOnly);
    let addr = start_server(auth, Some(mtls_server_config(&ca)));
    let identity = || identity_client(chain.clone(), key.clone());

    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(identity())).unwrap();
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    client.authenticate("write-token").unwrap();
    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(9))
        .unwrap());

    // A fresh server/store: the first scenario's write must not leak into
    // this one's read assertion below.
    let auth = ServeOptions::new(Some("read-token".into()), Some("write-token".into()))
        .with_certificate_class(chain[0].clone(), TokenClass::ReadOnly);
    let addr = start_server(auth, Some(mtls_server_config(&ca)));
    let mut client =
        SchemaDrivenClient::connect_with(addr, ConnectOptions::new().tls(identity())).unwrap();
    match client.authenticate("wrong-token") {
        Err(ClientError::Server(ErrorCode::Unauthenticated, _)) => {}
        other => panic!("a wrong token must be Unauthenticated, got {other:?}"),
    }
    // Still classed ReadOnly — the failed Authenticate left it as it was.
    assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    match client.update(Uuid::from_u128(1), "age", ScanValue::U32(9)) {
        Err(ClientError::Server(ErrorCode::Unauthorized, _)) => {}
        other => {
            panic!("a failed Authenticate must not clear the certificate class, got {other:?}")
        }
    }
}

/// Acceptance criterion 5: a certificates-only server (classes, no
/// tokens) serves a classed leaf at its class, and any admitted leaf not
/// in the map — even from a certificate-holding, CA-trusted client — is
/// `Unauthenticated` on every request, including the schema fetch
/// `connect_with` makes.
#[test]
fn certificates_only_server_leaves_an_unclassed_admitted_leaf_unauthenticated() {
    let (ca, ca_key) = throwaway_ca();
    let (classed_chain, classed_key) = client_identity(&ca, &ca_key);
    let (unclassed_chain, unclassed_key) = client_identity(&ca, &ca_key);
    let auth = ServeOptions::default()
        .with_certificate_class(classed_chain[0].clone(), TokenClass::ReadOnly);
    let addr = start_server(auth, Some(mtls_server_config(&ca)));

    let mut classed = SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(classed_chain, classed_key)),
    )
    .unwrap();
    assert_eq!(classed.scan("age").unwrap(), vec![ScanValue::U32(3)]);

    match SchemaDrivenClient::connect_with(
        addr,
        ConnectOptions::new().tls(identity_client(unclassed_chain, unclassed_key)),
    )
    .map(|_| ())
    {
        Err(ClientError::Server(ErrorCode::Unauthenticated, _)) => {}
        other => panic!(
            "an admitted leaf outside the map on a certificates-only server must be Unauthenticated, got {other:?}"
        ),
    }
}

/// Acceptance criterion 6, the PEM path: a two-block file classes both
/// leaves identically; `Io`/`Pem` errors surface, naming the same
/// `TlsConfigError` shapes `TlsConfig`'s own PEM constructors use.
#[test]
fn with_certificate_class_pem_file_classes_every_block_and_surfaces_errors() {
    use rusty_multimodal_db::server::TlsConfigError;
    let dir = unique_dir("server_tls_integration_class_pem");
    std::fs::create_dir_all(&dir).unwrap();

    let (ca, ca_key) = throwaway_ca();
    let signed_client_identity = |ca: &rcgen::Certificate, ca_key: &rcgen::KeyPair| {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["client".to_string()]).unwrap();
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        let cert = params.signed_by(&key, ca, ca_key).unwrap();
        let identity = (vec![cert.der().to_vec()], key.serialize_der());
        (cert, identity)
    };
    let (cert_a, (chain_a, key_a)) = signed_client_identity(&ca, &ca_key);
    let (cert_b, (chain_b, key_b)) = signed_client_identity(&ca, &ca_key);
    let leaves_pem_path = dir.join("leaves.pem");
    std::fs::write(
        &leaves_pem_path,
        format!("{}{}", cert_a.pem(), cert_b.pem()),
    )
    .unwrap();

    let auth = ServeOptions::default()
        .with_certificate_class_pem_file(&leaves_pem_path, TokenClass::ReadOnly)
        .unwrap();
    let addr = start_server(auth, Some(mtls_server_config(&ca)));
    for (chain, key) in [(chain_a, key_a), (chain_b, key_b)] {
        let mut client = SchemaDrivenClient::connect_with(
            addr,
            ConnectOptions::new().tls(identity_client(chain, key)),
        )
        .unwrap();
        assert_eq!(client.scan("age").unwrap(), vec![ScanValue::U32(3)]);
    }

    match ServeOptions::default()
        .with_certificate_class_pem_file(dir.join("missing.pem"), TokenClass::ReadOnly)
        .map(|_| ())
    {
        Err(TlsConfigError::Io(_)) => {}
        other => panic!("expected Io for a missing file, got {other:?}"),
    }
    let not_pem_path = dir.join("not-pem.txt");
    std::fs::write(&not_pem_path, b"not a certificate").unwrap();
    match ServeOptions::default()
        .with_certificate_class_pem_file(&not_pem_path, TokenClass::ReadOnly)
        .map(|_| ())
    {
        Err(TlsConfigError::Pem(_)) => {}
        other => panic!("expected Pem for non-PEM content, got {other:?}"),
    }
}
