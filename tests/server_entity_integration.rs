//! Real end-to-end coverage of `Entity` v2 (`ENT2-FR-001`–`007`,
//! ADR-0039, `docs/design/SERVER-ENTITY-V2-REDESIGN-DESIGN.md`) and its
//! `aliases`/normalized-name-lookup extension (`ENT3-FR-001`–`007`,
//! ADR-0040, `docs/design/SERVER-ENTITY-ALIASES-DESIGN.md`), plus
//! `aliases` on the wire at protocol 11 (`ENT4-FR-001`–`006`, ADR-0041,
//! `docs/design/SERVER-ENTITY-ALIASES-WIRE-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`, and raw framing where the
//! negotiated version itself is what is under test. `required-features =
//! ["server"]` only, no `research` — `Entity` is front-door, matching
//! `tests/server_reminder_integration.rs`'s own precedent.

use rusty_multimodal_db::generic::entity::{create_entity_production_stack, entity_id, Entity};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic::reminder::{
    create_reminder_production_stack, Reminder, ReminderStatus,
};
use rusty_multimodal_db::server::client::{
    ClientError, JoinedRowNamed, QueryResult, SchemaDrivenClient, SessionOptions,
};
use rusty_multimodal_db::server::entity::{
    EntityConnectionStore, FIELD_ALIASES, FIELD_MENTION_COUNT,
};
use rusty_multimodal_db::server::framing::{read_message, write_message};
use rusty_multimodal_db::server::protocol::{
    AggregateFn, AggregateSpec, CompareOp, ErrorCode, JoinRelation, Predicate, Request, Response,
    ScanValue, Selection, TransactionOp, ValueKind, PROTOCOL_VERSION,
};
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
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
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
            // `ENT4-FR-002` (ADR-0041, protocol 11): the raw stored list.
            (
                "aliases".to_string(),
                ScanValue::StrList(vec!["Ada".into(), "Countess of Lovelace".into()])
            ),
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

/// `ENT5` acceptance criteria 1 and 3 (ADR-0042) over a real socket: an
/// entity minted with `entity_id(&label)` is fetched by `GetById` on the
/// id re-derived from a differently-spelled query — one round trip, no
/// `FilterEq` — and `FilterEq` on `label` collapses an internal
/// whitespace run the same way. Its own server: the shared fixture's
/// `GROUP BY kind` counts must not change.
#[test]
fn get_by_derived_entity_id_resolves_a_name_in_one_round_trip() {
    let dir = unique_dir("entity_v5_derived_id");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entities.mmap");
    let entities = vec![Entity {
        id: entity_id("Grace Hopper"),
        label: "Grace Hopper".into(),
        kind: "person".into(),
        mention_count: 0,
        aliases: vec!["Amazing Grace".into()],
    }];
    let stack = create_entity_production_stack(entities, &[], &[], &path).unwrap();
    let connection_store = Arc::new(EntityConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    let mut client = SchemaDrivenClient::connect(addr).unwrap();

    let fields = client
        .get(entity_id("  grace   HOPPER\t"))
        .unwrap()
        .expect("the derived id from any spelling resolves in one GetById");
    assert_eq!(
        fields[0],
        ("label".to_string(), ScanValue::Str("Grace Hopper".into()))
    );
    // A different name is a different id — and a miss, not an error.
    assert!(client.get(entity_id("Grace Hopper II")).unwrap().is_none());
    // `ENT5-FR-001` over the wire: internal whitespace collapses in
    // `FilterEq` on `label`, for the label and for an alias alike.
    for query in ["grace   hopper", "Grace\tHopper", "amazing   grace"] {
        assert_eq!(
            client
                .filter_eq("label", ScanValue::Str(query.into()))
                .unwrap(),
            vec![entity_id("Grace Hopper")],
            "query {query:?}"
        );
    }
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
    // two flags are unchanged. `aliases` became a schema field at protocol
    // 11 (`ENT4-FR-002`, ADR-0041) with every flag `false` — see
    // `aliases_is_readable_at_protocol_11_and_projected_by_sql`.
    let label = client
        .schema()
        .fields
        .iter()
        .find(|f| f.name == "label")
        .expect("label is a schema field");
    assert!(label.capabilities.filter_eq);
    assert!(!label.capabilities.scan && !label.capabilities.update);
    let aliases = client
        .schema()
        .fields
        .iter()
        .find(|f| f.name == "aliases")
        .expect("aliases is a schema field since protocol 11");
    assert!(
        !aliases.capabilities.filter_eq
            && !aliases.capabilities.scan
            && !aliases.capabilities.update
    );
    // The SQL path is a full scan with exact-string equality, unchanged
    // by the index: still one row for the exact label, none for a
    // case-varied one — the two mechanisms are deliberately distinct.
    let result = client
        .query("SELECT label FROM entity WHERE label = 'Ada Lovelace'")
        .unwrap();
    match result {
        QueryResult::Rows(rows) => assert_eq!(rows.len(), 1),
        QueryResult::Groups(groups) => panic!("expected Rows, got Groups: {groups:?}"),
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
}

/// Raw-framing helper for the tests below: one request, one response, no
/// `SchemaDrivenClient` in between — so the negotiated version is exactly
/// what the test says it is (the precedent `transaction_applies_every_write`
/// set).
fn raw(wire: &mut TcpStream, req: &Request) -> Response {
    write_message(wire, req).unwrap();
    read_message(wire).unwrap()
}

fn tags(fields: &[(u16, ScanValue)]) -> Vec<u16> {
    fields.iter().map(|(tag, _)| *tag).collect()
}

/// `ENT4` acceptance criterion 2 (ADR-0041): over a version-11
/// connection, `GetById` returns four fields including `aliases` as the
/// raw stored list in stored order (an empty list is still a present
/// field); `SELECT aliases FROM entity` and `SELECT *` project it; the
/// schema describes it as `StrList` with every capability flag `false`.
/// No new client API — `get`/`query` return it as one more pair.
#[test]
fn aliases_is_readable_at_protocol_11_and_projected_by_sql() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    // Readable *since* 11 — not a pin on the current version (that is
    // `tests/server_protocol_version.rs`'s job).
    assert!(client.server_protocol_version() >= 11);

    let aliases = client
        .schema()
        .fields
        .iter()
        .find(|f| f.name == "aliases")
        .expect("aliases descriptor");
    assert_eq!(aliases.tag, FIELD_ALIASES);
    assert_eq!(aliases.value_kind, ValueKind::StrList);
    assert!(
        !aliases.capabilities.filter_eq
            && !aliases.capabilities.scan
            && !aliases.capabilities.update
    );

    let ada = client.get(Uuid::from_u128(1)).unwrap().unwrap();
    assert_eq!(ada.len(), 4);
    assert_eq!(
        ada[3],
        (
            "aliases".to_string(),
            // Raw and in stored order — not the lowercased index keys.
            ScanValue::StrList(vec!["Ada".into(), "Countess of Lovelace".into()])
        )
    );
    let engine = client.get(Uuid::from_u128(2)).unwrap().unwrap();
    assert_eq!(
        engine[3],
        ("aliases".to_string(), ScanValue::StrList(vec![]))
    );

    match client
        .query("SELECT aliases FROM entity WHERE label = 'London'")
        .unwrap()
    {
        QueryResult::Rows(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].1,
                vec![(
                    "aliases".to_string(),
                    ScanValue::StrList(vec!["Londinium".into()])
                )]
            );
        }
        QueryResult::Groups(groups) => panic!("expected Rows, got Groups: {groups:?}"),
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
    match client
        .query("SELECT * FROM entity WHERE label = 'Charles Babbage'")
        .unwrap()
    {
        QueryResult::Rows(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1.len(), 4);
            assert_eq!(
                rows[0].1[3],
                (
                    "aliases".to_string(),
                    ScanValue::StrList(vec!["Babbage".into()])
                )
            );
        }
        QueryResult::Groups(groups) => panic!("expected Rows, got Groups: {groups:?}"),
        QueryResult::Joined(joined) => panic!("expected Rows or Groups, got Joined: {joined:?}"),
    }
}

/// `ENT4` acceptance criterion 3 (`ENT4-FR-003`, ADR-0041): the same
/// `GetById`, `SELECT *`-shaped `Query`, and `DescribeSchema` over a
/// connection hand-negotiated at `Hello { 10 }`, and over a silent
/// (version-1) connection, return exactly the three-field shape `FR-042`
/// returned — no `aliases` pair, no `aliases` descriptor. The protocol's
/// first content-rewriting downgrade, proven against a real server: the
/// server has the field; these clients never see it.
#[test]
fn aliases_is_stripped_for_a_version_10_client_and_a_silent_client() {
    let addr = start_server();
    let ada = Uuid::from_u128(1);
    let every_row = Request::Query {
        select: Selection::All,
        filter: vec![],
        limit: None,
    };
    let three_fields = vec![0u16, 1, 2];

    let assert_fr_042_shape = |wire: &mut TcpStream, who: &str| {
        match raw(wire, &Request::GetById { id: ada }) {
            Response::Record { fields, .. } => {
                assert_eq!(tags(&fields), three_fields, "{who}: GetById");
            }
            other => panic!("{who}: expected Record, got {other:?}"),
        }
        match raw(wire, &every_row) {
            Response::Rows { rows } => {
                assert_eq!(rows.len(), 5, "{who}: every row still returned");
                for (_, fields) in &rows {
                    assert_eq!(tags(fields), three_fields, "{who}: Query row");
                }
            }
            other => panic!("{who}: expected Rows, got {other:?}"),
        }
        match raw(wire, &Request::DescribeSchema) {
            Response::Schema(schema) => {
                assert_eq!(schema.fields.len(), 3, "{who}: DescribeSchema");
                assert!(!schema.fields.iter().any(|f| f.name == "aliases"));
                assert!(!schema
                    .fields
                    .iter()
                    .any(|f| f.value_kind == ValueKind::StrList));
            }
            other => panic!("{who}: expected Schema, got {other:?}"),
        }
    };

    // A version-10 client — the build immediately before this one.
    let mut v10 = TcpStream::connect(addr).unwrap();
    v10.set_nodelay(true).unwrap();
    assert_eq!(
        raw(
            &mut v10,
            &Request::Hello {
                protocol_version: 10
            }
        ),
        Response::Hello {
            protocol_version: 10
        }
    );
    assert_fr_042_shape(&mut v10, "version 10");

    // A silent client — served at version 1, never said hello.
    let mut silent = TcpStream::connect(addr).unwrap();
    silent.set_nodelay(true).unwrap();
    assert_fr_042_shape(&mut silent, "silent");

    // Control: the same three requests at this build's version carry the
    // fourth field — the strip is the version's, not the server's.
    let mut v11 = TcpStream::connect(addr).unwrap();
    v11.set_nodelay(true).unwrap();
    assert_eq!(
        raw(
            &mut v11,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    match raw(&mut v11, &Request::GetById { id: ada }) {
        Response::Record { fields, .. } => assert_eq!(tags(&fields), vec![0, 1, 2, 3]),
        other => panic!("expected Record, got {other:?}"),
    }
    match raw(&mut v11, &Request::DescribeSchema) {
        Response::Schema(schema) => assert_eq!(schema.fields.len(), 4),
        other => panic!("expected Schema, got {other:?}"),
    }
}

/// `ENT4` acceptance criterion 4 (`ENT4-FR-002`/`004`/`005`, ADR-0041):
/// `aliases` is read-only from every direction. Client-side, with no
/// round trip: `filter_eq`/`update`/`Session::update` on `"aliases"` are
/// `Unsupported` (the existing capability checks), `WHERE aliases = 'x'`
/// and `GROUP BY aliases` are `Sql` errors. Server-side, against a raw
/// frame the client library would never send: `UpdateField` carrying a
/// `StrList` is `Unsupported`, a `Query` predicate carrying one is
/// `Malformed`, and `GROUP BY` the `aliases` tag is `Malformed` — so no
/// `StrList` can ever reach `Response::Groups`. No new `ErrorCode`.
#[test]
fn aliases_refuses_every_write_filter_and_group_client_and_server_side() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let list = ScanValue::StrList(vec!["x".into()]);

    assert!(matches!(
        client.filter_eq("aliases", ScanValue::Str("Ada".into())),
        Err(ClientError::Unsupported(_))
    ));
    assert!(matches!(
        client.update(Uuid::from_u128(1), "aliases", list.clone()),
        Err(ClientError::Unsupported(_))
    ));
    let mut session = client.begin().unwrap();
    assert!(matches!(
        session.update(Uuid::from_u128(1), "aliases", list.clone()),
        Err(ClientError::Unsupported(_))
    ));
    session.rollback().unwrap();
    assert!(matches!(
        client.query("SELECT label FROM entity WHERE aliases = 'Ada'"),
        Err(ClientError::Sql(_))
    ));
    assert!(matches!(
        client.query("SELECT aliases, COUNT(*) FROM entity GROUP BY aliases"),
        Err(ClientError::Sql(_))
    ));
    // The client refused before any frame: the connection is still good.
    assert!(client.get(Uuid::from_u128(1)).unwrap().is_some());

    let mut wire = TcpStream::connect(addr).unwrap();
    wire.set_nodelay(true).unwrap();
    assert_eq!(
        raw(
            &mut wire,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    assert!(matches!(
        raw(
            &mut wire,
            &Request::UpdateField {
                id: Uuid::from_u128(1),
                field: FIELD_ALIASES,
                value: list.clone(),
            }
        ),
        Response::Err {
            code: ErrorCode::Unsupported,
            ..
        }
    ));
    assert!(matches!(
        raw(
            &mut wire,
            &Request::Query {
                select: Selection::All,
                filter: vec![Predicate {
                    field: FIELD_ALIASES,
                    op: CompareOp::Eq,
                    value: list.clone(),
                }],
                limit: None,
            }
        ),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    assert!(matches!(
        raw(
            &mut wire,
            &Request::Aggregate {
                group_by: vec![FIELD_ALIASES],
                filter: vec![],
                aggregates: vec![AggregateSpec {
                    func: AggregateFn::Count,
                    field: None,
                }],
                limit: None,
            }
        ),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    // Nothing above changed the record.
    match raw(
        &mut wire,
        &Request::GetById {
            id: Uuid::from_u128(1),
        },
    ) {
        Response::Record { fields, .. } => assert_eq!(
            fields[3],
            (
                FIELD_ALIASES,
                ScanValue::StrList(vec!["Ada".into(), "Countess of Lovelace".into()])
            )
        ),
        other => panic!("expected Record, got {other:?}"),
    }
}

fn joined(result: QueryResult) -> Vec<JoinedRowNamed> {
    match result {
        QueryResult::Joined(rows) => rows,
        other => panic!("expected Joined, got {other:?}"),
    }
}

fn id_pairs(rows: &[JoinedRowNamed]) -> Vec<(u128, u128)> {
    let mut pairs: Vec<(u128, u128)> = rows
        .iter()
        .map(|r| (r.left_id.as_u128(), r.right_id.as_u128()))
        .collect();
    pairs.sort();
    pairs
}

/// `JOIN` acceptance criterion 2 (`JOIN-FR-001`–`006`, ADR-0044): over a
/// real socket, `SELECT a.label, b.label FROM entity a JOIN entity b ON
/// relates_to` returns the `relates_to` edge set in both orientations with
/// the right labels; `mentioned_with` the disjoint edge; `neighbors` the
/// union; a left filter, a right filter, and `LIMIT` each apply; `aliases`
/// rides in a joined row as a `StrList`; `DescribeRelations` lists the
/// three names `ON` may say and no `parent`/`children`. One round trip
/// where `traverse` + a `GetById` per id was the only way before.
#[test]
fn join_over_a_declared_relation_returns_both_endpoints_in_one_round_trip() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(client.server_protocol_version(), 12);

    let mut names: Vec<&str> = client.relations().iter().map(|r| r.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["mentioned_with", "neighbors", "relates_to"]);
    assert!(client.relations().iter().all(|r| r.target_table.is_none()));

    // The fixture's relates_to edges: 1–2, 2–3, 3–1, 3–4 — eight rows,
    // both orientations, SQL semantics (`JOIN-FR-004`).
    let rows = joined(
        client
            .query("SELECT a.label, b.label FROM entity a JOIN entity b ON relates_to")
            .unwrap(),
    );
    assert_eq!(
        id_pairs(&rows),
        vec![
            (1, 2),
            (1, 3),
            (2, 1),
            (2, 3),
            (3, 1),
            (3, 2),
            (3, 4),
            (4, 3)
        ]
    );
    let ada_to_engine = rows
        .iter()
        .find(|r| r.left_id == Uuid::from_u128(1) && r.right_id == Uuid::from_u128(2))
        .unwrap();
    assert_eq!(
        ada_to_engine.fields,
        vec![
            ("a.label".to_string(), ScanValue::Str("Ada Lovelace".into())),
            (
                "b.label".to_string(),
                ScanValue::Str("Analytical Engine".into())
            ),
        ]
    );

    // The disjoint edge, and the union of both labels.
    let rows = joined(
        client
            .query("SELECT a.label, b.label FROM entity a JOIN entity b ON mentioned_with")
            .unwrap(),
    );
    assert_eq!(id_pairs(&rows), vec![(1, 5), (5, 1)]);
    let rows = joined(
        client
            .query("SELECT a.label FROM entity a JOIN entity b ON neighbors")
            .unwrap(),
    );
    assert_eq!(rows.len(), 10);

    // Left filter: only Ada (1) and Babbage (5) are persons; 5 has no
    // relates_to edge.
    let rows = joined(
        client
            .query(
                "SELECT a.label, b.label FROM entity a JOIN entity b ON relates_to \
                 WHERE a.kind = 'person'",
            )
            .unwrap(),
    );
    assert_eq!(id_pairs(&rows), vec![(1, 2), (1, 3)]);
    // Right filter: only the Analytical Engine (2) has mention_count > 4.
    let rows = joined(
        client
            .query(
                "SELECT a.label, b.mention_count FROM entity a JOIN entity b ON relates_to \
                 WHERE b.mention_count > 4",
            )
            .unwrap(),
    );
    assert_eq!(id_pairs(&rows), vec![(1, 2), (3, 2)]);
    assert!(rows
        .iter()
        .all(|r| r.fields[1] == ("b.mention_count".to_string(), ScanValue::I64(5))));
    // Both filters and LIMIT compose.
    let rows = joined(
        client
            .query(
                "SELECT a.label FROM entity a JOIN entity b ON neighbors \
                 WHERE a.kind = 'person' AND b.kind = 'person' LIMIT 1",
            )
            .unwrap(),
    );
    assert_eq!(rows.len(), 1);
    let rows = joined(
        client
            .query("SELECT * FROM entity a JOIN entity b ON relates_to LIMIT 2")
            .unwrap(),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].fields.len(),
        8,
        "SELECT * projects every field of both sides"
    );
    assert_eq!(rows[0].fields[0].0, "a.label");
    assert_eq!(rows[0].fields[4].0, "b.label");

    // `aliases` rides in a joined row intact — a `StrList` at 12 (FR-044).
    let rows = joined(
        client
            .query("SELECT a.aliases, b.label FROM entity a JOIN entity b ON mentioned_with")
            .unwrap(),
    );
    let ada_side = rows
        .iter()
        .find(|r| r.left_id == Uuid::from_u128(1))
        .unwrap();
    assert_eq!(
        ada_side.fields[0],
        (
            "a.aliases".to_string(),
            ScanValue::StrList(vec!["Ada".into(), "Countess of Lovelace".into()])
        )
    );
}

/// `JOIN` acceptance criterion 6 (client side) and `JOIN-FR-007`: every
/// refusal is client-side with no frame sent — `GROUP BY`/an aggregate
/// with `JOIN`, an unqualified column, an unknown alias, an unknown
/// relation, `parent` (not listed for `Entity`), and a different right
/// table (ADR-0045, not yet). Server side, against raw frames the client
/// would never send: `right_table: Some(_)` is `Malformed`; an unlisted
/// relation is `Malformed`.
#[test]
fn join_refusals_are_client_side_and_the_raw_server_rejections_use_existing_codes() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    for sql in [
        "SELECT a.kind, COUNT(*) FROM entity a JOIN entity b ON neighbors GROUP BY kind",
        "SELECT label FROM entity a JOIN entity b ON neighbors",
        "SELECT c.label FROM entity a JOIN entity b ON neighbors",
        "SELECT a.label FROM entity a JOIN entity b ON knows",
        "SELECT a.label FROM entity a JOIN entity b ON parent",
        "SELECT a.label FROM entity a JOIN reminder b ON neighbors",
        "SELECT * FROM entity JOIN entity ON neighbors",
    ] {
        assert!(
            matches!(client.query(sql), Err(ClientError::Sql(_))),
            "{sql}"
        );
    }
    // The client refused before any frame: the connection is still good.
    assert!(client.get(Uuid::from_u128(1)).unwrap().is_some());

    let mut wire = TcpStream::connect(addr).unwrap();
    wire.set_nodelay(true).unwrap();
    assert_eq!(
        raw(
            &mut wire,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        Response::Hello {
            protocol_version: 12
        }
    );
    let spec = |relation: JoinRelation, right_table: Option<String>| {
        Request::Join(rusty_multimodal_db::server::protocol::JoinSpec {
            relation,
            right_table,
            left: Selection::All,
            right: Selection::All,
            left_filter: vec![],
            right_filter: vec![],
            limit: None,
        })
    };
    assert!(matches!(
        raw(
            &mut wire,
            &spec(JoinRelation::Neighbors(None), Some("customer".into()))
        ),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    assert!(matches!(
        raw(&mut wire, &spec(JoinRelation::Parent, None)),
        Response::Err {
            code: ErrorCode::Malformed,
            ..
        }
    ));
    match raw(
        &mut wire,
        &spec(JoinRelation::Neighbors(Some("mentioned_with".into())), None),
    ) {
        Response::JoinedRows { rows } => assert_eq!(rows.len(), 2),
        other => panic!("expected JoinedRows, got {other:?}"),
    }
}
