//! The per-request access log (`SERVER-001` FR-033, ADR-0031,
//! `docs/design/SERVER-ACCESS-LOG-DESIGN.md`): a record of every request a
//! connection's gates admit through to being answered — its kind, its
//! outcome's shape, who asked, at what class, when. A second, independent
//! sink family from [`super::audit`]'s: `Hello`, `Authenticate`, and every
//! gate-refused request are the audit log's territory and are never logged
//! here, so an operator's choice to turn on one never implies accepting the
//! other's cost — the audit log's per-decision volume, or this log's
//! per-request volume.
//!
//! An [`AccessSink`] is hung on `ServeOptions::with_access_log` — the same
//! object the audit sink and (if configured) the rate limiter hang on, so
//! `serve`'s signature is unchanged; `handle_connection` calls it once per
//! dispatched request, after the response is decided, outside every lock.
//! The default [`NoAccessLog`] is free; [`StderrAccessLog`] and
//! [`FileAccessLog`] write one documented line per event and are
//! *fail-open*, the same posture `audit.rs`'s sinks take: a write failure
//! drops the event, counts it, and prints one notice per process — the
//! access path can never fail or block a connection (`ACC-FR-006`).
//!
//! # Line format (`ACC-FR-002`)
//!
//! `access at=<unix seconds> peer=<addr|-> class=<class|-> request=<Kind>
//! outcome=<Ok|Err> [code=<ErrorCode>]`, space-separated, values without
//! spaces — a distinct leading key (`access` vs. `audit`) so the two
//! streams are never ambiguous even interleaved in one file.

use super::audit::RequestKind;
use super::protocol::ErrorCode;
use super::TokenClass;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A dispatched request's outcome shape — never its content. `NotFound`/
/// `NoParent` are `Ok` (this crate's own convention: a normal outcome, not
/// an error); a `Transaction` that fails validation is `Err` naming the
/// code, never the failing index or the operations themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Err(ErrorCode),
}

/// One access record (`ACC-FR-001`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEvent {
    /// Unix seconds when the request was answered.
    pub at: u64,
    /// The peer, if the OS could say.
    pub peer: Option<SocketAddr>,
    /// The class the connection was answering at — `Some(ReadWrite)` on
    /// every dispatched request through an unconfigured `ServeOptions`, per
    /// `AUTH-FR-007`; `Option` to mirror `AuditKind::Admitted`'s shape
    /// rather than assert something the type system need not.
    pub class: Option<TokenClass>,
    pub request: RequestKind,
    pub outcome: Outcome,
}

impl AccessEvent {
    /// Stamp one dispatched request's outcome with the current time.
    pub fn now(
        peer: Option<SocketAddr>,
        class: Option<TokenClass>,
        request: RequestKind,
        outcome: Outcome,
    ) -> Self {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            at,
            peer,
            class,
            request,
            outcome,
        }
    }

    /// The documented one-line form (`ACC-FR-002`) — the one place lines
    /// are built.
    pub fn line(&self) -> String {
        let mut line = format!(
            "access at={} peer={} class={} request={:?} outcome=",
            self.at,
            self.peer
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string()),
            match self.class {
                Some(c) => format!("{c:?}"),
                None => "-".to_string(),
            },
            self.request,
        );
        match self.outcome {
            Outcome::Ok => line.push_str("Ok"),
            Outcome::Err(code) => line.push_str(&format!("Err code={code:?}")),
        }
        line
    }
}

impl fmt::Display for AccessEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.line())
    }
}

/// Where access events go. Called synchronously on the connection's own
/// thread, outside every lock this crate holds; must never panic.
pub trait AccessSink: Send + Sync {
    fn record(&self, event: &AccessEvent);
}

/// The default: records nothing, costs nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAccessLog;

impl AccessSink for NoAccessLog {
    fn record(&self, _event: &AccessEvent) {}
}

/// Fail-open bookkeeping shared by the writing sinks: count dropped
/// events, print one notice per process — the same shape `audit.rs`'s
/// `Dropped` takes, kept separate so the two families stay independent.
#[derive(Debug, Default)]
struct Dropped {
    count: AtomicU64,
    noticed: AtomicBool,
}

impl Dropped {
    fn note(&self, what: &str, err: &io::Error) {
        self.count.fetch_add(1, Ordering::Relaxed);
        if !self.noticed.swap(true, Ordering::Relaxed) {
            eprintln!("access log sink failing ({what}): {err}; events are being dropped");
        }
    }
}

/// One line per event on standard error.
#[derive(Debug, Default)]
pub struct StderrAccessLog {
    lock: Mutex<()>,
    dropped: Dropped,
}

impl StderrAccessLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Events dropped because stderr could not be written.
    pub fn dropped(&self) -> u64 {
        self.dropped.count.load(Ordering::Relaxed)
    }
}

impl AccessSink for StderrAccessLog {
    fn record(&self, event: &AccessEvent) {
        let _guard = self.lock.lock();
        let mut err = io::stderr().lock();
        if let Err(e) = writeln!(err, "{}", event.line()).and_then(|()| err.flush()) {
            self.dropped.note("stderr", &e);
        }
    }
}

/// One line per event appended to a file — `write_all` + `flush` under a
/// mutex, no `fsync` (an `fsync` per request on a hostile peer's schedule
/// would be a denial-of-service lever). The operator owns permissions and
/// rotation.
#[derive(Debug)]
pub struct FileAccessLog {
    file: Mutex<BufWriter<File>>,
    dropped: Dropped,
}

impl FileAccessLog {
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

impl AccessSink for FileAccessLog {
    fn record(&self, event: &AccessEvent) {
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

    fn every_outcome() -> Vec<Outcome> {
        vec![Outcome::Ok, Outcome::Err(ErrorCode::Unauthorized)]
    }

    /// `ACC-FR-002`: the documented format, distinguishable from an audit
    /// line at a glance (the leading key).
    #[test]
    fn lines_follow_the_documented_format_and_are_distinct_from_audit_lines() {
        let peer: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        let ok = AccessEvent {
            at: 7,
            peer: Some(peer),
            class: Some(TokenClass::ReadOnly),
            request: RequestKind::GetById,
            outcome: Outcome::Ok,
        };
        assert_eq!(
            ok.line(),
            "access at=7 peer=127.0.0.1:4242 class=ReadOnly request=GetById outcome=Ok"
        );
        let err = AccessEvent {
            at: 7,
            peer: Some(peer),
            class: Some(TokenClass::ReadOnly),
            request: RequestKind::UpdateField,
            outcome: Outcome::Err(ErrorCode::Unauthorized),
        };
        assert_eq!(
            err.line(),
            "access at=7 peer=127.0.0.1:4242 class=ReadOnly request=UpdateField outcome=Err code=Unauthorized"
        );
        assert!(ok.line().starts_with("access "));
        assert!(!ok.line().starts_with("audit "));

        let unclassed = AccessEvent {
            at: 1,
            peer: None,
            class: None,
            request: RequestKind::ScanField,
            outcome: Outcome::Ok,
        };
        assert_eq!(
            unclassed.line(),
            "access at=1 peer=- class=- request=ScanField outcome=Ok"
        );

        for line in [ok.line(), err.line(), unclassed.line()] {
            let mut parts = line.split(' ');
            assert_eq!(parts.next(), Some("access"));
            assert!(parts.all(|kv| kv.split_once('=').is_some()), "{line}");
        }
    }

    /// `ACC-FR-005`: no line for any `RequestKind`/`Outcome` combination
    /// can carry a marker planted in a real request — the types have no
    /// field for one, so this is structural, checked the same way the
    /// audit log's secrecy is.
    #[test]
    fn no_line_can_contain_a_marker_value() {
        let marker = "planted-record-id-or-value-12345";
        for request in [
            RequestKind::GetById,
            RequestKind::FilterEq,
            RequestKind::ScanField,
            RequestKind::UpdateField,
            RequestKind::Parent,
            RequestKind::Children,
            RequestKind::Neighbors,
            RequestKind::DescribeSchema,
            RequestKind::Transaction,
            RequestKind::Begin,
            RequestKind::Commit,
            RequestKind::Rollback,
            RequestKind::BeginWith,
        ] {
            for outcome in every_outcome() {
                let line =
                    AccessEvent::now(None, Some(TokenClass::ReadWrite), request, outcome).line();
                assert!(!line.contains(marker), "{line}");
            }
        }
    }

    /// `ACC-FR-006`: `FileAccessLog` appends one line per event across
    /// opens; a sink whose file cannot be written drops events, counts
    /// them, and never fails the caller.
    #[test]
    fn file_access_log_appends_and_a_failing_file_drops_without_failing() {
        let dir = fresh_temp_dir("access_file").unwrap();
        let path = dir.join("access.log");
        {
            let sink = FileAccessLog::open(&path).unwrap();
            sink.record(&AccessEvent {
                at: 1,
                peer: None,
                class: Some(TokenClass::ReadWrite),
                request: RequestKind::GetById,
                outcome: Outcome::Ok,
            });
            sink.record(&AccessEvent {
                at: 2,
                peer: None,
                class: Some(TokenClass::ReadOnly),
                request: RequestKind::UpdateField,
                outcome: Outcome::Err(ErrorCode::Unauthorized),
            });
        }
        let sink = FileAccessLog::open(&path).unwrap();
        sink.record(&AccessEvent {
            at: 3,
            peer: None,
            class: Some(TokenClass::ReadWrite),
            request: RequestKind::GetById,
            outcome: Outcome::Ok,
        });
        assert_eq!(sink.dropped(), 0);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "access at=1 peer=- class=ReadWrite request=GetById outcome=Ok"
        );
        assert_eq!(
            lines[2],
            "access at=3 peer=- class=ReadWrite request=GetById outcome=Ok"
        );

        // A read-only handle: every write fails, nothing panics, drops count.
        let failing = FileAccessLog::from_file(File::open(&path).unwrap());
        failing.record(&AccessEvent {
            at: 4,
            peer: None,
            class: None,
            request: RequestKind::GetById,
            outcome: Outcome::Ok,
        });
        failing.record(&AccessEvent {
            at: 5,
            peer: None,
            class: None,
            request: RequestKind::GetById,
            outcome: Outcome::Ok,
        });
        assert_eq!(failing.dropped(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
    }
}
