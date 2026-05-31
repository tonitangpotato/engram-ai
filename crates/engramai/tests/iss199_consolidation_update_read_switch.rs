//! ISS-199 (Phase E read-cutover) — `update`/consolidation RMW
//! read-switch contract.
//!
//! Consolidation (`run_consolidation_cycle`) reads memories via
//! `all_in_namespace` (already on `nodes`) and writes each back via
//! `Storage::update`. Under `unified_substrate = true` (T34a) the
//! legacy `memories` row is never inserted, so `update_inner`'s
//! `SELECT rowid FROM memories WHERE id = ?` used to
//! `QueryReturnedNoRows` and abort the whole consolidation
//! transaction. The cutover makes `update` write `nodes` only
//! (via `update_memory_node_row`, with `nodes_fts` refreshed by the
//! `nodes_fts_au` trigger) and skip the legacy `memories` /
//! `memories_fts` maintenance under unified mode.
//!
//! Acceptance contract:
//!
//!   1. Unified update without a `memories` row succeeds (no
//!      `QueryReturnedNoRows`) and the mutation lands in `nodes`.
//!   2. Legacy update still mutates the `memories` row (regression
//!      guard — the legacy arm is unchanged).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 31, 4, 30, 0).unwrap();
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
        source: "iss199-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

#[test]
fn iss199_unified_update_without_memories_row_succeeds() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    // Add under unified mode: writes `nodes` only (no `memories` row).
    {
        let mut s = Storage::with_unified_substrate(&db, true).unwrap();
        s.add(&rec("m-1", "original"), "default").expect("add");

        // Precondition: no legacy `memories` row exists.
        let mem_count: i64 = s
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE id = 'm-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mem_count, 0, "unified add must not write memories");
    }

    // Update under unified mode — this is the consolidation RMW path.
    {
        let mut s = Storage::with_unified_substrate(&db, true).unwrap();
        let mut r = rec("m-1", "consolidated");
        r.importance = 0.9;
        r.consolidation_count = 3;
        // Must NOT return QueryReturnedNoRows.
        s.update(&r).expect("unified update must succeed without memories row");

        // Mutation must land in `nodes`.
        let (content, importance, consol): (String, f64, i64) = s
            .conn()
            .query_row(
                "SELECT content, importance, consolidation_count \
                 FROM nodes WHERE id = 'm-1' AND node_kind = 'memory'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "consolidated");
        assert!((importance - 0.9).abs() < 1e-9);
        assert_eq!(consol, 3);
    }
}

#[test]
fn iss199_legacy_update_still_mutates_memories() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    // Legacy add + update both touch the `memories` row.
    let mut s = Storage::new(&db).unwrap();
    s.add(&rec("m-1", "original"), "default").expect("add");

    let mut r = rec("m-1", "updated");
    r.importance = 0.7;
    s.update(&r).expect("legacy update");

    let (content, importance): (String, f64) = s
        .conn()
        .query_row(
            "SELECT content, importance FROM memories WHERE id = 'm-1'",
            params![],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(content, "updated");
    assert!((importance - 0.7).abs() < 1e-9);
}
