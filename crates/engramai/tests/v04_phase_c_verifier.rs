//! T27 — Phase C parity verifier integration tests.
//!
//! Skeleton iteration (this file): covers the memories→nodes driver
//! through invariants I1 (count parity) and I5 (FK closure). Other
//! drivers / invariants land in follow-up iterations of `verify.rs`.
//!
//! Test matrix this file pins:
//!
//! - `t27_clean_db_reports_ok` — empty DB, no edges, no memories →
//!   every count zero, no FK violations, `ok == true`.
//! - `t27_post_backfill_counts_match` — seed legacy memories, run
//!   T19 backfill, verifier reports matching counts and `ok == true`.
//! - `t27_detects_missing_node` — seed legacy memories, run backfill,
//!   then DELETE a nodes row out from under the unified side →
//!   verifier flags `delta=1` and `ok == false`.
//! - `t27_fk_violation_detected` — inject an edges row whose
//!   `source_id` points at a non-existent node (PRAGMA off) → I5
//!   surfaces it.
//! - `t27_namespace_filter_isolates_counts` — seed two namespaces;
//!   restrict the verifier to one of them → counts match the
//!   filtered namespace only.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::backfill_memories_to_nodes;
use engramai::substrate::verify::{verify_phase_c_parity, VerifyOpts};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 14, 0, 0).unwrap();
    let occurred = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: format!("content of {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: Some(occurred),
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 1,
        last_consolidated: Some(created),
        source: "t27-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: Some(serde_json::json!({"tag": "t27"})),
    }
}

/// Insert a memory only into the legacy `memories` table by going
/// through `Storage::add` (dual-write) and then stripping the
/// `nodes` row. Mirrors the helper in `v04_phase_c_backfill.rs`.
fn seed_legacy_only(storage: &mut Storage, record: &MemoryRecord, namespace: &str) {
    storage.add(record, namespace).expect("add");
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params![record.id])
        .expect("strip nodes row");
}

#[test]
fn t27_clean_db_reports_ok() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).expect("verify");

    assert!(report.ok, "fresh DB must verify clean: {report:?}");
    assert!(report.fk_violations.is_empty());
    // memories driver row exists and is zero-zero.
    let mem = report
        .counts
        .iter()
        .find(|c| c.legacy_table == "memories")
        .expect("memories driver row present");
    assert_eq!(mem.legacy_rows, 0);
    assert_eq!(mem.unified_rows, 0);
    assert_eq!(mem.delta, 0);
    assert!(mem.ok);
}

#[test]
fn t27_post_backfill_counts_match() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed 3 legacy-only memories + 1 dual-written.
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-c"), "default");
    storage.add(&sample_record("mem-d"), "default").unwrap();

    backfill_memories_to_nodes(&mut storage, None).unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();

    let mem = report
        .counts
        .iter()
        .find(|c| c.legacy_table == "memories")
        .unwrap();
    assert_eq!(mem.legacy_rows, 4, "4 memories total");
    assert_eq!(mem.unified_rows, 4, "all 4 backfilled");
    assert_eq!(mem.delta, 0);
    assert!(mem.ok);
    assert!(report.ok, "report should be ok after a complete backfill");
}

#[test]
fn t27_detects_missing_node() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    // Inject divergence: nuke one nodes row, leaving the legacy row
    // intact. This simulates either a Phase C bug or a manual DELETE.
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params!["mem-a"])
        .unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();

    let mem = report
        .counts
        .iter()
        .find(|c| c.legacy_table == "memories")
        .unwrap();
    assert_eq!(mem.legacy_rows, 2);
    assert_eq!(mem.unified_rows, 1);
    assert_eq!(mem.delta, 1, "one legacy row without a unified twin");
    assert!(!mem.ok, "pass-through driver with delta != 0 must fail I1");
    assert!(!report.ok);
}

#[test]
fn t27_fk_violation_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Engram turns on `PRAGMA foreign_keys=ON` per connection, so a
    // direct INSERT with a dangling target_id is normally rejected.
    // To simulate the failure mode I5 is meant to catch — a row that
    // bypassed FK enforcement (operator ran a maintenance script with
    // foreign_keys=OFF, or a future migration dropped + re-added a
    // node out from under existing edges) — we temporarily disable
    // FK enforcement for the injection, then re-enable it before
    // running the verifier.
    {
        let conn = storage.conn();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at)
             VALUES ('n-real', 'memory', 'default', 'x', 0.0, 0.0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at)
             VALUES ('e-bad', 'n-real', 'n-missing', 'associative', 'co_activated', 0.0, 0.0, 0.0)",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    }

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();

    assert_eq!(report.fk_violations.len(), 1);
    let v = &report.fk_violations[0];
    assert_eq!(v.edge_id, "e-bad");
    assert_eq!(v.side, "target");
    assert_eq!(v.missing_node_id, "n-missing");
    assert!(!report.ok);
}

#[test]
fn t27_namespace_filter_isolates_counts() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_only(&mut storage, &sample_record("mem-a"), "ns-foo");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "ns-foo");
    seed_legacy_only(&mut storage, &sample_record("mem-c"), "ns-bar");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    let opts = VerifyOpts {
        namespace: Some("ns-foo".to_string()),
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();

    let mem = report
        .counts
        .iter()
        .find(|c| c.legacy_table == "memories")
        .unwrap();
    assert_eq!(mem.legacy_rows, 2, "namespace filter should restrict legacy count");
    assert_eq!(mem.unified_rows, 2, "and the unified count too");
    assert!(mem.ok);
    assert!(report.ok);
}
