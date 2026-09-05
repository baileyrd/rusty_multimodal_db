//! `ECO-FR-008` (ii) (ADR-0043): the Python reference client
//! (`clients/python/`) driven against a real `Entity` server over a real
//! socket, at this build's protocol version and at a hand-negotiated
//! `Hello { 10 }` — the rule-3 proof from the other side of the wire.
//! The driver prints `key=value` lines; this test asserts them, then
//! confirms the Python client's one write through the Rust client.
//!
//! **This test needs `python3` on `PATH` and fails loudly without it.**
//! This repository's own posture (`a skipped test is not a test`) is why
//! it panics with a named message rather than silently passing;
//! `ubuntu-latest` and every developer machine this project has used
//! ship `python3`.

use rusty_multimodal_db::generic::entity::{create_entity_production_stack, Entity};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::server::client::SchemaDrivenClient;
use rusty_multimodal_db::server::entity::EntityConnectionStore;
use rusty_multimodal_db::server::protocol::{ScanValue, PROTOCOL_VERSION};
use rusty_multimodal_db::server::{serve, ServeOptions};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

/// The same five entities and edges `tests/server_entity_integration.rs`
/// uses, so the driver's expectations are the ones that suite already
/// proves for the Rust client.
fn start_server() -> SocketAddr {
    let dir = unique_dir("python_client");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entities.mmap");
    let e = |n: u128, label: &str, kind: &str, count: i64, aliases: &[&str]| Entity {
        id: Uuid::from_u128(n),
        label: label.into(),
        kind: kind.into(),
        mention_count: count,
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    };
    let entities = vec![
        e(
            1,
            "Ada Lovelace",
            "person",
            3,
            &["Ada", "Countess of Lovelace"],
        ),
        e(2, "Analytical Engine", "concept", 5, &[]),
        e(3, "London", "place", 1, &["Londinium"]),
        e(
            4,
            "Royal Society",
            "organization",
            2,
            &["The Society", "Babbage"],
        ),
        e(5, "Charles Babbage", "person", 2, &["Babbage"]),
    ];
    let relates_to = [(1u128, 2u128), (2, 3), (3, 1), (3, 4)]
        .map(|(a, b)| (Uuid::from_u128(a), Uuid::from_u128(b)));
    let mentioned_with = [(Uuid::from_u128(1), Uuid::from_u128(5))];
    let stack =
        create_entity_production_stack(entities, &relates_to, &mentioned_with, &path).unwrap();
    let store = Arc::new(EntityConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, store, ServeOptions::default()));
    addr
}

/// Run the driver once; return its `key=value` lines as a map. A missing
/// `python3` is a named panic, never a skip.
fn drive(addr: SocketAddr, hello: u32) -> HashMap<String, String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("python3")
        .arg("clients/python/driver.py")
        .arg(addr.to_string())
        .arg(hello.to_string())
        .current_dir(manifest)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "python3 is required by tests/server_python_client.rs (ECO-FR-008, ADR-0043) \
                 and could not be started: {e}. Install Python 3 or run without this test target."
            )
        });
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "driver.py (Hello {{ {hello} }}) exited with {}\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_python_reference_client_speaks_the_protocol_at_12_and_at_10() {
    let addr = start_server();

    // This build's version: four fields, the StrList, one of each read
    // shape, one write, and a protocol-12 join.
    let v12 = drive(addr, PROTOCOL_VERSION);
    let get = |k: &str| {
        v12.get(k)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("no {k}=: {v12:?}"))
    };
    assert_eq!(get("negotiated"), "12");
    assert_eq!(get("schema_fields"), "label,kind,mention_count,aliases");
    assert_eq!(get("relations"), "mentioned_with,neighbors,relates_to");
    assert_eq!(get("get_fields"), "4");
    assert_eq!(get("get_aliases"), "Ada|Countess of Lovelace");
    assert_eq!(get("get_missing"), "none");
    assert_eq!(
        get("filter_eq_ids"),
        "1",
        "FilterEq label 'ADA' resolves the alias"
    );
    assert_eq!(get("filter_eq_first"), "1");
    assert_eq!(get("query_rows"), "2", "two persons");
    assert_eq!(get("aggregate_groups"), "4", "four kinds");
    assert_eq!(get("update"), "true");
    assert_eq!(
        get("update_label"),
        "unsupported",
        "client-side capability check"
    );
    assert_eq!(
        get("neighbors"),
        "3",
        "1 relates to 2 and 3, is mentioned with 5"
    );
    assert_eq!(get("join_rows"), "8", "relates_to, both orientations");

    // A hand-negotiated version 10: the FR-042 three-field shape, no
    // aliases, no relation list, no join — rule 3 seen from Python.
    let v10 = drive(addr, 10);
    let get = |k: &str| {
        v10.get(k)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("no {k}=: {v10:?}"))
    };
    assert_eq!(get("negotiated"), "10");
    assert_eq!(get("schema_fields"), "label,kind,mention_count");
    assert_eq!(get("relations"), "");
    assert_eq!(get("get_fields"), "3");
    assert_eq!(get("get_aliases"), "-");
    assert!(
        get("join_rows").starts_with("unsupported"),
        "{}",
        get("join_rows")
    );
    assert_eq!(get("update"), "true");

    // The Python client's write is real: the Rust client sees 42.
    let mut rust = SchemaDrivenClient::connect(addr).unwrap();
    let ada = rust.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(ada
        .iter()
        .any(|(name, v)| name == "mention_count" && *v == ScanValue::I64(42)));
}
