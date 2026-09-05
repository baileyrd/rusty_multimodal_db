//! A minimal, real server binary for the `Entity` domain — wraps a
//! `GenericProductionStore<EntityProductionStack>` seeded from a small,
//! hand-written sample dataset (with `relates_to` edges) in an
//! `EntityConnectionStore` and serves it over TCP. Mirrors
//! `reminder_server.rs`'s own shape and env-var reading exactly
//! (`ENT-FR-006`, ADR-0037) — every operational knob `dog_server`/
//! `reminder_server` exposes is exposed here too, unchanged. See
//! `rusty_multimodal_db::server`'s own module docs for the protocol and
//! what this deliberately does not provide.
//!
//! # This is a local development tool, not a deployable service
//!
//! Authentication/authorization (`ServeOptions::from_env`, ADR-0012) and
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
//! by ADR-0026's group commit; and, since ADR-0031, an opt-in per-request
//! access log independent of the audit log, `SERVER_ACCESS_LOG` (`stderr`
//! or a file path)) — with none of them set, this behaves exactly as it
//! did before any of these features existed: no auth, no encryption, no
//! journal, no audit log (`SERVER_AUDIT_LOG`, ADR-0029: `stderr` or a file
//! path), no access log. Do not expose this beyond a trusted,
//! localhost/development network unless both auth and TLS are configured
//! — see ADR-0010's Consequences. Usage: `entity_server [host:port]`
//! (defaults to `127.0.0.1:7880`).

use rusty_multimodal_db::generic::entity::{create_entity_production_stack, Entity};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::server::access::{AccessSink, FileAccessLog, StderrAccessLog};
use rusty_multimodal_db::server::audit::{AuditSink, FileAudit, StderrAudit};
use rusty_multimodal_db::server::entity::EntityConnectionStore;
use rusty_multimodal_db::server::{serve, RateLimit, ServeOptions, TlsConfig, TokenClass};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

fn sample_entities() -> Vec<Entity> {
    vec![
        Entity {
            id: Uuid::from_u128(1),
            label: "Ada Lovelace".into(),
            kind: "person".into(),
            mention_count: 3,
            aliases: vec!["Ada".into(), "Countess of Lovelace".into()],
        },
        Entity {
            id: Uuid::from_u128(2),
            label: "Analytical Engine".into(),
            kind: "concept".into(),
            mention_count: 5,
            aliases: vec![],
        },
        Entity {
            id: Uuid::from_u128(3),
            label: "London".into(),
            kind: "place".into(),
            mention_count: 1,
            aliases: vec![],
        },
    ]
}

fn sample_relates_to_edges() -> Vec<(Uuid, Uuid)> {
    vec![(Uuid::from_u128(1), Uuid::from_u128(2))]
}

fn sample_mentioned_with_edges() -> Vec<(Uuid, Uuid)> {
    vec![(Uuid::from_u128(1), Uuid::from_u128(3))]
}

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7880".to_string());

    let dir = std::env::temp_dir().join(format!("entity_server_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating a scratch directory for the mmap-backed store");
    let path = dir.join("entities.mmap");

    let store = create_entity_production_stack(
        sample_entities(),
        &sample_relates_to_edges(),
        &sample_mentioned_with_edges(),
        &path,
    )
    .expect("creating the sample EntityProductionStack");
    // `SERVER_TXN_JOURNAL_PATH` (ADR-0025): with it, every transaction
    // batch is crash-atomic — journaled and fsync'd before its first
    // write, replayed on the next start. Set it the same way every start:
    // opening without it after a crash forgoes the replay.
    let connection_store = Arc::new(match std::env::var("SERVER_TXN_JOURNAL_PATH") {
        Ok(journal_path) => EntityConnectionStore::with_journal(
            GenericProductionStore::new(store),
            Path::new(&journal_path),
        )
        .unwrap_or_else(|e| panic!("SERVER_TXN_JOURNAL_PATH configured but invalid: {e}")),
        Err(_) => EntityConnectionStore::new(GenericProductionStore::new(store)),
    });
    let journaled = std::env::var_os("SERVER_TXN_JOURNAL_PATH").is_some();

    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("binding {addr}: {e}"));

    // `SERVER_AUDIT_LOG` (ADR-0029): `stderr`, or a file path appended to;
    // unset → no audit. An unopenable path is a startup error.
    let auth = match audit_sink_from(std::env::var("SERVER_AUDIT_LOG").ok().as_deref()) {
        Ok(Some(sink)) => ServeOptions::from_env().with_audit(sink),
        Ok(None) => ServeOptions::from_env(),
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
    // `SERVER_ACCESS_LOG` (ADR-0031, `ACC-FR-007`): `stderr`, or a file
    // path appended to; unset → no access log. Independent of
    // `SERVER_AUDIT_LOG` — an unopenable path is a startup error.
    let auth = match access_sink_from(std::env::var("SERVER_ACCESS_LOG").ok().as_deref()) {
        Ok(Some(sink)) => auth.with_access_log(sink),
        Ok(None) => auth,
        Err(e) => panic!("SERVER_ACCESS_LOG configured but invalid: {e}"),
    };
    let access_logged = std::env::var_os("SERVER_ACCESS_LOG").is_some();
    let audited = std::env::var_os("SERVER_AUDIT_LOG").is_some();
    let tls = match TlsConfig::from_env() {
        None => None,
        Some(Ok(tls)) => Some(tls),
        Some(Err(e)) => panic!(
            "SERVER_TLS_CERT_CHAIN_PATH/SERVER_TLS_PRIVATE_KEY_PATH/SERVER_TLS_CLIENT_CA_PATH configured but invalid: {e}"
        ),
    };
    // `SRV-FR-003` (ADR-0032): `TlsConfig` stays its own separately-fallible
    // construction step, folded in only after its `Result` is handled above.
    let options = match tls {
        Some(tls) => auth.with_tls(tls),
        None => auth,
    };
    eprintln!(
        "entity_server listening on {addr} (auth: {}, TLS: {}, transaction journal: {}, audit log: {}, auth rate limit: {}, access log: {} — see ADR-0012/ADR-0014/ADR-0023/ADR-0025/ADR-0029/ADR-0030/ADR-0031/ADR-0037; do not expose beyond a trusted network unless auth and TLS are both configured)",
        if options.is_configured() { "configured" } else { "NOT configured" },
        match options.tls() {
            None => "NOT configured",
            Some(tls) if tls.requires_client_certificate() => "configured, client certificate required",
            Some(_) => "configured",
        },
        if journaled { "configured" } else { "NOT configured" },
        if audited { "configured" } else { "NOT configured" },
        if rate_limited { "configured" } else { "lockout only (default)" },
        if access_logged { "configured" } else { "NOT configured" },
    );

    serve(listener, connection_store, options);
}

/// `SERVER_AUDIT_LOG`'s decision table (`AUD-FR-008`) — see
/// `dog_server`'s identical function for the full contract.
fn audit_sink_from(value: Option<&str>) -> std::io::Result<Option<Arc<dyn AuditSink>>> {
    match value {
        None => Ok(None),
        Some("stderr") => Ok(Some(Arc::new(StderrAudit::new()))),
        Some(path) => Ok(Some(Arc::new(FileAudit::open(Path::new(path))?))),
    }
}

/// `SERVER_ACCESS_LOG`'s decision table (`ACC-FR-007`) — see
/// `dog_server`'s identical function for the full contract.
fn access_sink_from(value: Option<&str>) -> std::io::Result<Option<Arc<dyn AccessSink>>> {
    match value {
        None => Ok(None),
        Some("stderr") => Ok(Some(Arc::new(StderrAccessLog::new()))),
        Some(path) => Ok(Some(Arc::new(FileAccessLog::open(Path::new(path))?))),
    }
}

/// `SERVER_AUTH_READ_ONLY_CLIENT_CERTS`/`SERVER_AUTH_READ_WRITE_CLIENT_CERTS`'s
/// decision table (`CLS-FR-005`, ADR-0028) — see `dog_server`'s identical
/// function for the full contract.
fn certificate_classes_from_env_values(
    mut auth: ServeOptions,
    read_only_certs: Option<&str>,
    read_write_certs: Option<&str>,
    client_ca_path: Option<&str>,
) -> Result<ServeOptions, String> {
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

/// `SERVER_AUTH_RATE_LIMIT`'s decision table (`RL-FR-006`) — see
/// `dog_server`'s identical function for the full contract.
fn rate_limit_from_env_value(
    auth: ServeOptions,
    value: Option<&str>,
) -> Result<ServeOptions, String> {
    match value {
        None => Ok(auth),
        Some(value) => RateLimit::parse(value)
            .map(|limit| auth.with_rate_limit(limit))
            .map_err(|e| format!("SERVER_AUTH_RATE_LIMIT configured but invalid: {e}")),
    }
}
