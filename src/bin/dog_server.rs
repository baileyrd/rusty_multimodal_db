//! A minimal, real server binary for the `Dog` domain — wraps a
//! `ProductionStore` seeded from a small, hand-written sample dataset (not
//! `generator`, which is research-gated) in a `DogConnectionStore` and
//! serves it over TCP. See `rusty_multimodal_db::server`'s own module docs
//! for the protocol and what this deliberately does not provide.
//!
//! # This is a local development tool, not a deployable service
//!
//! No authentication, no authorization, no transport encryption — per
//! ADR-0010 (Accepted), do not expose this beyond a trusted, localhost/
//! development network. Usage: `dog_server [host:port]` (defaults to
//! `127.0.0.1:7878`).

use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::serve;
use rusty_multimodal_db::ProductionStore;
use std::net::TcpListener;
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
    let connection_store = Arc::new(DogConnectionStore::new(store));

    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("binding {addr}: {e}"));
    eprintln!(
        "dog_server listening on {addr} (no auth, no encryption — trusted/localhost use only, see ADR-0010)"
    );

    serve(listener, connection_store);
}
