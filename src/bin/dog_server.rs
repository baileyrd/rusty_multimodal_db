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
//! naming the CA roots every client certificate must chain to; and
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
use rusty_multimodal_db::server::{serve, AuthConfig, TlsConfig};
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
    let audited = std::env::var_os("SERVER_AUDIT_LOG").is_some();
    let tls = match TlsConfig::from_env() {
        None => None,
        Some(Ok(tls)) => Some(tls),
        Some(Err(e)) => panic!(
            "SERVER_TLS_CERT_CHAIN_PATH/SERVER_TLS_PRIVATE_KEY_PATH/SERVER_TLS_CLIENT_CA_PATH configured but invalid: {e}"
        ),
    };
    eprintln!(
        "dog_server listening on {addr} (auth: {}, TLS: {}, transaction journal: {}, audit log: {} — see ADR-0012/ADR-0014/ADR-0023/ADR-0025/ADR-0029; do not expose beyond a trusted network unless auth and TLS are both configured)",
        if auth.is_configured() { "configured" } else { "NOT configured" },
        match &tls {
            None => "NOT configured",
            Some(tls) if tls.requires_client_certificate() => "configured, client certificate required",
            Some(_) => "configured",
        },
        if journaled { "configured" } else { "NOT configured" },
        if audited { "configured" } else { "NOT configured" },
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

#[cfg(test)]
mod tests {
    use super::*;

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
