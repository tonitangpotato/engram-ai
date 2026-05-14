// ISS-119: Phase B dual-write must round-trip `contradicts` and
// `contradicted_by` from MemoryRecord through `nodes.attributes` so the
// unified read path can reconstruct them.
//
// Companion writer-side fix: `merge_legacy_memory_attributes` (storage.rs)
// stamps both fields under reserved keys when the source is non-empty.
//
// Companion reader-side decoder: `row_to_record_from_node_impl` extracts
// them back out and strips the reserved keys from the returned
// `record.metadata` so the field looks identical to a legacy read.

use chrono::Utc;
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, Storage) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("engram.db");
    let s = Storage::new(&path).expect("open");
    (dir, s)
}

fn ingest(s: &mut Storage, id: &str, contradicts: Option<&str>, contradicted_by: Option<&str>,
          metadata: Option<serde_json::Value>) {
    let rec = MemoryRecord {
        id: id.into(),
        content: format!("c {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: Utc::now(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: String::new(),
        contradicts: contradicts.map(|s| s.to_string()),
        contradicted_by: contradicted_by.map(|s| s.to_string()),
        superseded_by: None,
        metadata,
    };
    s.add(&rec, "default").expect("add");
}

fn read_attributes_json(s: &Storage, id: &str) -> String {
    s.conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = ? AND node_kind = 'memory'",
            params![id],
            |r| r.get::<_, String>(0),
        )
        .expect("attributes")
}

#[test]
fn iss119_contradicts_stamped_into_nodes_attributes() {
    let (_d, mut s) = fresh();
    ingest(&mut s, "m1", Some("m_other"), None, None);

    let attrs = read_attributes_json(&s, "m1");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["_legacy_contradicts"], serde_json::json!("m_other"));
    assert!(v.get("_legacy_contradicted_by").is_none(),
            "absent contradicted_by stays absent");
}

#[test]
fn iss119_contradicted_by_stamped_into_nodes_attributes() {
    let (_d, mut s) = fresh();
    ingest(&mut s, "m1", None, Some("m_other"), None);

    let attrs = read_attributes_json(&s, "m1");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v["_legacy_contradicted_by"], serde_json::json!("m_other"));
}

#[test]
fn iss119_both_legacy_keys_stamped() {
    let (_d, mut s) = fresh();
    ingest(&mut s, "m1", Some("ca"), Some("cb"), None);
    let v: serde_json::Value =
        serde_json::from_str(&read_attributes_json(&s, "m1")).unwrap();
    assert_eq!(v["_legacy_contradicts"],     serde_json::json!("ca"));
    assert_eq!(v["_legacy_contradicted_by"], serde_json::json!("cb"));
}

#[test]
fn iss119_empty_strings_not_stamped() {
    // Legacy `memories.contradicts` defaults to '' for "no contradiction".
    // The dual-write should NOT stamp empty-string values into attributes —
    // would bloat every row's JSON.
    let (_d, mut s) = fresh();
    ingest(&mut s, "m1", Some(""), Some(""), None);

    let v: serde_json::Value =
        serde_json::from_str(&read_attributes_json(&s, "m1")).unwrap();
    assert!(v.get("_legacy_contradicts").is_none());
    assert!(v.get("_legacy_contradicted_by").is_none());
}

#[test]
fn iss119_user_metadata_preserved_alongside_legacy_keys() {
    // ingest with both user-supplied metadata + legacy fields.
    // After merge, attributes JSON should have the user keys AND the
    // reserved legacy keys side-by-side.
    let (_d, mut s) = fresh();
    let user_md = serde_json::json!({"tag": "important", "score": 0.9});
    ingest(&mut s, "m1", Some("c"), None, Some(user_md));

    let v: serde_json::Value =
        serde_json::from_str(&read_attributes_json(&s, "m1")).unwrap();
    assert_eq!(v["_legacy_contradicts"], serde_json::json!("c"));
    assert_eq!(v["tag"], serde_json::json!("important"));
    assert_eq!(v["score"], serde_json::json!(0.9));
}

#[test]
fn iss119_neither_field_keeps_metadata_json_unchanged() {
    // When BOTH legacy fields are None or empty, the dual-write should
    // NOT touch the attributes JSON. This guards against accidental
    // re-serialization (which would normalize whitespace/key-order).
    let (_d, mut s) = fresh();
    let user_md = serde_json::json!({"k": "v"});
    ingest(&mut s, "m1", None, None, Some(user_md.clone()));

    let attrs = read_attributes_json(&s, "m1");
    let v: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(v, user_md, "attributes round-trip unchanged");
    assert!(v.get("_legacy_contradicts").is_none());
    assert!(v.get("_legacy_contradicted_by").is_none());
}
