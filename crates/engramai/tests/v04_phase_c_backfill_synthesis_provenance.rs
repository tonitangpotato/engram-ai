//! T25 — Phase C backfill driver: synthesis_provenance → edges(kind=provenance, predicate=derived_from).
//!
//! Acceptance per design.md §3.3 + §4.5 + §5.3 + §6 verifier I5:
//!
//!   1. Every legacy `synthesis_provenance` row gets a matching
//!      `edges(edge_kind='provenance', predicate='derived_from')` row.
//!   2. `edges.id == legacy.id` (pass-through, NOT a hash). This is
//!      the contract for append-only edges per §3.2 (no partial
//!      unique index) + matches Phase B T16's behavior.
//!   3. `edges.source_id == legacy.insight_id`,
//!      `edges.target_id == legacy.source_id`. Direction is
//!      "derived from" → insight points to source.
//!   4. `edges.confidence == legacy.confidence` (NOT 1.0). This is
//!      the first Phase C driver to pass a legacy confidence column
//!      through and establishes the FINDING-3 policy: legacy-column-
//!      wins when present.
//!   5. `edges.namespace` derives from the insight memory's namespace
//!      (JOIN), because synthesis_provenance has no namespace column.
//!   6. Attributes embed gate_decision, gate_scores (parsed as nested
//!      JSON object, not quoted string), cluster_id,
//!      source_original_importance, synthesis_timestamp (verbatim
//!      RFC3339 string for forensic traceability).
//!   7. Endpoint missing in `nodes` → SKIPPED, counted in
//!      `rows_skipped_dangling_endpoint`. Recovers after T19.
//!   8. Idempotent rerun: same legacy row → counted in
//!      `rows_skipped_existing`, no duplicate edge.
//!   9. Namespace filter restricts via JOIN on
//!      `memories(insight_id).namespace`.
//!  10. Malformed `gate_scores` JSON is preserved as a string in
//!      attributes (not dropped, not crashed).
//!  11. NULL `source_original_importance` is omitted from attributes
//!      (not stored as a literal `null`).
//!  12. Counter invariant: rows_read = inserted + skipped_total + failed.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_memories_to_nodes, backfill_synthesis_provenance_to_edges,
};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn open_storage() -> (Storage, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let storage = Storage::new(&path).expect("open storage");
    (storage, dir)
}

fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: format!("content of {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "phase-c-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

/// Seed an `insight`/`source` memory pair. Strips the Phase B
/// dual-write nodes row so T25's FK guard fires until T19 runs.
fn seed_legacy_memory(storage: &mut Storage, id: &str, namespace: &str) {
    let rec = sample_record(id);
    storage.add(&rec, namespace).expect("Storage::add");
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params![id])
        .expect("strip nodes row");
}

/// Seed a legacy `synthesis_provenance` row directly via SQL.
#[allow(clippy::too_many_arguments)]
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
                id,
                insight_id,
                source_id,
                cluster_id,
                synthesis_timestamp,
                gate_decision,
                gate_scores,
                confidence,
                source_original_importance,
            ],
        )
        .expect("seed synthesis_provenance row");
}

/// Run T19 so T25's FK guard sees projected nodes for both endpoints.
fn run_node_prereq(storage: &mut Storage) {
    backfill_memories_to_nodes(storage, None).expect("T19");
}

// --------- Tests below ---------

#[test]
fn t25_single_row_writes_provenance_edge_with_legacy_id_passthrough() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-1", "default");
    seed_legacy_memory(&mut storage, "src-1", "default");
    seed_legacy_provenance(
        &storage,
        "prov-1",
        "ins-1",
        "src-1",
        "cluster-A",
        "2026-05-12T02:54:30.702859+00:00",
        "SYNTHESIZE",
        Some(r#"{"quality":0.45,"member_count":15}"#),
        0.82,
        Some(0.7),
    );
    run_node_prereq(&mut storage);

    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);

    // Critical: edge.id == legacy.id (pass-through).
    let (eid, kind, pred, src, tgt, conf, ns): (String, String, String, String, String, f64, String) =
        storage
            .conn()
            .query_row(
                "SELECT id, edge_kind, predicate, source_id, target_id, confidence, namespace \
                 FROM edges WHERE id = ?",
                params!["prov-1"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .expect("edge exists with legacy.id");
    assert_eq!(eid, "prov-1", "edge.id MUST equal legacy.id (no hash)");
    assert_eq!(kind, "provenance");
    assert_eq!(pred, "derived_from");
    assert_eq!(src, "ins-1", "source_id = insight_id (insight derived from source)");
    assert_eq!(tgt, "src-1");
    assert!((conf - 0.82).abs() < 1e-9, "FINDING-3: confidence passes through from legacy column, NOT hardcoded 1.0");
    assert_eq!(ns, "default", "namespace inherited from insight memory");
}

#[test]
fn t25_attributes_contain_gate_decision_and_parsed_gate_scores() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-2", "default");
    seed_legacy_memory(&mut storage, "src-2", "default");
    seed_legacy_provenance(
        &storage,
        "prov-2",
        "ins-2",
        "src-2",
        "cluster-B",
        "2026-05-12T02:54:30.702859+00:00",
        "SYNTHESIZE",
        Some(r#"{"quality":0.42,"type_diversity":3}"#),
        0.75,
        Some(0.6),
    );
    run_node_prereq(&mut storage);

    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let attrs_str: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = ?",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    let attrs: serde_json::Value = serde_json::from_str(&attrs_str).unwrap();
    assert_eq!(attrs["gate_decision"], "SYNTHESIZE");
    assert_eq!(attrs["cluster_id"], "cluster-B");
    assert_eq!(attrs["synthesis_timestamp"], "2026-05-12T02:54:30.702859+00:00");
    assert_eq!(attrs["source_original_importance"], 0.6);

    // gate_scores MUST be a nested object, NOT a quoted JSON-encoded string.
    assert!(attrs["gate_scores"].is_object(), "gate_scores parsed as nested JSON, not a string");
    assert!((attrs["gate_scores"]["quality"].as_f64().unwrap() - 0.42).abs() < 1e-9);
    assert_eq!(attrs["gate_scores"]["type_diversity"], 3);
}

#[test]
fn t25_dangling_endpoint_skipped_and_recovers_after_t19() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-3", "default");
    seed_legacy_memory(&mut storage, "src-3", "default");
    seed_legacy_provenance(
        &storage, "prov-3", "ins-3", "src-3", "cluster-C",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.5, None,
    );
    // Skip T19 — both endpoints are dangling.

    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 1, "dangling endpoint counts in skipped (via notes)");

    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id = ?", params!["prov-3"], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count, 0, "no edge written when endpoints missing");

    // Now run T19 → re-run T25 → recovery.
    run_node_prereq(&mut storage);
    let run2 = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run2.rows_inserted, 1, "T19 unblocks T25 on second run");

    let edge_count2: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id = ?", params!["prov-3"], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count2, 1);
}

#[test]
fn t25_idempotent_rerun_inserts_zero() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-4", "default");
    seed_legacy_memory(&mut storage, "src-4", "default");
    seed_legacy_provenance(
        &storage, "prov-4", "ins-4", "src-4", "cluster-D",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.6, None,
    );
    run_node_prereq(&mut storage);

    let run1 = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run1.rows_inserted, 1);

    let run2 = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run2.rows_read, 1);
    assert_eq!(run2.rows_inserted, 0, "second run is all-skipped");
    assert_eq!(run2.rows_skipped_existing, 1);

    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id = ?", params!["prov-4"], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count, 1, "still exactly one edge");
}

#[test]
fn t25_namespace_filter_restricts_via_insight_memory_namespace() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-A", "ns-a");
    seed_legacy_memory(&mut storage, "src-A", "ns-a");
    seed_legacy_memory(&mut storage, "ins-B", "ns-b");
    seed_legacy_memory(&mut storage, "src-B", "ns-b");
    seed_legacy_provenance(
        &storage, "prov-A", "ins-A", "src-A", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.8, None,
    );
    seed_legacy_provenance(
        &storage, "prov-B", "ins-B", "src-B", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.8, None,
    );
    run_node_prereq(&mut storage);

    let run = backfill_synthesis_provenance_to_edges(&mut storage, Some("ns-a")).unwrap();
    assert_eq!(run.rows_read, 1, "only ns-a's row is iterated");
    assert_eq!(run.rows_inserted, 1);

    let prov_a_ns: String = storage
        .conn()
        .query_row("SELECT namespace FROM edges WHERE id = ?", params!["prov-A"], |r| r.get(0))
        .unwrap();
    assert_eq!(prov_a_ns, "ns-a");

    let prov_b_exists: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id = ?", params!["prov-B"], |r| r.get(0))
        .unwrap();
    assert_eq!(prov_b_exists, 0, "ns-b not iterated; no edge");
}

#[test]
fn t25_confidence_passes_through_distinct_legacy_values() {
    // FINDING-3 cornerstone: legacy.confidence wins, NOT hardcoded 1.0.
    // Seed three rows with three different confidences; assert each
    // edge has its own legacy value.
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-x", "default");
    seed_legacy_memory(&mut storage, "src-p", "default");
    seed_legacy_memory(&mut storage, "src-q", "default");
    seed_legacy_memory(&mut storage, "src-r", "default");
    seed_legacy_provenance(
        &storage, "prov-p", "ins-x", "src-p", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.95, None,
    );
    seed_legacy_provenance(
        &storage, "prov-q", "ins-x", "src-q", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.50, None,
    );
    seed_legacy_provenance(
        &storage, "prov-r", "ins-x", "src-r", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.12, None,
    );
    run_node_prereq(&mut storage);
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let fetch_conf = |id: &str| -> f64 {
        storage
            .conn()
            .query_row(
                "SELECT confidence FROM edges WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!((fetch_conf("prov-p") - 0.95).abs() < 1e-9);
    assert!((fetch_conf("prov-q") - 0.50).abs() < 1e-9);
    assert!((fetch_conf("prov-r") - 0.12).abs() < 1e-9);
}

#[test]
fn t25_malformed_gate_scores_preserved_as_string() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-m", "default");
    seed_legacy_memory(&mut storage, "src-m", "default");
    seed_legacy_provenance(
        &storage, "prov-m", "ins-m", "src-m", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE",
        Some("{this is not json"), 0.8, None,
    );
    run_node_prereq(&mut storage);

    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run.rows_inserted, 1, "malformed gate_scores must not crash the driver");

    let attrs_str: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = ?",
            params!["prov-m"],
            |r| r.get(0),
        )
        .unwrap();
    let attrs: serde_json::Value = serde_json::from_str(&attrs_str).unwrap();
    // Preserved as a string so the operator can debug.
    assert!(attrs["gate_scores"].is_string());
    assert_eq!(attrs["gate_scores"].as_str().unwrap(), "{this is not json");
}

#[test]
fn t25_null_source_original_importance_omitted_from_attributes() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-n", "default");
    seed_legacy_memory(&mut storage, "src-n", "default");
    seed_legacy_provenance(
        &storage, "prov-n", "ins-n", "src-n", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.5, None,
    );
    run_node_prereq(&mut storage);
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let attrs_str: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = ?",
            params!["prov-n"],
            |r| r.get(0),
        )
        .unwrap();
    let attrs: serde_json::Value = serde_json::from_str(&attrs_str).unwrap();
    assert!(attrs.get("source_original_importance").is_none(),
            "NULL legacy column → key absent, not stored as JSON null");
}

#[test]
fn t25_empty_table_no_op() {
    let (mut storage, _dir) = open_storage();
    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 0);

    let count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE edge_kind = 'provenance'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn t25_synthesis_timestamp_parsed_to_recorded_at_epoch() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-t", "default");
    seed_legacy_memory(&mut storage, "src-t", "default");
    // RFC3339 → expected epoch:
    //   2026-05-12T02:54:30.702859+00:00 → 1778554470.702859
    let ts = "2026-05-12T02:54:30.702859+00:00";
    let expected_epoch = chrono::DateTime::parse_from_rfc3339(ts)
        .unwrap()
        .with_timezone(&Utc);
    let expected_f64 = expected_epoch.timestamp() as f64
        + (expected_epoch.timestamp_subsec_nanos() as f64 / 1e9);

    seed_legacy_provenance(
        &storage, "prov-t", "ins-t", "src-t", "cl",
        ts, "SYNTHESIZE", None, 0.6, None,
    );
    run_node_prereq(&mut storage);
    backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let (recorded, created, updated): (f64, f64, f64) = storage
        .conn()
        .query_row(
            "SELECT recorded_at, created_at, updated_at FROM edges WHERE id = ?",
            params!["prov-t"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!((recorded - expected_f64).abs() < 1e-3,
            "recorded_at = parsed synthesis_timestamp ({expected_f64} expected, got {recorded})");
    assert!((created - expected_f64).abs() < 1e-3);
    assert!((updated - expected_f64).abs() < 1e-3);
}

#[test]
fn t25_audit_row_records_run_outcome() {
    let (mut storage, _dir) = open_storage();
    seed_legacy_memory(&mut storage, "ins-a", "default");
    seed_legacy_memory(&mut storage, "src-a", "default");
    seed_legacy_provenance(
        &storage, "prov-a", "ins-a", "src-a", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.7, None,
    );
    run_node_prereq(&mut storage);
    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();

    let (table, read, inserted, skipped, failed, notes): (
        String, i64, i64, i64, i64, String,
    ) = storage
        .conn()
        .query_row(
            "SELECT legacy_table, rows_read, rows_inserted, \
             rows_skipped_existing, rows_failed, notes \
             FROM backfill_runs WHERE run_id = ?",
            params![run.run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(table, "synthesis_provenance");
    assert_eq!(read, 1);
    assert_eq!(inserted, 1);
    assert_eq!(skipped, 0);
    assert_eq!(failed, 0);

    let notes_val: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(notes_val["driver"], "backfill_synthesis_provenance_to_edges");
    assert_eq!(notes_val["confidence_policy"], "legacy.confidence pass-through (FINDING-3 reference impl)");
}

#[test]
fn t25_counter_invariant_holds_with_mixed_outcomes() {
    let (mut storage, _dir) = open_storage();
    // One ok row + one dangling row → rows_read=2, inserted=1, skipped=1.
    seed_legacy_memory(&mut storage, "ins-ok", "default");
    seed_legacy_memory(&mut storage, "src-ok", "default");
    seed_legacy_memory(&mut storage, "ins-bad", "default");
    seed_legacy_memory(&mut storage, "src-bad", "default");

    seed_legacy_provenance(
        &storage, "prov-ok", "ins-ok", "src-ok", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.7, None,
    );
    seed_legacy_provenance(
        &storage, "prov-bad", "ins-bad", "src-bad", "cl",
        "2026-05-12T00:00:00+00:00", "SYNTHESIZE", None, 0.7, None,
    );
    // Project nodes only for the OK pair.
    run_node_prereq(&mut storage);
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id IN ('ins-bad', 'src-bad')", [])
        .unwrap();

    let run = backfill_synthesis_provenance_to_edges(&mut storage, None).unwrap();
    // assert_counter_invariant() runs inside the driver and would
    // panic if the buckets didn't sum to rows_read. Verify the
    // returned values explicitly.
    assert_eq!(run.rows_read, 2);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 1, "dangling row + zero pre-existing collapse into skipped bucket");
    assert_eq!(run.rows_failed, 0);
    assert_eq!(run.rows_inserted + run.rows_skipped_existing + run.rows_failed, run.rows_read);
}
