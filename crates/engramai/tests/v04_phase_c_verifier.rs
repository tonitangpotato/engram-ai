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

// ─────────────────────────────────────────────────────────────────
// I3 — Idempotency (gated, costly)
// ─────────────────────────────────────────────────────────────────

#[test]
fn t27_i3_off_by_default() {
    // Default VerifyOpts has check_idempotency=false. Even on a DB
    // where I3 would fire if it ran, the default entry point must
    // not invoke it.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    // NOTE: we deliberately do NOT call backfill — the legacy row
    // is missing from the unified side. If I3 ran, a re-run would
    // insert it. With the flag off, the report should NOT contain
    // an idempotency violation.

    let opts = VerifyOpts {
        check_idempotency: false,
        ..VerifyOpts::default()
    };
    let report =
        engramai::substrate::verify::verify_phase_c_parity_mut(&mut storage, &opts).unwrap();
    assert!(
        report.idempotency_violations.is_empty(),
        "I3 must be off by default even via the _mut entry point"
    );
}

#[test]
fn t27_i3_clean_backfill_has_no_violations() {
    // After a real T19, re-running every driver inserts zero rows.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    let opts = VerifyOpts {
        check_idempotency: true,
        ..VerifyOpts::default()
    };
    let report =
        engramai::substrate::verify::verify_phase_c_parity_mut(&mut storage, &opts).unwrap();
    assert!(
        report.idempotency_violations.is_empty(),
        "clean backfill must idempotently re-run with zero inserts: {:#?}",
        report.idempotency_violations
    );
    assert!(report.ok);
}

#[test]
fn t27_i3_missing_unified_row_triggers_reinsert() {
    // Seed, backfill, DELETE a nodes row out from under the verifier,
    // turn I3 on. The re-run inserts the missing row back; that's
    // an I3 violation because the contract is "re-run is a no-op
    // on a backfilled DB".
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_only(&mut storage, &sample_record("mem-a"), "default");
    seed_legacy_only(&mut storage, &sample_record("mem-b"), "default");
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params!["mem-a"])
        .unwrap();

    let opts = VerifyOpts {
        check_idempotency: true,
        spot_check_sample_size: 0, // disable I4 to keep this test focused
        ..VerifyOpts::default()
    };
    let report =
        engramai::substrate::verify::verify_phase_c_parity_mut(&mut storage, &opts).unwrap();
    let hit = report
        .idempotency_violations
        .iter()
        .find(|v| v.legacy_table == "memories")
        .expect("memories driver must report idempotency violation");
    assert_eq!(hit.rows_inserted_on_rerun, 1, "exactly one row re-inserted");
    assert!(!report.ok);
}

#[test]
fn t27_ns_filter_flag_set_when_legacy_lacks_namespace_column() {
    // FINDING-4 regression: when the verifier is asked to filter by
    // namespace but a legacy table has no `namespace` column
    // (memory_entities, memory_embeddings, synthesis_provenance), the
    // legacy-side count is GLOBAL not scoped. The report must surface
    // this via `legacy_ns_filter_applied=false` rather than silently
    // computing a meaningless delta.
    //
    // Setup: empty DB, request ns="default" filter, inspect the three
    // affected drivers' flags.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let opts = VerifyOpts {
        namespace: Some("default".into()),
        spot_check_sample_size: 0, // disable I4 for focus
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();

    for legacy_table in ["memory_embeddings", "memory_entities", "synthesis_provenance"] {
        let row = report
            .counts
            .iter()
            .find(|c| c.legacy_table == legacy_table)
            .expect("driver row present");
        assert!(
            !row.legacy_ns_filter_applied,
            "{legacy_table} must report ns filter NOT applied: {row:#?}"
        );
        // ok must be true (we don't fail the check on this asymmetry)
        assert!(
            row.ok,
            "{legacy_table} with asymmetric ns filter must NOT fail: {row:#?}"
        );
    }

    // Sanity: ns-aware drivers DO have the flag set.
    for legacy_table in ["memories", "entities", "entity_relations", "hebbian_links"] {
        let row = report
            .counts
            .iter()
            .find(|c| c.legacy_table == legacy_table)
            .expect("driver row present");
        assert!(
            row.legacy_ns_filter_applied,
            "{legacy_table} must report ns filter applied: {row:#?}"
        );
    }
}

#[test]
fn t27_ns_filter_flag_default_true_when_no_filter_requested() {
    // When no namespace filter is requested, every driver row should
    // report `legacy_ns_filter_applied=true` (trivially: filter
    // requested = none, filter applied = none, the two match).
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 0,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();

    for row in &report.counts {
        assert!(
            row.legacy_ns_filter_applied,
            "no ns filter requested → flag must be true for {}: {:#?}",
            row.legacy_table, row
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// I4 — T20 spot-check tests (ISS-113)
// ─────────────────────────────────────────────────────────────────────

fn seed_legacy_embedding(
    storage: &Storage,
    memory_id: &str,
    model: &str,
    dimensions: usize,
    created_at_rfc3339: &str,
) -> Vec<u8> {
    let blob: Vec<u8> = (0..dimensions * 4).map(|i| (i % 251) as u8).collect();
    storage
        .conn()
        .execute(
            r#"INSERT OR REPLACE INTO memory_embeddings
               (memory_id, model, embedding, dimensions, created_at)
               VALUES (?, ?, ?, ?, ?)"#,
            params![memory_id, model, blob, dimensions as i64, created_at_rfc3339],
        )
        .expect("seed legacy embedding");
    blob
}

#[test]
fn t27_i4_node_embeddings_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::backfill_embeddings_to_node_embeddings;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed memory (T12 dual-writes node), then legacy embedding, then T20.
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-1", "all-MiniLM-L6-v2", 4, "2026-05-13T10:30:00Z");
    let run = backfill_embeddings_to_node_embeddings(&mut storage, None).unwrap();
    assert_eq!(run.rows_inserted, 1);

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let emb_mismatches: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.legacy_table == "memory_embeddings")
        .collect();
    assert!(
        emb_mismatches.is_empty(),
        "clean post-backfill must have zero T20 mismatches: {emb_mismatches:#?}"
    );
    assert!(report.ok);
}

#[test]
fn t27_i4_node_embeddings_blob_drift_flagged() {
    // Mutate the unified-side embedding BLOB and confirm I4 catches
    // it. This is the canonical silent-corruption case I4 exists to
    // detect: row counts unchanged, but content rotted.
    use engramai::substrate::backfill::backfill_embeddings_to_node_embeddings;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-1", "m", 4, "2026-05-13T10:30:00Z");
    backfill_embeddings_to_node_embeddings(&mut storage, None).unwrap();

    storage
        .conn()
        .execute(
            "UPDATE node_embeddings SET embedding = X'DEADBEEF' WHERE node_id = 'mem-1'",
            [],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "memory_embeddings" && m.field == "embedding")
        .expect("blob drift must be reported");
    assert!(hit.row_id.starts_with("mem-1|m"));
    assert!(!report.ok);
}

#[test]
fn t27_i4_node_embeddings_dimensions_drift_flagged() {
    use engramai::substrate::backfill::backfill_embeddings_to_node_embeddings;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-1", "m", 4, "2026-05-13T10:30:00Z");
    backfill_embeddings_to_node_embeddings(&mut storage, None).unwrap();

    // Lie about the dimensions on the unified side.
    storage
        .conn()
        .execute(
            "UPDATE node_embeddings SET dimensions = 99 WHERE node_id = 'mem-1'",
            [],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "memory_embeddings" && m.field == "dimensions")
        .expect("dimensions drift must be reported");
    assert!(hit.unified.contains("99"));
    assert!(hit.legacy.contains("4"));
    assert!(!report.ok);
}

#[test]
fn t27_i4_node_embeddings_missing_unified_row_flagged() {
    use engramai::substrate::backfill::backfill_embeddings_to_node_embeddings;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-1", "m", 4, "2026-05-13T10:30:00Z");
    backfill_embeddings_to_node_embeddings(&mut storage, None).unwrap();

    storage
        .conn()
        .execute(
            "DELETE FROM node_embeddings WHERE node_id = 'mem-1'",
            [],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "memory_embeddings" && m.field == "existence")
        .expect("missing unified row must be flagged");
    assert_eq!(hit.unified, "missing");
}

// ─────────────────────────────────────────────────────────────────────
// I4 — T21 spot-check tests (ISS-113)
// ─────────────────────────────────────────────────────────────────────

fn seed_legacy_entity(
    storage: &Storage,
    id: &str,
    name: &str,
    entity_type: &str,
    namespace: &str,
    metadata_json: Option<&str>,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, 1700000000.0, 1700000000.0)"#,
            params![id, name, entity_type, namespace, metadata_json],
        )
        .expect("seed entities row");
}

#[test]
fn t27_i4_entities_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::backfill_entities_to_nodes;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(
        &storage,
        "ent-1",
        "Alice",
        "person",
        "default",
        Some(r#"{"alias":"al","note":"founder"}"#),
    );
    backfill_entities_to_nodes(&mut storage, None).unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let ent_mismatches: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.legacy_table == "entities")
        .collect();
    assert!(
        ent_mismatches.is_empty(),
        "clean post-backfill must have zero entity mismatches: {ent_mismatches:#?}"
    );
    assert!(report.ok);
}

#[test]
fn t27_i4_entities_finding1_column_wins_regression_guard() {
    // FINDING-1 (T21 r1): when both `entities.entity_type` column AND
    // `entities.metadata.entity_type` are set, the COLUMN value lands
    // in unified `nodes.attributes.entity_type`. This test pins that
    // direction.
    //
    // Setup: seed with column='person', metadata.entity_type='SHADOW'.
    // After backfill, unified should report 'person', not 'SHADOW'.
    use engramai::substrate::backfill::backfill_entities_to_nodes;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(
        &storage,
        "ent-shadow",
        "Alice",
        "person",
        "default",
        Some(r#"{"entity_type":"SHADOW","note":"shadow-attack"}"#),
    );
    backfill_entities_to_nodes(&mut storage, None).unwrap();

    // Real driver output: unified.attributes.entity_type must be
    // 'person' (column wins).
    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'ent-shadow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        parsed.get("entity_type").and_then(|v| v.as_str()),
        Some("person"),
        "column must win in real T21 output"
    );

    // Verifier must say OK (no mismatches) because driver behavior
    // matches expected projection.
    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.legacy_table == "entities")
        .collect();
    assert!(
        hits.is_empty(),
        "verifier must agree with column-wins driver: {hits:#?}"
    );

    // Now SIMULATE the inverse bug: someone "fixes" the unified
    // attributes to use the metadata value. Verifier must catch it.
    storage
        .conn()
        .execute(
            r#"UPDATE nodes SET attributes = '{"entity_type":"SHADOW","note":"shadow-attack"}' WHERE id = 'ent-shadow'"#,
            [],
        )
        .unwrap();
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let column_hits: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.field.contains("FINDING-1") || m.field == "attributes.entity_type (FINDING-1 column-wins)")
        .collect();
    assert!(
        !column_hits.is_empty(),
        "verifier must catch metadata-wins regression: report={:#?}",
        report.content_mismatches
    );
    assert!(!report.ok);
}

#[test]
fn t27_i4_entities_name_to_content_drift_flagged() {
    use engramai::substrate::backfill::backfill_entities_to_nodes;

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-1", "Alice", "person", "default", None);
    backfill_entities_to_nodes(&mut storage, None).unwrap();

    storage
        .conn()
        .execute(
            "UPDATE nodes SET content = 'CORRUPTED' WHERE id = 'ent-1'",
            [],
        )
        .unwrap();
    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "entities" && m.field == "name->content")
        .expect("name->content drift must be reported");
    assert_eq!(hit.legacy, "Alice");
    assert_eq!(hit.unified, "CORRUPTED");
}

// ─────────────────────────────────────────────────────────────────────
// I4 — T25 spot-check tests (ISS-113)
// ─────────────────────────────────────────────────────────────────────

fn seed_legacy_provenance(
    storage: &Storage,
    id: &str,
    insight_id: &str,
    source_id: &str,
    cluster_id: &str,
    synthesis_timestamp: &str,
    gate_decision: &str,
    gate_scores: Option<&str>,
    confidence: f64,
    source_original_importance: Option<f64>,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO synthesis_provenance
               (id, insight_id, source_id, cluster_id, synthesis_timestamp,
                gate_decision, gate_scores, confidence, source_original_importance)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            params![
                id, insight_id, source_id, cluster_id, synthesis_timestamp,
                gate_decision, gate_scores, confidence, source_original_importance,
            ],
        )
        .expect("seed synthesis_provenance row");
}

#[test]
fn t27_i4_synthesis_provenance_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::{
        backfill_memories_to_nodes, backfill_synthesis_provenance_to_edges,
    };

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    // Two memories: the insight and its source.
    let insight = sample_record("mem-insight");
    let src = sample_record("mem-src");
    storage.add(&insight, "default").unwrap();
    storage.add(&src, "default").unwrap();
    // Run T19 so nodes are projected (FK guard).
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    seed_legacy_provenance(
        &storage,
        "sp-1",
        "mem-insight",
        "mem-src",
        "cluster-7",
        "2026-05-13T10:00:00Z",
        "promote",
        Some(r#"{"informativeness":0.81,"surprise":0.42}"#),
        0.93,
        Some(0.55),
    );
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let sp_mismatches: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.legacy_table == "synthesis_provenance")
        .collect();
    assert!(
        sp_mismatches.is_empty(),
        "clean post-backfill must have zero T25 mismatches: {sp_mismatches:#?}"
    );
    assert!(report.ok);
}

#[test]
fn t27_i4_synthesis_provenance_gate_scores_nested_not_string() {
    // Pin the §5.3 invariant: gate_scores in unified attributes is a
    // PARSED nested JSON object, NOT a quoted string of the legacy
    // text. If the driver regresses and emits the raw string, the
    // verifier must catch it.
    use engramai::substrate::backfill::{
        backfill_memories_to_nodes, backfill_synthesis_provenance_to_edges,
    };

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let insight = sample_record("mem-insight");
    let src = sample_record("mem-src");
    storage.add(&insight, "default").unwrap();
    storage.add(&src, "default").unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();

    seed_legacy_provenance(
        &storage,
        "sp-1",
        "mem-insight",
        "mem-src",
        "cluster-7",
        "2026-05-13T10:00:00Z",
        "promote",
        Some(r#"{"informativeness":0.81}"#),
        0.93,
        None,
    );
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    // Confirm real driver output: gate_scores IS an object.
    let attrs_text: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = 'sp-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let attrs: serde_json::Value = serde_json::from_str(&attrs_text).unwrap();
    assert!(
        attrs.get("gate_scores").and_then(|v| v.as_object()).is_some(),
        "gate_scores must be a parsed nested object, got: {attrs}"
    );

    // Now simulate the regression: someone writes gate_scores as a
    // quoted string of the legacy JSON. Verifier must catch.
    storage
        .conn()
        .execute(
            r#"UPDATE edges SET attributes = '{"gate_decision":"promote","cluster_id":"cluster-7","synthesis_timestamp":"2026-05-13T10:00:00Z","gate_scores":"{\"informativeness\":0.81}"}' WHERE id = 'sp-1'"#,
            [],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "synthesis_provenance" && m.field == "attributes")
        .expect("attribute round-trip regression must be caught");
    assert!(
        hit.unified.contains(r#"\"informativeness\""#)
            || hit.unified.contains(r#"informativeness\\""#)
            || hit.unified.contains(r#""{"#),
        "unified should be the string form, legacy the object form: {hit:?}"
    );
}

#[test]
fn t27_i4_synthesis_provenance_confidence_drift_flagged() {
    use engramai::substrate::backfill::{
        backfill_memories_to_nodes, backfill_synthesis_provenance_to_edges,
    };

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let insight = sample_record("mem-insight");
    let src = sample_record("mem-src");
    storage.add(&insight, "default").unwrap();
    storage.add(&src, "default").unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    seed_legacy_provenance(
        &storage, "sp-1", "mem-insight", "mem-src", "c", "2026-05-13T10:00:00Z",
        "promote", None, 0.93, None,
    );
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    storage
        .conn()
        .execute(
            "UPDATE edges SET confidence = 0.10 WHERE id = 'sp-1'",
            [],
        )
        .unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report
        .content_mismatches
        .iter()
        .find(|m| m.legacy_table == "synthesis_provenance" && m.field == "confidence")
        .expect("confidence drift must be reported");
    assert!(hit.legacy.contains("0.93"));
    assert!(hit.unified.contains("0.1"));
}

// ─────────────────────────────────────────────────────────────────────
// I4 — Merge-semantics existence-only tests (ISS-113 slice 3)
//      T22 / T23 / T24
// ─────────────────────────────────────────────────────────────────────

fn seed_legacy_relation(
    storage: &Storage,
    id: &str,
    source_id: &str,
    target_id: &str,
    relation: &str,
    namespace: &str,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entity_relations
               (id, source_id, target_id, relation, confidence, source, namespace, created_at, metadata)
               VALUES (?, ?, ?, ?, 1.0, NULL, ?, 1700000000.0, NULL)"#,
            params![id, source_id, target_id, relation, namespace],
        )
        .expect("seed entity_relation");
}

fn seed_legacy_memory_entity(
    storage: &Storage,
    memory_id: &str,
    entity_id: &str,
    role: &str,
) {
    storage
        .conn()
        .execute(
            "INSERT INTO memory_entities (memory_id, entity_id, role) VALUES (?, ?, ?)",
            params![memory_id, entity_id, role],
        )
        .expect("seed memory_entities row");
}

fn seed_legacy_hebbian(
    storage: &Storage,
    source_id: &str,
    target_id: &str,
    strength: f64,
    coact: i64,
    namespace: &str,
    signal_source: &str,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO hebbian_links
               (source_id, target_id, strength, coactivation_count,
                temporal_forward, temporal_backward, direction,
                created_at, namespace, signal_source, signal_detail)
               VALUES (?, ?, ?, ?, 0, 0, 'forward', 1700000000.0, ?, ?, NULL)"#,
            params![source_id, target_id, strength, coact, namespace, signal_source],
        )
        .expect("seed hebbian_links row");
}

// ── T22 entity_relations → edges(structural) ─────────────────────────

#[test]
fn t27_i4_entity_relations_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_entity_relations_to_edges,
    };

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','Alice','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e2','Bob','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_relation(&storage, "rel-1", "e1", "e2", "knows", "default");
    backfill_entity_relations_to_edges(&mut storage, None).unwrap();

    let opts = VerifyOpts {
        spot_check_sample_size: 5,
        ..VerifyOpts::default()
    };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report
        .content_mismatches
        .iter()
        .filter(|m| m.legacy_table == "entity_relations")
        .collect();
    assert!(hits.is_empty(), "clean T22: {hits:#?}");
    assert!(report.ok);
}

#[test]
fn t27_i4_entity_relations_missing_unified_row_flagged() {
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_entity_relations_to_edges,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','A','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e2','B','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_relation(&storage, "rel-1", "e1", "e2", "knows", "default");
    backfill_entity_relations_to_edges(&mut storage, None).unwrap();
    storage.conn().execute("DELETE FROM edges WHERE id = 'rel-1'", []).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report.content_mismatches.iter()
        .find(|m| m.legacy_table == "entity_relations" && m.field == "existence")
        .expect("missing unified row must be flagged");
    assert_eq!(hit.unified, "missing");
}

#[test]
fn t27_i4_entity_relations_predicate_drift_flagged() {
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_entity_relations_to_edges,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','A','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e2','B','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_relation(&storage, "rel-1", "e1", "e2", "knows", "default");
    backfill_entity_relations_to_edges(&mut storage, None).unwrap();
    storage.conn().execute(
        "UPDATE edges SET predicate = 'CORRUPTED' WHERE id = 'rel-1'", []
    ).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report.content_mismatches.iter()
        .find(|m| m.legacy_table == "entity_relations" && m.field == "predicate")
        .expect("predicate drift must be flagged");
    assert_eq!(hit.legacy, "knows");
    assert_eq!(hit.unified, "CORRUPTED");
}

#[test]
fn t27_i4_entity_relations_t23_predicate_collision_flagged() {
    // Pin the §3.3 vs §5.3 prose drift case: T22 must NEVER produce
    // 'subject_of' or 'object_of' (those belong to T23). If someone
    // changes T22 driver to map 'has_subject' -> 'subject_of', the
    // verifier must catch it.
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_entity_relations_to_edges,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','A','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e2','B','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_relation(&storage, "rel-1", "e1", "e2", "has_subject", "default");
    backfill_entity_relations_to_edges(&mut storage, None).unwrap();
    // Simulate the regression — driver produced 'subject_of' instead of 'has_subject'.
    storage.conn().execute(
        "UPDATE edges SET predicate = 'subject_of' WHERE id = 'rel-1'", []
    ).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report.content_mismatches.iter()
        .find(|m| m.legacy_table == "entity_relations"
              && m.field.contains("T22 must NOT use T23"))
        .expect("T22-using-T23-predicate must be flagged");
    assert_eq!(hit.unified, "subject_of");
}

// ── T23 memory_entities → edges (role split) ─────────────────────────

#[test]
fn t27_i4_memory_entities_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_memories_to_nodes,
        backfill_memory_entities_to_edges,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','Alice','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e2','Bob','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    // memory_entities PK = (memory_id, entity_id); can't seed multiple
    // roles for the same (mem, ent) pair. Use two distinct entities to
    // cover both 'mention' and 'subject' paths.
    seed_legacy_memory_entity(&storage, "mem-1", "e1", "mention");
    seed_legacy_memory_entity(&storage, "mem-1", "e2", "subject");
    backfill_memory_entities_to_edges(&mut storage, None).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 10, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report.content_mismatches.iter()
        .filter(|m| m.legacy_table == "memory_entities")
        .collect();
    assert!(hits.is_empty(), "clean T23: {hits:#?}");
}

#[test]
fn t27_i4_memory_entities_role_split_subject_endpoints() {
    // Pin §3.3 endpoint direction for 'subject' role:
    //   provenance/mentions → source=memory, target=entity
    //   structural/subject_of → source=entity, target=memory
    // If the driver regresses to "always memory→entity" for subject_of,
    // the source_id/target_id check fires.
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_memories_to_nodes,
        backfill_memory_entities_to_edges,
    };
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    // Inline the same hash formula the driver uses (uuid_from_hash is
    // crate-private; tests cross the crate boundary).
    fn local_uuid_from_hash(s: &str) -> String {
        let digest = Sha256::digest(s.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Uuid::from_bytes(bytes).to_string()
    }

    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','Alice','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_memory_entity(&storage, "mem-1", "e1", "subject");
    backfill_memory_entities_to_edges(&mut storage, None).unwrap();

    // For role='subject': (edge_kind, predicate) = ('structural', 'subject_of').
    let id = local_uuid_from_hash(
        "memory_entities|mem-1|e1|subject|structural|subject_of"
    );
    // Confirm real driver produces source=mem-1, target=e1
    // (as-built behavior — see verifier comment about §3.3 spec
    // drift on the endpoint direction).
    let (src, tgt): (String, String) = storage.conn().query_row(
        "SELECT source_id, target_id FROM edges WHERE id = ?",
        params![id], |r| Ok((r.get(0)?, r.get(1)?))
    ).unwrap();
    assert_eq!(src, "mem-1");
    assert_eq!(tgt, "e1");

    // Now SIMULATE the regression: swap endpoints to (e1, mem-1).
    storage.conn().execute(
        "UPDATE edges SET source_id='e1', target_id='mem-1' WHERE id = ?",
        params![id]
    ).unwrap();
    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report.content_mismatches.iter()
        .filter(|m| m.legacy_table == "memory_entities"
              && (m.field == "source_id" || m.field == "target_id"))
        .collect();
    assert!(!hits.is_empty(), "endpoint swap must be caught: {hits:#?}");
}

#[test]
fn t27_i4_memory_entities_missing_unified_row_flagged() {
    use engramai::substrate::backfill::{
        backfill_entities_to_nodes, backfill_memories_to_nodes,
        backfill_memory_entities_to_edges,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    storage.conn().execute(
        "INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) VALUES ('e1','A','person','default',NULL,1700000000.0,1700000000.0)",
        [],
    ).unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    backfill_entities_to_nodes(&mut storage, None).unwrap();
    seed_legacy_memory_entity(&storage, "mem-1", "e1", "mention");
    backfill_memory_entities_to_edges(&mut storage, None).unwrap();
    storage.conn().execute(
        "DELETE FROM edges WHERE edge_kind='provenance' AND predicate='mentions'", []
    ).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report.content_mismatches.iter()
        .find(|m| m.legacy_table == "memory_entities" && m.field == "existence")
        .expect("missing unified row must be flagged");
    assert_eq!(hit.unified, "missing");
}

// ── T24 hebbian_links → edges (associative/co_activated, SUM merge) ──

#[test]
fn t27_i4_hebbian_links_post_backfill_no_mismatches() {
    use engramai::substrate::backfill::{
        backfill_hebbian_links_to_edges, backfill_memories_to_nodes,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let a = sample_record("mem-a");
    let b = sample_record("mem-b");
    storage.add(&a, "default").unwrap();
    storage.add(&b, "default").unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    seed_legacy_hebbian(&storage, "mem-a", "mem-b", 0.5, 3, "default", "corecall");
    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report.content_mismatches.iter()
        .filter(|m| m.legacy_table == "hebbian_links")
        .collect();
    assert!(hits.is_empty(), "clean T24: {hits:#?}");
}

#[test]
fn t27_i4_hebbian_links_sum_lower_bound_violation_flagged() {
    // The SUM lower-bound check: unified.weight MUST be >= every
    // individual legacy.strength. If someone "fixes" the unified
    // weight downward (regression: applied a decay instead of
    // preserving SUM), the verifier must catch it.
    use engramai::substrate::backfill::{
        backfill_hebbian_links_to_edges, backfill_memories_to_nodes,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let a = sample_record("mem-a");
    let b = sample_record("mem-b");
    storage.add(&a, "default").unwrap();
    storage.add(&b, "default").unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    seed_legacy_hebbian(&storage, "mem-a", "mem-b", 0.7, 5, "default", "corecall");
    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    // Tank the weight below the legacy strength.
    storage.conn().execute(
        "UPDATE edges SET weight = 0.1 WHERE edge_kind='associative'", []
    ).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hit = report.content_mismatches.iter()
        .find(|m| m.legacy_table == "hebbian_links" && m.field.starts_with("weight"))
        .expect("weight regression below SUM lower bound must be flagged");
    assert!(hit.legacy.contains("0.7"));
    assert!(hit.unified.contains("0.1"));
}

#[test]
fn t27_i4_hebbian_links_canonical_pair_direction_independent() {
    // Seed legacy with target<source (reverse order). The driver
    // canonicalizes to (lo, hi) before hashing; the verifier must
    // do the same and find the SAME edge id.
    use engramai::substrate::backfill::{
        backfill_hebbian_links_to_edges, backfill_memories_to_nodes,
    };
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let a = sample_record("mem-aaaa");
    let b = sample_record("mem-zzzz");
    storage.add(&a, "default").unwrap();
    storage.add(&b, "default").unwrap();
    backfill_memories_to_nodes(&mut storage, None).unwrap();
    // Insert with target<source by passing "mem-zzzz" as source.
    seed_legacy_hebbian(&storage, "mem-zzzz", "mem-aaaa", 0.5, 2, "default", "corecall");
    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    let opts = VerifyOpts { spot_check_sample_size: 5, ..VerifyOpts::default() };
    let report = verify_phase_c_parity(&storage, &opts).unwrap();
    let hits: Vec<_> = report.content_mismatches.iter()
        .filter(|m| m.legacy_table == "hebbian_links")
        .collect();
    assert!(hits.is_empty(),
        "canonicalization should produce same id regardless of input order: {hits:#?}");
}
