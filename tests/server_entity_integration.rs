//! Real end-to-end coverage of `Entity` v2 (`ENT2-FR-001`–`007`,
//! ADR-0039, `docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md`) and its
//! `aliases`/normalized-name-lookup extension (`ENT3-FR-001`–`007`,
//! ADR-0040, `docs/design/SERVER-ENTITY-ALIASES-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`. `required-features =
//! ["server"]` only, no `research` — `Entity` is front-door, matching
//! `tests/server_reminder_integration.rs`'s own precedent.

use rusty_multimodal_db::generic::entity::{create_entity_production_stack, Entity};
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
            aliases: vec!["Londinium".into()],
        },
        Entity {
            id: Uuid::from_u128(4),
            label: "Royal Society".into(),
            kind: "organization".into(),
            mention_count: 2,
            // Deliberately collides with entity 5's own alias below
            // (`ENT3` acceptance criterion 4): both must come back.
            aliases: vec!["The Society".into(), "Babbage".into()],
        },
        Entity {
            id: Uuid::from_u128(5),
            label: "Charles Babbage".into(),
            kind: "person".into(),
            mention_count: 2,
            aliases: vec!["Babbage".into()],
        },
    ]
}

/// A triangle (1-2, 2-3, 3-1 — a real cycle) plus one extra `relates_to`
/// hop (3-4), so `traverse`'s bounds and cycle-safety are observable —
/// the same fixture `Entity` v1's own tests used. `mentioned_with` is a
/// disjoint edge (1-5), so relation-filtered traversal/neighbors are
/// distinguishable from the unfiltered union.
fn sample_relates_to_edges() -> Vec<(Uuid, Uuid)> {
    vec![
        (Uuid::from_u128(1), Uuid::from_u128(2)),
        (Uuid::from_u128(2), Uuid::from_u128(3)),
        (Uuid::from_u128(3), Uuid::from_u128(1)),
        (Uuid::from_u128(3), Uuid::from_u128(4)),
    ]
}

fn sample_mentioned_with_edges() -> Vec<(Uuid, Uuid)> {
    vec![(Uuid::from_u128(1), Uuid::from_u128(5))]
}

fn start_server() -> SocketAddr {
    let dir = unique_dir("entity_v2_integration");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entities.mmap");
    let stack = create_entity_production_stack(
        sample_entities(),
        &sample_relates_to_edges(),
        &sample_mentioned_with_edges(),
        &path,
    )
    .unwrap();
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
    let dir = unique_dir("entity_v2_traverse_gate");
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
            ("kind".to_string(), ScanValue::Str("person".into())),
            ("mention_count".to_string(), ScanValue::I64(3)),
        ]
    );
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());

    let counts = groups(
        client
            .query("SELECT kind, COUNT(*) FROM entity GROUP BY kind")
            .unwrap(),
    );
    assert_eq!(counts.len(), 4, "person(x2), concept, place, organization");
    for row in &counts {
        let expected = if row[0] == ("kind".to_string(), ScanValue::Str("person".into())) {
            2
        } else {
            1
        };
        assert_eq!(row[1], ("COUNT(*)".to_string(), ScanValue::I64(expected)));
    }
}

/// Acceptance criterion 3: `FilterEq` on `kind` is open-ended (any
/// string, no discriminant) and returns exactly the matching ids;
/// `FilterEq` on `mention_count` is client-side `Unsupported`. (`label`
/// became filterable in ADR-0040 — see the next test.)
#[test]
fn filter_eq_by_kind_open_ended_and_unsupported_fields() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut persons = client
        .filter_eq("kind", ScanValue::Str("person".into()))
        .unwrap();
    persons.sort();
    assert_eq!(persons, vec![Uuid::from_u128(1), Uuid::from_u128(5)]);
    assert!(client
        .filter_eq("kind", ScanValue::Str("nonexistent-kind".into()))
        .unwrap()
        .is_empty());

    assert!(matches!(
        client.filter_eq("mention_count", ScanValue::I64(0)),
        Err(ClientError::Unsupported(_))
    ));
}

/// `ENT3` acceptance criteria 3–4 (ADR-0040): `FilterEq` on `label` over
/// a real socket resolves the primary name and every alias, case- and
/// whitespace-insensitively; a miss is empty; two entities sharing a
/// normalized alias both come back. Through the existing `FilterEq`/
/// `ScanValue::Str`/`RecordList` shapes — the same wire bytes any
/// protocol-10 client already sends.
#[test]
fn filter_eq_by_label_resolves_label_and_aliases_case_insensitively() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let ada = vec![Uuid::from_u128(1)];

    for query in [
        "Ada Lovelace",
        "ada lovelace",
        "  ADA LOVELACE  ",
        "Ada",
        "countess OF lovelace",
    ] {
        assert_eq!(
            client
                .filter_eq("label", ScanValue::Str(query.into()))
                .unwrap(),
            ada,
            "query {query:?}"
        );
    }
    assert_eq!(
        client
            .filter_eq("label", ScanValue::Str("londinium".into()))
            .unwrap(),
        vec![Uuid::from_u128(3)]
    );
    assert!(client
        .filter_eq("label", ScanValue::Str("nobody".into()))
        .unwrap()
        .is_empty());
    // Exact-after-normalization only — no prefix/substring matching.
    assert!(client
        .filter_eq("label", ScanValue::Str("Ada Love".into()))
        .unwrap()
        .is_empty());

    // Criterion 4: a shared alias returns both owners, no silent pick.
    let mut babbage = client
        .filter_eq("label", ScanValue::Str("BABBAGE".into()))
        .unwrap();
    babbage.sort();
    assert_eq!(babbage, vec![Uuid::from_u128(4), Uuid::from_u128(5)]);
}

/// Acceptance criterion 4 (v2, narrowed — see `crate::generic::entity`'s
/// own module docs for why): `UpdateField` on `mention_count` succeeds
/// and is immediately visible; `UpdateField` on `label`/`kind` is
/// client-side `Unsupported` (`kind` moved to read-only in v2, unlike
/// v1).
#[test]
fn update_mention_count_kind_is_now_read_only() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    assert!(client
        .update(Uuid::from_u128(1), "mention_count", ScanValue::I64(4))
        .unwrap());
    let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(fields[2], ("mention_count".to_string(), ScanValue::I64(4)));

    assert!(matches!(
        client.update(Uuid::from_u128(1), "kind", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.update(Uuid::from_u128(1), "label", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 3 (`ENT2-FR-004`/`005`): `Neighbors` (unfiltered)
/// returns the union of both relations; `NeighborsByRelation` returns
/// exactly one relation's own edges; an unknown label is a server-side
/// `Malformed`; `ListRelationKinds` names both labels; `parent`/
/// `children` stay `Unsupported` unconditionally.
#[test]
fn neighbors_by_relation_unfiltered_union_and_list_relation_kinds() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut unfiltered = client.neighbors(Uuid::from_u128(1)).unwrap();
    unfiltered.sort();
    assert_eq!(
        unfiltered,
        vec![Uuid::from_u128(2), Uuid::from_u128(3), Uuid::from_u128(5)],
        "the union of relates_to {{2,3}} and mentioned_with {{5}}"
    );

    let mut relates_to = client
        .neighbors_by_relation(Uuid::from_u128(1), "relates_to")
        .unwrap();
    relates_to.sort();
    assert_eq!(relates_to, vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
    assert_eq!(
        client
            .neighbors_by_relation(Uuid::from_u128(1), "mentioned_with")
            .unwrap(),
        vec![Uuid::from_u128(5)]
    );
    match client.neighbors_by_relation(Uuid::from_u128(1), "unknown") {
        Err(ClientError::Server(
            rusty_multimodal_db::server::protocol::ErrorCode::Malformed,
            _,
        )) => {}
        other => panic!("expected a server-side Malformed rejection, got {other:?}"),
    }

    let mut kinds = client.list_relation_kinds().unwrap();
    kinds.sort();
    assert_eq!(
        kinds,
        vec!["mentioned_with".to_string(), "relates_to".to_string()]
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

/// Acceptance criterion 5 (`ENT2-FR-006`): `traverse`'s relation filter.
/// `None` walks both relations, matching `ADR-0037`'s own unfiltered
/// behavior; `Some("relates_to")`/`Some("mentioned_with")` each walk
/// only their own edges. Also re-proves the `max_depth`/`max_nodes`
/// bounds still hold with the filter unset.
#[test]
fn traverse_relation_filter_and_bounds() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let mut unfiltered = client.traverse(Uuid::from_u128(1), 3, 10, None).unwrap();
    unfiltered.sort();
    assert_eq!(
        unfiltered,
        vec![
            (Uuid::from_u128(1), 0),
            (Uuid::from_u128(2), 1),
            (Uuid::from_u128(3), 1),
            (Uuid::from_u128(4), 2),
            (Uuid::from_u128(5), 1),
        ],
        "every entity reachable via either relation, each at its true shortest-path depth"
    );

    let mut relates_to_only = client
        .traverse(Uuid::from_u128(1), 3, 10, Some("relates_to"))
        .unwrap();
    relates_to_only.sort();
    assert_eq!(
        relates_to_only,
        vec![
            (Uuid::from_u128(1), 0),
            (Uuid::from_u128(2), 1),
            (Uuid::from_u128(3), 1),
            (Uuid::from_u128(4), 2),
        ],
        "5 is unreachable via relates_to alone"
    );

    let mentioned_with_only = client
        .traverse(Uuid::from_u128(1), 3, 10, Some("mentioned_with"))
        .unwrap();
    assert_eq!(
        mentioned_with_only,
        vec![(Uuid::from_u128(1), 0), (Uuid::from_u128(5), 1)],
        "2/3/4 are unreachable via mentioned_with alone"
    );

    let depth_bounded = client
        .traverse(Uuid::from_u128(1), 1, 10, Some("relates_to"))
        .unwrap();
    assert_eq!(
        depth_bounded.len(),
        3,
        "max_depth = 1 stops before discovering 4"
    );

    let nodes_bounded = client
        .traverse(Uuid::from_u128(1), 3, 2, Some("relates_to"))
        .unwrap();
    assert_eq!(
        nodes_bounded.len(),
        2,
        "max_nodes = 2 stops the walk even though max_depth would allow more"
    );
}

/// Acceptance criterion 6: `traverse`/`neighbors_by_relation`/
/// `list_relation_kinds` are all `Err(ClientError::Unsupported(_))` with
/// no round trip against a domain whose schema reports
/// `relations.neighbors: false` — `Reminder`'s own shape, reused as the
/// negative fixture.
#[test]
fn traverse_and_relation_methods_are_unsupported_against_a_domain_with_no_neighbors_capability() {
    let addr = start_reminder_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert!(!client.schema().relations.neighbors);
    assert!(matches!(
        client.traverse(Uuid::from_u128(1), 3, 10, None),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.neighbors_by_relation(Uuid::from_u128(1), "relates_to"),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.list_relation_kinds(),
        Err(ClientError::Unsupported(_))
    ));
}

/// Acceptance criterion 7: `Request::Transaction` works against `Entity`
/// v2 with the same shape every other domain's own tests already
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
/// its own staged `mention_count` write; a snapshot-isolation session's
/// `Commit` succeeds normally with no conflicting external write.
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

/// Acceptance criterion 3 (sanity check on the schema itself, matching
/// `Order`/`Reminder`'s own "every capability flag false" precedent):
/// `label` has every capability flag `false` but is still fully
/// selectable/filterable via `Query` — a full scan needs no index.
#[test]
fn label_is_filterable_but_not_scannable_or_updatable_and_still_sql_queryable() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    // `ENT3-FR-006`: `filter_eq` flipped to `true` (ADR-0040); the other
    // two flags are unchanged. `aliases` is not a schema field at all.
    let label = client
        .schema()
        .fields
        .iter()
        .find(|f| f.name == "label")
        .expect("label is a schema field");
    assert!(label.capabilities.filter_eq);
    assert!(!label.capabilities.scan && !label.capabilities.update);
    assert!(!client.schema().fields.iter().any(|f| f.name == "aliases"));
    // The SQL path is a full scan with exact-string equality, unchanged
    // by the index: still one row for the exact label, none for a
    // case-varied one — the two mechanisms are deliberately distinct.
    let result = client
        .query("SELECT label FROM entity WHERE label = 'Ada Lovelace'")
        .unwrap();
    match result {
        QueryResult::Rows(rows) => assert_eq!(rows.len(), 1),
        QueryResult::Groups(groups) => panic!("expected Rows, got Groups: {groups:?}"),
    }
}
