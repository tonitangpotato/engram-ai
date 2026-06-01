//! T22 — Phase C backfill driver: entity_relations → edges(kind=structural).
//!
//! Acceptance per design.md §5.3 + project rules from T19/T20/T21:
//!
//!   1. Every legacy `entity_relations` row gets a matching
//!      `edges(edge_kind='structural')` row, with both endpoints
//!      resolving to `nodes(id)`.
//!   2. `entity_relations.relation → edges.predicate` (literal).
//!   3. `entity_relations.source` (free text) lands as
//!      `attributes.source`. `entity_relations.metadata` (JSON
//!      object) is merged into attributes with **existing-wins**:
//!      a metadata key named "source" cannot overwrite the column.
//!   4. Endpoints missing in `nodes` → SKIPPED (not failed) and
//!      counted in audit notes as `rows_skipped_dangling_endpoint`.
//!      Recovery: run T21, then re-run T22.
//!   5. Pre-existing structural edge with same id → Pass 2 merges
//!      legacy attributes in (existing-wins).
//!   6. Pre-existing non-structural edge with same id (assertion,
//!      associative, provenance) → refuse to merge, count in
//!      `rows_skipped_kind_mismatch` (ISS-112 §E renamed from
//!      `rows_existing_kind_mismatch` to signal subset relation to
//!      `rows_skipped_existing`).
//!   7. Idempotent rerun.
//!   8. Namespace filter respected.

use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_entities_to_nodes, backfill_entity_relations_to_edges,
};
use rusqlite::params;
use tempfile::tempdir;

/// Seed a legacy `entities` row so the FK guard in T22 has
/// something to point to. Returns nothing — we only care that the
/// id ends up in `nodes` once T21 has run.
fn seed_legacy_entity(storage: &Storage, id: &str, name: &str, namespace: &str) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
               VALUES (?, ?, 'person', ?, NULL, 1700000000.0, 1700000000.0)"#,
            params![id, name, namespace],
        )
        .expect("seed entity");
}

fn seed_legacy_relation(
    storage: &Storage,
    id: &str,
    source_id: &str,
    target_id: &str,
    relation: &str,
    confidence: f64,
    source_text: Option<&str>,
    namespace: &str,
    metadata: Option<&str>,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entity_relations
               (id, source_id, target_id, relation, confidence, source, namespace, created_at, metadata)
               VALUES (?, ?, ?, ?, ?, ?, ?, 1700000000.0, ?)"#,
            params![id, source_id, target_id, relation, confidence, source_text, namespace, metadata],
        )
        .expect("seed entity_relation");
}

/// Project the legacy entities into nodes so T22's FK guard
/// passes. Helper for tests that aren't testing the guard itself.
fn run_t21(storage: &mut Storage) {
    backfill_entities_to_nodes(storage, None).expect("T21 to project entities into nodes");
}

#[test]
fn t22_backfill_projects_legacy_relation_into_edges() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_entity(&storage, "ent-a", "Alice", "default");
    seed_legacy_entity(&storage, "ent-b", "Bob", "default");
    run_t21(&mut storage);

    seed_legacy_relation(
        &storage,
        "rel-1",
        "ent-a",
        "ent-b",
        "knows",
        0.8,
        Some("manual"),
        "default",
        Some(r#"{"extracted_from":"convo-42"}"#),
    );

    let run = backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);

    let (kind, predicate_kind, predicate, source, target, conf, attrs, ns): (
        String,
        String,
        String,
        String,
        String,
        f64,
        String,
        String,
    ) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate_kind, predicate, source_id, target_id, \
             confidence, attributes, namespace FROM edges WHERE id = 'rel-1'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(kind, "structural");
    assert_eq!(predicate_kind, "canonical");
    assert_eq!(predicate, "knows");
    assert_eq!(source, "ent-a");
    assert_eq!(target, "ent-b");
    assert!((conf - 0.8).abs() < 1e-9);
    assert_eq!(ns, "default");

    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["source"], "manual");
    assert_eq!(parsed["extracted_from"], "convo-42");
}

#[test]
fn t22_column_source_wins_over_metadata_source() {
    // Same lesson as T21 FINDING-1: when a metadata key collides
    // with a column-seeded attribute key, the COLUMN value wins.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);

    seed_legacy_relation(
        &storage,
        "rel-shadow",
        "ent-a",
        "ent-b",
        "knows",
        0.9,
        Some("from_column"),
        "default",
        Some(r#"{"source":"from_metadata","other":"keep_me"}"#),
    );

    backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id = 'rel-shadow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        parsed["source"], "from_column",
        "column.source seeded first wins over metadata.source"
    );
    assert_eq!(parsed["other"], "keep_me");
}

#[test]
fn t22_dangling_endpoint_skipped_then_recovered() {
    // The defining feature of T22 over T21: FK to nodes(id) on
    // both endpoints. If T21 hasn't been run, the endpoints don't
    // exist; T22 must skip rather than fail.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    // Note: do NOT run T21 yet.
    seed_legacy_relation(
        &storage, "rel-1", "ent-a", "ent-b", "knows", 1.0, None, "default", None,
    );

    let first = backfill_entity_relations_to_edges(&mut storage, None).expect("first run");
    assert_eq!(first.rows_read, 1);
    assert_eq!(first.rows_inserted, 0);
    // dangling_endpoint folds into rows_skipped_existing for the tally
    assert_eq!(first.rows_skipped_existing, 1);

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&first.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed_notes: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed_notes["rows_skipped_dangling_endpoint"], 1);

    // Edges table is still empty.
    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id='rel-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(edge_count, 0);

    // Now run T21 and re-run T22 — should land.
    run_t21(&mut storage);
    let second = backfill_entity_relations_to_edges(&mut storage, None).expect("second run");
    assert_eq!(second.rows_inserted, 1);

    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id='rel-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(edge_count, 1);
}

#[test]
fn t22_pre_existing_structural_edge_pass2_merges() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);

    // Plant a pre-existing structural edge with rich attributes —
    // simulates a hypothetical Phase B path that wrote it first.
    storage
        .conn()
        .execute(
            r#"INSERT INTO edges (id, source_id, target_id, edge_kind, predicate_kind, predicate,
                                  attributes, confidence, recorded_at, namespace, created_at, updated_at)
               VALUES ('rel-collide', 'ent-a', 'ent-b', 'structural', 'canonical', 'knows',
                       '{"source":"canonical_source","extra":"canonical_extra"}', 0.95,
                       1700000000.0, 'default', 1700000000.0, 1700000000.0)"#,
            [],
        )
        .unwrap();

    seed_legacy_relation(
        &storage,
        "rel-collide",
        "ent-a",
        "ent-b",
        "knows",
        0.5,
        Some("legacy_source"),
        "default",
        Some(r#"{"new_key":"legacy_value"}"#),
    );

    let run = backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");
    assert_eq!(
        run.rows_inserted, 0,
        "INSERT OR IGNORE no-op on existing id"
    );

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id='rel-collide'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    // existing-wins: canonical source stays
    assert_eq!(parsed["source"], "canonical_source");
    assert_eq!(parsed["extra"], "canonical_extra");
    // new key from legacy merged in
    assert_eq!(parsed["new_key"], "legacy_value");

    // confidence should NOT have been touched — Pass 2 only merges
    // attributes; canonical confidence stays at 0.95
    let conf: f64 = storage
        .conn()
        .query_row(
            "SELECT confidence FROM edges WHERE id='rel-collide'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!((conf - 0.95).abs() < 1e-9);
}

#[test]
fn t22_pre_existing_non_structural_edge_is_not_touched() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);

    // Plant an associative edge with the same id we'll seed as a
    // structural entity_relation. T22 Pass 2 must NOT merge.
    storage
        .conn()
        .execute(
            r#"INSERT INTO edges (id, source_id, target_id, edge_kind, predicate_kind, predicate,
                                  attributes, recorded_at, namespace, created_at, updated_at)
               VALUES ('collide-id', 'ent-a', 'ent-b', 'associative', 'canonical', 'co_activated',
                       '{"signal_source":"hebbian"}', 1700000000.0, 'default', 1700000000.0, 1700000000.0)"#,
            [],
        )
        .unwrap();

    seed_legacy_relation(
        &storage,
        "collide-id",
        "ent-a",
        "ent-b",
        "knows",
        0.5,
        None,
        "default",
        Some(r#"{"should_not_merge":"value"}"#),
    );

    let run = backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");
    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed_notes: serde_json::Value = serde_json::from_str(&notes).unwrap();
    // ISS-112 §E: counter renamed to `rows_skipped_kind_mismatch`
    // to signal it's a subset of `rows_skipped_existing`.
    assert_eq!(parsed_notes["rows_skipped_kind_mismatch"], 1);

    // associative edge attributes untouched
    let (kind, attrs): (String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, attributes FROM edges WHERE id='collide-id'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "associative");
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["signal_source"], "hebbian");
    assert!(parsed.get("should_not_merge").is_none());
}

#[test]
fn t22_idempotent_rerun_keeps_attributes_stable() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);
    seed_legacy_relation(
        &storage,
        "rel-idem",
        "ent-a",
        "ent-b",
        "knows",
        1.0,
        Some("s"),
        "default",
        Some(r#"{"k1":"v1"}"#),
    );

    backfill_entity_relations_to_edges(&mut storage, None).expect("first");
    let attrs1: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id='rel-idem'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let r2 = backfill_entity_relations_to_edges(&mut storage, None).expect("second");
    assert_eq!(r2.rows_inserted, 0);
    let attrs2: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id='rel-idem'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let p1: serde_json::Value = serde_json::from_str(&attrs1).unwrap();
    let p2: serde_json::Value = serde_json::from_str(&attrs2).unwrap();
    assert_eq!(p1, p2);
}

#[test]
fn t22_namespace_filter() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a-ns-a", "A", "ns-a");
    seed_legacy_entity(&storage, "ent-b-ns-a", "B", "ns-a");
    seed_legacy_entity(&storage, "ent-a-ns-b", "A", "ns-b");
    seed_legacy_entity(&storage, "ent-b-ns-b", "B", "ns-b");
    run_t21(&mut storage);

    seed_legacy_relation(
        &storage,
        "rel-a",
        "ent-a-ns-a",
        "ent-b-ns-a",
        "knows",
        1.0,
        None,
        "ns-a",
        None,
    );
    seed_legacy_relation(
        &storage,
        "rel-b",
        "ent-a-ns-b",
        "ent-b-ns-b",
        "knows",
        1.0,
        None,
        "ns-b",
        None,
    );

    let run = backfill_entity_relations_to_edges(&mut storage, Some("ns-a")).unwrap();
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);

    let a_present: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id='rel-a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let b_present: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges WHERE id='rel-b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(a_present, 1);
    assert_eq!(b_present, 0);
}

#[test]
fn t22_empty_table_completes_cleanly() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let run = backfill_entity_relations_to_edges(&mut storage, None).expect("empty");
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
}

#[test]
fn t22_malformed_metadata_does_not_fail_row() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);
    seed_legacy_relation(
        &storage,
        "rel-bad",
        "ent-a",
        "ent-b",
        "knows",
        1.0,
        Some("manual"),
        "default",
        Some("not-a-json-object"),
    );

    let run = backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_inserted, 1);

    let attrs: String = storage
        .conn()
        .query_row("SELECT attributes FROM edges WHERE id='rel-bad'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    // only the column-seeded `source` key, no merged data
    assert_eq!(parsed["source"], "manual");

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed_notes: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed_notes["rows_malformed_metadata"], 1);
}

#[test]
fn t22_null_source_and_null_metadata_yield_empty_attributes() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);
    seed_legacy_relation(
        &storage, "rel-null", "ent-a", "ent-b", "knows", 1.0, None, "default", None,
    );

    backfill_entity_relations_to_edges(&mut storage, None).expect("backfill");

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM edges WHERE id='rel-null'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    let obj = parsed.as_object().unwrap();
    assert!(
        obj.is_empty(),
        "NULL source + NULL metadata should yield empty attributes object, got {:?}",
        obj
    );
}

// ---------------------------------------------------------------------------
// ISS-112 §B cross-driver test-gap audit applied to T22 (entity_relations).
//
// Mirrors the §B-#1 (mutated-metadata rerun, existing-wins merge) pattern
// already covered for T21 in `iss112_d_idempotent_rerun_does_not_bump_updated_at`
// and related tests. T22 has full Pass-2 merge via
// `merge_attributes_existing_wins` (backfill.rs:1300) so the mutated-metadata
// contract applies: re-running with new metadata keys must add the new keys
// without disturbing existing ones, and existing keys must NOT be overwritten
// even when the legacy metadata has a different value.
// ---------------------------------------------------------------------------

#[test]
fn iss112_b_t22_mutated_metadata_rerun_existing_wins() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "default");
    seed_legacy_entity(&storage, "ent-b", "B", "default");
    run_t21(&mut storage);

    // Initial seed: legacy metadata declares `k1=v1`.
    seed_legacy_relation(
        &storage,
        "rel-mut",
        "ent-a",
        "ent-b",
        "knows",
        1.0,
        Some("s"),
        "default",
        Some(r#"{"k1":"v1"}"#),
    );

    let r1 = backfill_entity_relations_to_edges(&mut storage, None).expect("first run");
    assert_eq!(r1.rows_inserted, 1);

    let attrs1: String = storage
        .conn()
        .query_row("SELECT attributes FROM edges WHERE id='rel-mut'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let parsed1: serde_json::Value = serde_json::from_str(&attrs1).unwrap();
    assert_eq!(parsed1["k1"], "v1");

    // Snapshot updated_at after first run for §D idempotency-noise check.
    let updated_at_run1: f64 = storage
        .conn()
        .query_row("SELECT updated_at FROM edges WHERE id='rel-mut'", [], |r| {
            r.get(0)
        })
        .unwrap();

    // MUTATE the legacy metadata in two ways:
    //   - existing key `k1` changes from `v1` to `v1-changed` (must be
    //     dropped on merge — existing-wins keeps the edges row's `v1`)
    //   - new key `k2=v2` (must land on merge)
    storage
        .conn()
        .execute(
            r#"UPDATE entity_relations
                  SET metadata = ?
                WHERE id = 'rel-mut'"#,
            params![r#"{"k1":"v1-changed","k2":"v2"}"#],
        )
        .unwrap();

    // Re-run backfill — Pass 2 merge runs against the mutated metadata.
    let r2 = backfill_entity_relations_to_edges(&mut storage, None).expect("second run");
    assert_eq!(r2.rows_inserted, 0, "row already exists; Pass 1 must skip");

    let attrs2: String = storage
        .conn()
        .query_row("SELECT attributes FROM edges WHERE id='rel-mut'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let parsed2: serde_json::Value = serde_json::from_str(&attrs2).unwrap();

    // Existing-wins contract:
    assert_eq!(
        parsed2["k1"], "v1",
        "existing-wins: legacy k1=v1-changed must NOT overwrite edges k1=v1",
    );
    assert_eq!(
        parsed2["k2"], "v2",
        "merge: new key k2 from mutated legacy metadata must land",
    );

    // ISS-112 §D regression: updated_at MUST bump because Pass 2
    // actually added a new key (`k2`). Idempotent reruns (no change)
    // don't bump; mutated reruns that produce a real merge DO bump.
    let updated_at_run2: f64 = storage
        .conn()
        .query_row("SELECT updated_at FROM edges WHERE id='rel-mut'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        updated_at_run2 > updated_at_run1,
        "merge that adds a key MUST bump updated_at; was {updated_at_run1} -> {updated_at_run2}",
    );
}
