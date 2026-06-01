// ISS-120: Phase B dual-write must preserve the full `EntityKind` variant
// when projecting an entity into the unified `nodes` table.
//
// The unified `node_kind` column collapses every non-Topic `EntityKind`
// variant into the single string `"entity"`. To round-trip the original
// variant (Person / Concept / Place / …, plus `Other(s)`) through the
// unified read path, the writer stamps the serde-encoded `EntityKind`
// under the reserved attributes key `_legacy_kind`.
//
// Companion reader-side decoder: `extract_legacy_entity_kind` (graph/store.rs)
// strips the reserved key and reconstructs the `EntityKind`.
//
// These tests cover:
// 1. Canonical variant (Person)   → "person" stamped, node_kind=entity
// 2. Topic variant                 → "topic"  stamped, node_kind=topic
// 3. Other("robot") (normalized)   → {"other":"robot"} stamped, node_kind=entity
// 4. Pre-existing user attributes  → preserved alongside _legacy_kind
// 5. Round-trip through the reader → original EntityKind reconstructed exactly
// 6. Reader without _legacy_kind   → falls back to node_kind-derived default

use chrono::Utc;
use engramai::graph::storage_graph::init_graph_tables;
use engramai::graph::store::{extract_legacy_entity_kind, GraphWrite, SqliteGraphStore};
use engramai::graph::{Entity, EntityKind};
use rusqlite::{params, Connection};
use uuid::Uuid;

fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    init_graph_tables(&conn).expect("init graph tables");
    conn
}

fn sample_entity(name: &str, kind: EntityKind, attrs: serde_json::Value) -> Entity {
    let now = Utc::now();
    Entity {
        id: Uuid::new_v4(),
        canonical_name: name.into(),
        kind,
        summary: String::new(),
        attributes: attrs,
        history: vec![],
        merged_into: None,
        first_seen: now,
        last_seen: now,
        created_at: now,
        updated_at: now,
        episode_mentions: vec![],
        memory_mentions: vec![],
        activation: 0.0,
        importance: 0.5,
        identity_confidence: 0.8,
        agent_affect: None,
        arousal: 0.0,
        somatic_fingerprint: None,
        embedding: None,
    }
}

fn insert(conn: &mut Connection, e: &Entity) {
    let mut store = SqliteGraphStore::new(conn);
    GraphWrite::insert_entity(&mut store, e).expect("insert_entity");
}

fn read_node_attrs_and_kind(conn: &Connection, id: Uuid) -> (String, String) {
    conn.query_row(
        "SELECT attributes, node_kind FROM nodes WHERE id = ?1",
        params![id.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("nodes row")
}

#[test]
fn iss120_canonical_person_kind_stamped_into_attributes() {
    let mut conn = fresh_conn();
    let e = sample_entity("Alice", EntityKind::Person, serde_json::json!({}));
    let id = e.id;
    insert(&mut conn, &e);

    let (attrs, kind) = read_node_attrs_and_kind(&conn, id);
    assert_eq!(kind, "entity", "non-Topic must map to node_kind='entity'");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        v["_legacy_kind"],
        serde_json::json!("person"),
        "EntityKind::Person serde-encodes as the JSON string 'person'"
    );
}

#[test]
fn iss120_topic_kind_stamped_even_though_node_kind_already_carries_it() {
    // Topic is special — node_kind='topic' already preserves the variant.
    // We still stamp _legacy_kind so the reader has a single uniform code
    // path (no branch on node_kind).
    let mut conn = fresh_conn();
    let e = sample_entity("Rust GC topic", EntityKind::Topic, serde_json::json!({}));
    let id = e.id;
    insert(&mut conn, &e);

    let (attrs, kind) = read_node_attrs_and_kind(&conn, id);
    assert_eq!(kind, "topic");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["_legacy_kind"], serde_json::json!("topic"));
}

#[test]
fn iss120_other_kind_round_trips_as_object() {
    // EntityKind::Other(s) serde-encodes as {"other": "<normalized-s>"}.
    let mut conn = fresh_conn();
    let kind = EntityKind::other(" Robot "); // → normalized to "robot"
    let e = sample_entity("R2D2", kind.clone(), serde_json::json!({}));
    let id = e.id;
    insert(&mut conn, &e);

    let (attrs, node_kind) = read_node_attrs_and_kind(&conn, id);
    assert_eq!(node_kind, "entity");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["_legacy_kind"], serde_json::json!({"other": "robot"}));
}

#[test]
fn iss120_user_attributes_preserved_alongside_legacy_kind() {
    let mut conn = fresh_conn();
    let user_attrs = serde_json::json!({"score": 0.9, "tag": "important"});
    let e = sample_entity("Bob", EntityKind::Person, user_attrs);
    let id = e.id;
    insert(&mut conn, &e);

    let (attrs, _) = read_node_attrs_and_kind(&conn, id);
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["_legacy_kind"], serde_json::json!("person"));
    assert_eq!(v["score"], serde_json::json!(0.9));
    assert_eq!(v["tag"], serde_json::json!("important"));
}

#[test]
fn iss120_reader_round_trips_canonical_variants() {
    // Exercise the reader-side decoder for each canonical EntityKind.
    let mut conn = fresh_conn();
    let variants = vec![
        EntityKind::Person,
        EntityKind::Organization,
        EntityKind::Place,
        EntityKind::Concept,
        EntityKind::Event,
        EntityKind::Artifact,
        EntityKind::Topic,
    ];
    for (i, k) in variants.iter().enumerate() {
        let name = format!("e-{i}");
        let e = sample_entity(&name, k.clone(), serde_json::json!({"i": i}));
        let id = e.id;
        insert(&mut conn, &e);

        let (attrs, node_kind) = read_node_attrs_and_kind(&conn, id);
        let (decoded_kind, cleaned_attrs) =
            extract_legacy_entity_kind(&attrs, &node_kind).expect("decode");
        assert_eq!(&decoded_kind, k, "round-trip kind for {k:?}");

        let cleaned: serde_json::Value = serde_json::from_str(&cleaned_attrs).unwrap();
        assert!(
            cleaned.get("_legacy_kind").is_none(),
            "_legacy_kind must be stripped from returned metadata"
        );
        assert_eq!(cleaned["i"], serde_json::json!(i));
    }
}

#[test]
fn iss120_reader_round_trips_other_variant() {
    let mut conn = fresh_conn();
    let kind = EntityKind::other("custom");
    let e = sample_entity("X", kind.clone(), serde_json::json!({}));
    let id = e.id;
    insert(&mut conn, &e);

    let (attrs, node_kind) = read_node_attrs_and_kind(&conn, id);
    let (decoded_kind, _) = extract_legacy_entity_kind(&attrs, &node_kind).expect("decode");
    assert_eq!(decoded_kind, kind);
}

#[test]
fn iss120_reader_fallback_when_legacy_kind_absent() {
    // Simulate a row written by some hypothetical pre-ISS-120 path
    // that didn't stamp _legacy_kind. Reader should fall back on
    // node_kind:
    //   - node_kind='topic'  → EntityKind::Topic
    //   - node_kind='entity' → EntityKind::Other("entity") (loud signal)
    let attrs = r#"{"score":0.5}"#;
    let (k_topic, _) = extract_legacy_entity_kind(attrs, "topic").unwrap();
    assert_eq!(k_topic, EntityKind::Topic);

    let (k_entity, cleaned) = extract_legacy_entity_kind(attrs, "entity").unwrap();
    assert_eq!(k_entity, EntityKind::other("entity"));
    // Cleaned attributes should still be valid object with the user data.
    let v: serde_json::Value = serde_json::from_str(&cleaned).unwrap();
    assert_eq!(v["score"], serde_json::json!(0.5));
}

#[test]
fn iss120_reader_errors_on_corrupt_legacy_kind() {
    // If _legacy_kind is present but undecodable, the decoder must
    // fail loud rather than silently return a wrong variant.
    let attrs = r#"{"_legacy_kind": 42}"#;
    let result = extract_legacy_entity_kind(attrs, "entity");
    assert!(
        result.is_err(),
        "corrupt _legacy_kind must surface as GraphError, got {result:?}"
    );
}

#[test]
fn iss120_reader_t21_backfill_compat_canonical_entity_type() {
    // T21 Phase C backfill stamps `attributes.entity_type` as a flat
    // string from the legacy `entities.entity_type` column. When
    // `_legacy_kind` is absent but `entity_type` is present, the
    // decoder must wrap the canonical label back into the typed enum
    // without dropping the `entity_type` key (legacy consumers may
    // still read it).
    for (label, expected) in &[
        ("person", EntityKind::Person),
        ("organization", EntityKind::Organization),
        ("place", EntityKind::Place),
        ("concept", EntityKind::Concept),
        ("event", EntityKind::Event),
        ("artifact", EntityKind::Artifact),
        ("topic", EntityKind::Topic),
    ] {
        let attrs = format!(r#"{{"entity_type":"{label}","score":0.7}}"#);
        let (kind, cleaned) = extract_legacy_entity_kind(&attrs, "entity").unwrap();
        assert_eq!(&kind, expected, "wrap canonical {label}");
        let v: serde_json::Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(
            v["entity_type"],
            serde_json::json!(label),
            "entity_type key preserved for legacy consumers"
        );
        assert_eq!(v["score"], serde_json::json!(0.7));
    }
}

#[test]
fn iss120_reader_t21_backfill_compat_non_canonical_entity_type() {
    // T21 backfill rows with a non-canonical entity_type (e.g. some
    // historical value not in the EntityKind set) should wrap as
    // EntityKind::Other(s) with NFKC normalization.
    let attrs = r#"{"entity_type":"Robot"}"#;
    let (kind, _) = extract_legacy_entity_kind(attrs, "entity").unwrap();
    assert_eq!(kind, EntityKind::other("Robot"));
}
