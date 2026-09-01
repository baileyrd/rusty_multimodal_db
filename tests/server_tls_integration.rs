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
use rusty_multimodal_db::server::dog::{DogConnectionStore, FIELD_AGE};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{Request, Response, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig, TlsConfig};
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

fn start_server(auth: AuthConfig, tls: Option<TlsConfig>) -> std::net::SocketAddr {
    let dir = unique_dir("server_tls_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");

    let records = vec![DogRecord::new(Uuid::from_u128(1), "labrador", 3)];
    let store = ProductionStore::create(records, Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, auth, tls));
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
    let addr = start_server(AuthConfig::default(), Some(tls));

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

/// `TLS-FR-007`: composed with `AuthConfig`, `Authenticate`'s token now
/// travels over the encrypted channel — the handshake always completes
/// before any framed `Request` is ever read, so authenticating over TLS
/// works exactly like authenticating in plaintext.
#[test]
fn authentication_composes_with_tls() {
    let (cert_der, key_der) = self_signed_leaf();
    let tls = TlsConfig::new(vec![cert_der], key_der).unwrap();
    let auth = AuthConfig::new(None, Some("write-token".into()));
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
    let auth = AuthConfig::new(None, Some("super-secret-token-xyz123".into()));
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
    let addr = start_server(AuthConfig::default(), Some(tls));

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
    let addr = start_server(AuthConfig::default(), None);
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
