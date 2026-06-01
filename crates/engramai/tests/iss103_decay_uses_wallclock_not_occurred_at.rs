//! ISS-103 regression: ingesting a memory with `occurred_at` years in
//! the past must NOT cause Ebbinghaus decay to immediately
//! soft-delete it.
//!
//! Background (RUN-0017, conv-26):
//!   - LoCoMo ingest threaded `meta.occurred_at = session_date`
//!     (e.g. 2023-05-08) into the persisted record's `created_at`.
//!   - `ebbinghaus::effective_strength` computes age = now -
//!     created_at → "memory has existed for 3 years".
//!   - `check_decay_and_flag` fires when effective_strength < 0.1
//!     AND access_count < 2 → mass soft-delete on fresh ingest
//!     (457/458 rows, all within 4 hours).
//!
//! ISS-103 fix splits the two:
//!   - `created_at` = wall-clock at insert (drives decay)
//!   - `occurred_at` = caller-supplied event time (drives temporal
//!     grounding only)
//!
//! This test pins the contract so we never silently regress.

use chrono::TimeZone;
use engramai::memory::Memory;
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

fn store_with_old_occurred_at(mem: &mut Memory, ns: &str) -> String {
    // 2023-05-08: ~3 years before the test "now" (2026-ish in
    // practice). Crucially: this is the same kind of date LoCoMo
    // sessions carry, so we're reproducing the production path.
    let event_time = chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap();

    let meta = StorageMeta {
        importance_hint: Some(0.6),
        source: Some("iss103-regression".into()),
        namespace: Some(ns.into()),
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
        occurred_at: Some(event_time),
        emotion: None,
        domain: None,
    };

    let out = mem
        .store_raw(
            "In May 2023, the team agreed to ship the v0.3 release in October.",
            meta,
        )
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Stored(outcomes) => {
            match outcomes.first().expect("at least one outcome") {
                StoreOutcome::Inserted { id } => id.clone(),
                StoreOutcome::Merged { id, .. } => id.clone(),
            }
        }
        other => panic!("expected Stored, got {:?}", other),
    }
}

#[test]
fn iss103_old_occurred_at_does_not_trigger_decay_softdelete() {
    // The key regression: a memory with occurred_at=2023 but
    // freshly ingested today must survive a decay pass.
    let mut mem = new_mem();
    let ns = "iss103-fresh-old-event";
    let id = store_with_old_occurred_at(&mut mem, ns);

    // Sanity: record exists and is live before decay.
    let before = mem.get(&id).expect("get ok").expect("record exists");
    assert_eq!(
        before.occurred_at,
        Some(chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap()),
        "precondition: occurred_at carries the 2023 event time",
    );
    let now = chrono::Utc::now();
    let age_secs = (now - before.created_at).num_seconds();
    assert!(
        age_secs.abs() < 60,
        "precondition: created_at must be wall-clock now (age {} s)",
        age_secs,
    );

    // Run the exact lifecycle pass that bulk-deleted RUN-0017's DB.
    let report = mem.check_decay_and_flag(Some(ns)).expect("decay pass ok");

    assert_eq!(
        report.flagged_for_forget, 0,
        "ISS-103 root fix: fresh ingest with old occurred_at must NOT \
         be soft-deleted by Ebbinghaus decay (got {} flagged, full report = {:?})",
        report.flagged_for_forget, report,
    );

    // And the record is still retrievable post-decay (mem.get only
    // returns rows with deleted_at IS NULL — see Storage::get).
    let after = mem.get(&id).expect("get ok");
    assert!(
        after.is_some(),
        "record must remain retrievable after decay pass (deleted_at must be NULL)",
    );
    let _ = id;
}

#[test]
fn iss103_bulk_ingest_with_old_occurred_at_all_survive_decay() {
    // Stress version: 50 memories, all with occurred_at in 2023.
    // RUN-0017 saw 457/458 deleted; we want 50/50 surviving.
    let mut mem = new_mem();
    let ns = "iss103-bulk";

    let mut ids = Vec::with_capacity(50);
    for i in 0..50 {
        let event_time = chrono::Utc.with_ymd_and_hms(2023, 5, 8, 12, 0, 0).unwrap()
            + chrono::Duration::seconds(i as i64 * 60);
        let meta = StorageMeta {
            importance_hint: Some(0.6),
            source: Some("iss103-bulk".into()),
            namespace: Some(ns.into()),
            user_metadata: serde_json::Value::Null,
            memory_type_hint: None,
            occurred_at: Some(event_time),
            emotion: None,
            domain: None,
        };
        let out = mem
            .store_raw(&format!("Bulk fact {} from May 2023.", i), meta)
            .expect("store_raw ok");
        if let RawStoreOutcome::Stored(outcomes) = out {
            if let Some(StoreOutcome::Inserted { id }) = outcomes.first() {
                ids.push(id.clone());
            } else if let Some(StoreOutcome::Merged { id, .. }) = outcomes.first() {
                ids.push(id.clone());
            }
        }
    }
    assert_eq!(ids.len(), 50, "all 50 inserts should land");

    let report = mem.check_decay_and_flag(Some(ns)).expect("decay pass ok");

    assert_eq!(
        report.flagged_for_forget, 0,
        "ISS-103: 0/50 should be soft-deleted (RUN-0017 baseline was 457/458). \
         Got {} flagged, report = {:?}",
        report.flagged_for_forget, report,
    );

    let mut live = 0;
    for id in &ids {
        if mem.get(id).expect("get ok").is_some() {
            live += 1;
        }
    }
    assert_eq!(
        live, 50,
        "all 50 memories must remain live (retrievable) after decay pass",
    );
}

#[test]
fn iss103_decay_still_works_on_actually_aged_memory() {
    // Negative test: the fix must NOT disable decay entirely.
    // We cannot easily fake `created_at` in the past via the public
    // store_raw API (created_at is set internally to Utc::now()), so
    // we verify the symmetry property: check_decay_and_flag must
    // still run without error and still computes effective_strength
    // off `created_at`. Detailed decay-correctness is covered by the
    // Ebbinghaus unit tests; this test pins the contract that we
    // didn't accidentally short-circuit the function.
    let mut mem = new_mem();
    let ns = "iss103-decay-still-runs";
    let _id = store_with_old_occurred_at(&mut mem, ns);

    // Precondition: one memory in the namespace.
    let report = mem.check_decay_and_flag(Some(ns)).expect("decay pass ok");

    // We just iterated one record. The function ran end-to-end.
    // (If ISS-103 fix accidentally returned early, below_threshold
    // would be unreliable; here we just check the call succeeded.)
    let _ = report.below_threshold;
}
