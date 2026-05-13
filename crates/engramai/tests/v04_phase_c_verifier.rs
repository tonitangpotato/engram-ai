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

#[test]
fn t27_report_covers_all_seven_drivers() {
    // The report MUST list every Phase C driver, even on an empty DB.
    // Missing drivers would let a regression slip through unnoticed —
    // anyone reading the report assumes "no row = no problem", so
    // absent rows must mean absent drivers, not silent skips.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();

    let expected: &[(&str, &str, bool)] = &[
        // (legacy_table, unified_table, merge_semantics)
        ("memories", "nodes", false),                // T19
        ("memory_embeddings", "node_embeddings", false), // T20
        ("entities", "nodes", false),                // T21
        ("entity_relations", "edges", true),         // T22
        ("memory_entities", "edges", true),          // T23
        ("hebbian_links", "edges", true),            // T24
        ("synthesis_provenance", "edges", false),    // T25
    ];

    assert_eq!(
        report.counts.len(),
        expected.len(),
        "expected {} driver rows, got {}: {:#?}",
        expected.len(),
        report.counts.len(),
        report.counts,
    );
    for (legacy, unified, merge) in expected {
        let row = report
            .counts
            .iter()
            .find(|c| c.legacy_table == *legacy)
            .unwrap_or_else(|| panic!("driver row for legacy={legacy} missing"));
        assert_eq!(row.unified_table, *unified, "unified table for {legacy}");
        assert_eq!(
            row.merge_semantics, *merge,
            "merge_semantics flag for {legacy}"
        );
        assert_eq!(row.legacy_rows, 0, "{legacy} legacy count on empty DB");
        assert_eq!(row.unified_rows, 0, "{legacy} unified count on empty DB");
        assert!(row.ok, "{legacy} row.ok on empty DB");
    }
    assert!(report.ok);
}

#[test]
fn t27_hebbian_driver_counts_match_after_backfill() {
    // Positive test for the edges-driver fingerprint
    // (edge_kind='associative', predicate='co_activated').
    // Two distinct (a,b) pairs → two unified edges → I1 delta zero.
    use engramai::substrate::backfill::backfill_hebbian_links_to_edges;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed two memories so the hebbian endpoints have valid nodes
    // post-T19.
    storage.add(&sample_record("mem-x"), "default").unwrap();
    storage.add(&sample_record("mem-y"), "default").unwrap();
    storage.add(&sample_record("mem-z"), "default").unwrap();

    // Seed two distinct hebbian links at the legacy layer.
    storage
        .conn()
        .execute(
            r#"INSERT INTO hebbian_links
               (source_id, target_id, strength, coactivation_count,
                temporal_forward, temporal_backward, direction,
                created_at, namespace, signal_source, signal_detail)
               VALUES
               ('mem-x', 'mem-y', 0.5, 1, 1, 0, 'forward', 0.0, 'default', 'recall', NULL),
               ('mem-y', 'mem-z', 0.5, 1, 1, 0, 'forward', 0.0, 'default', 'recall', NULL)"#,
            [],
        )
        .unwrap();

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    let report = heb_report(&storage);
    assert_eq!(report.legacy_rows, 2);
    assert_eq!(report.unified_rows, 2);
    assert_eq!(report.delta, 0);
    assert!(report.merge_semantics, "hebbian driver has merge semantics");
    assert!(report.ok);
}

#[test]
fn t27_hebbian_driver_merge_semantics_allows_unified_less_than_legacy() {
    // Two legacy rows on the SAME canonical pair (forward + reverse)
    // collapse into ONE unified edge. A naive equality check would
    // mark this delta=1 as a failure; merge_semantics says delta>=0
    // is fine.
    use engramai::substrate::backfill::backfill_hebbian_links_to_edges;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    storage.add(&sample_record("mem-x"), "default").unwrap();
    storage.add(&sample_record("mem-y"), "default").unwrap();

    storage
        .conn()
        .execute(
            r#"INSERT INTO hebbian_links
               (source_id, target_id, strength, coactivation_count,
                temporal_forward, temporal_backward, direction,
                created_at, namespace, signal_source, signal_detail)
               VALUES
               ('mem-x', 'mem-y', 0.5, 1, 1, 0, 'forward', 0.0, 'default', 'recall', NULL),
               ('mem-y', 'mem-x', 0.5, 1, 0, 1, 'backward', 1.0, 'default', 'recall', NULL)"#,
            [],
        )
        .unwrap();

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    let report = heb_report(&storage);
    assert_eq!(report.legacy_rows, 2, "2 legacy direction rows");
    assert_eq!(report.unified_rows, 1, "merged into 1 canonical edge");
    assert_eq!(report.delta, 1);
    assert!(
        report.ok,
        "merge_semantics drivers MUST treat positive delta as ok"
    );
}

/// Helper: extract just the hebbian driver row.
fn heb_report(storage: &Storage) -> engramai::substrate::verify::DriverCounts {
    verify_phase_c_parity(storage, &VerifyOpts::default())
        .unwrap()
        .counts
        .into_iter()
        .find(|c| c.legacy_table == "hebbian_links")
        .expect("hebbian driver row")
}

// ─────────────────────────────────────────────────────────────────
// I2 — Audit row consistency
// ─────────────────────────────────────────────────────────────────

#[test]
fn t27_audit_clean_db_reports_no_violations() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();
    assert!(report.audit_violations.is_empty());
    assert!(report.ok);
}

#[test]
fn t27_audit_post_backfill_is_consistent() {
    // Real backfill driver writes a real audit row. Verifier must
    // not flag it.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();
    assert!(
        report.audit_violations.is_empty(),
        "real backfill must produce a consistent audit row: {:#?}",
        report.audit_violations
    );
    assert!(report.ok);
}

#[test]
fn t27_audit_corrupt_row_flagged() {
    // Inject a finished audit row whose counters DO NOT sum.
    // Simulates the failure mode where a writer crashes between
    // updating sub-counters and committing the final row, then
    // some recovery script marks the row finished anyway.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage
        .conn()
        .execute(
            r#"INSERT INTO backfill_runs
               (run_id, legacy_table, rows_read,
                rows_inserted, rows_skipped_existing, rows_failed,
                started_at, finished_at, notes)
               VALUES ('run-bad', 'memories', 10, 4, 2, 1, 0.0, 1.0, '{}')"#,
            [],
        )
        .unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();
    assert_eq!(report.audit_violations.len(), 1);
    let v = &report.audit_violations[0];
    assert_eq!(v.run_id, "run-bad");
    assert_eq!(v.legacy_table, "memories");
    assert_eq!(v.rows_read, 10);
    assert_eq!(v.computed_sum, 7, "4 + 2 + 1");
    assert!(!report.ok, "audit violation must drag report.ok to false");
}

#[test]
fn t27_audit_in_progress_row_not_flagged() {
    // A run with finished_at IS NULL is mid-execution. Its counters
    // are allowed to be partial; the verifier MUST skip it.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage
        .conn()
        .execute(
            r#"INSERT INTO backfill_runs
               (run_id, legacy_table, rows_read,
                rows_inserted, rows_skipped_existing, rows_failed,
                started_at, finished_at, notes)
               VALUES ('run-in-flight', 'memories', 100, 5, 0, 0, 0.0, NULL, '{}')"#,
            [],
        )
        .unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();
    assert!(
        report.audit_violations.is_empty(),
        "in-progress runs must NOT be flagged: {:#?}",
        report.audit_violations
    );
    assert!(report.ok);
}

// ─────────────────────────────────────────────────────────────────
// I4 — Content spot-check
// ─────────────────────────────────────────────────────────────────

#[test]
fn t27_i4_sample_size_zero_disables_check() {
    // sample_size=0 must skip the check entirely, even when there
    // would be visible divergence. Lets CI dial it down for fast
    // smoke runs.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    // Don't backfill — legacy has a row, unified doesn't.

    let opts = VerifyOpts {
        spot_check_sample_size: 0,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    assert!(report.content_mismatches.is_empty());
    // I1 will still mark counts non-ok, but that's a different
    // invariant. This test pins ONLY the I4 behavior.
}

#[test]
fn t27_i4_post_backfill_no_mismatches() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-c"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    let report = verify_phase_c_parity(&storage, &VerifyOpts::default()).unwrap();
    assert!(
        report.content_mismatches.is_empty(),
        "clean backfill must not produce content mismatches: {:#?}",
        report.content_mismatches
    );
    assert!(report.ok);
}

#[test]
fn t27_i4_content_drift_flagged() {
    // Seed, backfill, then mutate the unified content out from
    // under the verifier. Critical-field drift MUST be flagged
    // with field='content' and both sides recorded.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    storage
        .conn()
        .execute(
            "UPDATE nodes SET content = 'CORRUPTED' WHERE id = ?",
            params!["mem-a"],
        )
        .unwrap();

    // Sample size 5 so we definitely pick the lone row.
    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let content_hit = report
        .content_mismatches
        .iter()
        .find(|m| m.field == "content")
        .expect("content drift must be reported");
    assert_eq!(content_hit.row_id, "mem-a");
    assert!(content_hit.legacy.contains("content of mem-a"));
    assert!(content_hit.unified.contains("CORRUPTED"));
    assert!(!report.ok);
}

#[test]
fn t27_i4_attribute_key_order_not_flagged() {
    // Attribute JSON written in different key order MUST NOT trip
    // I4 — only value drift is a parity failure. This pins the
    // JSON-parsed-comparison semantics.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let rec = sample_record("mem-a");
    seed_legacy_only(&mut storage, &rec, "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    // Re-write both sides with semantically-identical but textually-
    // different JSON. `serde_json` happens to write objects in
    // insertion order, so we go via raw SQL to guarantee a different
    // textual representation.
    storage
        .conn()
        .execute(
            r#"UPDATE memories SET metadata = '{"tag":"t27","extra":"x"}' WHERE id = ?"#,
            params!["mem-a"],
        )
        .unwrap();
    storage
        .conn()
        .execute(
            r#"UPDATE nodes SET attributes = '{"extra":"x","tag":"t27"}' WHERE id = ?"#,
            params!["mem-a"],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    assert!(
        report.content_mismatches.is_empty(),
        "key-order-only difference must NOT be flagged: {:#?}",
        report.content_mismatches
    );
}

#[test]
fn t27_i4_sampling_is_deterministic() {
    // Two runs with the same seed against the same DB MUST select
    // the same row set. We can't observe the sample directly via
    // the public API, but we CAN make divergence in one row and
    // assert that a fixed seed either always hits it or always
    // misses it — never alternates between runs.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    // 20 rows so sample_size=1 has a meaningful selection space.
    for i in 0..20 {
        seed_legacy_only(&mut storage, &sample_record(&format!("mem-{i:02}")), "default");
    }
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    // Corrupt EVERY row's unified content so any sampled row will
    // produce exactly one ContentMismatch. Then run twice with the
    // same seed and assert the mismatch set is identical.
    storage
        .conn()
        .execute("UPDATE nodes SET content = 'CORRUPTED' WHERE node_kind = 'memory'", [])
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 3,
        spot_check_seed: 42,
        ..VerifyOpts::default()
    };
    let r1 = verify_phase_c_parity(&storage, &opts).unwrap();
    let r2 = verify_phase_c_parity(&storage, &opts).unwrap();

    let ids1: Vec<&String> = r1
        .content_mismatches
        .iter()
        .filter(|m| m.field == "content")
        .map(|m| &m.row_id)
        .collect();
    let ids2: Vec<&String> = r2
        .content_mismatches
        .iter()
        .filter(|m| m.field == "content")
        .map(|m| &m.row_id)
        .collect();
    assert_eq!(ids1.len(), 3, "sample of 3 should yield 3 content hits");
    assert_eq!(ids1, ids2, "same seed must produce identical sample");
}
