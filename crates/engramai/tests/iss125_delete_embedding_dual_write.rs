//! ISS-125 contract tests: delete_embedding dual-DELETEs to node_embeddings.
//!
//! `Storage::delete_embedding(memory_id, model)` was only DELETing
//! from the legacy `memory_embeddings` table while
//! `delete_all_embeddings` and `store_embedding` both already
//! dual-handle the unified `node_embeddings` table. This asymmetry
//! left orphan vectors in `node_embeddings` that the unified read
//! path would still return.
//!
//! These tests pin the dual-DELETE contract.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn rec(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: "embedding host".into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss125-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn count_legacy(s: &Storage, mem: &str, model: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = ?1 AND model = ?2",
            params![mem, model],
            |row| row.get(0),
        )
        .unwrap()
}

fn count_unified(s: &Storage, mem: &str, model: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id = ?1 AND model = ?2",
            params![mem, model],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn iss125_delete_embedding_clears_both_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("m-1"), "default").expect("add");

    let vec_a = vec![0.1f32; 4];
    s.store_embedding("m-1", &vec_a, "ollama/nomic-embed-text", 4)
        .expect("store_embedding");

    // Both substrates have the row.
    assert_eq!(count_legacy(&s, "m-1", "ollama/nomic-embed-text"), 1);
    assert_eq!(count_unified(&s, "m-1", "ollama/nomic-embed-text"), 1);

    // Delete it.
    s.delete_embedding("m-1", "ollama/nomic-embed-text")
        .expect("delete");

    // Both substrates are empty.
    assert_eq!(
        count_legacy(&s, "m-1", "ollama/nomic-embed-text"),
        0,
        "ISS-125: memory_embeddings must be cleared"
    );
    assert_eq!(
        count_unified(&s, "m-1", "ollama/nomic-embed-text"),
        0,
        "ISS-125: node_embeddings must be cleared (was an orphan before fix)"
    );
}

#[test]
fn iss125_delete_embedding_only_removes_matching_model() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("m-2"), "default").expect("add");

    let v = vec![0.1f32; 4];
    s.store_embedding("m-2", &v, "ollama/model-a", 4)
        .expect("store a");
    s.store_embedding("m-2", &v, "ollama/model-b", 4)
        .expect("store b");

    s.delete_embedding("m-2", "ollama/model-a")
        .expect("delete a");

    // model-a gone on both sides.
    assert_eq!(count_legacy(&s, "m-2", "ollama/model-a"), 0);
    assert_eq!(count_unified(&s, "m-2", "ollama/model-a"), 0);

    // model-b survives on both sides.
    assert_eq!(
        count_legacy(&s, "m-2", "ollama/model-b"),
        1,
        "ISS-125: model-b legacy row must survive deletion of model-a"
    );
    assert_eq!(
        count_unified(&s, "m-2", "ollama/model-b"),
        1,
        "ISS-125: model-b unified row must survive deletion of model-a"
    );
}

#[test]
fn iss125_delete_embedding_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("m-3"), "default").expect("add");

    let v = vec![0.1f32; 4];
    s.store_embedding("m-3", &v, "ollama/x", 4).expect("store");

    s.delete_embedding("m-3", "ollama/x").expect("delete 1");
    s.delete_embedding("m-3", "ollama/x")
        .expect("delete 2 (idempotent)");

    assert_eq!(count_legacy(&s, "m-3", "ollama/x"), 0);
    assert_eq!(count_unified(&s, "m-3", "ollama/x"), 0);
}

#[test]
fn iss125_delete_embedding_normalizes_model() {
    // store_embedding normalizes "nomic-embed-text" → "ollama/nomic-embed-text".
    // delete_embedding must apply the same normalization or the dual-DELETE
    // won't match the stored rows.
    let dir = tempdir().unwrap();
    let path = dir.path().join("d.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("m-4"), "default").expect("add");

    let v = vec![0.1f32; 4];
    // store with un-prefixed model (will be normalized to ollama/...)
    s.store_embedding("m-4", &v, "nomic-embed-text", 4)
        .expect("store");

    // Rows actually land under the normalized model id.
    assert_eq!(count_legacy(&s, "m-4", "ollama/nomic-embed-text"), 1);
    assert_eq!(count_unified(&s, "m-4", "ollama/nomic-embed-text"), 1);

    // Delete using the un-prefixed form — must normalize internally.
    s.delete_embedding("m-4", "nomic-embed-text")
        .expect("delete");

    assert_eq!(
        count_legacy(&s, "m-4", "ollama/nomic-embed-text"),
        0,
        "ISS-125: delete must normalize model id"
    );
    assert_eq!(
        count_unified(&s, "m-4", "ollama/nomic-embed-text"),
        0,
        "ISS-125: unified delete must normalize model id"
    );
}
