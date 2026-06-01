//! ISS-121 — Phase C backfill: `memories.deleted_at` → `nodes.deleted_at`.
//!
//! Companion to `tests/iss121_soft_delete_dual_write.rs` which covers
//! the writer-side fix. This test covers the backfill driver that
//! patches rows soft-deleted BEFORE the dual-write fix shipped.
//!
//! Acceptance:
//!
//!   1. Every `memories WHERE deleted_at IS NOT NULL` row that has a
//!      matching `nodes` row gets `nodes.deleted_at` populated with the
//!      same instant (within sub-second drift from RFC3339→epoch).
//!   2. Dangling rows (memories soft-deleted but no nodes mirror —
//!      pre-T12 ingest) are SKIPPED (not failed); counted separately.
//!   3. Idempotent: a second run is a no-op (zero `rows_inserted`).
//!   4. Audit row in `backfill_runs` opens at start, closes at end,
//!      satisfies the counter invariant
//!      `rows_read == rows_inserted + rows_skipped_existing + rows_failed`.
//!   5. Namespace filter: with `Some(ns)`, only rows in that namespace
//!      are touched.
//!   6. Corrupt RFC3339 dates are counted as `rows_failed` (do not
//!      halt the driver).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::backfill_soft_delete_into_nodes;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use tempfile::tempdir;

fn make_record(id: &str, ns_unused: &str) -> MemoryRecord {
    let _ = ns_unused;
    MemoryRecord {
        id: id.into(),
        content: format!("c {id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: Utc.with_ymd_and_hms(2026, 5, 13, 12, 0, 0).unwrap(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss121-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn fresh_storage() -> (tempfile::TempDir, Storage) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("engram.db");
    let storage = Storage::new(&path).expect("open");
    (dir, storage)
}

fn count_nodes_deleted(storage: &Storage) -> i64 {
    storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes \
             WHERE node_kind = 'memory' AND deleted_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn count_memories_deleted(storage: &Storage) -> i64 {
    storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE deleted_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn iss121_backfill_patches_legacy_soft_deletes_into_nodes() {
    let (_d, mut storage) = fresh_storage();

    // Seed 3 memories, dual-write IS active (T12), so all three get
    // nodes mirrors. Soft-delete two via the new dual-writing path
    // (which is correct already) and then SIMULATE a pre-fix state by
    // clearing nodes.deleted_at — that's exactly the scenario the
    // backfill patches.
    for id in ["a", "b", "c"] {
        storage.add(&make_record(id, "default"), "default").unwrap();
    }
    storage.soft_delete("a").unwrap();
    storage.soft_delete("b").unwrap();

    // Pre: dual-write already set both columns, so nodes shows 2 deleted.
    assert_eq!(count_nodes_deleted(&storage), 2);
    assert_eq!(count_memories_deleted(&storage), 2);

    // Simulate pre-fix history: blank out nodes.deleted_at.
    storage
        .conn()
        .execute(
            "UPDATE nodes SET deleted_at = NULL WHERE id IN ('a','b')",
            [],
        )
        .unwrap();
    assert_eq!(count_nodes_deleted(&storage), 0, "simulated pre-fix state");
    assert_eq!(count_memories_deleted(&storage), 2);

    // Backfill.
    let run = backfill_soft_delete_into_nodes(&mut storage, None).expect("backfill ok");
    assert_eq!(run.rows_read, 2);
    assert_eq!(run.rows_inserted, 2, "both legacy rows projected");
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);

    // Post: nodes shows 2 deleted again.
    assert_eq!(count_nodes_deleted(&storage), 2);
}

#[test]
fn iss121_backfill_is_idempotent() {
    let (_d, mut storage) = fresh_storage();
    storage
        .add(&make_record("x", "default"), "default")
        .unwrap();
    storage.soft_delete("x").unwrap();
    // Blank nodes side to give the backfill something to do.
    storage
        .conn()
        .execute("UPDATE nodes SET deleted_at = NULL", [])
        .unwrap();

    let r1 = backfill_soft_delete_into_nodes(&mut storage, None).unwrap();
    assert_eq!(r1.rows_inserted, 1, "first run patches the row");

    let r2 = backfill_soft_delete_into_nodes(&mut storage, None).unwrap();
    assert_eq!(r2.rows_inserted, 0, "second run is a no-op");
    assert_eq!(
        r2.rows_skipped_existing, 1,
        "row already has deleted_at set → counted as skipped"
    );
}

#[test]
fn iss121_backfill_namespace_filter() {
    let (_d, mut storage) = fresh_storage();
    storage.add(&make_record("a", "ns_a"), "ns_a").unwrap();
    storage.add(&make_record("b", "ns_b"), "ns_b").unwrap();
    storage.soft_delete("a").unwrap();
    storage.soft_delete("b").unwrap();
    storage
        .conn()
        .execute("UPDATE nodes SET deleted_at = NULL", [])
        .unwrap();

    let r = backfill_soft_delete_into_nodes(&mut storage, Some("ns_a")).unwrap();
    assert_eq!(r.rows_read, 1, "only ns_a row read");
    assert_eq!(r.rows_inserted, 1);

    // ns_b row should still be NULL on nodes side.
    let ns_b_state: Option<f64> = storage
        .conn()
        .query_row("SELECT deleted_at FROM nodes WHERE id = 'b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(ns_b_state.is_none(), "ns_b row untouched by ns_a filter");
}

#[test]
fn iss121_backfill_skips_dangling_legacy_row() {
    // Pre-T12 scenario: a memory exists with deleted_at set but no
    // corresponding nodes row at all. The backfill should NOT insert
    // a synthetic nodes row — that's T19's job. Skip and count.
    let (_d, mut storage) = fresh_storage();
    storage
        .add(&make_record("orphan", "default"), "default")
        .unwrap();
    storage.soft_delete("orphan").unwrap();
    // Delete the nodes mirror to simulate dangling.
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = 'orphan'", [])
        .unwrap();

    let r = backfill_soft_delete_into_nodes(&mut storage, None).unwrap();
    assert_eq!(r.rows_read, 1);
    assert_eq!(r.rows_inserted, 0);
    assert_eq!(r.rows_skipped_existing, 1, "dangling counted as skipped");
}

#[test]
fn iss121_backfill_counter_invariant_holds() {
    // rows_read == rows_inserted + rows_skipped_existing + rows_failed
    // is asserted internally by run.assert_counter_invariant. This
    // test just covers a mixed run that exercises all three buckets.
    let (_d, mut storage) = fresh_storage();
    storage
        .add(&make_record("normal", "default"), "default")
        .unwrap();
    storage
        .add(&make_record("orphan", "default"), "default")
        .unwrap();
    storage.soft_delete("normal").unwrap();
    storage.soft_delete("orphan").unwrap();
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = 'orphan'", [])
        .unwrap();
    storage
        .conn()
        .execute("UPDATE nodes SET deleted_at = NULL WHERE id = 'normal'", [])
        .unwrap();

    // Corrupt one row's RFC3339.
    storage
        .add(&make_record("corrupt", "default"), "default")
        .unwrap();
    storage
        .conn()
        .execute(
            "UPDATE memories SET deleted_at = 'not-a-date' WHERE id = 'corrupt'",
            [],
        )
        .unwrap();

    let r = backfill_soft_delete_into_nodes(&mut storage, None).unwrap();
    assert_eq!(r.rows_read, 3, "normal + orphan + corrupt");
    assert_eq!(r.rows_inserted, 1, "only normal patched");
    // skipped_existing counts the dangling orphan
    assert_eq!(r.rows_skipped_existing, 1);
    assert_eq!(r.rows_failed, 1, "corrupt RFC3339 → failed parse");
    // BackfillRun.assert_counter_invariant runs inside the driver.
}
