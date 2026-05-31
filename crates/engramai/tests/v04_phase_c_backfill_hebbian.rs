//! T24 — Phase C backfill driver: hebbian_links → edges(associative).
//!
//! Per design.md §3.3 + §4.3 + §5.3 (amended 2026-05-13), every legacy
//! `hebbian_links` row maps to one `edges` row with:
//!
//!   * `edge_kind = 'associative'`
//!   * `predicate = 'co_activated'`
//!   * Canonicalized `(source_id, target_id) = (min, max)`
//!   * `weight = SUM(strength)` per `(canonical_pair, namespace,
//!     signal_source)` group
//!   * Signal/temporal payload packed into `edges.attributes` JSON:
//!     `signal_source`, `signal_detail`, `coactivation_count`,
//!     `temporal_forward`, `temporal_backward`, `direction`
//!
//! Acceptance contract:
//!
//!   1. Single legacy row writes one canonicalized edge with all
//!      signal/temporal fields preserved.
//!   2. Two legacy rows for the same pair in opposite directions
//!      (`(A,B)` + `(B,A)`) merge into ONE edge whose weight is the
//!      sum and whose coactivation_count is the sum.
//!   3. Two legacy rows for the same pair with DIFFERENT signal_source
//!      produce TWO edges (signal_source is row-identity per §4.3).
//!   4. Endpoints missing in `nodes` are SKIPPED, counted as
//!      `rows_skipped_dangling_endpoint`. Recovery: run T19, re-run T24.
//!   5. Idempotent rerun: second invocation inserts zero rows.
//!   6. `namespace` and `created_at` come from the LEGACY ROW (the
//!      table has its own columns; no JOIN required).
//!   7. `created_at` for merged groups is the MIN across legacy rows
//!      (earliest observation wins).
//!   8. Deterministic edge id: byte-identical across re-runs and
//!      across direction orderings.
//!   9. Namespace filter restricts to legacy rows with that namespace
//!      (one-step filter, not via JOIN).
//!  10. Counter invariant `rows_read = inserted + skipped + failed`
//!      holds (asserted internally).
//!  11. Heterogeneous `direction` values across merged rows are packed
//!      as sorted JSON array in attributes.direction; homogeneous
//!      keeps the scalar string form.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_hebbian_links_to_edges, backfill_memories_to_nodes,
};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use serde_json::Value;
use tempfile::tempdir;

// ---------------------------------------------------------------------
// Seeding helpers.
// ---------------------------------------------------------------------

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

fn seed_legacy_memory(storage: &mut Storage, id: &str, namespace: &str) {
    let rec = sample_record(id);
    storage.add(&rec, namespace).expect("Storage::add");
    // ISS-199: do NOT strip the Phase B dual-written `nodes` row.
    // `hebbian_links.{source_id,target_id}` now FK→`nodes(id)` (re-pointed
    // by `migrate_hebbian_links_fk_to_nodes`) — the correct target, since
    // `record_coactivation*` writes links between memories that already
    // exist in `nodes`. Stripping the node would FK-787 at `seed_link`
    // time. The memory node is therefore always present; T19 in
    // `run_node_prereqs` is idempotent. The dangling-endpoint scenario is
    // seeded with FK enforcement OFF (legacy-row simulation) — see
    // `t24_dangling_endpoint_skipped_and_recovers_after_t19`.
}

/// Seed a Hebbian link row at the legacy table layer. Caller controls
/// the direction (source vs target order) so we can construct the
/// `(A,B) + (B,A)` collision scenario explicitly.
#[allow(clippy::too_many_arguments)]
fn seed_link(
    storage: &Storage,
    source_id: &str,
    target_id: &str,
    strength: f64,
    coact_count: i64,
    temporal_fwd: i64,
    temporal_bwd: i64,
    direction: &str,
    created_at: f64,
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
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)"#,
            params![
                source_id,
                target_id,
                strength,
                coact_count,
                temporal_fwd,
                temporal_bwd,
                direction,
                created_at,
                namespace,
                signal_source,
            ],
        )
        .expect("seed hebbian_links row");
}

fn run_node_prereqs(storage: &mut Storage) {
    backfill_memories_to_nodes(storage, None).expect("T19");
}

fn read_edge_attrs(storage: &Storage, id: &str) -> Value {
    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = ?",
            params![id],
            |r| r.get(0),
        )
        .expect("edge exists");
    serde_json::from_str(&attrs).expect("valid JSON")
}

fn read_edge(storage: &Storage, id: &str) -> (String, String, String, String, f64, String, f64) {
    storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate, source_id, target_id, weight, namespace, recorded_at \
             FROM edges WHERE id = ?",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, f64>(6)?,
                ))
            },
        )
        .expect("edge exists")
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[test]
fn t24_single_row_writes_canonicalized_edge_with_all_fields() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    // Note: seed with (B, A) — the LARGER id first — to verify
    // canonicalization. The edge should still have source=mem-A,
    // target=mem-B because lexically mem-A < mem-B.
    seed_link(
        &storage, "mem-B", "mem-A", 0.5, 3, 1, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    let run = backfill_hebbian_links_to_edges(&mut storage, None).expect("T24");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);

    let id: String = storage
        .conn()
        .query_row(
            "SELECT id FROM edges WHERE edge_kind = 'associative'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (kind, predicate, source, target, weight, ns, recorded) = read_edge(&storage, &id);
    assert_eq!(kind, "associative");
    assert_eq!(predicate, "co_activated");
    assert_eq!(source, "mem-A", "source = lexical min of pair");
    assert_eq!(target, "mem-B", "target = lexical max of pair");
    assert!((weight - 0.5).abs() < 1e-9);
    assert_eq!(ns, "default");
    assert!((recorded - 1_700_000_000.0).abs() < 1e-6);

    let attrs = read_edge_attrs(&storage, &id);
    assert_eq!(attrs["signal_source"], "corecall");
    assert_eq!(attrs["coactivation_count"], 3);
    assert_eq!(attrs["temporal_forward"], 1);
    assert_eq!(attrs["temporal_backward"], 0);
    assert_eq!(attrs["direction"], "bidirectional");
    assert_eq!(attrs["signal_detail"], "");
}

#[test]
fn t24_collision_pair_merges_with_sum_semantics() {
    // The 119-pair production reality: same canonical pair has both
    // (A,B) and (B,A) legacy rows, same signal_source. They MUST
    // merge into one edge whose weight/count/temporal counters are
    // the SUM of the inputs and whose recorded_at is the MIN.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    // Earlier observation, weaker strength
    seed_link(
        &storage, "mem-A", "mem-B", 0.3, 2, 1, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    // Later observation, stronger strength
    seed_link(
        &storage, "mem-B", "mem-A", 0.7, 5, 0, 3, "bidirectional",
        1_700_000_500.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    let run = backfill_hebbian_links_to_edges(&mut storage, None).expect("T24");
    assert_eq!(run.rows_read, 2);
    assert_eq!(
        run.rows_inserted, 2,
        "rows_inserted counts LEGACY rows collapsed, not edges produced"
    );
    assert_eq!(run.rows_skipped_existing, 0);

    // Exactly ONE edge in the unified table.
    let edge_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_kind = 'associative'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(edge_count, 1, "two legacy rows merged into one unified edge");

    let id: String = storage
        .conn()
        .query_row(
            "SELECT id FROM edges WHERE edge_kind = 'associative'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (_, _, source, target, weight, _, recorded) = read_edge(&storage, &id);
    assert_eq!(source, "mem-A");
    assert_eq!(target, "mem-B");
    assert!(
        (weight - 1.0).abs() < 1e-9,
        "weight = SUM(strength) = 0.3 + 0.7"
    );
    assert!(
        (recorded - 1_700_000_000.0).abs() < 1e-6,
        "recorded_at = MIN(created_at) — earliest observation wins"
    );

    let attrs = read_edge_attrs(&storage, &id);
    assert_eq!(attrs["coactivation_count"], 7, "2 + 5");
    assert_eq!(attrs["temporal_forward"], 1, "1 + 0");
    assert_eq!(attrs["temporal_backward"], 3, "0 + 3");
    assert_eq!(attrs["direction"], "bidirectional", "homogeneous → scalar");
}

#[test]
fn t24_different_signal_source_produces_separate_edges() {
    // §4.3 row-identity contract: distinct signal_source → distinct
    // edges, even for the same canonical pair. Note: the LEGACY
    // table's PK is `(source_id, target_id)` and does NOT include
    // signal_source, so we can't seed two rows of (A,B) with
    // different signal_source directly. We approximate the scenario
    // by seeding opposite-direction rows with different
    // signal_source values — the canonicalization merges the pair
    // BUT the signal_source dimension keeps them as distinct edges.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.4, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    seed_link(
        &storage, "mem-B", "mem-A", 0.6, 2, 0, 0, "bidirectional",
        1_700_000_100.0, "default", "multi",
    );
    run_node_prereqs(&mut storage);

    let run = backfill_hebbian_links_to_edges(&mut storage, None).expect("T24");
    assert_eq!(run.rows_inserted, 2);

    let edge_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_kind = 'associative'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(edge_count, 2, "two signal sources → two edges");

    let signal_sources: Vec<String> = {
        let conn = storage.conn();
        let mut stmt = conn
            .prepare(
                "SELECT json_extract(attributes, '$.signal_source') \
                 FROM edges WHERE edge_kind = 'associative' \
                 ORDER BY json_extract(attributes, '$.signal_source')",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(signal_sources, vec!["corecall".to_string(), "multi".into()]);
}

#[test]
fn t24_dangling_endpoint_skipped_and_recovers_after_t19() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );

    // ISS-199: `hebbian_links.{source,target}` now FK→`nodes(id)`, so a
    // link cannot be inserted against a missing node. To exercise the
    // backfill's defensive dangling-endpoint skip we simulate a legacy
    // row: drop the node WITH FK enforcement OFF, leaving the link
    // pointing at an absent node (exactly the state a row written under
    // the old `memories(id)` FK lands in before T19 lifts the node).
    storage
        .conn()
        .execute_batch(
            "PRAGMA foreign_keys=OFF; \
             DELETE FROM nodes; \
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();

    let run1 = backfill_hebbian_links_to_edges(&mut storage, None).expect("T24");
    assert_eq!(run1.rows_read, 1);
    assert_eq!(run1.rows_inserted, 0);
    assert_eq!(run1.rows_skipped_existing, 1, "1 dangling, 0 existing");
    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count, 0);

    // Now run T19 → both nodes appear → re-run T24 → edge inserts.
    run_node_prereqs(&mut storage);
    let run2 = backfill_hebbian_links_to_edges(&mut storage, None).expect("T24 retry");
    assert_eq!(run2.rows_inserted, 1, "recovery after T19");
    assert_eq!(run2.rows_skipped_existing, 0);
}

#[test]
fn t24_idempotent_rerun_inserts_zero() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    let r1 = backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    assert_eq!(r1.rows_inserted, 1);

    let r2 = backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    assert_eq!(r2.rows_inserted, 0, "rerun is no-op");
    assert_eq!(r2.rows_skipped_existing, 1);

    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count, 1);
}

#[test]
fn t24_namespace_filter_restricts_to_matching_legacy_rows() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "ns-a");
    seed_legacy_memory(&mut storage, "mem-B", "ns-a");
    seed_legacy_memory(&mut storage, "mem-C", "ns-b");
    seed_legacy_memory(&mut storage, "mem-D", "ns-b");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "ns-a", "corecall",
    );
    seed_link(
        &storage, "mem-C", "mem-D", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "ns-b", "corecall",
    );
    run_node_prereqs(&mut storage);

    let run_a = backfill_hebbian_links_to_edges(&mut storage, Some("ns-a")).unwrap();
    assert_eq!(run_a.rows_read, 1, "filter restricts at SELECT");
    assert_eq!(run_a.rows_inserted, 1);

    let edge_ns: String = storage
        .conn()
        .query_row("SELECT namespace FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_ns, "ns-a");

    // ns-b run picks up the other row, untouched by the first pass.
    let run_b = backfill_hebbian_links_to_edges(&mut storage, Some("ns-b")).unwrap();
    assert_eq!(run_b.rows_inserted, 1);
}

#[test]
fn t24_deterministic_id_byte_identical_across_runs() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    let id1: String = storage
        .conn()
        .query_row("SELECT id FROM edges WHERE edge_kind='associative'", [], |r| r.get(0))
        .unwrap();

    // Wipe edges and re-run; id should match.
    storage.conn().execute("DELETE FROM edges", []).unwrap();
    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    let id2: String = storage
        .conn()
        .query_row("SELECT id FROM edges WHERE edge_kind='associative'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(id1, id2, "edges.id is deterministic across runs");
}

#[test]
fn t24_direction_array_for_heterogeneous_merge() {
    // Most production rows are 'bidirectional', but if a future
    // signal ever writes 'directed' rows, the merge MUST surface
    // both values. Pack as a sorted JSON array.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    seed_link(
        &storage, "mem-B", "mem-A", 0.5, 1, 0, 0, "directed",
        1_700_000_100.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    let id: String = storage
        .conn()
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();
    let attrs = read_edge_attrs(&storage, &id);
    assert!(
        attrs["direction"].is_array(),
        "heterogeneous directions packed as array, got {:?}",
        attrs["direction"]
    );
    let arr = attrs["direction"].as_array().unwrap();
    let mut strs: Vec<String> = arr
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    strs.sort();
    assert_eq!(strs, vec!["bidirectional".to_string(), "directed".into()]);
}

#[test]
fn t24_empty_table_no_op() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let run = backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 0);
}

#[test]
fn t24_audit_notes_capture_merged_collision_pairs() {
    // The most diagnostic stat: how many canonical pairs had both
    // (A,B) AND (B,A) rows. Must be surfaced in backfill_runs.notes
    // because it tells the operator whether the merge policy fired.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_legacy_memory(&mut storage, "mem-C", "default");
    seed_legacy_memory(&mut storage, "mem-D", "default");
    // Pair (A,B) has both directions — a collision.
    seed_link(
        &storage, "mem-A", "mem-B", 0.3, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    seed_link(
        &storage, "mem-B", "mem-A", 0.3, 1, 0, 0, "bidirectional",
        1_700_000_100.0, "default", "corecall",
    );
    // Pair (C,D) is one-directional only.
    seed_link(
        &storage, "mem-C", "mem-D", 0.3, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();

    let notes_json: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE legacy_table = 'hebbian_links' \
             ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let notes: Value = serde_json::from_str(&notes_json).unwrap();
    assert_eq!(
        notes["merged_collision_pairs"], 1,
        "exactly one canonical pair (A,B) had collisions"
    );
}

#[test]
fn t24_canonicalization_collapses_direction_to_same_id() {
    // Direct id-level proof of the canonicalization invariant.
    // Seed (B,A); separately compute what (A,B) would have hashed
    // to by running on a parallel DB; assert the ids match.
    let tmp1 = tempdir().unwrap();
    let mut s1 = Storage::new(tmp1.path().join("engram.db")).unwrap();
    seed_legacy_memory(&mut s1, "mem-A", "default");
    seed_legacy_memory(&mut s1, "mem-B", "default");
    seed_link(
        &s1, "mem-B", "mem-A", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut s1);
    backfill_hebbian_links_to_edges(&mut s1, None).unwrap();
    let id1: String = s1
        .conn()
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();

    let tmp2 = tempdir().unwrap();
    let mut s2 = Storage::new(tmp2.path().join("engram.db")).unwrap();
    seed_legacy_memory(&mut s2, "mem-A", "default");
    seed_legacy_memory(&mut s2, "mem-B", "default");
    seed_link(
        &s2, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut s2);
    backfill_hebbian_links_to_edges(&mut s2, None).unwrap();
    let id2: String = s2
        .conn()
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();

    assert_eq!(id1, id2, "(A,B) and (B,A) hash to the SAME canonical id");
}

#[test]
fn t24_temporal_counters_summed_and_preserved() {
    // §4.6 differential decay depends on these counters; the
    // backfill MUST NOT silently drop or zero them. Verify the
    // production-shape case: ~606 rows have non-zero temporal_fwd.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 2, 1, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    let id: String = storage
        .conn()
        .query_row("SELECT id FROM edges", [], |r| r.get(0))
        .unwrap();
    let attrs = read_edge_attrs(&storage, &id);
    assert_eq!(attrs["temporal_forward"], 2);
    assert_eq!(attrs["temporal_backward"], 1);
}

#[test]
fn t24_partial_unique_index_satisfied_post_merge() {
    // Sanity check: post-backfill, the partial unique index
    // idx_edges_assoc_unique should not be violated (would have
    // panicked the test anyway, but assert structurally).
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_memory(&mut storage, "mem-A", "default");
    seed_legacy_memory(&mut storage, "mem-B", "default");
    seed_legacy_memory(&mut storage, "mem-C", "default");
    seed_link(
        &storage, "mem-A", "mem-B", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    seed_link(
        &storage, "mem-A", "mem-C", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    seed_link(
        &storage, "mem-B", "mem-C", 0.5, 1, 0, 0, "bidirectional",
        1_700_000_000.0, "default", "corecall",
    );
    run_node_prereqs(&mut storage);

    backfill_hebbian_links_to_edges(&mut storage, None).unwrap();
    let count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(DISTINCT id) FROM edges WHERE edge_kind = 'associative'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "three distinct canonical pairs → three edges");
}
