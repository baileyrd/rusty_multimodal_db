//! A minimal, real server binary for the `Dog` domain — wraps a
//! `ProductionStore` seeded from a small, hand-written sample dataset (not
//! `generator`, which is research-gated) in a `DogConnectionStore` and
//! serves it over TCP. See `rusty_multimodal_db::server`'s own module docs
//! for the protocol and what this deliberately does not provide.
//!
//! # This is a local development tool, not a deployable service
//!
//! Authentication/authorization (`AuthConfig::from_env`, ADR-0012) and
//! native transport encryption (`TlsConfig::from_env`, ADR-0014) are both
//! real and opt-in via the process environment
//! (`SERVER_AUTH_READ_ONLY_TOKEN`/`SERVER_AUTH_READ_WRITE_TOKEN`,
//! `SERVER_TLS_CERT_CHAIN_PATH`/`SERVER_TLS_PRIVATE_KEY_PATH`, and —
//! for mutual TLS, ADR-0023 — an optional `SERVER_TLS_CLIENT_CA_PATH`
//! naming the CA roots every client certificate must chain to; and, since
//! ADR-0028, `SERVER_AUTH_READ_ONLY_CLIENT_CERTS`/
//! `SERVER_AUTH_READ_WRITE_CLIENT_CERTS` (`:`-separated PEM files)
//! classing a presented certificate by exact match — refused at startup
//! without `SERVER_TLS_CLIENT_CA_PATH` (`CLS-FR-005`); and, since
//! ADR-0030, an opt-in per-peer failed-`Authenticate` budget,
//! `SERVER_AUTH_RATE_LIMIT="<failures>/<seconds>"` (a per-connection
//! lockout after five failures is on by default, not configurable); and
//! `SERVER_TXN_JOURNAL_PATH`, ADR-0025, making every transaction batch
//! crash-atomic at one `fsync` per batch, shared across concurrent batches
//! by ADR-0026's group commit) — with none of them set, this
//! behaves exactly as it did before any of these features existed: no
//! auth, no encryption, no journal, no audit log (`SERVER_AUDIT_LOG`, ADR-0029: `stderr` or a file path). Do not expose this beyond a
//! trusted, localhost/development network unless both are configured —
//! see ADR-0010's Consequences. Usage: `dog_server [host:port]` (defaults
//! to `127.0.0.1:7878`).

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::audit::{AuditSink, FileAudit, StderrAudit};
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::{serve, AuthConfig, RateLimit, TlsConfig, TokenClass};
use rusty_multimodal_db::ProductionStore;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

fn sample_records() -> Vec<DogRecord> {
    vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
        DogRecord::new(Uuid::from_u128(3), "poodle", 2),
    ]
}

fn sample_edges() -> Vec<(Uuid, Uuid)> {
    vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
}

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7878".to_string());

    let dir = std::env::temp_dir().join(format!("dog_server_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating a scratch directory for the mmap-backed store");
    let path = dir.join("dogs.mmap");

    let store = ProductionStore::create(sample_records(), sample_edges(), &path)
        .expect("creating the sample ProductionStore");
    // `SERVER_TXN_JOURNAL_PATH` (ADR-0025): with it, every transaction
    // batch is crash-atomic — journaled and fsync'd before its first
    // write, replayed on the next start. Set it the same way every start:
    // opening without it after a crash forgoes the replay.
    let connection_store = Arc::new(match std::env::var("SERVER_TXN_JOURNAL_PATH") {
        Ok(journal_path) => DogConnectionStore::with_journal(store, Path::new(&journal_path))
            .unwrap_or_else(|e| panic!("SERVER_TXN_JOURNAL_PATH configured but invalid: {e}")),
        Err(_) => DogConnectionStore::new(store),
    });
    let journaled = std::env::var_os("SERVER_TXN_JOURNAL_PATH").is_some();

    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("binding {addr}: {e}"));

    // `SERVER_AUDIT_LOG` (ADR-0029): `stderr`, or a file path appended to;
    // unset → no audit. An unopenable path is a startup error.
    let auth = match audit_sink_from(std::env::var("SERVER_AUDIT_LOG").ok().as_deref()) {
        Ok(Some(sink)) => AuthConfig::from_env().with_audit(sink),
        Ok(None) => AuthConfig::from_env(),
        Err(e) => panic!("SERVER_AUDIT_LOG configured but invalid: {e}"),
    };
    // `SERVER_AUTH_READ_ONLY_CLIENT_CERTS`/`SERVER_AUTH_READ_WRITE_CLIENT_CERTS`
    // (ADR-0028, `CLS-FR-005`): certificate-classed connections.
    let auth = certificate_classes_from_env_values(
        auth,
        std::env::var("SERVER_AUTH_READ_ONLY_CLIENT_CERTS")
            .ok()
            .as_deref(),
        std::env::var("SERVER_AUTH_READ_WRITE_CLIENT_CERTS")
            .ok()
            .as_deref(),
        std::env::var("SERVER_TLS_CLIENT_CA_PATH").ok().as_deref(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // `SERVER_AUTH_RATE_LIMIT` (ADR-0030, `RL-FR-006`): an opt-in
    // per-peer failed-`Authenticate` budget; the per-connection lockout
    // at `MAX_AUTH_FAILURES` is always on and has no variable.
    let auth = rate_limit_from_env_value(
        auth,
        std::env::var("SERVER_AUTH_RATE_LIMIT").ok().as_deref(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let rate_limited = auth.rate_limit().is_some();
    let audited = std::env::var_os("SERVER_AUDIT_LOG").is_some();
    let tls = match TlsConfig::from_env() {
        None => None,
        Some(Ok(tls)) => Some(tls),
        Some(Err(e)) => panic!(
            "SERVER_TLS_CERT_CHAIN_PATH/SERVER_TLS_PRIVATE_KEY_PATH/SERVER_TLS_CLIENT_CA_PATH configured but invalid: {e}"
        ),
    };
    eprintln!(
        "dog_server listening on {addr} (auth: {}, TLS: {}, transaction journal: {}, audit log: {}, auth rate limit: {} — see ADR-0012/ADR-0014/ADR-0023/ADR-0025/ADR-0029/ADR-0030; do not expose beyond a trusted network unless auth and TLS are both configured)",
        if auth.is_configured() { "configured" } else { "NOT configured" },
        match &tls {
            None => "NOT configured",
            Some(tls) if tls.requires_client_certificate() => "configured, client certificate required",
            Some(_) => "configured",
        },
        if journaled { "configured" } else { "NOT configured" },
        if audited { "configured" } else { "NOT configured" },
        if rate_limited { "configured" } else { "lockout only (default)" },
    );

    serve(listener, connection_store, auth, tls);
}

/// `SERVER_AUDIT_LOG`'s decision table (`AUD-FR-008`), factored so a test
/// can drive it without touching the process environment: unset → no
/// sink; `stderr` → [`StderrAudit`]; anything else → a [`FileAudit`] at
/// that path, an unopenable one an error.
fn audit_sink_from(value: Option<&str>) -> std::io::Result<Option<Arc<dyn AuditSink>>> {
    match value {
        None => Ok(None),
        Some("stderr") => Ok(Some(Arc::new(StderrAudit::new()))),
        Some(path) => Ok(Some(Arc::new(FileAudit::open(Path::new(path))?))),
    }
}

/// `SERVER_AUTH_READ_ONLY_CLIENT_CERTS`/`SERVER_AUTH_READ_WRITE_CLIENT_CERTS`'s
/// decision table (`CLS-FR-005`, ADR-0028), factored so a test can drive
/// it without touching the process environment: either variable, each a
/// `:`-separated list of PEM files, is applied to `auth` via
/// `AuthConfig::with_certificate_class_pem_file`; an unreadable or
/// non-PEM file names the variable in its error. Either variable set
/// while `client_ca_path` is not is refused outright — a class map on a
/// server that never asks for a certificate is inert, and inert security
/// configuration is a mistake to refuse, not honor.
fn certificate_classes_from_env_values(
    mut auth: AuthConfig,
    read_only_certs: Option<&str>,
    read_write_certs: Option<&str>,
    client_ca_path: Option<&str>,
) -> Result<AuthConfig, String> {
    if (read_only_certs.is_some() || read_write_certs.is_some()) && client_ca_path.is_none() {
        return Err(
            "SERVER_AUTH_READ_ONLY_CLIENT_CERTS/SERVER_AUTH_READ_WRITE_CLIENT_CERTS is set but \
             SERVER_TLS_CLIENT_CA_PATH is not — a certificate class map is inert without a \
             client-certificate-requiring TLS config"
                .to_string(),
        );
    }
    for (variable, paths, class) in [
        (
            "SERVER_AUTH_READ_ONLY_CLIENT_CERTS",
            read_only_certs,
            TokenClass::ReadOnly,
        ),
        (
            "SERVER_AUTH_READ_WRITE_CLIENT_CERTS",
            read_write_certs,
            TokenClass::ReadWrite,
        ),
    ] {
        if let Some(paths) = paths {
            for path in paths.split(':') {
                auth = auth
                    .with_certificate_class_pem_file(path, class)
                    .map_err(|e| format!("{variable} configured but invalid: {e}"))?;
            }
        }
    }
    Ok(auth)
}

/// `SERVER_AUTH_RATE_LIMIT`'s decision table (`RL-FR-006`), factored so a
/// test can drive it without touching the process environment: unset →
/// `auth` unchanged; set → `RateLimit::parse`d and applied via
/// `AuthConfig::with_rate_limit`; a malformed value names the variable in
/// its error.
fn rate_limit_from_env_value(auth: AuthConfig, value: Option<&str>) -> Result<AuthConfig, String> {
    match value {
        None => Ok(auth),
        Some(value) => RateLimit::parse(value)
            .map(|limit| auth.with_rate_limit(limit))
            .map_err(|e| format!("SERVER_AUTH_RATE_LIMIT configured but invalid: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RL-FR-006`: the startup decision table for `SERVER_AUTH_RATE_LIMIT`.
    #[test]
    fn rate_limit_from_env_value_follows_the_documented_table() {
        let auth = rate_limit_from_env_value(AuthConfig::default(), None).unwrap();
        assert!(auth.rate_limit().is_none());

        let auth = rate_limit_from_env_value(AuthConfig::default(), Some("10/60")).unwrap();
        assert_eq!(
            auth.rate_limit(),
            Some(RateLimit {
                failures: 10,
                window: std::time::Duration::from_secs(60)
            })
        );

        let err =
            rate_limit_from_env_value(AuthConfig::default(), Some("not-a-limit")).unwrap_err();
        assert!(err.contains("SERVER_AUTH_RATE_LIMIT"));
    }

    /// `CLS-FR-005`: the startup decision table for the certificate-class
    /// environment variables, driven directly rather than through the
    /// process environment.
    #[test]
    fn certificate_classes_from_env_values_follows_the_documented_table() {
        // Nothing set: a no-op, `auth` stays whatever it was.
        let auth =
            certificate_classes_from_env_values(AuthConfig::default(), None, None, None).unwrap();
        assert!(!auth.is_configured());

        // Either variable set without the client CA path is refused.
        assert!(certificate_classes_from_env_values(
            AuthConfig::default(),
            Some("some/path.pem"),
            None,
            None
        )
        .is_err());
        assert!(certificate_classes_from_env_values(
            AuthConfig::default(),
            None,
            Some("some/path.pem"),
            None
        )
        .is_err());

        // A valid PEM file classes its certificate and configures `auth`.
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "dog_server_certificate_classes_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("leaf.pem");
        std::fs::write(&path, cert.pem()).unwrap();
        let auth = certificate_classes_from_env_values(
            AuthConfig::default(),
            Some(path.to_str().unwrap()),
            None,
            Some("dummy-ca-path"),
        )
        .unwrap();
        assert!(auth.is_configured());

        // An unreadable file names the variable in its error.
        let missing = dir.join("missing.pem");
        let err = certificate_classes_from_env_values(
            AuthConfig::default(),
            Some(missing.to_str().unwrap()),
            None,
            Some("dummy-ca-path"),
        )
        .unwrap_err();
        assert!(err.contains("SERVER_AUTH_READ_ONLY_CLIENT_CERTS"));
    }

    #[test]
    fn audit_sink_from_follows_the_documented_table() {
        assert!(audit_sink_from(None).unwrap().is_none());
        assert!(audit_sink_from(Some("stderr")).unwrap().is_some());
        let dir = std::env::temp_dir().join(format!("dog_server_audit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.log");
        assert!(audit_sink_from(Some(path.to_str().unwrap()))
            .unwrap()
            .is_some());
        assert!(path.exists());
        let unopenable = dir.join("missing").join("audit.log");
        assert!(audit_sink_from(Some(unopenable.to_str().unwrap())).is_err());
    }
}
