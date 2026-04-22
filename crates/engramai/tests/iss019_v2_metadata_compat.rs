//! ISS-019 Step 7a — read-path backward compatibility for v1 metadata.
//!
//! Constructs a v1 metadata JSON row **by hand** (simulating an old
//! DB entry written before the v2 layout existed), inserts it directly
//! into the memories table, and verifies the read path parses it
//! correctly without going through the write path.

use engramai::storage::Storage;
use rusqlite::params;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn manual_v1_metadata_parses_correctly() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("test.db");
    let storage = Storage::new(db.to_str().unwrap()).unwrap();

    // Hand-built v1 blob — mimics what pre-Step-7a engram wrote.
    let v1_metadata = json!({
        "dimensions": {
            "participants": "alice, bob",
            "temporal": "2026-04-22",
            "causation": "kickoff meeting",
            "valence": 0.4,
            "domain": "coding",
            "confidence": "likely",
            "tags": ["standup", "planning"]
        },
        "type_weights": {
            "episodic": 1.2, "factual": 1.0, "procedural": 0.8,
            "semantic": 1.0, "emotional": 0.5
        },
        "merge_count": 2,
        "merge_history": [
            {"ts": 1700000000, "sim": 0.93, "content_updated": false,
             "prev_content_len": 50, "new_content_len": 48}
        ]
    });

    // Direct INSERT bypasses write path — this is the whole point.
    let id = "v1_legacy_row_001";
    let content = "Alice and Bob had a kickoff meeting";
    storage
        .conn()
        .execute(
            "INSERT INTO memories (id, content, importance, memory_type, layer, metadata, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                content,
                0.6_f64,
                "episodic",
                "working",
                serde_json::to_string(&v1_metadata).unwrap(),
                1700000000_f64,
            ],
        )
        .unwrap();

    // Read the stored metadata string directly; parse it and feed
    // the JSON + content to the dual-path reader.
    let stored_meta: String = storage
        .conn()
        .query_row(
            "SELECT metadata FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .unwrap();
    let meta_val: serde_json::Value = serde_json::from_str(&stored_meta).unwrap();

    let dims = engramai::dimensions::Dimensions::from_stored_metadata(
        &meta_val, content,
    )
    .unwrap();

    assert_eq!(dims.participants.as_deref(), Some("alice, bob"));
    assert_eq!(dims.causation.as_deref(), Some("kickoff meeting"));
    assert!((dims.valence.get() - 0.4).abs() < 1e-9);
    assert_eq!(dims.domain, engramai::dimensions::Domain::Coding);
    assert_eq!(dims.tags.len(), 2);
    assert!(dims.tags.contains("standup"));
}

#[test]
fn v1_and_v2_equivalent_layouts_yield_identical_dimensions() {
    let v1 = json!({
        "dimensions": {
            "participants": "alice",
            "valence": 0.3,
            "domain": "research",
            "confidence": "confident",
            "tags": ["idea"]
        },
        "type_weights": {"episodic": 1.0, "factual": 1.0, "procedural": 1.0,
                         "semantic": 1.0, "emotional": 1.0}
    });
    let v2 = json!({
        "engram": {
            "version": 2,
            "dimensions": {
                "core_fact": "note",
                "participants": "alice",
                "valence": 0.3,
                "domain": "research",
                "confidence": "confident",
                "tags": ["idea"],
                "type_weights": {"episodic": 1.0, "factual": 1.0,
                                 "procedural": 1.0, "semantic": 1.0,
                                 "emotional": 1.0}
            },
            "merge_count": 0,
            "merge_history": []
        },
        "user": {}
    });

    let d1 =
        engramai::dimensions::Dimensions::from_stored_metadata(&v1, "note").unwrap();
    let d2 =
        engramai::dimensions::Dimensions::from_stored_metadata(&v2, "note").unwrap();

    assert_eq!(d1.participants, d2.participants);
    assert!((d1.valence.get() - d2.valence.get()).abs() < 1e-9);
    assert_eq!(d1.domain, d2.domain);
    assert_eq!(d1.confidence, d2.confidence);
    assert_eq!(d1.tags, d2.tags);
}
