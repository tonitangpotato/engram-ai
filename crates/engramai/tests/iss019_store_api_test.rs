//! Integration tests for the ISS-019 typed write-path API.
//!
//! Covers:
//! - `store_enriched` on extractor-less engine → Inserted outcome,
//!   metadata blob is v1-legacy-shape with valid dimensions.
//! - `store_raw` on extractor-less engine → minimal Dimensions path,
//!   returns `Stored(vec![Inserted])`.
//! - `store_raw` with duplicate content → second call merges into
//!   the first via the existing dedup pipeline (outcome = Merged).
//! - `store_raw` with empty / whitespace content → Skipped(TooShort).

use engramai::dimensions::Importance;
use engramai::enriched::EnrichedMemory;
use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::store_api::{QuarantineReason, RawStoreOutcome, StorageMeta, StoreOutcome};
use std::error::Error as StdError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

#[test]
fn store_enriched_inserts_with_legacy_metadata_shape() {
    let mut mem = new_mem();

    let em = EnrichedMemory::minimal(
        "ISS-019 Step 4 is live",
        Importance::new(0.6),
        Some("test".to_string()),
        None,
    )
    .expect("minimal accepts real content");

    let out = mem.store_enriched(em).expect("store_enriched ok");

    match out {
        StoreOutcome::Inserted { id } => {
            assert!(!id.is_empty(), "inserted id should be non-empty");

            // Read the row back and confirm metadata uses the v2
            // namespaced layout: engram.dimensions + engram.dimensions.type_weights.
            // (ISS-019 Step 7a migrated writes from v1 flat to v2.)
            // We access via a recall round-trip, which exercises the
            // existing read path and guarantees nothing regressed.
            let results = mem
                .recall("ISS-019 Step 4", 5, None, None)
                .expect("recall ok");
            assert!(!results.is_empty(), "should recall the stored memory");
            let meta = results[0]
                .record
                .metadata
                .as_ref()
                .expect("metadata populated");
            let engram = meta
                .get("engram")
                .expect("v2 layout: metadata.engram namespace present");
            let dims = engram
                .get("dimensions")
                .expect("v2 layout: engram.dimensions present");
            assert!(
                dims.get("type_weights").is_some(),
                "engram.dimensions.type_weights present"
            );

            // Scalar fields ISS-020 P0.0 requires on every row.
            assert!(dims.get("valence").is_some(), "valence always present");
            assert!(
                dims.get("confidence").is_some(),
                "confidence always present"
            );
            assert!(dims.get("domain").is_some(), "domain always present");
        }
        StoreOutcome::Merged { .. } => panic!("fresh engine should not merge"),
    }
}

#[test]
fn store_raw_without_extractor_uses_minimal_dimensions() {
    let mut mem = new_mem();
    let meta = StorageMeta {
        importance_hint: Some(0.4),
        source: Some("unit".to_string()),
        namespace: None,
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
    };

    let out = mem
        .store_raw("potato likes cake", meta)
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert_eq!(outcomes.len(), 1, "minimal path produces one outcome");
            assert!(matches!(outcomes[0], StoreOutcome::Inserted { .. }));
        }
        other => panic!("expected Stored, got {other:?}"),
    }
}

#[test]
fn store_raw_empty_content_is_skipped_too_short() {
    let mut mem = new_mem();
    let out = mem
        .store_raw("   \n\t", StorageMeta::default())
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Skipped { reason, .. } => {
            assert_eq!(reason, engramai::store_api::SkipReason::TooShort);
        }
        other => panic!("expected Skipped, got {other:?}"),
    }
}

#[test]
fn store_raw_user_metadata_is_preserved() {
    let mut mem = new_mem();
    let meta = StorageMeta {
        importance_hint: Some(0.5),
        source: None,
        namespace: None,
        user_metadata: serde_json::json!({"callsite": "test-xyz"}),
        memory_type_hint: None,
    };
    let out = mem
        .store_raw("user meta round-trip", meta)
        .expect("store_raw ok");
    let id = match out {
        RawStoreOutcome::Stored(outs) => match &outs[0] {
            StoreOutcome::Inserted { id } => id.clone(),
            other => panic!("unexpected outcome: {other:?}"),
        },
        other => panic!("unexpected raw outcome: {other:?}"),
    };
    assert!(!id.is_empty());

    // Read back via recall; assert the user-metadata key survives
    // under `user.*` (v2 layout — migrated from top-level in Step 7a).
    let results = mem
        .recall("user meta round-trip", 3, None, None)
        .expect("recall ok");
    let meta = results[0]
        .record
        .metadata
        .as_ref()
        .expect("metadata populated");
    assert_eq!(
        meta.get("user")
            .and_then(|u| u.get("callsite"))
            .and_then(|v| v.as_str()),
        Some("test-xyz"),
        "user metadata key round-trips under user.* (v2 layout)"
    );
}

// ─────────────────────────────────────────────────────────────────────
// ISS-019 Step 6: quarantine persistence + retry API integration tests
// ─────────────────────────────────────────────────────────────────────

/// Test extractor whose success/failure behavior is programmable.
///
/// - `fail_until` — first N `extract()` calls return Err (simulating
///   a transient outage such as an API rate-limit or network blip).
/// - `make_fact` — closure that constructs the ExtractedFact returned
///   on a successful call.
struct ProgrammableExtractor {
    calls:       Arc<AtomicUsize>,
    fail_until:  usize,
}

impl ProgrammableExtractor {
    fn new(fail_until: usize) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (Self { calls: Arc::clone(&calls), fail_until }, calls)
    }
}

impl MemoryExtractor for ProgrammableExtractor {
    fn extract(&self, text: &str) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_until {
            // Simulated extractor-level failure (transient).
            Err(format!("simulated transient extractor error (call {})", n).into())
        } else {
            // Success path — produce exactly one fact that carries the
            // first ~32 chars of the content so the caller can assert
            // on it.
            let fact = ExtractedFact {
                core_fact: text.chars().take(80).collect(),
                importance: 0.6,
                confidence: "confident".into(),
                domain: "general".into(),
                ..Default::default()
            };
            Ok(vec![fact])
        }
    }
}

#[test]
fn store_raw_extractor_error_persists_quarantine_row() {
    // A brand-new memory engine + a failing extractor.
    let mut mem = new_mem();
    let (extractor, calls) = ProgrammableExtractor::new(999); // always fails
    mem.set_extractor(Box::new(extractor));

    let meta = StorageMeta {
        importance_hint: Some(0.7),
        source: Some("integration-test".into()),
        namespace: Some("ns-q".into()),
        user_metadata: serde_json::json!({ "retry_tag": "first-attempt" }),
        memory_type_hint: None,
    };

    let out = mem
        .store_raw("transient extractor failure target content", meta)
        .expect("store_raw returns Ok even when extractor fails");

    // Must be Quarantined, not Skipped and not Stored.
    match out {
        RawStoreOutcome::Quarantined { id, reason } => {
            assert!(
                !id.as_str().is_empty(),
                "quarantine id should be non-empty"
            );
            assert!(
                matches!(reason, QuarantineReason::ExtractorError(_)),
                "expected ExtractorError, got {:?}",
                reason
            );
        }
        other => panic!("expected Quarantined, got {:?}", other),
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1, "extractor called exactly once");

    // Row should be live and counted.
    assert_eq!(mem.count_quarantine().expect("count_quarantine ok"), 1);
}

#[test]
fn store_raw_extractor_error_dedups_on_repeat() {
    // Same content + same failure reason should NOT produce two
    // quarantine rows. Dedup is by content_hash on live rows.
    let mut mem = new_mem();
    let (extractor, _) = ProgrammableExtractor::new(999);
    mem.set_extractor(Box::new(extractor));

    let meta = StorageMeta::default();
    let _ = mem.store_raw("dedupable failing content", meta.clone()).unwrap();
    let _ = mem.store_raw("dedupable failing content", meta).unwrap();

    assert_eq!(
        mem.count_quarantine().unwrap(),
        1,
        "quarantine dedup on repeated failure of same content"
    );
}

#[test]
fn retry_quarantined_recovers_transient_failure() {
    // Core acceptance test per design §12 Step 6:
    //   "induce extractor failure, confirm row lands in quarantine,
    //    retry recovers it."
    //
    // Scenario: extractor fails on first call, succeeds on the second.
    // store_raw quarantines, then retry_quarantined runs the extractor
    // again and promotes the row into memories.
    let mut mem = new_mem();
    let (extractor, calls) = ProgrammableExtractor::new(1);
    mem.set_extractor(Box::new(extractor));

    let meta = StorageMeta {
        importance_hint: Some(0.8),
        source: Some("retry-test".into()),
        namespace: None, // default namespace so `recall()` finds it
        user_metadata: serde_json::Value::Null,
        memory_type_hint: None,
    };

    let first = mem
        .store_raw("content we expect to eventually succeed", meta)
        .unwrap();
    assert!(matches!(first, RawStoreOutcome::Quarantined { .. }));
    assert_eq!(mem.count_quarantine().unwrap(), 1);

    // Retry pass — the extractor now succeeds on call #2.
    let report = mem.retry_quarantined(10).expect("retry ok");
    assert_eq!(report.attempted, 1);
    assert_eq!(
        report.recovered.len(),
        1,
        "one row promoted into memories"
    );
    assert!(
        report.still_failing.is_empty(),
        "no rows should still be failing, got {:?}",
        report.still_failing
    );
    assert!(
        report.permanently_rejected.is_empty(),
        "not enough attempts to flip any row to permanently_rejected"
    );

    // Quarantine is now empty; content is recallable from the main table.
    assert_eq!(mem.count_quarantine().unwrap(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "one initial + one retry");

    let hits = mem
        .recall("content we expect", 5, None, None)
        .expect("recall ok");
    assert!(
        !hits.is_empty(),
        "recovered memory must be recallable from main table"
    );
}

#[test]
fn retry_quarantined_permanent_rejection_after_max_attempts() {
    // An extractor that never succeeds. After 5 retries (on top of
    // the initial failed store_raw call that bumps attempts implicitly
    // to 0 → 1 on the first retry), the row must flip to
    // permanently_rejected and appear in the report.
    let mut mem = new_mem();
    let (extractor, _) = ProgrammableExtractor::new(999); // always fails
    mem.set_extractor(Box::new(extractor));

    let meta = StorageMeta::default();
    let _ = mem
        .store_raw("unrecoverable content", meta)
        .expect("initial store_raw returns quarantine outcome");
    assert_eq!(mem.count_quarantine().unwrap(), 1);

    // Memory::QUARANTINE_MAX_ATTEMPTS is 5. Run enough retry passes
    // to exhaust the counter; each pass bumps attempts by 1.
    let mut hit_rejection = false;
    for _ in 0..Memory::QUARANTINE_MAX_ATTEMPTS {
        let report = mem.retry_quarantined(10).expect("retry ok");
        // While the row is live, each pass sees it once.
        if !report.permanently_rejected.is_empty() {
            assert_eq!(report.permanently_rejected.len(), 1);
            hit_rejection = true;
            break;
        }
        assert_eq!(report.still_failing.len(), 1);
        assert!(report.recovered.is_empty());
    }

    assert!(
        hit_rejection,
        "row must be flipped to permanently_rejected within QUARANTINE_MAX_ATTEMPTS passes"
    );

    // Once rejected, list-for-retry excludes it — live count is 0.
    assert_eq!(
        mem.count_quarantine().unwrap(),
        0,
        "permanently_rejected rows are not counted as live"
    );

    // Further retry passes are a no-op (nothing live to retry).
    let noop = mem.retry_quarantined(10).unwrap();
    assert_eq!(noop.attempted, 0);
}

#[test]
fn purge_rejected_quarantine_respects_ttl() {
    // Build a rejected row, then call purge with a TTL in the past
    // (negative) so the row qualifies for deletion.
    let mut mem = new_mem();
    let (extractor, _) = ProgrammableExtractor::new(999);
    mem.set_extractor(Box::new(extractor));

    mem.store_raw("to be rejected and purged", StorageMeta::default())
        .unwrap();
    for _ in 0..Memory::QUARANTINE_MAX_ATTEMPTS {
        let _ = mem.retry_quarantined(10).unwrap();
    }

    // Negative TTL → cutoff in the future → rejected rows with any
    // last_attempt_at in the past are eligible.
    let purged = mem
        .purge_rejected_quarantine(Some(-9999))
        .expect("purge ok");
    assert_eq!(purged, 1, "one permanently-rejected row purged");
}
