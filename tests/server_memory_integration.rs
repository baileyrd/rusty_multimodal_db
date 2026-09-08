//! Real end-to-end coverage of the `Memory` domain (`MEM-FR-001`–`008`,
//! ADR-0048, `docs/design/SERVER-MEMORY-DOMAIN-DESIGN.md`) — a real
//! `TcpListener`, a real `SchemaDrivenClient`, and no wire change at all:
//! the consumer's `add`/`get`/`list` shapes over the requests that
//! already exist. `required-features = ["server"]` only, no `research`.

use rusty_multimodal_db::generic::entity::{
    create_entity_production_stack, open_entity_production_stack_portable, Entity,
};
use rusty_multimodal_db::generic::memory::{
    create_memory_production_stack, open_memory_production_stack_portable, Memory,
};
use rusty_multimodal_db::generic::production::GenericProductionStore;
use rusty_multimodal_db::generic::relation::open_or_create_relation_production_stack;
use rusty_multimodal_db::server::client::{
    BatchOp, ClientError, GuardedReplace, QueryResult, SchemaDrivenClient,
};
use rusty_multimodal_db::server::entity::EntityConnectionStore;
use rusty_multimodal_db::server::memory::MemoryConnectionStore;
use rusty_multimodal_db::server::protocol::{CompareOp, ErrorCode, ScanValue, WriteResult};
use rusty_multimodal_db::server::relation::RelationConnectionStore;
use rusty_multimodal_db::server::{serve, serve_tables, ConnectionStore, ServeOptions};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;
use uuid::Uuid;

fn unique_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{label}_{}_{n}", std::process::id()))
}

fn memory(n: u128, content: &str, category: &str, sensitive: bool) -> Memory {
    Memory {
        id: Uuid::from_u128(n),
        content: content.into(),
        category: category.into(),
        tags: vec!["sample".into(), format!("n{n}")],
        source: if n.is_multiple_of(2) {
            "import".into()
        } else {
            "manual".into()
        },
        metadata_json: "{}".into(),
        created_at_unix_ms: 1_000 * n as i64,
        updated_at_unix_ms: 1_000 * n as i64,
        memory_type: "unclassified".into(),
        status: "active".into(),
        sensitive,
        access_count: 0,
        deleted_at_unix_ms: 0,
        node_id: String::new(),
    }
}

fn sample_memories() -> Vec<Memory> {
    vec![
        memory(1, "Prefers merge commits", "preference", false),
        memory(2, "Records are insertable since ADR-0046", "fact", false),
        memory(3, "A private note", "general", true),
        memory(4, "Another general note", "general", false),
    ]
}

fn start_server() -> SocketAddr {
    start_server_at(unique_dir("memory_integration"))
}

fn start_server_at(dir: std::path::PathBuf) -> SocketAddr {
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("memories.mmap");
    let stack = if path.exists() {
        open_memory_production_stack_portable(&path).unwrap()
    } else {
        create_memory_production_stack(sample_memories(), &[], &path).unwrap()
    };
    let connection_store = Arc::new(MemoryConnectionStore::new(GenericProductionStore::new(
        stack,
    )));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, connection_store, ServeOptions::default()));
    addr
}

fn rows(result: QueryResult) -> Vec<(Uuid, Vec<(String, ScanValue)>)> {
    match result {
        QueryResult::Rows(rows) => rows,
        other => panic!("expected Rows, got {other:?}"),
    }
}

fn groups(result: QueryResult) -> Vec<Vec<(String, ScanValue)>> {
    match result {
        QueryResult::Groups(groups) => groups,
        other => panic!("expected Groups, got {other:?}"),
    }
}

/// `MEM` acceptance criterion 2: `GetById` returns all eleven fields by
/// name with `tags` as a list; the consumer's `list` filters — category,
/// source, the sensitive gate — are `FilterEq` and `Query`; `GROUP BY
/// category` counts match.
#[test]
fn get_list_filters_and_group_by_category_match_the_consumers_shapes() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let names: Vec<&str> = client
        .schema()
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "content",
            "category",
            "tags",
            "source",
            "metadata_json",
            "created_at_unix_ms",
            "updated_at_unix_ms",
            "memory_type",
            "status",
            "sensitive",
            "access_count",
            "deleted_at_unix_ms",
            "node_id"
        ]
    );
    let fields = client.get(Uuid::from_u128(3)).unwrap().unwrap();
    assert_eq!(
        fields[0],
        (
            "content".to_string(),
            ScanValue::Str("A private note".into())
        )
    );
    assert_eq!(
        fields[2],
        (
            "tags".to_string(),
            ScanValue::StrList(vec!["sample".into(), "n3".into()])
        )
    );
    assert_eq!(fields[9], ("sensitive".to_string(), ScanValue::Bool(true)));

    // `list(category = general)` — `FilterEq` on the indexed field.
    let mut general = client
        .filter_eq("category", ScanValue::Str("general".into()))
        .unwrap();
    general.sort();
    assert_eq!(general, vec![Uuid::from_u128(3), Uuid::from_u128(4)]);
    // The default list hides sensitive memories — a `WHERE` on a Bool.
    let visible = rows(
        client
            .query("SELECT content FROM memory WHERE sensitive = false")
            .unwrap(),
    );
    assert_eq!(visible.len(), 3);
    // `list(source = import)` — a read-only field is still queryable.
    let imported = rows(
        client
            .query("SELECT content FROM memory WHERE source = 'import'")
            .unwrap(),
    );
    assert_eq!(imported.len(), 2);
    // `GROUP BY category`.
    let counts = groups(
        client
            .query("SELECT category, COUNT(*) FROM memory GROUP BY category")
            .unwrap(),
    );
    let general_count = counts
        .iter()
        .find(|g| g[0] == ("category".to_string(), ScanValue::Str("general".into())))
        .unwrap();
    assert_eq!(
        general_count[1],
        ("COUNT(*)".to_string(), ScanValue::I64(2))
    );
}

/// `MEM` acceptance criterion 3: the consumer's `add_memory` is one
/// `insert` with every field; the retrieval counter is the one
/// `update`; content is read-only; and a server restarted on the same
/// directory serves the inserted memory with its tags and counter.
#[test]
fn insert_a_memory_bump_its_access_count_and_a_restart_serves_it() {
    let dir = unique_dir("memory_insert");
    let addr = start_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128(77);
    client
        .insert(
            id,
            &[
                ("content", ScanValue::Str("The Memory domain exists".into())),
                ("category", ScanValue::Str("fact".into())),
                (
                    "tags",
                    ScanValue::StrList(vec!["adr-0048".into(), "milestone".into()]),
                ),
                ("source", ScanValue::Str("claude-code".into())),
                ("metadata_json", ScanValue::Str(r#"{"pr":200}"#.into())),
                ("created_at_unix_ms", ScanValue::I64(77_000)),
                ("updated_at_unix_ms", ScanValue::I64(77_000)),
                ("memory_type", ScanValue::Str("decision".into())),
                ("status", ScanValue::Str("active".into())),
                ("sensitive", ScanValue::Bool(false)),
                ("access_count", ScanValue::I64(0)),
                ("deleted_at_unix_ms", ScanValue::I64(0)),
                ("node_id", ScanValue::Str(String::new())),
            ],
        )
        .unwrap();
    assert!(client
        .update(id, "access_count", ScanValue::I64(1))
        .unwrap());
    match client.update(id, "access_count", ScanValue::I64(-1)) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    assert!(matches!(
        client.update(id, "content", ScanValue::Str("x".into())),
        Err(ClientError::Unsupported(_))
    ));
    let facts = rows(
        client
            .query("SELECT content, access_count FROM memory WHERE category = 'fact'")
            .unwrap(),
    );
    assert_eq!(facts.len(), 2);
    drop(client);

    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got[0],
        (
            "content".to_string(),
            ScanValue::Str("The Memory domain exists".into())
        )
    );
    assert_eq!(
        got[2],
        (
            "tags".to_string(),
            ScanValue::StrList(vec!["adr-0048".into(), "milestone".into()])
        )
    );
    assert_eq!(
        got[4],
        (
            "metadata_json".to_string(),
            ScanValue::Str(r#"{"pr":200}"#.into())
        )
    );
    assert_eq!(got[10], ("access_count".to_string(), ScanValue::I64(1)));
}

/// `TBL-FR-008` (ADR-0050) on a one-table `Memory` server: the one
/// relation, `mentions`, is listed with its rows in `entity`; a same-
/// table `JOIN memory b ON mentions` is refused client-side; `parent`
/// is `Unsupported`; a one-table server lists its one table and `Use`
/// of any other name is `Malformed`.
#[test]
fn a_one_table_memory_server_lists_mentions_as_foreign_and_itself_as_the_only_table() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(client.relations().len(), 1);
    assert_eq!(client.relations()[0].name, "mentions");
    assert_eq!(
        client.relations()[0].target_table.as_deref(),
        Some("entity")
    );
    assert!(matches!(
        client.parent(Uuid::from_u128(1)),
        Err(ClientError::Unsupported(_))
    ));
    assert_eq!(
        client.neighbors(Uuid::from_u128(1)).unwrap(),
        Vec::<Uuid>::new()
    );
    assert!(matches!(
        client.query("SELECT a.content, b.content FROM memory a JOIN memory b ON mentions"),
        Err(ClientError::Sql(_))
    ));
    assert_eq!(
        client.list_tables().unwrap(),
        (vec!["memory".to_string()], "memory".to_string())
    );
    assert!(client.use_table("memory").is_ok());
    match client.use_table("entity") {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    // A link to an entity on a server with no entity table: the server
    // cannot check the far end, so it is `Unsupported` (`TBL-FR-007`).
    match client.link(Uuid::from_u128(1), Uuid::from_u128(0xada), "mentions") {
        Err(ClientError::Server(ErrorCode::Unsupported, _)) => {}
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

/// `REP` acceptance criterion 2 (ADR-0049) on `Memory` over a socket —
/// the consumer's `update_memory`: `replace` with every field rewrites
/// `content`, `category`, `tags`, `metadata_json`, `sensitive`, and the
/// counter at once; every read sees the new version (`get`, `filter_eq`
/// on the new and the old category, `WHERE sensitive`); an unknown id
/// is `Ok(false)` with nothing created; a malformed list and a negative
/// counter are `Malformed` with nothing written; `upsert` replaces an
/// existing record and creates a missing one (`SessionOpen` inside a
/// session is `tests/server_transaction_integration.rs`'s); and a server restarted on the same directory serves
/// the replaced version.
#[test]
fn replace_a_memory_over_the_wire_every_read_sees_it_and_a_restart_serves_it() {
    let dir = unique_dir("memory_replace");
    let addr = start_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128(1);
    let before = client.get(id).unwrap().unwrap();
    assert_eq!(
        before[1],
        ("category".to_string(), ScanValue::Str("preference".into()))
    );
    let edited: Vec<(&str, ScanValue)> = vec![
        ("content", ScanValue::Str("memory 1, revised".into())),
        ("category", ScanValue::Str("decision".into())),
        ("tags", ScanValue::StrList(vec!["revised".into()])),
        ("source", ScanValue::Str("manual".into())),
        ("metadata_json", ScanValue::Str(r#"{"edited":true}"#.into())),
        ("created_at_unix_ms", ScanValue::I64(1_000)),
        ("updated_at_unix_ms", ScanValue::I64(2_000)),
        ("memory_type", ScanValue::Str("decision".into())),
        ("status", ScanValue::Str("active".into())),
        ("sensitive", ScanValue::Bool(true)),
        ("access_count", ScanValue::I64(4)),
        ("deleted_at_unix_ms", ScanValue::I64(0)),
        ("node_id", ScanValue::Str(String::new())),
    ];
    assert!(client.replace(id, &edited).unwrap());
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got[0],
        (
            "content".to_string(),
            ScanValue::Str("memory 1, revised".into())
        )
    );
    assert_eq!(got[9], ("sensitive".to_string(), ScanValue::Bool(true)));
    assert_eq!(got[10], ("access_count".to_string(), ScanValue::I64(4)));
    assert_eq!(
        client
            .filter_eq("category", ScanValue::Str("decision".into()))
            .unwrap(),
        vec![id]
    );
    assert!(!client
        .filter_eq("category", ScanValue::Str("preference".into()))
        .unwrap()
        .contains(&id));
    let sensitive = rows(
        client
            .query("SELECT content FROM memory WHERE sensitive = true")
            .unwrap(),
    );
    assert_eq!(sensitive.len(), 2, "memory 1 joined memory 3");
    assert_eq!(
        rows(client.query("SELECT content FROM memory").unwrap()).len(),
        4,
        "no new record"
    );

    // An unknown id: `Ok(false)`, nothing created.
    assert!(!client.replace(Uuid::from_u128(99), &edited).unwrap());
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());
    // Refusals write nothing.
    let mut negative = edited.clone();
    negative[10] = ("access_count", ScanValue::I64(-1));
    match client.replace(id, &negative) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    match client.replace(id, &edited[..12]) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    assert!(matches!(
        client.replace(id, &[("no_such_field", ScanValue::I64(0))]),
        Err(ClientError::UnknownField(_))
    ));
    assert_eq!(
        client.get(id).unwrap().unwrap(),
        got,
        "nothing written on refusal"
    );

    // `upsert`: an existing id is replaced (`false`), a new one created (`true`).
    let mut again = edited.clone();
    again[10] = ("access_count", ScanValue::I64(5));
    assert!(!client.upsert(id, &again).unwrap());
    assert_eq!(
        client.get(id).unwrap().unwrap()[10],
        ("access_count".to_string(), ScanValue::I64(5))
    );
    assert!(client.upsert(Uuid::from_u128(99), &edited).unwrap());
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_some());

    drop(client);

    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got[0],
        (
            "content".to_string(),
            ScanValue::Str("memory 1, revised".into())
        )
    );
    assert_eq!(
        got[2],
        (
            "tags".to_string(),
            ScanValue::StrList(vec!["revised".into()])
        )
    );
    assert_eq!(got[10], ("access_count".to_string(), ScanValue::I64(5)));
    let mut decisions = client
        .filter_eq("category", ScanValue::Str("decision".into()))
        .unwrap();
    decisions.sort();
    assert_eq!(decisions, vec![id, Uuid::from_u128(99)], "both survived");
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_some());
}

/// `GRD-FR-005`/`006` (ADR-0054), over a real socket: a guarded replace
/// is last-writer-wins on `updated_at_unix_ms` — a newer version
/// replaces, an older one is refused with nothing written, an unknown id
/// is `NotFound`; a guard of the wrong kind is the server's `Malformed`,
/// an ordering guard on a `Str` field and an unknown guard field are
/// refused locally; the winner survives a restart.
#[test]
fn a_guarded_replace_is_last_writer_wins_over_the_wire_and_survives_a_restart() {
    let dir = unique_dir("memory_replace_if");
    let addr = start_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128(1);
    let stored = client.get(id).unwrap().unwrap();
    assert_eq!(
        stored[6],
        ("updated_at_unix_ms".to_string(), ScanValue::I64(1_000))
    );
    let version = |content: &str, updated_at: i64| -> Vec<(&'static str, ScanValue)> {
        vec![
            ("content", ScanValue::Str(content.into())),
            ("category", ScanValue::Str("preference".into())),
            ("tags", ScanValue::StrList(vec!["sync".into()])),
            ("source", ScanValue::Str("hub".into())),
            ("metadata_json", ScanValue::Str("{}".into())),
            ("created_at_unix_ms", ScanValue::I64(1_000)),
            ("updated_at_unix_ms", ScanValue::I64(updated_at)),
            ("memory_type", ScanValue::Str("unclassified".into())),
            ("status", ScanValue::Str("active".into())),
            ("sensitive", ScanValue::Bool(false)),
            ("access_count", ScanValue::I64(0)),
            ("deleted_at_unix_ms", ScanValue::I64(0)),
            ("node_id", ScanValue::Str("node-b".into())),
        ]
    };
    let lww = |mine: i64| ("updated_at_unix_ms", CompareOp::Lt, ScanValue::I64(mine));

    // A newer version wins.
    let newer = version("from node B, newer", 5_000);
    assert_eq!(
        client.replace_if(id, &newer, lww(5_000)).unwrap(),
        GuardedReplace::Replaced
    );
    assert_eq!(
        client.get(id).unwrap().unwrap()[0],
        (
            "content".to_string(),
            ScanValue::Str("from node B, newer".into())
        )
    );
    // An older version loses: refused, nothing written.
    let older = version("from node A, stale", 3_000);
    assert_eq!(
        client.replace_if(id, &older, lww(3_000)).unwrap(),
        GuardedReplace::Refused
    );
    assert_eq!(
        client.get(id).unwrap().unwrap()[0],
        (
            "content".to_string(),
            ScanValue::Str("from node B, newer".into())
        )
    );
    // An equal timestamp loses too (strict `Lt`), a compare-and-swap holds.
    assert_eq!(
        client.replace_if(id, &newer, lww(5_000)).unwrap(),
        GuardedReplace::Refused
    );
    assert_eq!(
        client
            .replace_if(
                id,
                &version("cas", 6_000),
                ("updated_at_unix_ms", CompareOp::Eq, ScanValue::I64(5_000)),
            )
            .unwrap(),
        GuardedReplace::Replaced
    );
    // An unknown id.
    assert_eq!(
        client
            .replace_if(Uuid::from_u128(99), &newer, lww(9_000))
            .unwrap(),
        GuardedReplace::NotFound
    );
    assert!(client.get(Uuid::from_u128(99)).unwrap().is_none());
    // Guard refusals: wrong kind (server), ordering on Str and unknown
    // field (client, no frame). Nothing written by any of them.
    let before = client.get(id).unwrap().unwrap();
    match client.replace_if(
        id,
        &newer,
        (
            "updated_at_unix_ms",
            CompareOp::Lt,
            ScanValue::Str("x".into()),
        ),
    ) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    assert!(matches!(
        client.replace_if(
            id,
            &newer,
            ("content", CompareOp::Lt, ScanValue::Str("x".into()))
        ),
        Err(ClientError::Unsupported("ordering guard"))
    ));
    assert!(matches!(
        client.replace_if(
            id,
            &newer,
            ("no_such_field", CompareOp::Eq, ScanValue::I64(0))
        ),
        Err(ClientError::UnknownField(_))
    ));
    assert_eq!(client.get(id).unwrap().unwrap(), before);
    drop(client);

    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let got = client.get(id).unwrap().unwrap();
    assert_eq!(
        got[0],
        ("content".to_string(), ScanValue::Str("cas".into()))
    );
    assert_eq!(
        got[6],
        ("updated_at_unix_ms".to_string(), ScanValue::I64(6_000))
    );
}

/// `PAG-FR-004`/`005` (ADR-0055), over a real socket: pages by
/// `updated_at_unix_ms` walk the table in `(value, id)` order — the last
/// row's key as the next cursor, a cursor at the maximum id as "after
/// time *t*", a tie broken by id; the client refuses a `Str` order and a
/// zero limit locally.
#[test]
fn ordered_keyset_pages_by_updated_at_walk_the_table_over_the_wire() {
    let addr = start_server();
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let ids = |rows: &[(Uuid, Vec<(String, ScanValue)>)]| {
        rows.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    };
    let id = Uuid::from_u128;
    // Four sample memories, updated_at 1000·n; a fifth ties with the fourth.
    let mut fifth: Vec<(&str, ScanValue)> = client
        .get(id(4))
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|(name, value)| (Box::leak(name.into_boxed_str()) as &str, value))
        .collect();
    fifth[0] = ("content", ScanValue::Str("a tie".into()));
    client.insert(id(5), &fifth).unwrap();

    let first = client.page("updated_at_unix_ms", None, 2).unwrap();
    assert_eq!(ids(&first), vec![id(1), id(2)]);
    // `ORD-FR-005` (ADR-0059): `updated_at_unix_ms` is answered from the
    // sorted index; every other orderable field still takes the scan
    // path, and the two agree.
    let by_created = client.page("created_at_unix_ms", None, 2).unwrap();
    assert_eq!(ids(&by_created), ids(&first));
    assert_eq!(first[0].1.len(), 13, "every field of each record");
    let (last_id, last_fields) = &first[1];
    let cursor = (last_fields[6].1.clone(), *last_id);
    assert_eq!(cursor, (ScanValue::I64(2_000), id(2)));
    let second = client.page("updated_at_unix_ms", Some(cursor), 2).unwrap();
    assert_eq!(ids(&second), vec![id(3), id(4)]);
    let third = client
        .page(
            "updated_at_unix_ms",
            Some((ScanValue::I64(4_000), id(4))),
            2,
        )
        .unwrap();
    assert_eq!(ids(&third), vec![id(5)], "the tie broken by id");
    let after_third = client
        .page(
            "updated_at_unix_ms",
            Some((ScanValue::I64(4_000), id(5))),
            2,
        )
        .unwrap();
    assert!(after_third.is_empty(), "the walk ends");
    // "Everything updated after 3000": a cursor at the maximum id.
    let since = client
        .page(
            "updated_at_unix_ms",
            Some((ScanValue::I64(3_000), Uuid::from_u128(u128::MAX))),
            100,
        )
        .unwrap();
    assert_eq!(ids(&since), vec![id(4), id(5)]);

    assert!(matches!(
        client.page("content", None, 10),
        Err(ClientError::Unsupported("page order"))
    ));
    assert!(matches!(
        client.page("updated_at_unix_ms", None, 0),
        Err(ClientError::Unsupported("page limit"))
    ));
    assert!(matches!(
        client.page("no_such_field", None, 10),
        Err(ClientError::UnknownField(_))
    ));
    match client.page(
        "updated_at_unix_ms",
        Some((ScanValue::Str("x".into()), id(1))),
        10,
    ) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed for a wrong-kind cursor, got {other:?}"),
    }
}

fn sample_entities() -> Vec<Entity> {
    let entity = |n: u128, label: &str, kind: &str| Entity {
        id: Uuid::from_u128(n),
        label: label.into(),
        kind: kind.into(),
        mention_count: 0,
        aliases: vec![],
    };
    vec![
        entity(0xada, "Ada Lovelace", "person"),
        entity(0xe1e, "Analytical Engine", "artifact"),
        entity(0x10d, "London", "place"),
    ]
}

/// `TBL-FR-001` (ADR-0050): one listener, two tables — `memory` (primary)
/// with its `mentions` edges seeded, and `entity`. Reopened from the
/// files alone when the directory already holds a store.
fn start_two_table_server_at(dir: std::path::PathBuf) -> SocketAddr {
    std::fs::create_dir_all(&dir).unwrap();
    let memories = dir.join("memories.mmap");
    let entities = dir.join("entities.mmap");
    let memory_stack = if memories.exists() {
        open_memory_production_stack_portable(&memories).unwrap()
    } else {
        create_memory_production_stack(
            sample_memories(),
            &[
                (Uuid::from_u128(1), Uuid::from_u128(0xada)),
                (Uuid::from_u128(2), Uuid::from_u128(0xe1e)),
                (Uuid::from_u128(2), Uuid::from_u128(0xada)),
            ],
            &memories,
        )
        .unwrap()
    };
    let entity_stack = if entities.exists() {
        open_entity_production_stack_portable(&entities).unwrap()
    } else {
        create_entity_production_stack(sample_entities(), &[], &[], &entities).unwrap()
    };
    let memory: Arc<dyn ConnectionStore> = Arc::new(MemoryConnectionStore::new(
        GenericProductionStore::new(memory_stack),
    ));
    let entity: Arc<dyn ConnectionStore> = Arc::new(EntityConnectionStore::new(
        GenericProductionStore::new(entity_stack),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        serve_tables(
            listener,
            vec![
                ("memory".to_string(), memory),
                ("entity".to_string(), entity),
            ],
            0,
            ServeOptions::default(),
        )
    });
    addr
}

/// `TBL` acceptance criteria 1–3 (ADR-0050) over a socket — the
/// consumer's `memory_entities` path end to end: `ListTables`; the
/// cross-table `SELECT m.content, e.label FROM memory m JOIN entity e ON
/// mentions` in one round trip, with a right-side `WHERE` on the entity
/// table's own field; `mentions` read from both ends; `use_table`
/// switching every table-less request (the schema, `get`, a `Query`)
/// and back; a runtime `link` to an entity checked against the entity
/// table (`RecordNotFound` for an unknown one, `Malformed` for a label
/// the domain lacks); an unknown table `Malformed`; and a second server
/// on the same directory serving the edge and the join.
#[test]
fn two_tables_on_one_connection_join_across_use_and_link_and_survive_a_restart() {
    let dir = unique_dir("memory_two_tables");
    let addr = start_two_table_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let (m1, m2, m3) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let (ada, engine, london) = (
        Uuid::from_u128(0xada),
        Uuid::from_u128(0xe1e),
        Uuid::from_u128(0x10d),
    );
    assert_eq!(
        client.list_tables().unwrap(),
        (
            vec!["memory".to_string(), "entity".to_string()],
            "memory".to_string()
        )
    );
    assert_eq!(client.table(), None, "on the primary until a Use");

    // Criterion 3: the cross-table join, one round trip, both sides named.
    let joined = match client
        .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions")
        .unwrap()
    {
        QueryResult::Joined(rows) => rows,
        other => panic!("expected Joined, got {other:?}"),
    };
    assert_eq!(joined.len(), 3);
    let mut pairs: Vec<(Uuid, Uuid)> = joined.iter().map(|r| (r.left_id, r.right_id)).collect();
    pairs.sort();
    assert_eq!(pairs, vec![(m1, ada), (m2, ada), (m2, engine)]);
    let ada_row = joined.iter().find(|r| r.right_id == ada).unwrap();
    assert!(ada_row
        .fields
        .iter()
        .any(|(name, v)| name == "e.label" && *v == ScanValue::Str("Ada Lovelace".into())));
    assert!(ada_row.fields.iter().any(|(name, _)| name == "m.content"));
    // A right-side WHERE resolves against the entity table's schema.
    let people = match client
        .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions WHERE e.kind = 'artifact'")
        .unwrap()
    {
        QueryResult::Joined(rows) => rows,
        other => panic!("expected Joined, got {other:?}"),
    };
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].right_id, engine);
    // Wrong table for the relation, or the FROM table itself: refused client-side.
    assert!(matches!(
        client.query("SELECT m.content, x.content FROM memory m JOIN memory x ON mentions"),
        Err(ClientError::Sql(_))
    ));

    // Both ends of `mentions`: the entities a memory mentions, and the
    // memories that mention an entity (the consumer's two lookups).
    assert_eq!(
        client.neighbors_by_relation(m1, "mentions").unwrap(),
        vec![ada]
    );
    let mut about_ada = client.neighbors_by_relation(ada, "mentions").unwrap();
    about_ada.sort();
    assert_eq!(about_ada, vec![m1, m2]);

    // Criterion 2: `Use` switches every table-less request, and back.
    client.use_table("entity").unwrap();
    assert_eq!(client.table(), Some("entity"));
    let names: Vec<&str> = client
        .schema()
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, vec!["label", "kind", "mention_count", "aliases"]);
    assert_eq!(
        client.get(london).unwrap().unwrap()[0],
        ("label".to_string(), ScanValue::Str("London".into()))
    );
    assert!(
        client.get(m1).unwrap().is_none(),
        "a memory id is not an entity"
    );
    let places = rows(
        client
            .query("SELECT label FROM entity WHERE kind = 'place'")
            .unwrap(),
    );
    assert_eq!(places.len(), 1);
    client.use_table("memory").unwrap();
    assert_eq!(client.table(), Some("memory"));
    assert_eq!(client.schema().fields.len(), 13);
    match client.use_table("customer") {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    assert_eq!(client.table(), Some("memory"), "unchanged on refusal");

    // `TBL-FR-007`: a runtime link, its far end checked in the entity table.
    client.link(m3, london, "mentions").unwrap();
    client.link(m3, london, "mentions").unwrap();
    match client.link(m3, Uuid::from_u128(0xbad), "mentions") {
        Err(ClientError::Server(ErrorCode::RecordNotFound, _)) => {}
        other => panic!("expected RecordNotFound, got {other:?}"),
    }
    match client.link(m3, london, "relates_to") {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    match client.link(Uuid::from_u128(0xbad), london, "mentions") {
        Err(ClientError::Server(ErrorCode::RecordNotFound, _)) => {}
        other => panic!("expected RecordNotFound, got {other:?}"),
    }
    assert_eq!(
        client.neighbors_by_relation(london, "mentions").unwrap(),
        vec![m3]
    );
    drop(client);

    // A second server on the same directory serves the edge and the join.
    let addr = start_two_table_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let joined = match client
        .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions WHERE e.kind = 'place'")
        .unwrap()
    {
        QueryResult::Joined(rows) => rows,
        other => panic!("expected Joined, got {other:?}"),
    };
    assert_eq!(joined.len(), 1);
    assert_eq!((joined[0].left_id, joined[0].right_id), (m3, london));
    client.use_table("entity").unwrap();
    assert!(client.get(london).unwrap().is_some());
}

/// `DEL` acceptance criteria 2–3 (ADR-0051) on the two-table server —
/// the consumer's `delete_memory` and entity delete: deleting a memory
/// removes it from every read and its `mentions` edge from the entity's
/// side; a repeat is `Ok(false)`; deleting an **entity** (`Use entity`)
/// detaches every memory's `mentions` edge to it on the memory table
/// (`DEL-FR-007`), the memories themselves intact, and the cross-table
/// join shrinks accordingly; the deleted ids can be inserted again; and
/// a second server on the same directory serves the deletions.
#[test]
fn delete_a_memory_and_an_entity_across_tables_and_a_restart_serves_it() {
    let dir = unique_dir("memory_delete");
    let addr = start_two_table_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let (m1, m2) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let (ada, engine) = (Uuid::from_u128(0xada), Uuid::from_u128(0xe1e));
    let joined = |client: &mut SchemaDrivenClient| match client
        .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions")
        .unwrap()
    {
        QueryResult::Joined(rows) => {
            let mut pairs: Vec<(Uuid, Uuid)> =
                rows.iter().map(|r| (r.left_id, r.right_id)).collect();
            pairs.sort();
            pairs
        }
        other => panic!("expected Joined, got {other:?}"),
    };
    assert_eq!(
        joined(&mut client),
        vec![(m1, ada), (m2, ada), (m2, engine)]
    );

    // Criterion 2: delete a memory.
    assert!(client.delete(m1).unwrap());
    assert!(client.get(m1).unwrap().is_none());
    assert!(!client.delete(m1).unwrap(), "a repeat is NotFound");
    assert_eq!(
        rows(client.query("SELECT content FROM memory").unwrap()).len(),
        3
    );
    assert_eq!(
        client.neighbors_by_relation(ada, "mentions").unwrap(),
        vec![m2]
    );
    assert_eq!(joined(&mut client), vec![(m2, ada), (m2, engine)]);

    // Criterion 3: delete an entity from the entity table — the memory
    // table's edges to it go (`DEL-FR-007`), the memory stays.
    client.use_table("entity").unwrap();
    assert!(client.delete(ada).unwrap());
    assert!(client.get(ada).unwrap().is_none());
    assert!(!client.delete(ada).unwrap());
    client.use_table("memory").unwrap();
    assert!(client.get(m2).unwrap().is_some(), "the memory is intact");
    assert_eq!(
        client.neighbors_by_relation(m2, "mentions").unwrap(),
        vec![engine]
    );
    assert_eq!(
        client.neighbors_by_relation(ada, "mentions").unwrap(),
        Vec::<Uuid>::new()
    );
    assert_eq!(joined(&mut client), vec![(m2, engine)]);

    // The deleted memory's id can be inserted again — a fresh record, no edges.
    client
        .insert(
            m1,
            &[
                ("content", ScanValue::Str("memory 1, reborn".into())),
                ("category", ScanValue::Str("general".into())),
                ("tags", ScanValue::StrList(vec![])),
                ("source", ScanValue::Str("manual".into())),
                ("metadata_json", ScanValue::Str("{}".into())),
                ("created_at_unix_ms", ScanValue::I64(9_000)),
                ("updated_at_unix_ms", ScanValue::I64(9_000)),
                ("memory_type", ScanValue::Str("unclassified".into())),
                ("status", ScanValue::Str("active".into())),
                ("sensitive", ScanValue::Bool(false)),
                ("access_count", ScanValue::I64(0)),
                ("deleted_at_unix_ms", ScanValue::I64(0)),
                ("node_id", ScanValue::Str(String::new())),
            ],
        )
        .unwrap();
    assert_eq!(
        client.neighbors_by_relation(m1, "mentions").unwrap(),
        Vec::<Uuid>::new()
    );
    drop(client);

    let addr = start_two_table_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(
        client.get(m1).unwrap().unwrap()[0],
        (
            "content".to_string(),
            ScanValue::Str("memory 1, reborn".into())
        )
    );
    assert_eq!(
        client.neighbors_by_relation(ada, "mentions").unwrap(),
        Vec::<Uuid>::new()
    );
    assert_eq!(joined(&mut client), vec![(m2, engine)]);
    client.use_table("entity").unwrap();
    assert!(client.get(ada).unwrap().is_none());
    assert!(client.get(engine).unwrap().is_some());
}

/// `CMP` acceptance criterion 2 (ADR-0052) on the two-table server: after
/// inserts, links, and deletes, `compact` on each table reports what it
/// reclaimed, every read before and after agrees, a second `compact`
/// reclaims nothing, and a restart on the compacted files serves the
/// same data.
#[test]
fn compact_each_table_over_the_wire_and_a_restart_serves_the_same_data() {
    let dir = unique_dir("memory_compact");
    let addr = start_two_table_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let (m1, m3) = (Uuid::from_u128(1), Uuid::from_u128(3));
    let (ada, london) = (Uuid::from_u128(0xada), Uuid::from_u128(0x10d));
    client.link(m3, london, "mentions").unwrap();
    assert!(client.delete(m1).unwrap());
    let joined = |client: &mut SchemaDrivenClient| match client
        .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions")
        .unwrap()
    {
        QueryResult::Joined(rows) => {
            let mut pairs: Vec<(Uuid, Uuid)> =
                rows.iter().map(|r| (r.left_id, r.right_id)).collect();
            pairs.sort();
            pairs
        }
        other => panic!("expected Joined, got {other:?}"),
    };
    let before = joined(&mut client);
    let contents_before = rows(client.query("SELECT content FROM memory").unwrap()).len();

    let report = client.compact().unwrap();
    assert_eq!(report.records, 3);
    assert_eq!(report.slots_reclaimed, 1);
    assert_eq!(report.log_entries_folded, 1, "the tombstone");
    assert_eq!(report.edge_logs_folded, 1, "the runtime link");
    assert_eq!(joined(&mut client), before);
    assert_eq!(
        rows(client.query("SELECT content FROM memory").unwrap()).len(),
        contents_before
    );
    let again = client.compact().unwrap();
    assert_eq!(again.records, 3);
    assert_eq!(
        again.slots_reclaimed + again.log_entries_folded + again.edge_logs_folded,
        0
    );

    client.use_table("entity").unwrap();
    assert!(client.delete(ada).unwrap());
    let entity_report = client.compact().unwrap();
    assert_eq!(entity_report.records, 2);
    assert_eq!(entity_report.slots_reclaimed, 1);
    client.use_table("memory").unwrap();
    let after_cascade = joined(&mut client);
    assert!(after_cascade.iter().all(|(_, e)| *e != ada));
    drop(client);

    let addr = start_two_table_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    assert_eq!(joined(&mut client), after_cascade);
    assert!(client.get(m1).unwrap().is_none());
    assert_eq!(
        client.neighbors_by_relation(m3, "mentions").unwrap(),
        vec![london]
    );
}

/// `SYN-FR-001`/`003` (ADR-0056), over a real socket: the two sync fields
/// with their sentinels — a live record inserts with `0`/`""`; a replace
/// stamps a deletion and a node; `Page` by `deleted_at_unix_ms` lists the
/// live rows first; `Query` finds the purge set; `GROUP BY node_id` counts
/// per node with `""` its own group; a negative stamp is `Malformed`; both
/// survive a restart.
#[test]
fn sync_fields_carry_sentinels_and_serve_the_purge_set_over_the_wire() {
    let dir = unique_dir("memory_sync_fields");
    let addr = start_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128;
    let fields = client.get(id(1)).unwrap().unwrap();
    assert_eq!(fields.len(), 13);
    assert_eq!(
        fields[11],
        ("deleted_at_unix_ms".to_string(), ScanValue::I64(0))
    );
    assert_eq!(
        fields[12],
        ("node_id".to_string(), ScanValue::Str(String::new()))
    );

    // Tombstone memory 2 from node "laptop" at t=7000, attribute memory 3
    // to "desktop", leave 1 and 4 live and unattributed.
    let stamped = |mut fields: Vec<(String, ScanValue)>, deleted_at: i64, node: &str| {
        fields[11].1 = ScanValue::I64(deleted_at);
        fields[12].1 = ScanValue::Str(node.into());
        fields
            .into_iter()
            .map(|(name, value)| (Box::leak(name.into_boxed_str()) as &str, value))
            .collect::<Vec<(&str, ScanValue)>>()
    };
    let two = stamped(client.get(id(2)).unwrap().unwrap(), 7_000, "laptop");
    assert!(client.replace(id(2), &two).unwrap());
    let three = stamped(client.get(id(3)).unwrap().unwrap(), 0, "desktop");
    assert!(client.replace(id(3), &three).unwrap());

    // Live rows first: three zeros, then the tombstone.
    let page = client.page("deleted_at_unix_ms", None, 10).unwrap();
    let stamps: Vec<i64> = page
        .iter()
        .map(|(_, f)| match &f[11].1 {
            ScanValue::I64(v) => *v,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(stamps, vec![0, 0, 0, 7_000]);
    assert_eq!(page[3].0, id(2));
    // The purge set before a cutoff.
    let purge = rows(
        client
            .query(
                "SELECT content FROM memory WHERE deleted_at_unix_ms > 0 AND deleted_at_unix_ms < 8000",
            )
            .unwrap(),
    );
    assert_eq!(purge.len(), 1);
    assert_eq!(purge[0].0, id(2));
    // Per-node counts, the empty node its own group.
    let groups = match client
        .query("SELECT node_id, COUNT(*) FROM memory GROUP BY node_id")
        .unwrap()
    {
        QueryResult::Groups(groups) => groups,
        other => panic!("expected Groups, got {other:?}"),
    };
    let mut counts: Vec<(String, i64)> = groups
        .into_iter()
        .map(|g| {
            let node = match g.iter().find(|(name, _)| name == "node_id") {
                Some((_, ScanValue::Str(s))) => s.clone(),
                other => panic!("{other:?}"),
            };
            let n = match g.iter().find(|(name, _)| name.starts_with("COUNT")) {
                Some((_, ScanValue::I64(n))) => *n,
                other => panic!("{other:?}"),
            };
            (node, n)
        })
        .collect();
    counts.sort();
    assert_eq!(
        counts,
        vec![
            (String::new(), 2),
            ("desktop".to_string(), 1),
            ("laptop".to_string(), 1)
        ]
    );
    // A negative stamp is refused with nothing written.
    let negative = stamped(client.get(id(1)).unwrap().unwrap(), -1, "");
    match client.replace(id(1), &negative) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    drop(client);

    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let two = client.get(id(2)).unwrap().unwrap();
    assert_eq!(two[11].1, ScanValue::I64(7_000));
    assert_eq!(two[12].1, ScanValue::Str("laptop".into()));
    assert_eq!(client.get(id(1)).unwrap().unwrap()[11].1, ScanValue::I64(0));
}

/// `CNT-FR-002`–`004` (ADR-0057), over a real socket on the two-table
/// server: `count_edges("mentions")` is one round trip and tracks a link
/// and a cascading entity delete; an unknown label is `Malformed`; the
/// entity table counts its own labels.
#[test]
fn count_edges_is_one_round_trip_and_tracks_links_and_deletes() {
    let addr = start_two_table_server_at(unique_dir("memory_count_edges"));
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let before = client.count_edges("mentions").unwrap();
    assert_eq!(before, 3, "the three sample mentions");
    client
        .link(Uuid::from_u128(1), Uuid::from_u128(0xe1e), "mentions")
        .unwrap();
    assert_eq!(client.count_edges("mentions").unwrap(), 4);
    match client.count_edges("no_such_label") {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
    // A cascading entity delete drops every mention of it.
    client.use_table("entity").unwrap();
    assert_eq!(
        client.count_edges("relates_to").unwrap(),
        0,
        "no entity edges seeded"
    );
    assert!(client.delete(Uuid::from_u128(0xada)).unwrap());
    client.use_table("memory").unwrap();
    assert_eq!(
        client.count_edges("mentions").unwrap(),
        2,
        "both mentions of 0xada gone"
    );
}

/// `REL-FR-005` (ADR-0058): the three-table server the binary runs —
/// `relation` open-or-created beside the two-table helper's stores.
fn start_three_table_server_at(dir: std::path::PathBuf) -> SocketAddr {
    std::fs::create_dir_all(&dir).unwrap();
    let memories = dir.join("memories.mmap");
    let entities = dir.join("entities.mmap");
    let memory_stack = if memories.exists() {
        open_memory_production_stack_portable(&memories).unwrap()
    } else {
        create_memory_production_stack(sample_memories(), &[], &memories).unwrap()
    };
    let entity_stack = if entities.exists() {
        open_entity_production_stack_portable(&entities).unwrap()
    } else {
        create_entity_production_stack(sample_entities(), &[], &[], &entities).unwrap()
    };
    let relation_stack =
        open_or_create_relation_production_stack(&dir.join("relations.mmap")).unwrap();
    let memory: Arc<dyn ConnectionStore> = Arc::new(MemoryConnectionStore::new(
        GenericProductionStore::new(memory_stack),
    ));
    let entity: Arc<dyn ConnectionStore> = Arc::new(EntityConnectionStore::new(
        GenericProductionStore::new(entity_stack),
    ));
    let relation: Arc<dyn ConnectionStore> = Arc::new(RelationConnectionStore::new(
        GenericProductionStore::new(relation_stack),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        serve_tables(
            listener,
            vec![
                ("memory".to_string(), memory),
                ("entity".to_string(), entity),
                ("relation".to_string(), relation),
            ],
            0,
            ServeOptions::default(),
        )
    });
    addr
}

/// `REL-FR-004`/`005` (ADR-0058), over a real socket: the `relation` table
/// as the hub's `entity_relations` — insert two directed edges, out-edges
/// by `FilterEq subject`, in-edges by `Query object`, one label by
/// `Query`, `Page` by `updated_at_unix_ms`, a last-writer-wins
/// `ReplaceIf`, a count by `Aggregate`, a delete; the empty endpoint
/// refused; everything surviving a restart.
#[test]
fn the_relation_table_serves_directed_open_label_edges_over_the_wire() {
    let dir = unique_dir("relation_table");
    let addr = start_three_table_server_at(dir.clone());
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let (names, primary) = client.list_tables().unwrap();
    assert_eq!(names, vec!["memory", "entity", "relation"]);
    assert_eq!(primary, "memory");
    client.use_table("relation").unwrap();
    assert_eq!(client.schema().fields.len(), 7);
    let edge = |subject: &str, label: &str, object: &str, updated_at: i64| {
        vec![
            ("subject", ScanValue::Str(subject.into())),
            ("relation", ScanValue::Str(label.into())),
            ("object", ScanValue::Str(object.into())),
            ("created_at_unix_ms", ScanValue::I64(1_000)),
            ("updated_at_unix_ms", ScanValue::I64(updated_at)),
            ("node_id", ScanValue::Str("laptop".into())),
            ("deleted_at_unix_ms", ScanValue::I64(0)),
        ]
    };
    let id = Uuid::from_u128;
    client
        .insert(
            id(1),
            &edge("aaaaaaaaaaaa", "works_with", "bbbbbbbbbbbb", 1_000),
        )
        .unwrap();
    client
        .insert(
            id(2),
            &edge("aaaaaaaaaaaa", "located_in", "cccccccccccc", 2_000),
        )
        .unwrap();
    client
        .insert(
            id(3),
            &edge("bbbbbbbbbbbb", "works_with", "aaaaaaaaaaaa", 3_000),
        )
        .unwrap();
    match client.insert(id(4), &edge("", "works_with", "aaaaaaaaaaaa", 4_000)) {
        Err(ClientError::Server(ErrorCode::Malformed, _)) => {}
        other => panic!("expected Malformed for an empty subject, got {other:?}"),
    }

    // Out-edges of aaa through the index; in-edges of aaa through Query.
    let mut out = client
        .filter_eq("subject", ScanValue::Str("aaaaaaaaaaaa".into()))
        .unwrap();
    out.sort();
    assert_eq!(out, vec![id(1), id(2)]);
    let inbound = rows(
        client
            .query("SELECT relation FROM relation WHERE object = 'aaaaaaaaaaaa'")
            .unwrap(),
    );
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].0, id(3));
    let by_label = rows(
        client
            .query("SELECT subject FROM relation WHERE relation = 'works_with'")
            .unwrap(),
    );
    assert_eq!(by_label.len(), 2);
    // The pull: pages by updated_at.
    let page = client.page("updated_at_unix_ms", None, 2).unwrap();
    assert_eq!(
        page.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![id(1), id(2)]
    );
    // The merge: last-writer-wins on updated_at.
    assert_eq!(
        client
            .replace_if(
                id(1),
                &edge("aaaaaaaaaaaa", "works_with", "bbbbbbbbbbbb", 9_000),
                ("updated_at_unix_ms", CompareOp::Lt, ScanValue::I64(9_000)),
            )
            .unwrap(),
        GuardedReplace::Replaced
    );
    assert_eq!(
        client
            .replace_if(
                id(1),
                &edge("aaaaaaaaaaaa", "works_with", "bbbbbbbbbbbb", 500),
                ("updated_at_unix_ms", CompareOp::Lt, ScanValue::I64(500)),
            )
            .unwrap(),
        GuardedReplace::Refused
    );
    // The count.
    let groups = match client.query("SELECT COUNT(*) FROM relation").unwrap() {
        QueryResult::Groups(groups) => groups,
        other => panic!("expected Groups, got {other:?}"),
    };
    assert_eq!(groups[0][0].1, ScanValue::I64(3));
    assert!(client.delete(id(2)).unwrap());
    drop(client);

    let addr = start_three_table_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    client.use_table("relation").unwrap();
    let one = client.get(id(1)).unwrap().unwrap();
    assert_eq!(one[4].1, ScanValue::I64(9_000), "the winner survived");
    assert!(client.get(id(2)).unwrap().is_none());
    assert_eq!(
        client
            .filter_eq("subject", ScanValue::Str("aaaaaaaaaaaa".into()))
            .unwrap(),
        vec![id(1)]
    );
}

/// `WBT` acceptance (ADR-0060): a pipelined batch applies each op and
/// records its own outcome; an atomic batch applies all under one lock on
/// success; an atomic batch whose validation fails applies nothing and
/// names the first failing op.
#[test]
fn write_batch_pipelined_applies_each_and_atomic_is_all_or_nothing() {
    let dir = unique_dir("memory_write_batch");
    let addr = start_server_at(dir);
    let mut client = SchemaDrivenClient::connect(addr).unwrap();
    let id = Uuid::from_u128;
    let full = |content: &str, stamp: i64| -> Vec<(&'static str, ScanValue)> {
        vec![
            ("content", ScanValue::Str(content.into())),
            ("category", ScanValue::Str("general".into())),
            ("tags", ScanValue::StrList(vec!["batch".into()])),
            ("source", ScanValue::Str("test".into())),
            ("metadata_json", ScanValue::Str("{}".into())),
            ("created_at_unix_ms", ScanValue::I64(stamp)),
            ("updated_at_unix_ms", ScanValue::I64(stamp)),
            ("memory_type", ScanValue::Str("unclassified".into())),
            ("status", ScanValue::Str("active".into())),
            ("sensitive", ScanValue::Bool(false)),
            ("access_count", ScanValue::I64(0)),
            ("deleted_at_unix_ms", ScanValue::I64(0)),
            ("node_id", ScanValue::Str(String::new())),
        ]
    };

    // Pipelined: two inserts, a delete of an absent id (soft NotFound),
    // and a mentions link from an inserted memory to a (foreign) entity.
    let a = full("first", 1_000);
    let b = full("second", 2_000);
    let results = client
        .write_batch(
            &[
                BatchOp::Insert {
                    id: id(101),
                    fields: &a,
                },
                BatchOp::Insert {
                    id: id(102),
                    fields: &b,
                },
                BatchOp::Delete { id: id(999) },
                BatchOp::Link {
                    left: id(101),
                    right: id(0xe1),
                    relation: "mentions",
                },
            ],
            false,
        )
        .unwrap();
    assert_eq!(
        results,
        vec![
            WriteResult::Inserted,
            WriteResult::Inserted,
            WriteResult::NotFound,
            WriteResult::Failed(ErrorCode::Unsupported),
        ]
    );
    assert!(client.get(id(101)).unwrap().is_some());

    // Atomic success: two more inserts, applied under one lock.
    let c = full("third", 3_000);
    let d = full("fourth", 4_000);
    let ok = client
        .write_batch(
            &[
                BatchOp::Insert {
                    id: id(103),
                    fields: &c,
                },
                BatchOp::Insert {
                    id: id(104),
                    fields: &d,
                },
            ],
            true,
        )
        .unwrap();
    assert_eq!(ok, vec![WriteResult::Inserted, WriteResult::Inserted]);

    // Atomic abort: a valid insert then a short (malformed) field list.
    // The batch names the second op and applies nothing.
    let e = full("fifth", 5_000);
    let short = vec![("content", ScanValue::Str("incomplete".into()))];
    match client.write_batch(
        &[
            BatchOp::Insert {
                id: id(105),
                fields: &e,
            },
            BatchOp::Insert {
                id: id(106),
                fields: &short,
            },
        ],
        true,
    ) {
        Err(ClientError::TransactionFailed { index, code, .. }) => {
            assert_eq!(index, 1);
            assert_eq!(code, ErrorCode::Malformed);
        }
        other => panic!("expected TransactionFailed, got {other:?}"),
    }
    assert!(
        client.get(id(105)).unwrap().is_none(),
        "an atomic abort applies nothing, not even the valid op before the failure"
    );
}

fn joined_rows(result: QueryResult) -> usize {
    match result {
        QueryResult::Joined(rows) => rows.len(),
        other => panic!("expected Joined, got {other:?}"),
    }
}

/// R1/R2/R5: real cross-table links and cascades agree for every request mode,
/// including reverse adjacency, joins, counts, and persisted detach on reopen.
#[test]
fn batch_cross_table_link_delete_matches_single_and_survives_reopen() {
    for mode in [None, Some(false), Some(true)] {
        let dir = unique_dir("batch_cross_table_delete");
        let addr = start_three_table_server_at(dir.clone());
        let mut client = SchemaDrivenClient::connect(addr).unwrap();
        let memory = Uuid::from_u128(1);
        let entity = Uuid::from_u128(0xada);
        assert_eq!(client.count_edges("mentions").unwrap(), 0);
        match mode {
            None => client.link(memory, entity, "mentions").unwrap(),
            Some(atomic) => assert_eq!(
                client
                    .write_batch(
                        &[BatchOp::Link {
                            left: memory,
                            right: entity,
                            relation: "mentions",
                        }],
                        atomic
                    )
                    .unwrap(),
                vec![WriteResult::Linked]
            ),
        }
        assert_eq!(
            client.neighbors_by_relation(memory, "mentions").unwrap(),
            vec![entity]
        );
        assert_eq!(
            client.neighbors_by_relation(entity, "mentions").unwrap(),
            vec![memory]
        );
        assert_eq!(client.count_edges("mentions").unwrap(), 1);
        assert_eq!(
            joined_rows(
                client
                    .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions")
                    .unwrap()
            ),
            1
        );
        client.use_table("entity").unwrap();
        match mode {
            None => assert!(client.delete(entity).unwrap()),
            Some(atomic) => assert_eq!(
                client
                    .write_batch(&[BatchOp::Delete { id: entity }], atomic)
                    .unwrap(),
                vec![WriteResult::Deleted]
            ),
        }
        // A second delete is NotFound, including both batch modes.
        match mode {
            None => assert!(!client.delete(entity).unwrap()),
            Some(atomic) => assert_eq!(
                client
                    .write_batch(&[BatchOp::Delete { id: entity }], atomic)
                    .unwrap(),
                vec![WriteResult::NotFound]
            ),
        }
        client.use_table("memory").unwrap();
        assert!(client
            .neighbors_by_relation(memory, "mentions")
            .unwrap()
            .is_empty());
        assert!(client
            .neighbors_by_relation(entity, "mentions")
            .unwrap()
            .is_empty());
        assert_eq!(client.count_edges("mentions").unwrap(), 0);
        assert!(
            joined_rows(
                client
                    .query("SELECT m.content, e.label FROM memory m JOIN entity e ON mentions")
                    .unwrap()
            ) == 0
        );
        drop(client);
        // As in the existing reopen tests, the old listener is idle; all
        // subsequent access is through new stacks opened from durable files.
        let mut reopened = SchemaDrivenClient::connect(start_three_table_server_at(dir)).unwrap();
        assert!(reopened
            .neighbors_by_relation(memory, "mentions")
            .unwrap()
            .is_empty());
        assert_eq!(reopened.count_edges("mentions").unwrap(), 0);
        reopened.use_table("entity").unwrap();
        assert!(reopened.get(entity).unwrap().is_none());
    }
}

/// R1/R3/R5: foreign misses, missing-table errors and own-table dependencies.
#[test]
fn batch_cross_table_preconditions_and_dependencies() {
    for single_table in [false, true] {
        for atomic in [false, true] {
            let addr = if single_table {
                start_server()
            } else {
                start_three_table_server_at(unique_dir("batch_cross_table_preconditions"))
            };
            let mut client = SchemaDrivenClient::connect(addr).unwrap();
            let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
            let fields: Vec<_> = fields
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone()))
                .collect();
            let new = Uuid::from_u128(101);
            let missing = Uuid::from_u128(0xbad);
            let expected = if single_table {
                ErrorCode::Unsupported
            } else {
                ErrorCode::RecordNotFound
            };
            assert!(
                matches!(client.link(Uuid::from_u128(1), missing, "mentions"),
                Err(ClientError::Server(code, _)) if code == expected)
            );
            let result = client.write_batch(
                &[
                    BatchOp::Insert {
                        id: new,
                        fields: &fields,
                    },
                    BatchOp::Link {
                        left: new,
                        right: missing,
                        relation: "mentions",
                    },
                    BatchOp::Delete {
                        id: Uuid::from_u128(2),
                    },
                ],
                atomic,
            );
            if atomic {
                assert!(
                    matches!(result, Err(ClientError::TransactionFailed { index: 1, code, .. }) if code == expected)
                );
            } else {
                assert_eq!(
                    result.unwrap(),
                    vec![
                        WriteResult::Inserted,
                        WriteResult::Failed(expected),
                        WriteResult::Deleted
                    ]
                );
            }
            assert_eq!(client.get(new).unwrap().is_none(), atomic);
            assert_eq!(client.get(Uuid::from_u128(2)).unwrap().is_some(), atomic);
            assert_eq!(client.count_edges("mentions").unwrap(), 0);
        }
    }
    for atomic in [false, true] {
        let mut client = SchemaDrivenClient::connect(start_three_table_server_at(unique_dir(
            "batch_dependencies",
        )))
        .unwrap();
        let memory = Uuid::from_u128(101);
        let entity = Uuid::from_u128(0xada);
        let fields = client.get(Uuid::from_u128(1)).unwrap().unwrap();
        let fields: Vec<_> = fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        assert_eq!(
            client
                .write_batch(
                    &[
                        BatchOp::Insert {
                            id: memory,
                            fields: &fields
                        },
                        BatchOp::Link {
                            left: memory,
                            right: entity,
                            relation: "mentions"
                        },
                    ],
                    atomic
                )
                .unwrap(),
            vec![WriteResult::Inserted, WriteResult::Linked]
        );
        let result = client.write_batch(
            &[
                BatchOp::Delete { id: memory },
                BatchOp::Link {
                    left: memory,
                    right: entity,
                    relation: "mentions",
                },
            ],
            atomic,
        );
        if atomic {
            assert!(matches!(
                result,
                Err(ClientError::TransactionFailed {
                    index: 1,
                    code: ErrorCode::RecordNotFound,
                    ..
                })
            ));
            assert_eq!(client.count_edges("mentions").unwrap(), 1);
        } else {
            assert_eq!(
                result.unwrap(),
                vec![
                    WriteResult::Deleted,
                    WriteResult::Failed(ErrorCode::RecordNotFound)
                ]
            );
            assert_eq!(client.count_edges("mentions").unwrap(), 0);
        }
        assert_eq!(client.get(memory).unwrap().is_some(), atomic);
    }
}

/// Entity relations have two own-table endpoints: both must track earlier
/// inserts/deletes. A local failure before a foreign failure wins by index.
#[test]
fn batch_entity_dependencies_rejection_preserves_cross_table_edges() {
    for atomic in [false, true] {
        let mut client = SchemaDrivenClient::connect(start_three_table_server_at(unique_dir(
            "batch_entity_dependencies",
        )))
        .unwrap();
        let ada = Uuid::from_u128(0xada);
        let new = Uuid::from_u128(101);
        client.link(Uuid::from_u128(1), ada, "mentions").unwrap();
        client.use_table("entity").unwrap();
        let fields = client.get(ada).unwrap().unwrap();
        let fields: Vec<_> = fields
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        assert_eq!(
            client
                .write_batch(
                    &[
                        BatchOp::Insert {
                            id: new,
                            fields: &fields
                        },
                        BatchOp::Link {
                            left: ada,
                            right: new,
                            relation: "knows"
                        },
                    ],
                    atomic
                )
                .unwrap(),
            vec![WriteResult::Inserted, WriteResult::Linked]
        );
        let result = client.write_batch(
            &[
                BatchOp::Delete { id: ada },
                BatchOp::Link {
                    left: new,
                    right: ada,
                    relation: "knows",
                },
            ],
            atomic,
        );
        if atomic {
            assert!(matches!(
                result,
                Err(ClientError::TransactionFailed {
                    index: 1,
                    code: ErrorCode::RecordNotFound,
                    ..
                })
            ));
        } else {
            assert_eq!(
                result.unwrap(),
                vec![
                    WriteResult::Deleted,
                    WriteResult::Failed(ErrorCode::RecordNotFound)
                ]
            );
        }
        assert_eq!(client.get(ada).unwrap().is_some(), atomic);
        client.use_table("memory").unwrap();
        assert_eq!(client.count_edges("mentions").unwrap(), u64::from(atomic));
        // A later bad field list must not mask the earlier missing own endpoint.
        let short = [("content", ScanValue::Str("incomplete".into()))];
        assert!(matches!(
            client.write_batch(
                &[
                    BatchOp::Link {
                        left: Uuid::from_u128(999),
                        right: Uuid::from_u128(0xe1e),
                        relation: "mentions"
                    },
                    BatchOp::Insert {
                        id: Uuid::from_u128(102),
                        fields: &short
                    },
                ],
                true
            ),
            Err(ClientError::TransactionFailed {
                index: 0,
                code: ErrorCode::RecordNotFound,
                ..
            })
        ));
        // Earlier local validation wins over a later foreign miss as well.
        assert!(matches!(
            client.write_batch(
                &[
                    BatchOp::Insert {
                        id: Uuid::from_u128(102),
                        fields: &short
                    },
                    BatchOp::Link {
                        left: Uuid::from_u128(1),
                        right: Uuid::from_u128(999),
                        relation: "mentions"
                    },
                ],
                true
            ),
            Err(ClientError::TransactionFailed {
                index: 0,
                code: ErrorCode::Malformed,
                ..
            })
        ));
        assert!(client.get(Uuid::from_u128(102)).unwrap().is_none());
    }
}

/// R4: competing socket writers either link before the delete (which then
/// detaches), or reject after it. Neither ordering can leave a dangling edge.
#[test]
fn batch_concurrent_link_and_delete_leave_no_dangling_edges() {
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;
    let addr = start_three_table_server_at(unique_dir("batch_concurrent_delete"));
    let mut observer = SchemaDrivenClient::connect(addr).unwrap();
    let memory_fields = observer.get(Uuid::from_u128(1)).unwrap().unwrap();
    observer.use_table("entity").unwrap();
    let entity_fields = observer.get(Uuid::from_u128(0xada)).unwrap().unwrap();
    for i in 0..12 {
        let entity = Uuid::from_u128(200 + i);
        let memory = Uuid::from_u128(100 + i);
        observer.use_table("entity").unwrap();
        let fields: Vec<_> = entity_fields
            .iter()
            .map(|(n, v)| (n.as_str(), v.clone()))
            .collect();
        observer.insert(entity, &fields).unwrap();
        let mut linker = SchemaDrivenClient::connect(addr).unwrap();
        let mut deleter = SchemaDrivenClient::connect(addr).unwrap();
        deleter.use_table("entity").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let other_barrier = Arc::clone(&barrier);
        let (done, wait) = mpsc::channel();
        let other_done = done.clone();
        let fields = memory_fields.clone();
        let linker_thread = thread::spawn(move || {
            let fields: Vec<_> = fields
                .iter()
                .map(|(n, v)| (n.as_str(), v.clone()))
                .collect();
            barrier.wait();
            let result = linker.write_batch(
                &[
                    BatchOp::Insert {
                        id: memory,
                        fields: &fields,
                    },
                    BatchOp::Link {
                        left: memory,
                        right: entity,
                        relation: "mentions",
                    },
                ],
                true,
            );
            assert!(
                matches!(&result, Ok(results) if results == &vec![WriteResult::Inserted, WriteResult::Linked])
                    || matches!(
                        result,
                        Err(ClientError::TransactionFailed {
                            index: 1,
                            code: ErrorCode::RecordNotFound,
                            ..
                        })
                    )
            );
            done.send(()).unwrap();
        });
        let deleter_thread = thread::spawn(move || {
            other_barrier.wait();
            if i % 2 == 0 {
                assert!(deleter.delete(entity).unwrap());
            } else {
                assert_eq!(
                    deleter
                        .write_batch(&[BatchOp::Delete { id: entity }], true)
                        .unwrap(),
                    vec![WriteResult::Deleted]
                );
            }
            other_done.send(()).unwrap();
        });
        let completed =
            (0..2).try_for_each(|_| wait.recv_timeout(Duration::from_secs(10)).map(|_| ()));
        for writer in [linker_thread, deleter_thread] {
            // A disconnected sender may have panicked: join to propagate its
            // original assertion. Preserve the deadline for an actual deadlock.
            if !matches!(completed, Err(mpsc::RecvTimeoutError::Timeout)) || writer.is_finished() {
                writer
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            }
        }
        completed.expect("writers complete without a lock cycle");
        observer.use_table("memory").unwrap();
        assert!(observer
            .neighbors_by_relation(memory, "mentions")
            .unwrap()
            .is_empty());
        assert_eq!(observer.count_edges("mentions").unwrap(), 0);
    }
}
