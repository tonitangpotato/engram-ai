//! T20 — Phase C backfill driver: memory_embeddings → node_embeddings.
//!
//! Acceptance per design.md §5.3 + module docs in
//! `crates/engramai/src/substrate/backfill.rs`:
//!
//!   1. Every legacy `memory_embeddings` row whose parent `memories`
//!      row has a matching `nodes` row gets a matching
//!      `node_embeddings` row. BLOB bytes are byte-identical between
//!      source and projection.
//!   2. T19 prerequisite: rows whose parent `nodes` row is missing
//!      are SKIPPED (not failed) with `rows_skipped_missing_node`
//!      recorded in the audit notes. Re-running after T19 picks them
//!      up.
//!   3. Idempotent: re-run inserts zero rows.
//!   4. `created_at` TEXT (RFC3339) → REAL (epoch) round-trips
//!      correctly. Malformed dates fall back to `now()` with the
//!      count surfaced via `rows_failed_parse_used_now` in notes.
//!   5. Namespace filter: with `Some(ns)` only embeddings whose
//!      `memories.namespace = ns` are projected.
//!   6. Multi-model: a memory with multiple `(model, embedding)`
//!      rows gets all of them projected.
//!   7. FK enforcement: never violates `node_embeddings.node_id REFERENCES nodes(id)`.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{
    backfill_embeddings_to_node_embeddings, backfill_memories_to_nodes,
};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: format!("c {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.5,
        core_strength: 0.5,
        importance: 0.5,
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

/// Seed a legacy embedding row directly via raw SQL. We don't go
/// through `Storage::store_embedding` because that path will be
/// dual-write-augmented in a future task; here we want the
/// pre-Phase-B state.
fn seed_legacy_embedding(
    storage: &Storage,
    memory_id: &str,
    model: &str,
    dimensions: usize,
    created_at_rfc3339: &str,
) -> Vec<u8> {
    // Deterministic bytes so byte-equality is easy to assert.
    let blob: Vec<u8> = (0..dimensions * 4)
        .map(|i| (i % 251) as u8)
        .collect();
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
fn t20_backfill_projects_embeddings_byte_equal() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed memory + run T19 so the parent nodes row exists.
    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    // No T19 needed — Storage::add already dual-wrote the nodes row
    // via T12. We're testing T20 in isolation here.

    let blob = seed_legacy_embedding(&storage, "mem-1", "all-MiniLM-L6-v2", 384, "2026-05-13T10:30:00Z");

    let run = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);

    // Read back from node_embeddings and verify byte-equal projection.
    let (got_blob, got_dim, got_epoch): (Vec<u8>, i64, f64) = storage
        .conn()
        .query_row(
            "SELECT embedding, dimensions, created_at FROM node_embeddings WHERE node_id = ? AND model = ?",
            params!["mem-1", "all-MiniLM-L6-v2"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(got_blob, blob, "embedding BLOB must be byte-identical");
    assert_eq!(got_dim, 384);

    // 2026-05-13T10:30:00Z is 1778667000 epoch seconds.
    let expected_epoch = Utc.with_ymd_and_hms(2026, 5, 13, 10, 30, 0).unwrap();
    let expected_f64 = expected_epoch.timestamp() as f64;
    assert!(
        (got_epoch - expected_f64).abs() < 0.001,
        "created_at epoch mismatch: got {got_epoch}, want {expected_f64}"
    );
}

#[test]
fn t20_idempotent_rerun() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let m = sample_record("mem-1");
    storage.add(&m, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-1", "model-a", 4, "2026-01-01T00:00:00Z");

    let r1 = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("first");
    assert_eq!(r1.rows_inserted, 1);

    let r2 = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("second");
    assert_eq!(r2.rows_read, 1);
    assert_eq!(r2.rows_inserted, 0, "rerun must be no-op");
    assert_eq!(r2.rows_skipped_existing, 1);
}

#[test]
fn t20_skips_orphan_embeddings_when_node_missing() {
    // Embedding exists for a memory_id with no `nodes` row.
    // Simulates: T19 was filtered to one namespace, T20 runs
    // unfiltered. The orphan should be SKIPPED (not failed), with
    // the FK never being exercised.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Insert a legacy memory and seed its embedding WHILE the nodes
    // row still exists, then strip the nodes row to simulate "T19
    // hasn't reached this namespace yet."
    //
    // ISS-199: `memory_embeddings.memory_id` now FK→`nodes(id)`, so an
    // embedding cannot be inserted against a missing node. To exercise
    // the backfill's defensive dangling-endpoint skip we reproduce the
    // legacy state by seeding first, then dropping the node with FK
    // enforcement OFF (exactly the state a row written under the old
    // `memories(id)` FK lands in before T19 lifts the node).
    let m = sample_record("mem-orphan");
    storage.add(&m, "ns-skipped").unwrap();
    seed_legacy_embedding(&storage, "mem-orphan", "model-x", 4, "2026-01-01T00:00:00Z");
    storage
        .conn()
        .execute_batch(
            "PRAGMA foreign_keys=OFF; \
             DELETE FROM nodes WHERE id = 'mem-orphan'; \
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();

    // Also seed a normal memory + embedding (so the run has mixed state).
    let normal = sample_record("mem-normal");
    storage.add(&normal, "default").unwrap();
    seed_legacy_embedding(&storage, "mem-normal", "model-x", 4, "2026-01-01T00:00:00Z");

    let run = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_read, 2);
    assert_eq!(run.rows_inserted, 1, "only the normal embedding lands");
    assert_eq!(run.rows_skipped_existing, 1, "orphan counted as skipped");
    assert_eq!(run.rows_failed, 0, "skipped, not failed");

    // Verify node_embeddings has exactly the normal row, not the orphan.
    let count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM node_embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);

    // notes JSON should record the skip count.
    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed["rows_skipped_missing_node"], 1);

    // After running T19 on the missing namespace, the orphan can land.
    let _ = backfill_memories_to_nodes(&mut storage, Some("ns-skipped"))
        .expect("recover the missing node");
    let run2 =
        backfill_embeddings_to_node_embeddings(&mut storage, None).expect("retry T20");
    assert_eq!(run2.rows_inserted, 1, "orphan now lands");
    let final_count: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM node_embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(final_count, 2);
}

#[test]
fn t20_multi_model_per_memory() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-mm");
    storage.add(&m, "default").unwrap();

    seed_legacy_embedding(&storage, "mem-mm", "model-a", 4, "2026-01-01T00:00:00Z");
    seed_legacy_embedding(&storage, "mem-mm", "model-b", 8, "2026-01-02T00:00:00Z");

    let run = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_inserted, 2, "both models should be projected");

    let models: Vec<String> = storage
        .conn()
        .prepare("SELECT model FROM node_embeddings WHERE node_id='mem-mm' ORDER BY model")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(models, vec!["model-a".to_string(), "model-b".to_string()]);
}

#[test]
fn t20_namespace_filter() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let a = sample_record("mem-a");
    let b = sample_record("mem-b");
    storage.add(&a, "ns-a").unwrap();
    storage.add(&b, "ns-b").unwrap();
    seed_legacy_embedding(&storage, "mem-a", "m", 4, "2026-01-01T00:00:00Z");
    seed_legacy_embedding(&storage, "mem-b", "m", 4, "2026-01-01T00:00:00Z");

    let run = backfill_embeddings_to_node_embeddings(&mut storage, Some("ns-a")).unwrap();
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);

    let a_present: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id='mem-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let b_present: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id='mem-b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(a_present, 1);
    assert_eq!(b_present, 0, "ns-b must not be touched");
}

#[test]
fn t20_malformed_created_at_uses_fallback() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let m = sample_record("mem-bad");
    storage.add(&m, "default").unwrap();

    // Insert garbage in created_at to exercise the parse fallback.
    storage
        .conn()
        .execute(
            r#"INSERT INTO memory_embeddings (memory_id, model, embedding, dimensions, created_at)
               VALUES ('mem-bad', 'm', X'00010203', 1, 'not-a-date')"#,
            [],
        )
        .unwrap();

    let before = Utc::now().timestamp() as f64 - 1.0;
    let run = backfill_embeddings_to_node_embeddings(&mut storage, None).expect("backfill");
    let after = Utc::now().timestamp() as f64 + 1.0;
    assert_eq!(run.rows_inserted, 1, "row should still land on bad date");

    let got_epoch: f64 = storage
        .conn()
        .query_row(
            "SELECT created_at FROM node_embeddings WHERE node_id='mem-bad'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        got_epoch >= before && got_epoch <= after,
        "fallback should be ~now() ({before}..{after}); got {got_epoch}"
    );

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed["rows_failed_parse_used_now"], 1);
}

#[test]
fn t20_empty_table_completes_cleanly() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let run =
        backfill_embeddings_to_node_embeddings(&mut storage, None).expect("backfill empty");
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
}
