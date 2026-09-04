//! The authentication audit log (`SERVER-001` FR-029, ADR-0029,
//! `docs/design/SERVER-AUTH-AUDIT-DESIGN.md`): a record of every decision
//! the server's three gates take — admission (the TLS handshake),
//! authentication (`Authenticate`), authorization (every
//! `Unauthenticated`/`Unauthorized` refusal) — plus the disconnect, and
//! nothing else. No successful request is recorded; no token, no
//! certificate, no record id, no value ever appears in an event. The
//! peer address and a Unix-seconds timestamp are the identifiers.
//!
//! An [`AuditSink`] is hung on `ServeOptions::with_audit` — the policy
//! object every gate already consults — so `serve`'s signature is
//! unchanged; `handle_connection` calls it at its existing gates, after
//! the decision and before the response, outside every lock. The
//! default [`NoAudit`] is free; [`StderrAudit`] and [`FileAudit`] write
//! one documented line per event and are *fail-open*: a write failure
//! drops the event, counts it, and prints one notice per process — the
//! audit path can never fail or block a connection (`AUD-FR-006`).
//!
//! # Line format (`AUD-FR-002`)
//!
//! `audit at=<unix seconds> peer=<addr|-> event=<Kind> [field=value ...]`,
//! space-separated, values without spaces (addresses, enum names,
//! numbers), so `grep`/`awk` suffice. The one free-text field,
//! `HandshakeFailed`'s `reason`, has its whitespace replaced by `_`.

use super::protocol::{ErrorCode, Request};
use super::TokenClass;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How a connection was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Plain,
    Tls,
    /// TLS with a client certificate required and verified.
    MutualTls,
}

/// The variant of a refused request — the name only, never a payload.
/// Exhaustive over `Request`, so a new variant cannot skip the decision.
/// `#[non_exhaustive]` since `RL-FR-004` (ADR-0030): designed to grow with
/// the gates, and no downstream crate matches it exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestKind {
    GetById,
    FilterEq,
    ScanField,
    UpdateField,
    Parent,
    Children,
    Neighbors,
    DescribeSchema,
    Authenticate,
    Transaction,
    Hello,
    Begin,
    Commit,
    Rollback,
    BeginWith,
    Query,
    Aggregate,
    /// Protocol 10, ADR-0039.
    NeighborsByRelation,
    /// Protocol 10, ADR-0039.
    ListRelationKinds,
}

impl RequestKind {
    pub fn of(req: &Request) -> Self {
        match req {
            Request::GetById { .. } => RequestKind::GetById,
            Request::FilterEq { .. } => RequestKind::FilterEq,
            Request::ScanField { .. } => RequestKind::ScanField,
            Request::UpdateField { .. } => RequestKind::UpdateField,
            Request::Parent { .. } => RequestKind::Parent,
            Request::Children { .. } => RequestKind::Children,
            Request::Neighbors { .. } => RequestKind::Neighbors,
            Request::DescribeSchema => RequestKind::DescribeSchema,
            Request::Authenticate { .. } => RequestKind::Authenticate,
            Request::Transaction { .. } => RequestKind::Transaction,
            Request::Hello { .. } => RequestKind::Hello,
            Request::Begin => RequestKind::Begin,
            Request::Commit => RequestKind::Commit,
            Request::Rollback => RequestKind::Rollback,
            Request::BeginWith { .. } => RequestKind::BeginWith,
            Request::Query { .. } => RequestKind::Query,
            Request::Aggregate { .. } => RequestKind::Aggregate,
            Request::NeighborsByRelation { .. } => RequestKind::NeighborsByRelation,
            Request::ListRelationKinds => RequestKind::ListRelationKinds,
        }
    }
}

/// What happened (`AUD-FR-001`). `#[non_exhaustive]` since `RL-FR-004`
/// (ADR-0030): designed to grow with the gates — `LockedOut`/`Throttled`
/// are its first growth — and no downstream crate matches it exhaustively.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuditKind {
    /// A connection past the handshake, and the class it starts at
    /// (`None`: it must authenticate). `classed_by_certificate`
    /// (`ADR-0029`'s fourth revisit trigger, taken once `ADR-0028`
    /// landed): whether `initial_class` came from a matched, configured
    /// certificate (`CLS-FR-004`) rather than `ServeOptions::is_configured`'s
    /// unauthenticated/`ReadWrite` default — always `false` when
    /// `initial_class` is `None`, since a certificate that classes a
    /// connection always sets it.
    Admitted {
        transport: Transport,
        initial_class: Option<TokenClass>,
        classed_by_certificate: bool,
    },
    /// The TLS handshake was refused; `reason` is the TLS error's
    /// `Display`, whitespace folded.
    HandshakeFailed { reason: String },
    /// `Authenticate` matched a configured token.
    Authenticated { class: TokenClass },
    /// `Authenticate` matched nothing. The token is not recorded.
    AuthenticationFailed,
    /// A request refused by a gate.
    Refused {
        class: Option<TokenClass>,
        request: RequestKind,
        code: ErrorCode,
    },
    /// The connection ended.
    Disconnected,
    /// `RL-FR-001` (ADR-0030): the per-connection lockout closed the
    /// connection after this many failed `Authenticate`s.
    LockedOut { failures: u32 },
    /// `RL-FR-002` (ADR-0030): `Authenticate` was refused before any
    /// comparison because the peer is over its configured budget.
    Throttled { failures: u32 },
}

/// One audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// Unix seconds when the decision was taken.
    pub at: u64,
    /// The peer, if the OS could say.
    pub peer: Option<SocketAddr>,
    pub kind: AuditKind,
}

impl AuditEvent {
    /// Stamp `kind` with the current time.
    pub fn now(peer: Option<SocketAddr>, kind: AuditKind) -> Self {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self { at, peer, kind }
    }

    /// The documented one-line form — the one place lines are built.
    pub fn line(&self) -> String {
        let mut line = format!(
            "audit at={} peer={} event=",
            self.at,
            self.peer
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        match &self.kind {
            AuditKind::Admitted {
                transport,
                initial_class,
                classed_by_certificate,
            } => {
                line.push_str("Admitted transport=");
                line.push_str(&format!("{transport:?}"));
                line.push_str(" initial_class=");
                line.push_str(&class_name(*initial_class));
                line.push_str(&format!(" classed_by_certificate={classed_by_certificate}"));
            }
            AuditKind::HandshakeFailed { reason } => {
                line.push_str("HandshakeFailed reason=");
                line.push_str(&fold(reason));
            }
            AuditKind::Authenticated { class } => {
                line.push_str("Authenticated class=");
                line.push_str(&class_name(Some(*class)));
            }
            AuditKind::AuthenticationFailed => line.push_str("AuthenticationFailed"),
            AuditKind::Refused {
                class,
                request,
                code,
            } => {
                line.push_str(&format!(
                    "Refused class={} request={request:?} code={code:?}",
                    class_name(*class)
                ));
            }
            AuditKind::Disconnected => line.push_str("Disconnected"),
            AuditKind::LockedOut { failures } => {
                line.push_str(&format!("LockedOut failures={failures}"));
            }
            AuditKind::Throttled { failures } => {
                line.push_str(&format!("Throttled failures={failures}"));
            }
        }
        line
    }
}

fn class_name(class: Option<TokenClass>) -> String {
    match class {
        Some(c) => format!("{c:?}"),
        None => "-".to_string(),
    }
}

fn fold(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join("_")
}

impl fmt::Display for AuditEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.line())
    }
}

/// Where audit events go. Called synchronously on the connection's own
/// thread, outside every lock this crate holds; must never panic.
pub trait AuditSink: Send + Sync {
    fn record(&self, event: &AuditEvent);
}

/// The default: records nothing, costs nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAudit;

impl AuditSink for NoAudit {
    fn record(&self, _event: &AuditEvent) {}
}

/// Fail-open bookkeeping shared by the writing sinks: count dropped
/// events, print one notice per process.
#[derive(Debug, Default)]
struct Dropped {
    count: AtomicU64,
    noticed: AtomicBool,
}

impl Dropped {
    fn note(&self, what: &str, err: &io::Error) {
        self.count.fetch_add(1, Ordering::Relaxed);
        if !self.noticed.swap(true, Ordering::Relaxed) {
            eprintln!("audit sink failing ({what}): {err}; events are being dropped");
        }
    }
}

/// One line per event on standard error.
#[derive(Debug, Default)]
pub struct StderrAudit {
    lock: Mutex<()>,
    dropped: Dropped,
}

impl StderrAudit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Events dropped because stderr could not be written.
    pub fn dropped(&self) -> u64 {
        self.dropped.count.load(Ordering::Relaxed)
    }
}

impl AuditSink for StderrAudit {
    fn record(&self, event: &AuditEvent) {
        let _guard = self.lock.lock();
        let mut err = io::stderr().lock();
        if let Err(e) = writeln!(err, "{}", event.line()).and_then(|()| err.flush()) {
            self.dropped.note("stderr", &e);
        }
    }
}

/// One line per event appended to a file — `write_all` + `flush` under
/// a mutex, no `fsync` (an `fsync` per refusal on a hostile peer's
/// schedule would be a denial-of-service lever). The operator owns
/// permissions and rotation.
#[derive(Debug)]
pub struct FileAudit {
    file: Mutex<BufWriter<File>>,
    dropped: Dropped,
}

impl FileAudit {
    /// Open (or create) `path` for appending.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(BufWriter::new(file)),
            dropped: Dropped::default(),
        })
    }

    /// Events dropped because the file could not be written.
    pub fn dropped(&self) -> u64 {
        self.dropped.count.load(Ordering::Relaxed)
    }

    /// Wrap an already-open handle — the failure test's read-only file.
    #[cfg(test)]
    fn from_file(file: File) -> Self {
        Self {
            file: Mutex::new(BufWriter::new(file)),
            dropped: Dropped::default(),
        }
    }
}

impl AuditSink for FileAudit {
    fn record(&self, event: &AuditEvent) {
        let Ok(mut file) = self.file.lock() else {
            return;
        };
        if let Err(e) = writeln!(file, "{}", event.line()).and_then(|()| file.flush()) {
            self.dropped.note("file", &e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fresh_temp_dir;

    fn every_kind() -> Vec<AuditKind> {
        vec![
            AuditKind::Admitted {
                transport: Transport::MutualTls,
                initial_class: None,
                classed_by_certificate: false,
            },
            AuditKind::HandshakeFailed {
                reason: "no certificates presented".into(),
            },
            AuditKind::Authenticated {
                class: TokenClass::ReadOnly,
            },
            AuditKind::AuthenticationFailed,
            AuditKind::Refused {
                class: Some(TokenClass::ReadOnly),
                request: RequestKind::UpdateField,
                code: ErrorCode::Unauthorized,
            },
            AuditKind::Disconnected,
            AuditKind::LockedOut { failures: 5 },
            AuditKind::Throttled { failures: 3 },
        ]
    }

    /// `AUD-FR-002`: the documented format — `key=value`, no spaces
    /// inside a value, the free-text reason folded.
    #[test]
    fn lines_follow_the_documented_format() {
        let peer: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        let lines: Vec<String> = every_kind()
            .into_iter()
            .map(|kind| {
                AuditEvent {
                    at: 7,
                    peer: Some(peer),
                    kind,
                }
                .line()
            })
            .collect();
        assert_eq!(
            lines[0],
            "audit at=7 peer=127.0.0.1:4242 event=Admitted transport=MutualTls initial_class=- classed_by_certificate=false"
        );
        assert_eq!(
            lines[1],
            "audit at=7 peer=127.0.0.1:4242 event=HandshakeFailed reason=no_certificates_presented"
        );
        assert_eq!(
            lines[2],
            "audit at=7 peer=127.0.0.1:4242 event=Authenticated class=ReadOnly"
        );
        assert_eq!(
            lines[3],
            "audit at=7 peer=127.0.0.1:4242 event=AuthenticationFailed"
        );
        assert_eq!(
            lines[4],
            "audit at=7 peer=127.0.0.1:4242 event=Refused class=ReadOnly request=UpdateField code=Unauthorized"
        );
        assert_eq!(
            lines[5],
            "audit at=7 peer=127.0.0.1:4242 event=Disconnected"
        );
        assert_eq!(
            lines[6],
            "audit at=7 peer=127.0.0.1:4242 event=LockedOut failures=5"
        );
        assert_eq!(
            lines[7],
            "audit at=7 peer=127.0.0.1:4242 event=Throttled failures=3"
        );
        for line in &lines {
            let mut parts = line.split(' ');
            assert_eq!(parts.next(), Some("audit"));
            assert!(parts.all(|kv| kv.split_once('=').is_some()), "{line}");
        }
        assert_eq!(
            AuditEvent {
                at: 1,
                peer: None,
                kind: AuditKind::Disconnected
            }
            .line(),
            "audit at=1 peer=- event=Disconnected"
        );
    }

    /// `AUD-FR-007`: no variant can carry a secret — the event types have
    /// no field for one, and every line is built from those fields alone.
    #[test]
    fn no_line_can_contain_a_token_or_a_certificate() {
        let secret = "rw-secret-token";
        let cert = "MIIB-certificate-bytes";
        for kind in every_kind() {
            let line = AuditEvent::now(None, kind).line();
            assert!(!line.contains(secret) && !line.contains(cert), "{line}");
        }
        assert!(
            RequestKind::of(&Request::Authenticate {
                token: secret.into()
            }) == RequestKind::Authenticate
        );
    }

    /// `AUD-FR-002`/`AUD-FR-006`: `FileAudit` appends one line per event
    /// across opens; a sink whose file cannot be written drops events,
    /// counts them, and never fails the caller.
    #[test]
    fn file_audit_appends_and_a_failing_file_drops_without_failing() {
        let dir = fresh_temp_dir("audit_file").unwrap();
        let path = dir.join("audit.log");
        {
            let sink = FileAudit::open(&path).unwrap();
            sink.record(&AuditEvent {
                at: 1,
                peer: None,
                kind: AuditKind::Disconnected,
            });
            sink.record(&AuditEvent {
                at: 2,
                peer: None,
                kind: AuditKind::AuthenticationFailed,
            });
        }
        let sink = FileAudit::open(&path).unwrap();
        sink.record(&AuditEvent {
            at: 3,
            peer: None,
            kind: AuditKind::Disconnected,
        });
        assert_eq!(sink.dropped(), 0);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "audit at=1 peer=- event=Disconnected");
        assert_eq!(lines[2], "audit at=3 peer=- event=Disconnected");

        // A read-only handle: every write fails, nothing panics, drops count.
        let failing = FileAudit::from_file(File::open(&path).unwrap());
        failing.record(&AuditEvent {
            at: 4,
            peer: None,
            kind: AuditKind::Disconnected,
        });
        failing.record(&AuditEvent {
            at: 5,
            peer: None,
            kind: AuditKind::Disconnected,
        });
        assert_eq!(failing.dropped(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
    }
}
