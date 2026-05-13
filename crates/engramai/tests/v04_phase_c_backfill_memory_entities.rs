//! T23 — Phase C backfill driver: memory_entities → edges.
//!
//! Per design.md §3.3 + §5.3, the projection splits by `role`:
//!
//!   * `role = 'mention' / '' / unknown / 'triple'`
//!         → `edge_kind='provenance', predicate='mentions'`
//!   * `role = 'subject'` → `edge_kind='structural', predicate='subject_of'`
//!   * `role = 'object'`  → `edge_kind='structural', predicate='object_of'`
//!
//! Acceptance contract:
//!
//!   1. Every legacy `memory_entities` row gets exactly one matching
//!      `edges` row whose kind/predicate match the table above and
//!      whose endpoints are valid `nodes(id)` references.
//!   2. `namespace` and `created_at` are derived from the parent
//!      memory (the link table has no own columns for these).
//!   3. Endpoints missing in `nodes` (T19 or T21 not yet run for
//!      that NS) are SKIPPED, not failed, and counted in audit notes
//!      as `rows_skipped_dangling_endpoint`. Recovery is "run T19+T21
//!      first, then re-run T23".
//!   4. Idempotent rerun: a second invocation inserts zero rows and
//!      counts the prior inserts as `rows_skipped_existing`.
//!   5. Non-canonical roles (`'triple'`, free-form) are folded onto
//!      `provenance/mentions` BUT the raw role is preserved in
//!      `edges.attributes.legacy_role` for audit traceability.
//!      Canonical roles produce `attributes = '{}'`.
//!   6. Deterministic edge `id`: re-running on an unmodified DB
//!      produces byte-identical edges.id values.
//!   7. Namespace filter restricts to memory_entities rows whose
//!      PARENT MEMORY's namespace matches.
//!   8. The counter invariant
//!      `rows_read = inserted + skipped + failed` holds on every run
//!      (asserted internally by `BackfillRun::assert_counter_invariant`).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_entities_to_nodes, backfill_memories_to_nodes,
    backfill_memory_entities_to_edges,
};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use serde_json::Value;
use tempfile::tempdir;

// ---------------------------------------------------------------------
// Seeding helpers.
//
// For memories: we use Storage::add (the production write path) and
// then strip the dual-written `nodes` row — same convention as the
// T19 test (`seed_legacy_only`). This simulates a row that pre-dates
// T12 and lets us exercise the full backfill path.
//
// For entities: there is no production-grade "add entity" API used
// outside the resolution pipeline, so we INSERT raw and let T21 lift
// it into `nodes`. (Mirrors the T22 test convention.)
//
// For memory_entities link rows: production code uses
// `Storage::link_memory_entity`, but that method also dual-writes to
// `edges` once T-future lands; for now we write raw to keep the test
// invariant ("backfill is what produces the edge row") absolutely
// clean.
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
    // Strip the Phase B dual-write so we exercise T19's backfill
    // path (and the FK guard in T23 sees an initially-missing
    // memory node until T19 runs).
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params![id])
        .expect("strip nodes row");
}

fn seed_legacy_entity(storage: &Storage, id: &str, name: &str, namespace: &str) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entities
               (id, name, entity_type, namespace, metadata, created_at, updated_at)
               VALUES (?, ?, 'person', ?, NULL, 1700000000.0, 1700000000.0)"#,
            params![id, name, namespace],
        )
        .expect("seed entity");
}

fn seed_link(storage: &Storage, memory_id: &str, entity_id: &str, role: &str) {
    storage
        .conn()
        .execute(
            "INSERT INTO memory_entities (memory_id, entity_id, role) VALUES (?, ?, ?)",
            params![memory_id, entity_id, role],
        )
        .expect("seed memory_entities row");
}

/// Run T19+T21 so that T23 has both endpoint kinds present in `nodes`.
fn run_node_prereqs(storage: &mut Storage) {
    backfill_memories_to_nodes(storage, None).expect("T19");
    backfill_entities_to_nodes(storage, None).expect("T21");
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[test]
fn t23_canonical_mention_role_writes_provenance_edge() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-alice", "Alice", "default");
    seed_link(&storage, "mem-1", "ent-alice", "mention");
    run_node_prereqs(&mut storage);

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);

    let (kind, predicate, source, target, ns, attrs): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate, source_id, target_id, namespace, attributes \
             FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(kind, "provenance");
    assert_eq!(predicate, "mentions");
    assert_eq!(source, "mem-1");
    assert_eq!(target, "ent-alice");
    assert_eq!(ns, "default", "namespace must come from parent memory");
    assert_eq!(
        attrs, "{}",
        "canonical roles must produce empty attributes (no legacy_role audit field)"
    );
}

#[test]
fn t23_empty_role_treated_as_mention() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", ""); // empty string
    run_node_prereqs(&mut storage);

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23");
    assert_eq!(run.rows_inserted, 1);

    let (kind, predicate, attrs): (String, String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate, attributes FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "provenance");
    assert_eq!(predicate, "mentions");
    assert_eq!(
        attrs, "{}",
        "empty role is a canonical-equivalent of 'mention', no audit field"
    );
}

#[test]
fn t23_subject_role_writes_structural_subject_of() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-alice", "Alice", "default");
    seed_link(&storage, "mem-1", "ent-alice", "subject");
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23");

    let (kind, predicate): (String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "structural");
    assert_eq!(predicate, "subject_of");
}

#[test]
fn t23_object_role_writes_structural_object_of() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-alice", "Alice", "default");
    seed_link(&storage, "mem-1", "ent-alice", "object");
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23");

    let (kind, predicate): (String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(kind, "structural");
    assert_eq!(predicate, "object_of");
}

#[test]
fn t23_triple_role_folds_to_mention_but_records_audit_field() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "triple");
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23");

    let (kind, predicate, attrs): (String, String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate, attributes FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "provenance");
    assert_eq!(predicate, "mentions");

    let parsed: Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        parsed.get("legacy_role").and_then(|v| v.as_str()),
        Some("triple"),
        "triple is a known free-form role; raw value must be preserved for audit"
    );
}

#[test]
fn t23_unknown_role_folds_to_mention_with_audit_and_counter() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "wildcard-role-xyz");
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23");

    let (kind, predicate, attrs): (String, String, String) = storage
        .conn()
        .query_row(
            "SELECT edge_kind, predicate, attributes FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "provenance");
    assert_eq!(predicate, "mentions");
    let parsed: Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        parsed.get("legacy_role").and_then(|v| v.as_str()),
        Some("wildcard-role-xyz")
    );

    // Counter must be visible in audit notes too.
    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(
        parsed.get("rows_normalized_legacy_role").and_then(|v| v.as_u64()),
        Some(1)
    );
    let samples = parsed
        .get("unknown_role_samples")
        .and_then(|v| v.as_array())
        .expect("unknown_role_samples array");
    assert!(samples.iter().any(|v| v.as_str() == Some("wildcard-role-xyz")));
}

#[test]
fn t23_skips_dangling_endpoint_and_recovers_after_prereqs() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "mention");
    // NOTE: do NOT run T19/T21 yet — endpoints are dangling.

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(
        run.rows_skipped_existing, 1,
        "dangling endpoint is rolled into rows_skipped_existing for the counter invariant; \
         detail lives in audit notes"
    );

    // Now back-fill the prerequisite kinds and rerun: T23 should
    // succeed.
    run_node_prereqs(&mut storage);
    let run2 = backfill_memory_entities_to_edges(&mut storage, None).expect("T23 rerun");
    assert_eq!(run2.rows_read, 1);
    assert_eq!(run2.rows_inserted, 1);
    assert_eq!(run2.rows_skipped_existing, 0);
}

#[test]
fn t23_is_idempotent_on_rerun() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "mention");
    run_node_prereqs(&mut storage);

    let r1 = backfill_memory_entities_to_edges(&mut storage, None).expect("T23 first");
    assert_eq!(r1.rows_inserted, 1);

    let r2 = backfill_memory_entities_to_edges(&mut storage, None).expect("T23 rerun");
    assert_eq!(r2.rows_read, 1);
    assert_eq!(r2.rows_inserted, 0);
    assert_eq!(r2.rows_skipped_existing, 1);

    // edges table should have exactly one row total.
    let count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "rerun must not duplicate the edge");
}

#[test]
fn t23_namespace_filter_uses_parent_memory_namespace() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-a", "ns-a");
    seed_legacy_memory(&mut storage, "mem-b", "ns-b");
    seed_legacy_entity(&storage, "ent-a", "EA", "ns-a");
    seed_legacy_entity(&storage, "ent-b", "EB", "ns-b");
    seed_link(&storage, "mem-a", "ent-a", "mention");
    seed_link(&storage, "mem-b", "ent-b", "mention");

    // Project both NS into nodes so endpoint FKs are satisfied.
    backfill_memories_to_nodes(&mut storage, None).expect("T19 all-ns");
    backfill_entities_to_nodes(&mut storage, None).expect("T21 all-ns");

    let run = backfill_memory_entities_to_edges(&mut storage, Some("ns-a")).expect("T23 ns-a");
    assert_eq!(run.rows_read, 1, "filter must restrict to ns-a's parent memory");
    assert_eq!(run.rows_inserted, 1);

    // edges table should have ns-a's row only.
    let (count_a, count_b): (i64, i64) = (
        storage
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE namespace = 'ns-a'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        storage
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE namespace = 'ns-b'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
    );
    assert_eq!(count_a, 1);
    assert_eq!(count_b, 0, "ns-b backfill not yet run");
}

#[test]
fn t23_deterministic_id_byte_identical_on_rerun() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "mention");
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23 first");

    let id_before: String = storage
        .conn()
        .query_row(
            "SELECT id FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| r.get(0),
        )
        .unwrap();

    // Rerun: id MUST be identical (same legacy row → same hash).
    backfill_memory_entities_to_edges(&mut storage, None).expect("T23 rerun");
    let id_after: String = storage
        .conn()
        .query_row(
            "SELECT id FROM edges WHERE source_id = ?",
            params!["mem-1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        id_before, id_after,
        "deterministic id must be byte-identical across runs"
    );
}

#[test]
fn t23_empty_table_is_clean_no_op() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23 on empty");
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);

    let edge_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(edge_count, 0);
}

#[test]
fn t23_many_rows_mixed_roles_counter_invariant_holds() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // 12 entities + 4 memories. Each memory mentions 3 entities with
    // a mix of canonical and non-canonical roles. The driver must
    // bucket every row correctly and the counter invariant must
    // hold.
    for i in 0..4 {
        seed_legacy_memory(&mut storage, &format!("mem-{i}"), "default");
    }
    for j in 0..12 {
        seed_legacy_entity(&storage, &format!("ent-{j}"), &format!("E{j}"), "default");
    }

    // Roles cycle through canonical + non-canonical:
    let role_cycle = ["mention", "subject", "object", "triple", "custom-1", ""];
    for i in 0..4 {
        for k in 0..3 {
            let j = i * 3 + k; // 0..12
            let role = role_cycle[(i * 3 + k) % role_cycle.len()];
            seed_link(&storage, &format!("mem-{i}"), &format!("ent-{j}"), role);
        }
    }
    run_node_prereqs(&mut storage);

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23");
    assert_eq!(run.rows_read, 12);
    assert_eq!(run.rows_inserted, 12);
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);
    // Counter invariant is asserted internally; this just confirms
    // it ran without panicking.

    // Verify the kind split: 'mention' / '' / 'triple' / 'custom-1'
    // all go to provenance; 'subject' / 'object' go to structural.
    let provenance_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_kind = 'provenance'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let structural_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE edge_kind = 'structural'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // role_cycle has 6 entries: mention, subject, object, triple, custom-1, ''
    // For 12 rows in cycle order: 2 each of mention, subject, object,
    // triple, custom-1, '' → 8 provenance + 4 structural.
    assert_eq!(provenance_count + structural_count, 12);
    assert_eq!(structural_count, 4, "2× subject + 2× object");
    assert_eq!(provenance_count, 8, "2× each of mention/triple/custom-1/'' ");

    // Audit notes should record 4 normalized rows (2× triple + 2× custom-1)
    // and 2 distinct unknown-role samples.
    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(
        parsed.get("rows_normalized_legacy_role").and_then(|v| v.as_u64()),
        Some(4)
    );
    assert_eq!(
        parsed.get("unknown_role_distinct_count").and_then(|v| v.as_u64()),
        Some(2),
        "'triple' and 'custom-1' are the 2 distinct non-canonical roles"
    );
    assert_eq!(
        parsed.get("unknown_role_samples_truncated").and_then(|v| v.as_bool()),
        Some(false)
    );
}

#[test]
fn t23_unknown_role_samples_truncated_flag_set_above_cap() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // 12 distinct non-canonical roles to overflow the 10-sample cap.
    for j in 0..12 {
        let mem = format!("mem-{j}");
        let ent = format!("ent-{j}");
        seed_legacy_memory(&mut storage, &mem, "default");
        seed_legacy_entity(&storage, &ent, &format!("E{j}"), "default");
        seed_link(&storage, &mem, &ent, &format!("oddrole-{j}"));
    }
    run_node_prereqs(&mut storage);

    backfill_memory_entities_to_edges(&mut storage, None).expect("T23");

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(
        parsed.get("unknown_role_distinct_count").and_then(|v| v.as_u64()),
        Some(12)
    );
    let samples = parsed
        .get("unknown_role_samples")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(samples.len(), 10, "cap = 10 distinct samples");
    assert_eq!(
        parsed.get("unknown_role_samples_truncated").and_then(|v| v.as_bool()),
        Some(true),
        "must signal truncation when distinct count > sample cap"
    );
}

#[test]
fn t23_mismatched_kind_at_same_id_counted_not_silently_skipped() {
    // Defense-in-depth: under design contract, the deterministic id
    // includes (edge_kind, predicate), so a pre-existing edge with
    // the same id ALWAYS has matching kind. This test exercises the
    // mismatched-kind audit counter by pre-seeding an edges row with
    // the deterministic id but a different kind. If a future bug
    // ever breaks the hash invariant, this counter is what makes the
    // breakage visible (rather than silently skipping data).
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_memory(&mut storage, "mem-1", "default");
    seed_legacy_entity(&storage, "ent-1", "X", "default");
    seed_link(&storage, "mem-1", "ent-1", "mention");
    run_node_prereqs(&mut storage);

    // Compute the deterministic id the driver will derive for this
    // row (must match the driver's hash_input exactly).
    let hash_input = format!(
        "memory_entities|{}|{}|{}|{}|{}",
        "mem-1", "ent-1", "mention", "provenance", "mentions"
    );
    let id = {
        use sha2::{Digest, Sha256};
        use uuid::Uuid;
        let digest = Sha256::digest(hash_input.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Uuid::from_bytes(bytes).to_string()
    };

    // Pre-seed an edges row at that id but with edge_kind='containment'
    // to simulate a hash-invariant breakage.
    storage
        .conn()
        .execute(
            r#"INSERT INTO edges (
                id, source_id, target_id, target_literal,
                edge_kind, predicate_kind, predicate,
                summary, attributes, confidence,
                recorded_at, resolution_method,
                namespace, created_at, updated_at
               ) VALUES (?, ?, ?, NULL,
                'containment', 'canonical', 'contains',
                '', '{}', 1.0,
                1700000000.0, 'direct',
                'default', 1700000000.0, 1700000000.0)"#,
            params![id, "mem-1", "ent-1"],
        )
        .unwrap();

    let run = backfill_memory_entities_to_edges(&mut storage, None).expect("T23");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 1);

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(
        parsed.get("rows_skipped_mismatched_kind").and_then(|v| v.as_u64()),
        Some(1),
        "mismatched-kind row must be visible in audit, not silently lumped into skipped_existing"
    );
}
