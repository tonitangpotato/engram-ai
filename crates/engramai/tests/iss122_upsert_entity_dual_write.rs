// ISS-122: Storage::upsert_entity must dual-write to the unified `nodes`
// table so post-cutover entity writes show up to unified readers.
//
// The legacy path before this fix wrote only to `entities`. T13 covered
// the v0.3 resolution-pipeline `Entity` writes (graph_entities → nodes)
// but the v0.2 `Storage::upsert_entity` was missed.
//
// Acceptance:
// 1. Fresh insert writes 1 legacy row + 1 unified row, same id.
// 2. Idempotent re-upsert with identical args writes no new rows.
// 3. Metadata merge on re-upsert with a new metadata key writes it
//    to both substrates with existing-wins polarity on the unified
//    side (entity_type from the column always wins).
// 4. The unified row carries `node_kind='entity'`, `content=name`,
//    `attributes={"entity_type": "...", ...metadata}` matching the
//    T21 backfill projection (so a future re-run of T21 over this
//    row is a no-op).

use engramai::storage::Storage;
use rusqlite::{params, OptionalExtension};
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, Storage) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("engram.db");
    let s = Storage::new(&path).expect("open");
    (dir, s)
}

fn read_nodes_row(s: &Storage, id: &str) -> Option<(String, String, String, String, f64, f64)> {
    s.conn()
        .query_row(
            "SELECT node_kind, content, namespace, attributes, created_at, updated_at \
             FROM nodes WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .expect("query")
}

fn read_legacy_row(s: &Storage, id: &str) -> Option<(String, String, String, Option<String>)> {
    s.conn()
        .query_row(
            "SELECT name, entity_type, namespace, metadata FROM entities WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .expect("query")
}

#[test]
fn iss122_fresh_upsert_writes_both_substrates() {
    let (_d, s) = fresh();
    let eid = s
        .upsert_entity("Alice", "person", "default", None)
        .expect("upsert");

    // Legacy
    let legacy = read_legacy_row(&s, &eid).expect("legacy row");
    assert_eq!(legacy.0, "Alice");
    assert_eq!(legacy.1, "person");
    assert_eq!(legacy.2, "default");
    assert_eq!(legacy.3, None);

    // Unified
    let (kind, content, ns, attrs, _ct, _ut) = read_nodes_row(&s, &eid).expect("nodes row");
    assert_eq!(kind, "entity");
    assert_eq!(content, "Alice");
    assert_eq!(ns, "default");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["entity_type"], serde_json::json!("person"));
}

#[test]
fn iss122_idempotent_re_upsert_no_duplicate_rows() {
    let (_d, s) = fresh();
    let id1 = s.upsert_entity("Bob", "person", "default", None).unwrap();
    let id2 = s.upsert_entity("Bob", "person", "default", None).unwrap();
    assert_eq!(id1, id2, "deterministic id");

    let cnt_legacy: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM entities WHERE id=?",
            params![id1],
            |r| r.get(0),
        )
        .unwrap();
    let cnt_node: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM nodes WHERE id=?", params![id1], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cnt_legacy, 1, "no legacy duplicate");
    assert_eq!(cnt_node, 1, "no nodes duplicate");
}

#[test]
fn iss122_metadata_merge_existing_wins_on_unified() {
    let (_d, s) = fresh();
    // First insert seeds entity_type=person, no metadata.
    let id = s.upsert_entity("Carol", "person", "default", None).unwrap();

    // Re-upsert with metadata that tries to redefine entity_type.
    // The column value MUST win on the unified side (existing-wins
    // matches T21 contract).
    let md = r#"{"entity_type":"NOT_PERSON","extra":"x"}"#;
    let _ = s
        .upsert_entity("Carol", "person", "default", Some(md))
        .unwrap();

    let (_, _, _, attrs, _, _) = read_nodes_row(&s, &id).expect("nodes row");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        v["entity_type"],
        serde_json::json!("person"),
        "column-derived entity_type wins on collision"
    );
    assert_eq!(v["extra"], serde_json::json!("x"), "new key was added");
}

#[test]
fn iss122_first_insert_metadata_seeds_unified_attributes() {
    // If metadata is supplied on the FIRST insert, those keys must
    // land in the unified row.
    let (_d, s) = fresh();
    let md = r#"{"score":0.9,"tag":"important"}"#;
    let id = s.upsert_entity("Dave", "concept", "ns1", Some(md)).unwrap();

    let (kind, _, ns, attrs, _, _) = read_nodes_row(&s, &id).expect("nodes row");
    assert_eq!(kind, "entity");
    assert_eq!(ns, "ns1");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["entity_type"], serde_json::json!("concept"));
    assert_eq!(v["score"], serde_json::json!(0.9));
    assert_eq!(v["tag"], serde_json::json!("important"));
}

#[test]
fn iss122_malformed_metadata_dropped_from_unified_kept_on_legacy() {
    // Malformed metadata (not a JSON object) must NOT crash. Legacy
    // column keeps the literal string (backward compat); unified
    // projection just omits the bad keys (T21 parity).
    let (_d, s) = fresh();
    let id = s
        .upsert_entity("Eve", "person", "default", Some("not json"))
        .unwrap();

    let legacy = read_legacy_row(&s, &id).expect("legacy");
    assert_eq!(legacy.3, Some("not json".to_string()));

    let (_, _, _, attrs, _, _) = read_nodes_row(&s, &id).expect("nodes");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    // entity_type still present (column-seeded), no other keys.
    assert_eq!(v["entity_type"], serde_json::json!("person"));
    assert_eq!(v.as_object().unwrap().len(), 1, "no other keys leaked");
}

#[test]
fn iss122_namespace_round_trip() {
    let (_d, s) = fresh();
    let id = s.upsert_entity("Faye", "person", "tenant_a", None).unwrap();
    let (_, _, ns, _, _, _) = read_nodes_row(&s, &id).expect("nodes");
    assert_eq!(ns, "tenant_a");
}
