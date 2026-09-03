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
use rusty_multimodal_db::server::client::{ClientError, SchemaDrivenClient, SessionOptions};
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::employee::EmployeeConnectionStore;
use rusty_multimodal_db::server::order::OrderConnectionStore;
use rusty_multimodal_db::server::protocol::{ErrorCode, ParentLookup, ScanValue};
use rusty_multimodal_db::server::{serve, ServeOptions};
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
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
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
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
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
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
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

fn value_of(client: &mut SchemaDrivenClient, id: Uuid, field: &str) -> ScanValue {
    client
        .get(id)
        .unwrap()
        .unwrap()
        .into_iter()
        .find_map(|(name, value)| (name == field).then_some(value))
        .unwrap_or_else(|| panic!("no field {field:?}"))
}

/// `SESS-FR-008` (design criterion 7, `SERVER-001-FR-024`): `begin` →
/// `Session::update` (the staged index) → `commit` applies; `rollback`
/// and a dropped `Session` discard; a rejected commit is
/// `ClientError::TransactionFailed` naming the staged index with nothing
/// applied; the client-side capability check still runs before staging.
/// All three domains, by field name only.
#[test]
fn sessions_stage_commit_and_roll_back_on_every_domain() {
    let mut dog = SchemaDrivenClient::connect(start_dog_server()).unwrap();
    assert_eq!(dog.server_protocol_version(), 7);

    let mut s = dog.begin().unwrap();
    assert_eq!(
        s.update(Uuid::from_u128(1), "age", ScanValue::U32(7))
            .unwrap(),
        0
    );
    assert_eq!(
        s.update(Uuid::from_u128(2), "age", ScanValue::U32(8))
            .unwrap(),
        1
    );
    s.commit().unwrap();
    assert_eq!(
        value_of(&mut dog, Uuid::from_u128(1), "age"),
        ScanValue::U32(7)
    );
    assert_eq!(
        value_of(&mut dog, Uuid::from_u128(2), "age"),
        ScanValue::U32(8)
    );

    let mut s = dog.begin().unwrap();
    s.update(Uuid::from_u128(1), "age", ScanValue::U32(99))
        .unwrap();
    s.rollback().unwrap();
    assert_eq!(
        value_of(&mut dog, Uuid::from_u128(1), "age"),
        ScanValue::U32(7)
    );

    {
        let mut s = dog.begin().unwrap();
        s.update(Uuid::from_u128(1), "age", ScanValue::U32(99))
            .unwrap();
        // Dropped without commit or rollback: rolled back best-effort.
    }
    assert_eq!(
        value_of(&mut dog, Uuid::from_u128(1), "age"),
        ScanValue::U32(7)
    );

    let mut s = dog.begin().unwrap();
    assert!(matches!(
        s.update(Uuid::from_u128(1), "breed", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        s.update(Uuid::from_u128(1), "nonexistent", ScanValue::U32(0)),
        Err(ClientError::UnknownField(_))
    ));
    assert_eq!(
        s.update(Uuid::from_u128(99), "age", ScanValue::U32(1))
            .unwrap(),
        0
    );
    match s.commit() {
        Err(ClientError::TransactionFailed {
            index: 0,
            code: ErrorCode::RecordNotFound,
            ..
        }) => {}
        other => panic!("expected TransactionFailed at staged index 0, got {other:?}"),
    }
    assert_eq!(
        value_of(&mut dog, Uuid::from_u128(1), "age"),
        ScanValue::U32(7)
    );

    let mut order = SchemaDrivenClient::connect(start_order_server()).unwrap();
    let mut s = order.begin().unwrap();
    assert_eq!(
        s.update(Uuid::from_u128(1), "amount_cents", ScanValue::I64(9_000))
            .unwrap(),
        0
    );
    s.commit().unwrap();
    assert_eq!(
        value_of(&mut order, Uuid::from_u128(1), "amount_cents"),
        ScanValue::I64(9_000)
    );

    let mut employee = SchemaDrivenClient::connect(start_employee_server()).unwrap();
    let mut s = employee.begin().unwrap();
    assert_eq!(
        s.update(
            Uuid::from_u128(1),
            "salary_cents",
            ScanValue::I64(1_500_000)
        )
        .unwrap(),
        0
    );
    s.commit().unwrap();
    assert_eq!(
        value_of(&mut employee, Uuid::from_u128(1), "salary_cents"),
        ScanValue::I64(1_500_000)
    );
}

/// `RYW-FR-007` (design criterion 6): `begin_read_your_writes` opens a
/// session whose `get` shows its staged writes; a plain `begin` session's
/// `get` shows committed state; both commit as before.
#[test]
fn read_your_writes_sessions_see_their_own_staged_writes_through_the_client() {
    let mut dog = SchemaDrivenClient::connect(start_dog_server()).unwrap();
    let id = Uuid::from_u128(1);
    let before = value_of(&mut dog, id, "age");

    let mut s = dog.begin().unwrap();
    assert!(!s.read_your_writes());
    s.update(id, "age", ScanValue::U32(21)).unwrap();
    let plain = s.get(id).unwrap().unwrap();
    assert!(
        plain.contains(&("age".to_string(), before.clone())),
        "plain session reads committed: {plain:?}"
    );
    s.rollback().unwrap();

    let mut s = dog.begin_read_your_writes().unwrap();
    assert!(s.read_your_writes());
    s.update(id, "age", ScanValue::U32(22)).unwrap();
    let seen = s.get(id).unwrap().unwrap();
    assert!(
        seen.contains(&("age".to_string(), ScanValue::U32(22))),
        "own write visible: {seen:?}"
    );
    assert!(seen.iter().any(|(name, _)| name == "breed"));
    assert!(s.get(Uuid::from_u128(99)).unwrap().is_none());
    s.commit().unwrap();
    assert_eq!(value_of(&mut dog, id, "age"), ScanValue::U32(22));
}

/// `STV-FR-004`: `begin_with(SessionOptions)` — a validating session's
/// `update` reports a bad write as `ClientError::Server` with the code
/// `commit` would have given and stages nothing; no options is `begin`;
/// both options compose.
#[test]
fn validating_sessions_report_bad_writes_at_update_through_the_client() {
    let mut dog = SchemaDrivenClient::connect(start_dog_server()).unwrap();
    let id = Uuid::from_u128(1);

    let mut s = dog
        .begin_with(SessionOptions::new().validate_on_stage())
        .unwrap();
    assert!(s.validate_on_stage() && !s.read_your_writes());
    assert!(matches!(
        s.update(Uuid::from_u128(99), "age", ScanValue::U32(1)),
        Err(ClientError::Server(ErrorCode::RecordNotFound, _))
    ));
    assert_eq!(s.update(id, "age", ScanValue::U32(31)).unwrap(), 0);
    s.commit().unwrap();
    assert_eq!(value_of(&mut dog, id, "age"), ScanValue::U32(31));

    let mut s = dog
        .begin_with(SessionOptions::new().validate_on_stage().read_your_writes())
        .unwrap();
    assert!(s.validate_on_stage() && s.read_your_writes());
    assert_eq!(s.update(id, "age", ScanValue::U32(32)).unwrap(), 0);
    assert!(s
        .get(id)
        .unwrap()
        .unwrap()
        .contains(&("age".to_string(), ScanValue::U32(32))));
    s.rollback().unwrap();

    let mut s = dog.begin_with(SessionOptions::new()).unwrap();
    assert!(!s.validate_on_stage() && !s.read_your_writes());
    assert_eq!(
        s.update(Uuid::from_u128(99), "age", ScanValue::U32(1))
            .unwrap(),
        0
    );
    assert!(matches!(
        s.commit(),
        Err(ClientError::TransactionFailed {
            index: 0,
            code: ErrorCode::RecordNotFound,
            ..
        })
    ));
}
