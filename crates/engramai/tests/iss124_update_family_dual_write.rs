//! ISS-124 contract tests: UPDATE family dual-writes to nodes.
//!
//! Three memory-mutating writers must mirror their `UPDATE memories
//! SET ...` into the parallel `nodes` row when one exists:
//!
//!   - `Storage::update` (full record overwrite)
//!   - `Storage::update_content` (content + metadata only)
//!   - `Storage::update_importance` (single-column synthesis bump)
//!
//! Each test:
//!   1. Seeds via `Storage::add` (T12 dual-writes to both substrates).
//!   2. Mutates via the writer under test.
//!   3. Asserts the `nodes` row reflects the mutation (column-level
//!      checks, plus `nodes_fts` for content changes).
//!
//! See `.gid/issues/ISS-124/issue.md` for the full failure analysis.

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
        source: "iss124-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

#[test]
fn iss124_update_dual_writes_to_nodes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();

    let mut r = rec("m-1", "original content");
    r.importance = 0.3;
    r.memory_type = MemoryType::Factual;
    s.add(&r, "default").expect("add");

    // Mutate everything that update() touches.
    let mut r2 = r.clone();
    r2.content = "new content".into();
    r2.importance = 0.9;
    r2.memory_type = MemoryType::Episodic;
    r2.layer = MemoryLayer::Working;
    r2.working_strength = 0.7;
    r2.core_strength = 0.2;
    r2.pinned = true;
    r2.consolidation_count = 5;
    r2.source = "iss124-mutated".into();
    s.update(&r2).expect("update");

    // Verify nodes row reflects the mutation.
    let (content, mtype, layer, importance, working, core, pinned, consol, source): (
        String, String, String, f64, f64, f64, i64, i64, String,
    ) = s.conn().query_row(
        "SELECT content, memory_type, layer, importance,
                working_strength, core_strength, pinned,
                consolidation_count, source
         FROM nodes WHERE id = ?1 AND node_kind = 'memory'",
        params!["m-1"],
        |row| Ok((
            row.get(0)?, row.get(1)?, row.get(2)?,
            row.get(3)?, row.get(4)?, row.get(5)?,
            row.get(6)?, row.get(7)?, row.get(8)?,
        )),
    ).expect("nodes row missing");

    assert_eq!(content, "new content");
    assert_eq!(mtype, "episodic");
    assert_eq!(layer, "working");
    assert!((importance - 0.9).abs() < 1e-9);
    assert!((working - 0.7).abs() < 1e-9);
    assert!((core - 0.2).abs() < 1e-9);
    assert_eq!(pinned, 1);
    assert_eq!(consol, 5);
    assert_eq!(source, "iss124-mutated");
}

#[test]
fn iss124_update_content_dual_writes_to_nodes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b.db");
    let mut s = Storage::new(&path).unwrap();

    let r = rec("m-2", "before");
    s.add(&r, "default").expect("add");

    s.update_content("m-2", "after", None).expect("update_content");

    // (1) Column on nodes reflects the new content.
    let content: String = s.conn().query_row(
        "SELECT content FROM nodes WHERE id = ?1 AND node_kind = 'memory'",
        params!["m-2"],
        |row| row.get(0),
    ).expect("nodes row missing");
    assert_eq!(content, "after");

    // (2) FTS on the unified substrate finds the new content, not the old.
    let unified = Storage::with_unified_substrate(&path, true).unwrap();
    let hits: Vec<String> = unified.search_fts("after", 10)
        .expect("unified search")
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(hits, vec!["m-2".to_string()],
        "ISS-124: unified search_fts must find updated content");

    let stale: Vec<String> = unified.search_fts("before", 10)
        .expect("unified search before")
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert!(stale.is_empty(),
        "ISS-124: unified search_fts must NOT find pre-update content");
}

#[test]
fn iss124_update_importance_dual_writes_to_nodes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c.db");
    let mut s = Storage::new(&path).unwrap();

    let mut r = rec("m-3", "fixed content");
    r.importance = 0.3;
    s.add(&r, "default").expect("add");

    s.update_importance("m-3", 0.9).expect("update_importance");

    let importance: f64 = s.conn().query_row(
        "SELECT importance FROM nodes WHERE id = ?1 AND node_kind = 'memory'",
        params!["m-3"],
        |row| row.get(0),
    ).expect("nodes row missing");

    assert!((importance - 0.9).abs() < 1e-9,
        "ISS-124: update_importance must mirror onto nodes (got {})", importance);
}

#[test]
fn iss124_update_idempotent_no_divergence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("d.db");
    let mut s = Storage::new(&path).unwrap();

    let r = rec("m-4", "idempotency");
    s.add(&r, "default").expect("add");

    let mut r2 = r.clone();
    r2.importance = 0.7;

    // Re-call update twice with the same shape.
    s.update(&r2).expect("update 1");
    s.update(&r2).expect("update 2");

    let importance: f64 = s.conn().query_row(
        "SELECT importance FROM nodes WHERE id = ?1",
        params!["m-4"],
        |row| row.get(0),
    ).expect("nodes row missing");
    assert!((importance - 0.7).abs() < 1e-9);
}

#[test]
fn iss124_update_on_missing_node_is_silent_noop() {
    // Simulates the pre-T26c backfill state where a legacy
    // `memories` row exists but the `nodes` row hasn't been
    // backfilled. The update must NOT error — it should just be a
    // zero-rows-affected no-op on the nodes side. Backfill closes
    // the gap later.
    let dir = tempdir().unwrap();
    let path = dir.path().join("e.db");
    let mut s = Storage::new(&path).unwrap();

    // Seed via add (dual-writes to nodes).
    let r = rec("m-5", "exists");
    s.add(&r, "default").expect("add");

    // Wipe the nodes row to simulate the legacy-only state.
    s.conn().execute(
        "DELETE FROM nodes WHERE id = ?1",
        params!["m-5"],
    ).expect("wipe nodes");

    // update / update_content / update_importance must all succeed
    // even though the nodes mirror has no row to update.
    let mut r2 = r.clone();
    r2.importance = 0.9;
    s.update(&r2).expect("update on missing node");
    s.update_content("m-5", "new", None).expect("update_content on missing node");
    s.update_importance("m-5", 0.5).expect("update_importance on missing node");

    // Confirm: still no nodes row (no accidental re-creation).
    let count: i64 = s.conn().query_row(
        "SELECT COUNT(*) FROM nodes WHERE id = ?1",
        params!["m-5"],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 0,
        "ISS-124: UPDATE family must NOT INSERT nodes rows — backfill owns that");
}

#[test]
fn iss124_update_content_preserves_legacy_shim_keys() {
    // Ensures ISS-119 invariants survive update_content: if
    // _legacy_contradicts is stamped in nodes.attributes by the
    // insert path, update_content must NOT drop it when it
    // replaces nodes.attributes wholesale.
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.db");
    let mut s = Storage::new(&path).unwrap();

    let mut r = rec("m-6", "initial");
    r.contradicts = Some("other-memory-id".into());
    s.add(&r, "default").expect("add");

    // Verify the shim key landed in nodes.attributes.
    let attrs_before: Option<String> = s.conn().query_row(
        "SELECT attributes FROM nodes WHERE id = ?1",
        params!["m-6"],
        |row| row.get(0),
    ).unwrap();
    let attrs_before = attrs_before.expect("attributes JSON missing");
    assert!(attrs_before.contains("_legacy_contradicts"),
        "ISS-119 precondition: _legacy_contradicts must be stamped on insert. Got: {}", attrs_before);

    // update_content with new metadata that does NOT contain the shim key.
    s.update_content("m-6", "rewritten", Some(serde_json::json!({"user_tag": "v2"})))
        .expect("update_content");

    let attrs_after: Option<String> = s.conn().query_row(
        "SELECT attributes FROM nodes WHERE id = ?1",
        params!["m-6"],
        |row| row.get(0),
    ).unwrap();
    let attrs_after = attrs_after.expect("attributes JSON missing post-update");
    assert!(attrs_after.contains("_legacy_contradicts"),
        "ISS-124+ISS-119: update_content must preserve _legacy_* shim keys. Got: {}", attrs_after);
    assert!(attrs_after.contains("\"user_tag\""),
        "ISS-124: update_content must apply the new user metadata. Got: {}", attrs_after);
}
