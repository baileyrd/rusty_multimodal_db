//! Real end-to-end coverage of [`SchemaDrivenClient`]
//! (`src/server/client.rs`) — the client half of ADR-0011's schema
//! discovery, promoted from `tests/server_{dog,order,employee}_integration.rs`'s
//! own one-off schema-driven tests into real, reusable library API. This
//! file never imports a `FIELD_*` constant from `server::{dog,order,
//! employee}` — every field this test drives is addressed by name, the
//! same discipline the client itself follows, proving the promotion
//! didn't quietly reintroduce compile-time domain knowledge through the
//! test's own back door.
//!
//! All three domains in one file, `required-features = ["server",
//! "research"]` — matching `benches/server.rs`'s own precedent for a
//! single target that needs every domain adapter at once.

use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Order, OrderStatus,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic_spike::employee_impl::{
    create_employee_production_stack, Department, Employee,
};
use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::client::{ClientError, SchemaDrivenClient};
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::employee::EmployeeConnectionStore;
use rusty_multimodal_db::server::order::OrderConnectionStore;
use rusty_multimodal_db::server::protocol::{ParentLookup, ScanValue};
use rusty_multimodal_db::server::{serve, AuthConfig};
use rusty_multimodal_db::ProductionStore;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s identical helper for why this
/// needs to be unique per call, not just per process.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn start_dog_server() -> SocketAddr {
    let dir = unique_dir("schema_client_dog");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let records = vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "labrador", 5),
    ];
    let edges = vec![(Uuid::from_u128(1), Uuid::from_u128(2))];
    let store = ProductionStore::create(records, edges, &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default()));
    addr
}

fn start_order_server() -> SocketAddr {
    let dir = unique_dir("schema_client_order");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("amount.mmap");
    let orders = vec![
        Order {
            id: Uuid::from_u128(1),
            customer_id: Uuid::from_u128(100),
            amount_cents: 2_500,
            status: OrderStatus::Shipped,
            created_at_unix_ms: 1_000,
            discount_cents: 0,
        },
        Order {
            id: Uuid::from_u128(2),
            customer_id: Uuid::from_u128(100),
            amount_cents: 4_200,
            status: OrderStatus::Pending,
            created_at_unix_ms: 2_000,
            discount_cents: 0,
        },
    ];
    let stack = create_order_production_stack(orders, &path).unwrap();
    let connection_store = Arc::new(OrderConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default()));
    addr
}

fn start_employee_server() -> SocketAddr {
    let dir = unique_dir("schema_client_employee");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("salary.mmap");
    let employees = vec![
        Employee {
            id: Uuid::from_u128(1),
            name: "Alex".into(),
            department: Department::Engineering,
            salary_cents: 1_200_000,
            manager_id: None,
        },
        Employee {
            id: Uuid::from_u128(2),
            name: "Bel".into(),
            department: Department::Engineering,
            salary_cents: 950_000,
            manager_id: Some(Uuid::from_u128(1)),
        },
    ];
    let edges = vec![];
    let stack = create_employee_production_stack(employees, &edges, &path).unwrap();
    let connection_store = Arc::new(EmployeeConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, AuthConfig::default()));
    addr
}

#[test]
fn dog_client_discovers_and_drives_by_name_with_no_field_constants() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(client.schema().relations.neighbors);
    assert!(!client.schema().relations.parent_children);

    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(fields
        .iter()
        .any(|(name, value)| name == "breed" && *value == ScanValue::Str("labrador".into())));
    assert!(fields
        .iter()
        .any(|(name, value)| name == "age" && *value == ScanValue::U32(3)));
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());

    let ages = client.scan("age").unwrap();
    let mut ages_sorted: Vec<u32> = ages
        .into_iter()
        .map(|v| match v {
            ScanValue::U32(n) => n,
            other => panic!("expected U32, got {other:?}"),
        })
        .collect();
    ages_sorted.sort();
    assert_eq!(ages_sorted, vec![3, 5]);

    assert!(client
        .update(Uuid::from_u128(1), "age", ScanValue::U32(4))
        .unwrap());
    assert!(!client
        .update(Uuid::from_u128(99), "age", ScanValue::U32(4))
        .unwrap());

    // Real symmetric relation.
    assert_eq!(
        client.neighbors(Uuid::from_u128(1)).unwrap(),
        vec![Uuid::from_u128(2)]
    );

    // Dog has no directed relation and no filter_eq-capable field —
    // checked client-side, no round trip needed.
    assert!(matches!(
        client.parent(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.children(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.filter_eq("breed", ScanValue::Str("labrador".into())),
        Err(ClientError::Unsupported(_))
    ));
}

#[test]
fn order_client_discovers_and_drives_by_name_with_no_field_constants() {
    let addr = start_order_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(client.schema().relations.parent_children);
    assert!(!client.schema().relations.neighbors);

    // Shipped == 1, per server::order's own status_to_u32 mapping — the
    // client doesn't need to know that encoding, only what GetById
    // returned for order 1's discovered "status" field.
    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    let status_value = fields
        .iter()
        .find(|(name, _)| name == "status")
        .map(|(_, v)| v.clone())
        .expect("GetById should include the status field");

    assert_eq!(
        client.filter_eq("status", status_value).unwrap(),
        vec![Uuid::from_u128(1)]
    );

    let amounts = client.scan("amount_cents").unwrap();
    let mut amounts_sorted: Vec<i64> = amounts
        .into_iter()
        .map(|v| match v {
            ScanValue::I64(n) => n,
            other => panic!("expected I64, got {other:?}"),
        })
        .collect();
    amounts_sorted.sort();
    assert_eq!(amounts_sorted, vec![2_500, 4_200]);

    assert!(client
        .update(Uuid::from_u128(1), "amount_cents", ScanValue::I64(9_000))
        .unwrap());

    // Parent: an Order id in, a Customer id out.
    assert_eq!(
        client.parent(Uuid::from_u128(1)).unwrap(),
        ParentLookup::Parent(Uuid::from_u128(100))
    );
    // Children: a Customer id in, Order ids out.
    let mut children = client.children(Uuid::from_u128(100)).unwrap();
    children.sort();
    assert_eq!(children, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);

    // No symmetric relation for Order/Customer — checked client-side.
    assert!(matches!(
        client.neighbors(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));

    // Read-only fields (never part of the durable stack this server
    // wraps) exist in the schema but support none of the three
    // operations — checked client-side too, not just server-side.
    assert!(matches!(
        client.update(Uuid::from_u128(1), "created_at_unix_ms", ScanValue::I64(0)),
        Err(ClientError::Unsupported(_))
    ));
}

#[test]
fn employee_client_discovers_and_drives_by_name_with_no_field_constants() {
    let addr = start_employee_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    // The first domain where both relation kinds are real.
    assert!(client.schema().relations.parent_children);
    assert!(client.schema().relations.neighbors);

    let fields = client.get(Uuid::from_u128(2)).unwrap().unwrap();
    assert!(fields
        .iter()
        .any(|(name, value)| name == "name" && *value == ScanValue::Str("Bel".into())));

    let engineers = client.filter_eq("department", ScanValue::U32(0)).unwrap();
    assert_eq!(engineers.len(), 2);

    assert!(client
        .update(
            Uuid::from_u128(2),
            "salary_cents",
            ScanValue::I64(1_000_000)
        )
        .unwrap());

    assert_eq!(
        client.parent(Uuid::from_u128(2)).unwrap(),
        ParentLookup::Parent(Uuid::from_u128(1))
    );
    assert_eq!(
        client.children(Uuid::from_u128(1)).unwrap(),
        vec![Uuid::from_u128(2)]
    );
    // No collaboration edges in this fixture — a real, empty relation
    // result, not an error.
    assert_eq!(
        client.neighbors(Uuid::from_u128(2)).unwrap(),
        Vec::<Uuid>::new()
    );
}

#[test]
fn an_unknown_field_name_is_a_client_side_error_no_round_trip_needed() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(matches!(
        client.filter_eq("nonexistent_field", ScanValue::U32(0)),
        Err(ClientError::UnknownField(name)) if name == "nonexistent_field"
    ));
    assert!(matches!(
        client.scan("nonexistent_field"),
        Err(ClientError::UnknownField(_))
    ));
    assert!(matches!(
        client.update(Uuid::from_u128(1), "nonexistent_field", ScanValue::U32(0)),
        Err(ClientError::UnknownField(_))
    ));
}
