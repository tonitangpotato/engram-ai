//! ISS-112 §A atomicity regression for the T19 backfill driver.
//!
//! This is its OWN test binary on purpose. The fault-injection flag
//! it uses (`test_hooks::FAULT_INJECT_BETWEEN_PASSES`) is a process-
//! global atomic, so any other parallel test in the same binary
//! that calls `backfill_memories_to_nodes` would racily see the
//! injected fault. Cargo runs each `tests/*.rs` as a separate
//! process, so isolating the fault-injection test here is sufficient
//! to keep it correct without serializing every other backfill test.
//!
//! The `sample_record` and `seed_legacy_only` helpers below mirror
//! the ones in `v04_phase_c_backfill.rs`. The duplication is
//! deliberate — exposing them as `pub(crate)` would not help
//! integration tests (which see the lib as an external crate), and
//! exposing them at `pub` widens the engramai surface for a
//! test-only helper. The duplication is small (~30 LOC) and stable.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::backfill::{backfill_memories_to_nodes, test_hooks};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap();
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
        source: "phase-c-atomicity-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: Some(serde_json::json!({"tag": "phase-c-atomicity"})),
    }
}

fn seed_legacy_only(storage: &mut Storage, record: &MemoryRecord, namespace: &str) {
    storage.add(record, namespace).expect("add");
    storage
        .conn()
        .execute("DELETE FROM nodes WHERE id = ?", params![record.id])
        .expect("strip nodes row");
}

/// ISS-112 §A root fix: Pass 1 and Pass 2 share a single transaction
/// so the data write is atomic. Before the fix Pass 1 had its own
/// tx that committed independently, then Pass 2 ran outside any tx
/// — a crash between the two would leave `nodes` rows with stale
/// `superseded_by = NULL` that should have been linked.
///
/// This test forces a failure between the two passes (via
/// `test_hooks::FAULT_INJECT_BETWEEN_PASSES`) and asserts:
///
///   1. The driver returns an error.
///   2. The `nodes` table contains **zero** memory rows from this
///      run — Pass 1's inserts rolled back with the shared tx.
///   3. The audit row in `backfill_runs` is still present with
///      `finished_at IS NULL`, preserving the crash-detector
///      affordance that operators rely on
///      (`SELECT * FROM backfill_runs WHERE finished_at IS NULL`).
#[test]
fn t19_iss112a_pass1_rolls_back_when_pass2_aborts() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed 3 legacy-only memories so Pass 1 has real work to do.
    for i in 0..3 {
        let r = sample_record(&format!("mem-atomicity-{i}"));
        seed_legacy_only(&mut storage, &r, "default");
    }

    // Sanity: nodes table starts clean for these ids.
    let pre_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE node_kind = 'memory'
             AND id LIKE 'mem-atomicity-%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pre_count, 0, "test precondition: nodes is empty");

    // Arm the fault injector and run the backfill. Always disarm
    // (even if assertions fail) so a panic in this test never
    // poisons a re-run.
    test_hooks::FAULT_INJECT_BETWEEN_PASSES.store(true, Ordering::SeqCst);
    let result = backfill_memories_to_nodes(&mut storage, None);
    test_hooks::FAULT_INJECT_BETWEEN_PASSES.store(false, Ordering::SeqCst);

    // (1) Driver returned an error.
    assert!(
        result.is_err(),
        "fault-injected backfill should return Err, got: {:?}",
        result
    );

    // (2) nodes table has zero memory rows from this run — Pass 1's
    // inserts must have rolled back when the shared tx was dropped.
    let post_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE node_kind = 'memory'
             AND id LIKE 'mem-atomicity-%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        post_count, 0,
        "ISS-112 §A: Pass 1 must roll back when Pass 2 aborts \
         (single-tx atomicity contract violated)"
    );

    // (3) Crash-detector affordance preserved: the audit row was
    // INSERTed outside the work tx, so the orphan
    // `finished_at IS NULL` row is still there for operators.
    let orphan_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM backfill_runs
             WHERE legacy_table = 'memories' AND finished_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        orphan_count, 1,
        "audit row must survive the rollback as a crash detector — \
         operators query `WHERE finished_at IS NULL` to find orphans"
    );
}
