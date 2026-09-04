//! Real end-to-end coverage of the `Entity` domain (`ENT-FR-001`–`008`,
//! ADR-0037, `docs/design/SERVER-ENTITY-DOMAIN-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`. `required-features =
//! ["server"]` only, no `research` — `Entity` is front-door, matching
//! `tests/server_reminder_integration.rs`'s own precedent.

use rusty_multimodal_db::generic::entity::{create_entity_production_stack, Entity, EntityKind};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic::reminder::{
    create_reminder_production_stack, Reminder, ReminderStatus,
};
use rusty_multimodal_db::server::client::{
    ClientError, QueryResult, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::entity::{EntityConnectionStore, FIELD_MENTION_COUNT};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{Request, Response, ScanValue, TransactionOp};
use rusty_multimodal_db::server::reminder::ReminderConnectionStore;
use rusty_multimodal_db::server::{serve, ServeOptions};
use std::net::{SocketAddr, TcpListener, TcpStream};
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

fn sample_entities() -> Vec<Entity> {
    vec![
        Entity {
            id: Uuid::from_u128(1),
            label: "Ada Lovelace".into(),
            kind: EntityKind::Person,
            mention_count: 3,
        },
        Entity {
            id: Uuid::from_u128(2),
            label: "Analytical Engine".into(),
            kind: EntityKind::Concept,
            mention_count: 5,
        },
        Entity {
            id: Uuid::from_u128(3),
            label: "London".into(),
            kind: EntityKind::Place,
            mention_count: 1,
        },
        Entity {
            id: Uuid::from_u128(4),
            label: "Royal Society".into(),
            kind: EntityKind::Organization,
            mention_count: 2,
        },
    ]
}

/// A triangle (1-2, 2-3, 3-1 — a real cycle) plus one extra hop (3-4),
/// so `traverse`'s `max_depth`/`max_nodes` bounds and cycle-safety are
/// all observable: from `1`, `2`/`3` are depth 1, `4` is depth 2.
fn sample_edges() -> Vec<(Uuid, Uuid)> {
    vec![
        (Uuid::from_u128(1), Uuid::from_u128(2)),
        (Uuid::from_u128(2), Uuid::from_u128(3)),
        (Uuid::from_u128(3), Uuid::from_u128(1)),
        (Uuid::from_u128(3), Uuid::from_u128(4)),
    ]
}

fn start_server() -> SocketAddr {
    let dir = unique_dir("entity_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entities.mmap");
    let stack = create_entity_production_stack(sample_entities(), &sample_edges(), &path).unwrap();
    let connection_store = Arc::new(EntityConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

/// A `Reminder` server — `relations.neighbors: false` — used only to
/// prove `traverse`'s client-side capability gate (`ENT-FR-007`).
fn start_reminder_server() -> SocketAddr {
    let dir = unique_dir("entity_traverse_gate");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reminders.mmap");
    let reminders = vec![Reminder {
        id: Uuid::from_u128(1),
        title: "Pay rent".into(),
        due_at_unix_ms: 1_000,
        status: ReminderStatus::Pending,
    }];
    let stack = create_reminder_production_stack(reminders, &path).unwrap();
    let connection_store = Arc::new(ReminderConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn groups(result: QueryResult) -> Vec<Vec<(String, ScanValue)>> {
    match result {
        QueryResult::Groups(groups) => groups,
        QueryResult::Rows(rows) => panic!("expected QueryResult::Groups, got Rows: {rows:?}"),
    }
}

/// Acceptance criterion 2: `GetById` returns every field, and a
/// `GROUP BY kind` count via `Aggregate` matches a hand-computed tally.
#[test]
fn get_returns_every_field_and_group_by_kind_counts_correctly() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(
        fields,
        vec![
            ("label".to_string(), ScanValue::Str("Ada Lovelace".into())),
            ("kind".to_string(), ScanValue::U32(0)),
            ("mention_count".to_string(), ScanValue::I64(3)),
        ]
    );
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());

    let counts = groups(
        client
            .query("SELECT kind, COUNT(*) FROM entity GROUP BY kind")
            .unwrap(),
    );
    assert_eq!(
        counts.len(),
        4,
        "Person, Concept, Place, Organization — one each"
    );
    for row in &counts {
        assert_eq!(row[1], ("COUNT(*)".to_string(), ScanValue::I64(1)));
    }
}

/// Acceptance criterion 3: `FilterEq` on `kind` returns exactly the
/// matching ids; `FilterEq` on `label`/`mention_count` is client-side
/// `Unsupported`, no round trip.
#[test]
fn filter_eq_by_kind_and_unsupported_fields() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert_eq!(
        client.filter_eq("kind", ScanValue::U32(0)).unwrap(),
        vec![Uuid::from_u128(1)],
        "Person"
    );
    assert!(client
        .filter_eq("kind", ScanValue::U32(4))
        .unwrap()
        .is_empty());

    assert!(matches!(
        client.filter_eq("mention_count", ScanValue::I64(0)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.filter_eq("label", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 4: `UpdateField`/`Session::update` on
/// `mention_count` succeeds and is immediately visible; `UpdateField`
/// on `label`/`kind` is client-side `Unsupported`.
#[test]
fn update_mention_count_and_unsupported_fields() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(client
        .update(Uuid::from_u128(1), "mention_count", ScanValue::I64(4))
        .unwrap());
    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(fields[2], ("mention_count".to_string(), ScanValue::I64(4)));

    assert!(matches!(
        client.update(Uuid::from_u128(1), "kind", ScanValue::U32(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.update(Uuid::from_u128(1), "label", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 5: `Neighbors` returns exactly the entities
/// connected by a `relates_to` edge, both directions (symmetric);
/// `parent`/`children` are `Unsupported` unconditionally.
#[test]
fn neighbors_reflects_relates_to_both_directions_and_parent_children_are_unsupported() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut neighbors_of_1 = client.neighbors(Uuid::from_u128(1)).unwrap();
    neighbors_of_1.sort();
    assert_eq!(neighbors_of_1, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
    // The symmetric direction: 4 was only ever named as (3, 4), never (4, 3).
    assert_eq!(
        client.neighbors(Uuid::from_u128(4)).unwrap(),
        vec![Uuid::from_u128(3)]
    );

    assert!(matches!(
        client.parent(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.children(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));

    let schema = client.schema();
    assert!(schema.relations.neighbors);
    assert!(!schema.relations.parent_children);
}

/// Acceptance criterion 6: `SchemaDrivenClient::traverse` from `1` over
/// the fixture graph (a real cycle, `1`-`2`-`3`-`1`, plus `4` one hop
/// further out) returns every reachable entity paired with its true
/// shortest-path hop distance, no duplicate, no infinite loop; the
/// `max_depth` bound stops discovery at `4` (depth 2); the `max_nodes`
/// bound can cut a walk off before its `max_depth` is even reached.
#[test]
fn traverse_returns_correct_hop_distances_and_respects_both_bounds() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut full = client.traverse(Uuid::from_u128(1), 3, 10).unwrap();
    full.sort();
    assert_eq!(
        full,
        vec![
            (Uuid::from_u128(1), 0),
            (Uuid::from_u128(2), 1),
            (Uuid::from_u128(3), 1),
            (Uuid::from_u128(4), 2),
        ],
        "every entity, each at its true shortest-path hop distance, no duplicate"
    );

    let mut depth_bounded = client.traverse(Uuid::from_u128(1), 1, 10).unwrap();
    depth_bounded.sort();
    assert_eq!(
        depth_bounded,
        vec![
            (Uuid::from_u128(1), 0),
            (Uuid::from_u128(2), 1),
            (Uuid::from_u128(3), 1),
        ],
        "max_depth = 1 stops before discovering 4"
    );

    let nodes_bounded = client.traverse(Uuid::from_u128(1), 3, 2).unwrap();
    assert_eq!(
        nodes_bounded.len(),
        2,
        "max_nodes = 2 stops the walk even though max_depth would allow more: {nodes_bounded:?}"
    );
    assert_eq!(nodes_bounded[0], (Uuid::from_u128(1), 0));
}

/// Acceptance criterion 6 (continued): `traverse` is `Err(ClientError::
/// Unsupported("traverse"))` with no round trip against a domain whose
/// schema reports `relations.neighbors: false` — `Reminder`'s own
/// shape, reused here as the negative fixture.
#[test]
fn traverse_is_unsupported_against_a_domain_with_no_neighbors_capability() {
    let addr = start_reminder_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(!client.schema().relations.neighbors);
    assert!(matches!(
        client.traverse(Uuid::from_u128(1), 3, 10),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 7: `Request::Transaction` works against
/// `Entity` with the same shape every other domain's own tests already
/// establish — full success applying every write. Issued via raw
/// framing (matching `tests/server_transaction_integration.rs`'s own
/// precedent), since `SchemaDrivenClient` has no `Transaction`
/// convenience method.
#[test]
fn transaction_applies_every_write() {
    let addr = start_server();
    let mut wire = TcpStream::connect(addr).unwrap();
    wire.set_nodelay(true).unwrap();

    let resp: Response = {
        write_message(
            &mut wire,
            &Request::Transaction {
                updates: vec![
                    TransactionOp {
                        id: Uuid::from_u128(1),
                        field: FIELD_MENTION_COUNT,
                        value: ScanValue::I64(10),
                    },
                    TransactionOp {
                        id: Uuid::from_u128(2),
                        field: FIELD_MENTION_COUNT,
                        value: ScanValue::I64(20),
                    },
                ],
            },
        )
        .unwrap();
        read_message(&mut wire).unwrap()
    };
    assert_eq!(resp, Response::Ok);

    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(
        client.get(Uuid::from_u128(1)).unwrap().unwrap()[2],
        ("mention_count".to_string(), ScanValue::I64(10))
    );
    assert_eq!(
        client.get(Uuid::from_u128(2)).unwrap().unwrap()[2],
        ("mention_count".to_string(), ScanValue::I64(20))
    );
}

/// Acceptance criterion 7 (continued): a read-your-writes session sees
/// its own staged `mention_count` write; a snapshot-isolation
/// session's `Commit` succeeds normally with no conflicting external
/// write — the same session-composition shape every other domain's
/// own tests already establish.
#[test]
fn sessions_compose_with_entity_the_same_as_every_other_domain() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut session = client
        .begin_with(SessionOptions::new().read_your_writes())
        .unwrap();
    session
        .update(Uuid::from_u128(1), "mention_count", ScanValue::I64(9))
        .unwrap();
    let overlaid = session.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert!(overlaid
        .iter()
        .any(|(name, value)| name == "mention_count" && *value == ScanValue::I64(9)));
    session.rollback().unwrap();

    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let mut session = client
        .begin_with(SessionOptions::new().snapshot_isolation())
        .unwrap();
    assert!(session.get(Uuid::from_u128(2)).unwrap().is_some());
    session
        .update(Uuid::from_u128(3), "mention_count", ScanValue::I64(7))
        .unwrap();
    session.commit().unwrap();
}

/// Acceptance criterion 5 (sanity check on the schema itself, matching
/// `Order`/`Reminder`'s own "every capability flag false" precedent):
/// `label` has every capability flag `false` but is still fully
/// selectable/filterable via `Query` — a full scan needs no index.
#[test]
fn label_has_every_capability_flag_false_but_is_still_queryable() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(!client.schema().fields.iter().any(|f| f.name == "label"
        && (f.capabilities.filter_eq || f.capabilities.scan || f.capabilities.update)));
    let result = client
        .query("SELECT label FROM entity WHERE label = 'Ada Lovelace'")
        .unwrap();
    match result {
        QueryResult::Rows(rows) => assert_eq!(rows.len(), 1),
        QueryResult::Groups(groups) => panic!("expected Rows, got Groups: {groups:?}"),
    }
}
