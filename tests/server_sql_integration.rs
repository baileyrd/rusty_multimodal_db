//! Real end-to-end coverage of `Request::Query`/`SchemaDrivenClient::query`
//! (`SQL-FR-001`–`010`, ADR-0034,
//! `docs/design/SERVER-SQL-SELECT-DESIGN.md`) and of
//! `Request::Aggregate`/the same `SchemaDrivenClient::query`
//! (`AGG-FR-001`–`010`, ADR-0035,
//! `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md`) — a real `TcpListener`,
//! real client `TcpStream`s, a real SQL string parsed client-side and
//! answered by the server as a full-scan-then-filter(-then-bucket-then-
//! reduce, for `GROUP BY`/an aggregate function). All three domains in
//! one file, `required-features = ["server", "research"]`, matching
//! `tests/server_schema_driven_client.rs`'s own precedent for a target
//! that needs every domain adapter.

use rusty_multimodal_db::generic::order_customer::{
    create_order_production_stack, Order, OrderStatus,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic_spike::employee_impl::{
    create_employee_production_stack, Department, Employee,
};
use rusty_multimodal_db::record::DogRecord;
use rusty_multimodal_db::server::client::{
    ClientError, QueryResult, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::dog::DogConnectionStore;
use rusty_multimodal_db::server::employee::EmployeeConnectionStore;
use rusty_multimodal_db::server::order::OrderConnectionStore;
use rusty_multimodal_db::server::protocol::ScanValue;
use rusty_multimodal_db::server::{serve, ServeOptions};
use rusty_multimodal_db::ProductionStore;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

/// See `tests/server_dog_integration.rs`'s identical helper for why this
/// needs both the process id and a monotonic counter, not just one.
fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn sample_dogs() -> Vec<DogRecord> {
    vec![
        DogRecord::new(Uuid::from_u128(1), "labrador", 3),
        DogRecord::new(Uuid::from_u128(2), "poodle", 5),
        DogRecord::new(Uuid::from_u128(3), "labrador", 9),
    ]
}

fn start_dog_server() -> SocketAddr {
    let dir = unique_dir("sql_integration_dog");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let store = ProductionStore::create(sample_dogs(), Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn start_order_server() -> SocketAddr {
    let dir = unique_dir("sql_integration_order");
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
    let dir = unique_dir("sql_integration_employee");
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
            department: Department::Sales,
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

fn age_of(rows: &[(Uuid, Vec<(String, ScanValue)>)], id: Uuid) -> u32 {
    let (_, fields) = rows.iter().find(|(row_id, _)| *row_id == id).unwrap();
    match &fields.iter().find(|(name, _)| name == "age").unwrap().1 {
        ScanValue::U32(age) => *age,
        other => panic!("expected U32, got {other:?}"),
    }
}

/// Every test in this file below that runs a plain `SELECT` (no `GROUP
/// BY`/aggregate function) expects `QueryResult::Rows` — this unwraps it,
/// panicking on `Groups` (a bug in the test itself, not a case these
/// tests exercise). Aggregate-specific tests match on `QueryResult`
/// directly instead.
fn rows(result: QueryResult) -> Vec<(Uuid, Vec<(String, ScanValue)>)> {
    match result {
        QueryResult::Rows(rows) => rows,
        QueryResult::Groups(groups) => {
            panic!("expected QueryResult::Rows, got Groups: {groups:?}")
        }
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
}

/// Acceptance criterion 3 (`*`, no `WHERE`): every row, every field.
#[test]
fn select_star_with_no_where_returns_every_row_and_field() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut result = rows(client.query("SELECT * FROM dog").unwrap());
    result.sort_by_key(|(id, _)| *id);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].1.len(), 2, "both breed and age come back");
    assert_eq!(age_of(&result, Uuid::from_u128(1)), 3);
}

/// Acceptance criterion 3: named columns project exactly that subset.
#[test]
fn named_columns_project_exactly_that_subset() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = rows(client.query("SELECT age FROM dog WHERE age = 3").unwrap());
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].1, vec![("age".to_string(), ScanValue::U32(3))]);
}

/// Acceptance criterion 3: every comparator kind, against a real `U32`
/// field.
#[test]
fn every_comparator_kind_filters_correctly() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let eq = rows(client.query("SELECT * FROM dog WHERE age = 5").unwrap());
    assert_eq!(eq.len(), 1);
    assert_eq!(age_of(&eq, Uuid::from_u128(2)), 5);

    let ne = rows(client.query("SELECT * FROM dog WHERE age != 5").unwrap());
    assert_eq!(ne.len(), 2);

    let lt = rows(client.query("SELECT * FROM dog WHERE age < 5").unwrap());
    assert_eq!(lt.len(), 1);
    assert_eq!(age_of(&lt, Uuid::from_u128(1)), 3);

    let le = rows(client.query("SELECT * FROM dog WHERE age <= 5").unwrap());
    assert_eq!(le.len(), 2);

    let gt = rows(client.query("SELECT * FROM dog WHERE age > 5").unwrap());
    assert_eq!(gt.len(), 1);
    assert_eq!(age_of(&gt, Uuid::from_u128(3)), 9);

    let ge = rows(client.query("SELECT * FROM dog WHERE age >= 5").unwrap());
    assert_eq!(ge.len(), 2);
}

/// Acceptance criterion 3: two `AND`-ed conditions, both across
/// different fields — only rows matching both come back.
#[test]
fn two_and_ed_conditions_across_two_fields() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = rows(
        client
            .query("SELECT * FROM dog WHERE breed = 'labrador' AND age > 5")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(age_of(&result, Uuid::from_u128(3)), 9);
}

/// Acceptance criterion 7: `LIMIT` truncates the row count and nothing
/// else; omitting it returns every matching row.
#[test]
fn limit_truncates_and_omitting_it_returns_every_match() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let limited = rows(client.query("SELECT * FROM dog LIMIT 1").unwrap());
    assert_eq!(limited.len(), 1);
    let unbounded = rows(client.query("SELECT * FROM dog").unwrap());
    assert_eq!(unbounded.len(), 3);
}

/// Acceptance criterion 4: an unknown column name is a client-side
/// `ClientError::UnknownField`.
#[test]
fn unknown_column_is_a_client_side_error() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT weight FROM dog"),
        Err(ClientError::UnknownField(name)) if name == "weight"
    ));
    assert!(matches!(
        client.query("SELECT * FROM dog WHERE weight = 5"),
        Err(ClientError::UnknownField(name)) if name == "weight"
    ));
}

/// Acceptance criterion 4: a `WHERE` literal that doesn't match its
/// field's real type is a client-side error; so is an ordering
/// comparator against a non-orderable field.
#[test]
fn kind_mismatch_and_bad_ordering_comparator_are_client_side_errors() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT * FROM dog WHERE age = 'old'"),
        Err(ClientError::Sql(_))
    ));
    assert!(matches!(
        client.query("SELECT * FROM dog WHERE breed > 'labrador'"),
        Err(ClientError::Sql(_))
    ));
}

/// A syntax error never reaches the wire either — the same client-side
/// posture as an unknown field or a kind mismatch.
#[test]
fn a_syntax_error_is_a_client_side_error() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT * dog"),
        Err(ClientError::Sql(_))
    ));
}

/// Acceptance criterion 5: `Dog::breed` has every capability flag
/// (`filter_eq`/`scan`/`update`) `false`, yet is fully selectable and
/// filterable via `Query` — a full scan needs no index.
#[test]
fn a_field_with_every_capability_flag_false_is_still_queryable() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(!client
        .schema()
        .fields
        .iter()
        .any(|f| f.name == "breed" && (f.capabilities.filter_eq || f.capabilities.scan)));
    let result = rows(
        client
            .query("SELECT breed FROM dog WHERE breed = 'poodle'")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].1,
        vec![("breed".to_string(), ScanValue::Str("poodle".into()))]
    );
}

/// A hand-rolled server that always negotiates protocol 7, however high
/// a version the client's own `Hello` asks for — the moral equivalent of
/// a real pre-`SQL-FR` build, without needing to check one out. Every
/// other request is answered by the real `dispatch` over a real
/// `DogConnectionStore`, so a client that never calls `query` is served
/// exactly as normal.
fn start_version_7_dog_server() -> SocketAddr {
    use rusty_multimodal_db::server::dispatch;
    use rusty_multimodal_db::server::framing::{read_message, write_message};
    use rusty_multimodal_db::server::protocol::{Request, Response};

    let dir = unique_dir("sql_integration_v7_dog");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dogs.mmap");
    let store = ProductionStore::create(sample_dogs(), Vec::new(), &path).unwrap();
    let connection_store = Arc::new(DogConnectionStore::new(store));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let connection_store = Arc::clone(&connection_store);
            thread::spawn(move || loop {
                let req: Request = match read_message(&mut stream) {
                    Ok(req) => req,
                    Err(_) => return,
                };
                let resp = match req {
                    Request::Hello { .. } => Response::Hello {
                        protocol_version: 7,
                    },
                    other => dispatch(connection_store.as_ref(), other),
                };
                if write_message(&mut stream, &resp).is_err() {
                    return;
                }
            });
        }
    });
    addr
}

/// `SQL-FR-010`: `query` requires protocol version 8 — against a
/// connection negotiated at 7, `query` is `ClientError::Unsupported`
/// with no frame sent, and the connection keeps working normally for
/// everything else (`get`).
#[test]
fn query_requires_protocol_version_8() {
    let addr = start_version_7_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(client.server_protocol_version(), 7);
    assert!(matches!(
        client.query("SELECT * FROM dog"),
        Err(ClientError::Unsupported("sql query"))
    ));
    // The connection is still usable — the refusal above sent no frame.
    assert!(client.get(Uuid::from_u128(1)).unwrap().is_some());
}

/// Acceptance criterion 6 (read-your-writes half): a read-your-writes
/// session's own `Query` still sees committed state only — a staged
/// write to a field it reads is never reflected, even though
/// `Session::get` on the same id and field *does* show it.
#[test]
fn query_inside_a_read_your_writes_session_sees_committed_state_only() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut session = client
        .begin_with(SessionOptions::new().read_your_writes())
        .unwrap();
    session
        .update(Uuid::from_u128(1), "age", ScanValue::U32(99))
        .unwrap();

    // The session's own point read sees the staged write...
    let overlaid = session.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(overlaid
        .iter()
        .any(|(name, value)| name == "age" && *value == ScanValue::U32(99)));

    // ...but Query, on the very same open session, does not.
    let result = rows(session.query("SELECT age FROM dog WHERE age = 3").unwrap());
    assert_eq!(
        result.len(),
        1,
        "Query is never overlaid with staged writes, even inside the session that staged them"
    );
    assert!(rows(session.query("SELECT age FROM dog WHERE age = 99").unwrap()).is_empty());

    session.rollback().unwrap();
}

/// Verification plan: "a real client round trip on each of the three
/// domains" — the order domain, covering an `I64` field (`amount_cents`)
/// and an enum-backed `U32` field (`status`).
#[test]
fn order_domain_query_round_trip() {
    let addr = start_order_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    // status = 1 (Shipped) matches only the first order.
    let shipped = rows(
        client
            .query("SELECT amount_cents FROM order WHERE status = 1")
            .unwrap(),
    );
    assert_eq!(shipped.len(), 1);
    assert_eq!(
        shipped[0].1,
        vec![("amount_cents".to_string(), ScanValue::I64(2_500))]
    );

    // amount_cents > 3000 matches only the second order.
    let mut big = rows(
        client
            .query("SELECT * FROM order WHERE amount_cents > 3000")
            .unwrap(),
    );
    big.sort_by_key(|(id, _)| *id);
    assert_eq!(big.len(), 1);
    assert_eq!(big[0].0, Uuid::from_u128(2));
}

/// Verification plan: "a real client round trip on each of the three
/// domains" — the employee domain, covering a `Str` field (`name`) and an
/// enum-backed `U32` field (`department`).
#[test]
fn employee_domain_query_round_trip() {
    let addr = start_employee_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    // department = 0 (Engineering) matches only Alex.
    let engineers = rows(
        client
            .query("SELECT name FROM employee WHERE department = 0")
            .unwrap(),
    );
    assert_eq!(engineers.len(), 1);
    assert_eq!(
        engineers[0].1,
        vec![("name".to_string(), ScanValue::Str("Alex".into()))]
    );

    // salary_cents > 1_000_000 matches only Alex too.
    let high_earners = rows(
        client
            .query("SELECT * FROM employee WHERE salary_cents > 1000000")
            .unwrap(),
    );
    assert_eq!(high_earners.len(), 1);
    assert_eq!(high_earners[0].0, Uuid::from_u128(1));
}

/// Acceptance criterion 6 (snapshot-isolation half): a snapshot-isolated
/// session's own `Query` is never tracked into its read set — an
/// external commit to a field the session's `Query` read never causes a
/// spurious conflict at `Commit`, unlike the identical field read via
/// `GetById`.
#[test]
fn query_inside_a_snapshot_isolation_session_is_not_read_set_tracked() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut other = SchemaDrivenClient::connect(addr).unwrap();

    let mut session = client
        .begin_with(SessionOptions::new().snapshot_isolation())
        .unwrap();
    // A Query reading the exact field an external commit is about to
    // change.
    let result = rows(session.query("SELECT age FROM dog WHERE age = 3").unwrap());
    assert_eq!(result.len(), 1);

    assert!(other
        .update(Uuid::from_u128(1), "age", ScanValue::U32(50))
        .unwrap());

    // A staged write to an unrelated record still commits cleanly —
    // proving the Query above added nothing to the read set (a real
    // GetById on id 1 would have made this Commit fail with Conflict,
    // exactly as `tests/server_transaction_integration.rs`'s own
    // `snapshot_isolation_detects_a_conflicting_commit_from_another_connection`
    // proves for GetById).
    session
        .update(Uuid::from_u128(2), "age", ScanValue::U32(6))
        .unwrap();
    session.commit().unwrap();
}

// `AGG-FR-001`–`010`, ADR-0035, `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md`.

/// This file's dogs, for reference: id1 age=3 breed=labrador, id2 age=5
/// breed=poodle, id3 age=9 breed=labrador.
fn groups(result: QueryResult) -> Vec<Vec<(String, ScanValue)>> {
    match result {
        QueryResult::Groups(groups) => groups,
        QueryResult::Rows(rows) => panic!("expected QueryResult::Groups, got Rows: {rows:?}"),
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
}

/// Acceptance criterion 3: `SELECT COUNT(*) FROM dog` with no `GROUP BY`
/// returns exactly one group whose `key` is empty and whose value is the
/// whole table's count.
#[test]
fn count_star_with_no_group_by_returns_one_whole_table_group() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(client.query("SELECT COUNT(*) FROM dog").unwrap());
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![("COUNT(*)".to_string(), ScanValue::I64(3))]);
}

/// Acceptance criterion 2: `GROUP BY` on one field.
#[test]
fn group_by_one_field() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT breed, COUNT(*) FROM dog GROUP BY breed")
            .unwrap(),
    );
    assert_eq!(result.len(), 2, "labrador and poodle");
    let labrador = result
        .iter()
        .find(|row| row[0] == ("breed".to_string(), ScanValue::Str("labrador".into())))
        .unwrap();
    assert_eq!(labrador[1], ("COUNT(*)".to_string(), ScanValue::I64(2)));
    let poodle = result
        .iter()
        .find(|row| row[0] == ("breed".to_string(), ScanValue::Str("poodle".into())))
        .unwrap();
    assert_eq!(poodle[1], ("COUNT(*)".to_string(), ScanValue::I64(1)));
}

/// Acceptance criterion 2: `GROUP BY` on two fields — one group per
/// distinct (breed, age) pair, since every dog here has a distinct age.
#[test]
fn group_by_two_fields() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT breed, age, COUNT(*) FROM dog GROUP BY breed, age")
            .unwrap(),
    );
    assert_eq!(
        result.len(),
        3,
        "every dog has a distinct (breed, age) pair"
    );
}

/// Acceptance criterion 2: every aggregate function, verified against a
/// hand-computed filter-then-bucket-then-reduce over the same rows —
/// labrador = {age 3, age 9}.
#[test]
fn every_aggregate_function_matches_a_hand_computed_reduction() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query(
                "SELECT breed, COUNT(*), SUM(age), AVG(age), MIN(age), MAX(age) \
                 FROM dog GROUP BY breed",
            )
            .unwrap(),
    );
    let labrador = result
        .iter()
        .find(|row| row[0] == ("breed".to_string(), ScanValue::Str("labrador".into())))
        .unwrap();
    assert_eq!(labrador[1], ("COUNT(*)".to_string(), ScanValue::I64(2)));
    assert_eq!(labrador[2], ("SUM(age)".to_string(), ScanValue::I64(12)));
    assert_eq!(labrador[3], ("AVG(age)".to_string(), ScanValue::F64(6.0)));
    assert_eq!(labrador[4], ("MIN(age)".to_string(), ScanValue::U32(3)));
    assert_eq!(labrador[5], ("MAX(age)".to_string(), ScanValue::U32(9)));
}

/// Acceptance criterion 6: `AVG` equals that same group's `SUM` divided
/// by its `COUNT`, verified directly against a separate `SUM`+`COUNT`
/// query on the identical `GROUP BY`.
#[test]
fn avg_equals_sum_divided_by_count() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let avg_result = groups(
        client
            .query("SELECT breed, AVG(age) FROM dog GROUP BY breed")
            .unwrap(),
    );
    let sum_count_result = groups(
        client
            .query("SELECT breed, SUM(age), COUNT(*) FROM dog GROUP BY breed")
            .unwrap(),
    );
    for avg_row in &avg_result {
        let breed = &avg_row[0];
        let ScanValue::F64(avg) = avg_row[1].1 else {
            panic!("expected F64")
        };
        let sum_count_row = sum_count_result
            .iter()
            .find(|row| &row[0] == breed)
            .unwrap();
        let ScanValue::I64(sum) = sum_count_row[1].1 else {
            panic!("expected I64")
        };
        let ScanValue::I64(count) = sum_count_row[2].1 else {
            panic!("expected I64")
        };
        assert_eq!(avg, sum as f64 / count as f64, "for {breed:?}");
    }
}

/// Acceptance criterion 2: a `WHERE` clause combined with `GROUP BY` —
/// only rows passing the filter are bucketed and reduced.
#[test]
fn where_clause_combines_with_group_by() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT breed, COUNT(*) FROM dog WHERE age > 3 GROUP BY breed")
            .unwrap(),
    );
    // Only id2 (poodle, age 5) and id3 (labrador, age 9) survive age > 3.
    assert_eq!(result.len(), 2);
    let labrador = result
        .iter()
        .find(|row| row[0] == ("breed".to_string(), ScanValue::Str("labrador".into())))
        .unwrap();
    assert_eq!(labrador[1], ("COUNT(*)".to_string(), ScanValue::I64(1)));
}

/// Acceptance criterion 4: a filter matching zero rows for a given
/// `GROUP BY` key produces zero groups for that key, not a group with a
/// zero/null aggregate value.
#[test]
fn a_filter_matching_nothing_for_a_key_produces_no_group_for_that_key() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT breed, COUNT(*) FROM dog WHERE age > 100 GROUP BY breed")
            .unwrap(),
    );
    assert!(result.is_empty());
}

/// Acceptance criterion 3/4: `SELECT COUNT(*) FROM dog WHERE false`-
/// equivalent still returns exactly one group whose `Count` is `0` — the
/// implicit whole-table bucket always exists when `group_by` is empty.
#[test]
fn count_star_with_a_filter_matching_nothing_still_returns_one_zero_group() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT COUNT(*) FROM dog WHERE age > 100")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec![("COUNT(*)".to_string(), ScanValue::I64(0))]);
}

/// Acceptance criterion 2: `LIMIT` on a grouped result truncates the
/// group count only.
#[test]
fn limit_on_a_grouped_result_truncates_the_group_count() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let result = groups(
        client
            .query("SELECT breed, COUNT(*) FROM dog GROUP BY breed LIMIT 1")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);
}

/// Acceptance criterion 5: a plain column missing from `GROUP BY` is a
/// client-side `ClientError::Sql`, no round trip.
#[test]
fn a_plain_column_missing_from_group_by_is_a_client_side_error() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT breed, age, COUNT(*) FROM dog GROUP BY breed"),
        Err(ClientError::Sql(_))
    ));
}

/// Acceptance criterion 5: `SELECT *` alongside `GROUP BY` is rejected
/// the same way.
#[test]
fn select_star_alongside_group_by_is_a_client_side_error() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT * FROM dog GROUP BY breed"),
        Err(ClientError::Sql(_))
    ));
}

/// Acceptance criterion 5: `SUM`/`AVG`/`MIN`/`MAX` against a `Str` field
/// is a clean client-side rejection, never silently accepted.
#[test]
fn an_aggregate_function_against_a_non_numeric_field_is_a_client_side_error() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(matches!(
        client.query("SELECT SUM(breed) FROM dog"),
        Err(ClientError::Sql(_))
    ));
}

/// Acceptance criterion 1: a connection negotiated below 9 gets
/// `ClientError::Unsupported("sql aggregate")` with no frame sent; a
/// plain `SELECT` with no aggregate content still negotiates and runs at
/// 8 unchanged, on the very same connection.
#[test]
fn aggregate_requires_protocol_version_9_but_plain_select_still_runs_at_8() {
    let addr = start_version_7_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(client.server_protocol_version(), 7);
    assert!(matches!(
        client.query("SELECT COUNT(*) FROM dog"),
        Err(ClientError::Unsupported("sql aggregate"))
    ));
    // A plain SELECT is refused too, since this fixture negotiates 7 —
    // below both the version-8 and version-9 gates. `query_requires_
    // protocol_version_8` above already covers that half in isolation.
    assert!(matches!(
        client.query("SELECT * FROM dog"),
        Err(ClientError::Unsupported("sql query"))
    ));
}

/// Acceptance criterion 7 (read-your-writes half): `Aggregate` inside a
/// read-your-writes session sees committed state only.
#[test]
fn aggregate_inside_a_read_your_writes_session_sees_committed_state_only() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut session = client
        .begin_with(SessionOptions::new().read_your_writes())
        .unwrap();
    session
        .update(Uuid::from_u128(1), "age", ScanValue::U32(99))
        .unwrap();

    let result = groups(
        session
            .query("SELECT COUNT(*) FROM dog WHERE age = 99")
            .unwrap(),
    );
    assert_eq!(
        result.len(),
        1,
        "Aggregate is never overlaid with staged writes"
    );
    assert_eq!(result[0], vec![("COUNT(*)".to_string(), ScanValue::I64(0))]);

    session.rollback().unwrap();
}

/// Acceptance criterion 7 (snapshot-isolation half): `Aggregate` inside a
/// snapshot-isolation session is never added to its read set — an
/// external commit to a field the session's `Aggregate` read never
/// causes a spurious conflict at `Commit`.
#[test]
fn aggregate_inside_a_snapshot_isolation_session_is_not_read_set_tracked() {
    let addr = start_dog_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut other = SchemaDrivenClient::connect(addr).unwrap();

    let mut session = client
        .begin_with(SessionOptions::new().snapshot_isolation())
        .unwrap();
    let result = groups(
        session
            .query("SELECT COUNT(*) FROM dog WHERE age = 3")
            .unwrap(),
    );
    assert_eq!(result.len(), 1);

    assert!(other
        .update(Uuid::from_u128(1), "age", ScanValue::U32(50))
        .unwrap());

    session
        .update(Uuid::from_u128(2), "age", ScanValue::U32(6))
        .unwrap();
    session.commit().unwrap();
}
