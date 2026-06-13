//! ISS-201 lever-(b): storage round-trip for entity-set synthesis.
//!
//! Verifies `gather_entity_set_candidates` reads the `edges` table correctly
//! (degree + distinct-object gates, object-literal decoding) and that
//! `upsert_set_memory` is idempotent on the deterministic id.

use engramai::storage::Storage;
use rusqlite::params;

/// Build a tiny graph: subject entity `Audrey` with N object entities linked
/// by `related_to` structural edges. Returns the Storage.
fn build_graph(unified: bool) -> Storage {
    let storage = Storage::with_unified_substrate(":memory:", unified).unwrap();
    let conn = storage.conn();
    let now = engramai::storage::now_f64();

    // Subject entity node.
    conn.execute(
        "INSERT INTO nodes (id, node_kind, namespace, layer, memory_type, content, summary, \
         attributes, created_at, updated_at, working_strength, core_strength, importance, \
         consolidation_count, pinned, source, fts_rowid) \
         VALUES ('ent-audrey','entity','default','core','factual','Audrey','','{}', ?1, ?1, \
         0.5,0.5,0.5,0,0,'test', NULL)",
        params![now],
    )
    .unwrap();

    // Provenance memory node referenced by every structural edge's
    // `source_memory_id`. `edges.source_memory_id REFERENCES nodes(id)`
    // (ISS-198 FK re-point), so this row must exist before any edge is
    // inserted or the inserts FK-787.
    conn.execute(
        "INSERT INTO nodes (id, node_kind, namespace, layer, memory_type, content, summary, \
         attributes, created_at, updated_at, working_strength, core_strength, importance, \
         consolidation_count, pinned, source, fts_rowid) \
         VALUES ('mem-1','memory','default','working','episodic','Audrey facts','','{}', ?1, ?1, \
         0.5,0.5,0.5,0,0,'test', NULL)",
        params![now],
    )
    .unwrap();

    // Object entity nodes + structural edges. Pets + hobbies + noise.
    let objs = [
        ("ent-pepper", "Pepper"),
        ("ent-precious", "Precious"),
        ("ent-panda", "Panda"),
        ("ent-hiking", "hiking"),
        ("ent-birdw", "bird-watching"),
        ("ent-photo", "photo"),
        ("ent-gf", "girlfriend"),
    ];
    for (i, (oid, name)) in objs.iter().enumerate() {
        conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, layer, memory_type, content, summary, \
             attributes, created_at, updated_at, working_strength, core_strength, importance, \
             consolidation_count, pinned, source, fts_rowid) \
             VALUES (?1,'entity','default','core','factual',?2,'','{}', ?3, ?3, \
             0.5,0.5,0.5,0,0,'test', NULL)",
            params![oid, name, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (id, source_id, target_id, target_literal, edge_kind, \
             predicate_kind, predicate, weight, confidence, namespace, created_at, updated_at, \
             recorded_at, source_memory_id) \
             VALUES (?1, 'ent-audrey', ?2, NULL, 'structural', 'canonical', 'related_to', \
             ?3, 1.0, 'default', ?4, ?4, ?4, 'mem-1')",
            params![format!("edge-{i}"), oid, (objs.len() - i) as f64, now],
        )
        .unwrap();
    }
    storage
}

#[test]
fn gather_finds_high_degree_subject_with_all_objects() {
    let storage = build_graph(true);
    let cands = storage
        .gather_entity_set_candidates(Some("default"), 6, 3, 60)
        .unwrap();
    assert_eq!(cands.len(), 1, "Audrey should be the single candidate");
    let c = &cands[0];
    assert_eq!(c.entity_name, "Audrey");
    assert_eq!(c.objects.len(), 7, "all 7 object edges gathered");
    let names: Vec<&str> = c.objects.iter().map(|o| o.object.as_str()).collect();
    assert!(names.contains(&"Pepper"));
    assert!(names.contains(&"bird-watching"));
    assert!(names.contains(&"girlfriend")); // noise included — LLM discards it later
    assert!(c.objects.iter().all(|o| o.predicate == "related_to"));
    assert_eq!(c.objects[0].source_memory_id.as_deref(), Some("mem-1"));
}

#[test]
fn degree_gate_excludes_low_degree_entities() {
    let storage = build_graph(true);
    // min_degree 8 > Audrey's 7 → no candidates.
    let cands = storage
        .gather_entity_set_candidates(Some("default"), 8, 3, 60)
        .unwrap();
    assert!(cands.is_empty());
}

#[test]
fn max_objects_caps_prompt_size() {
    let storage = build_graph(true);
    let cands = storage
        .gather_entity_set_candidates(Some("default"), 6, 3, 4)
        .unwrap();
    assert_eq!(cands[0].objects.len(), 4, "capped to top-4 by weight");
    // Highest weight (edge-0 → Pepper, weight 7) must survive the cap.
    assert_eq!(cands[0].objects[0].object, "Pepper");
}

#[test]
fn upsert_set_memory_is_idempotent() {
    let storage = build_graph(true);
    let id = "eset-deadbeef";
    let meta = r#"{"is_entity_set":true,"attribute":"pets"}"#;

    assert!(!storage.memory_exists(id).unwrap());
    storage
        .upsert_set_memory(id, "Audrey's pets: Pepper, Precious", 0.85, Some(meta), Some("default"))
        .unwrap();
    assert!(storage.memory_exists(id).unwrap());

    // Second upsert (e.g. set grew) → still one row, content refreshed.
    storage
        .upsert_set_memory(
            id,
            "Audrey's pets: Pepper, Precious, Panda",
            0.85,
            Some(meta),
            Some("default"),
        )
        .unwrap();

    let conn = storage.conn();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM nodes WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1, "idempotent — no duplicate row");
    let content: String = conn
        .query_row("SELECT content FROM nodes WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(content, "Audrey's pets: Pepper, Precious, Panda");
    let kind: String = conn
        .query_row("SELECT node_kind FROM nodes WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kind, "memory", "set-memory is a normal memory, not an insight");
}

#[test]
fn set_memory_is_fts_searchable() {
    let storage = build_graph(true);
    storage
        .upsert_set_memory(
            "eset-abc",
            "Audrey's pets: Pepper, Precious, Panda",
            0.85,
            None,
            Some("default"),
        )
        .unwrap();
    let conn = storage.conn();
    // The memories_fts index must contain the set-memory content.
    let hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'pets'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(hits >= 1, "set-memory must be FTS-searchable for retrieval");
}
