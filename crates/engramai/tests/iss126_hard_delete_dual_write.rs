//! ISS-126 contract tests: Storage::delete (hard) dual-DELETEs nodes.
//!
//! Hard delete previously only purged legacy `memories` + `memories_fts`,
//! leaving the unified `nodes` row orphaned. Under unified_substrate,
//! deleted memories would still appear in search results.
//!
//! The fix cascades through edges → node_embeddings → nodes in that
//! order because edges.source_id/target_id are `ON DELETE RESTRICT`.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
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
        source: "iss126-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn nodes_count(s: &Storage, id: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn iss126_delete_clears_nodes_row() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();

    s.add(&rec("m-1", "hello world"), "default").expect("add");

    // Precondition: dual-write put the row on both substrates.
    assert_eq!(nodes_count(&s, "m-1"), 1, "T12 dual-write precondition");

    s.delete("m-1").expect("hard delete");

    assert_eq!(
        nodes_count(&s, "m-1"),
        0,
        "ISS-126: hard delete must remove the nodes row"
    );

    // Legacy side also clean.
    let legacy_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE id = ?1",
            params!["m-1"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 0);
}

#[test]
fn iss126_delete_clears_unified_fts_search() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b.db");
    let mut s = Storage::new(&path).unwrap();

    s.add(&rec("m-2", "uniqueterm content"), "default")
        .expect("add");
    s.delete("m-2").expect("hard delete");

    let unified = Storage::with_unified_substrate(&path, true).unwrap();
    let hits: Vec<String> = unified
        .search_fts("uniqueterm", 10)
        .expect("search")
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert!(
        hits.is_empty(),
        "ISS-126: hard-deleted memory must not appear in unified FTS, got {:?}",
        hits
    );
}

#[test]
fn iss126_delete_cascades_through_edges() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c.db");
    let mut s = Storage::new(&path).unwrap();

    // Seed a memory and an entity, then link them — link_memory_entity
    // creates an edge in unified substrate (ISS-123 fix). Hard
    // deleting the memory must cascade through that edge or the
    // ON DELETE RESTRICT FK will fail.
    s.add(&rec("m-3", "linked memory"), "default").expect("add");
    let ent_id = s
        .upsert_entity("Alice", "person", "default", Some("{}"))
        .expect("upsert entity");

    s.link_memory_entity("m-3", &ent_id, "mention")
        .expect("link");

    // Precondition: an edge exists pointing at m-3.
    let edge_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE source_id = ?1 OR target_id = ?1",
            params!["m-3"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(edge_count >= 1, "ISS-126 precondition: edge must exist");

    // Hard delete should succeed (will fail without cascade due to RESTRICT FK).
    s.delete("m-3").expect("hard delete with edges");

    // Nodes row gone.
    assert_eq!(nodes_count(&s, "m-3"), 0);

    // No dangling edges pointing at m-3.
    let after_edge_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE source_id = ?1 OR target_id = ?1",
            params!["m-3"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after_edge_count, 0,
        "ISS-126: hard delete must cascade through edges referencing the deleted memory"
    );

    // Entity should still exist (independent node).
    let ent_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = ?1",
            params![&ent_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ent_count, 1,
        "ISS-126: deleting a memory must NOT take down linked entities"
    );
}

#[test]
fn iss126_delete_clears_node_embeddings() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("d.db");
    let mut s = Storage::new(&path).unwrap();

    s.add(&rec("m-4", "has embedding"), "default").expect("add");
    s.store_embedding("m-4", &vec![0.1f32; 4], "ollama/model-x", 4)
        .expect("store embedding");

    // Precondition.
    let emb_count: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id = ?1",
            params!["m-4"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(emb_count, 1);

    s.delete("m-4").expect("hard delete");

    let after: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id = ?1",
            params!["m-4"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, 0,
        "ISS-126: hard delete must purge node_embeddings rows for the deleted memory"
    );
}

#[test]
fn iss126_delete_on_missing_id_is_noop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("e.db");
    let mut s = Storage::new(&path).unwrap();

    // Deleting a non-existent id must succeed cleanly.
    s.delete("never-existed").expect("delete missing id");
    // Repeated delete (after seed+delete) also clean.
    s.add(&rec("m-5", "tmp"), "default").expect("add");
    s.delete("m-5").expect("first delete");
    s.delete("m-5").expect("second delete (idempotent)");
}
