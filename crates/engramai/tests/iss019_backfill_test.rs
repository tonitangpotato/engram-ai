//! ISS-019 Step 7b — end-to-end v1 → backfill_queue → v2 pipeline.
//!
//! These tests insert v1-shaped rows into `memories` via raw SQL
//! (bypassing the write path entirely, to simulate pre-ISS-019 data),
//! then drive the two explicit API entry points added in Step 7b:
//!
//!   1. `scan_and_enqueue_backfill` — classifies rows, enqueues
//!      LowDimLegacy ones into `backfill_queue`.
//!   2. `backfill_dimensions` — drains the queue, re-runs a stub
//!      extractor, merges new dimensions into the existing row,
//!      rewriting its metadata to the v2 layout.
//!
//! See design §6 (Migration strategy) and step7a-handoff.md (deferred
//! Step 7b scope).

use engramai::extractor::{ExtractedFact, MemoryExtractor};
use engramai::memory::Memory;
use engramai::storage::Storage;
use rusqlite::params;
use serde_json::json;
use std::error::Error as StdError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

// ---------------------------------------------------------------------
// Stub extractor — produces one fact with rich dimensional signature
// drawn from the input content. Used to verify backfill actually adds
// dimensions to a row that previously had none.
// ---------------------------------------------------------------------

struct StubExtractor {
    calls: Arc<AtomicUsize>,
    fail_first_n: usize,
}

impl StubExtractor {
    fn new(fail_first_n: usize) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: Arc::clone(&calls),
                fail_first_n,
            },
            calls,
        )
    }
}

impl MemoryExtractor for StubExtractor {
    fn extract(
        &self,
        text: &str,
    ) -> Result<Vec<ExtractedFact>, Box<dyn StdError + Send + Sync>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_first_n {
            return Err(format!("stub extractor forced failure {n}").into());
        }
        // Synthesise a dimensional-rich fact from the input. The
        // classifier only checks field presence, so concrete values
        // don't matter — just ensure the fields are non-empty.
        let fact = ExtractedFact {
            core_fact: text.chars().take(120).collect(),
            participants: Some("alice, bob".into()),
            temporal: Some("2026-04-22".into()),
            causation: Some("stub-extractor-backfill".into()),
            importance: 0.6,
            confidence: "likely".into(),
            domain: "coding".into(),
            valence: 0.15,
            tags: vec!["backfill".into()],
            ..Default::default()
        };
        Ok(vec![fact])
    }
}

// ---------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------

/// Insert a v1-shape row into `memories` via raw SQL.
fn insert_v1_row(
    storage: &Storage,
    id: &str,
    content: &str,
    metadata: serde_json::Value,
) {
    storage
        .conn()
        .execute(
            "INSERT INTO memories
              (id, content, importance, memory_type, layer, metadata, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                content,
                0.5_f64,
                "episodic",
                "working",
                serde_json::to_string(&metadata).unwrap(),
                1700000000_f64,
            ],
        )
        .unwrap();
}

/// Open a Memory engine against a concrete SQLite path (not :memory:)
/// so we can seed it first via a separate Storage handle, then reopen.
fn new_mem_at(path: &std::path::Path) -> Memory {
    Memory::new(path.to_str().unwrap(), None).expect("memory engine boots")
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

/// scan_and_enqueue_backfill correctly classifies each v1 shape and
/// only enqueues LowDimLegacy rows.
#[test]
fn scan_classifies_and_enqueues_only_lowdim_legacy_rows() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("scan.db");

    // Seed via raw SQL before opening the Memory engine.
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        // v1 "HasExtractorData" — should NOT be enqueued.
        insert_v1_row(
            &storage,
            "mem-has-data",
            "content with enough length to pass the 40-char threshold easily here",
            json!({
                "dimensions": {
                    "participants": "alice",
                    "domain": "coding",
                    "valence": 0.1
                }
            }),
        );
        // v1 "DimensionsEmpty" — long content → ENQUEUE.
        insert_v1_row(
            &storage,
            "mem-empty",
            "long enough content with more than forty characters so it enqueues",
            json!({ "dimensions": {} }),
        );
        // v1 "MissingCoreDimensions" — long content → ENQUEUE.
        insert_v1_row(
            &storage,
            "mem-missing-core",
            "pre-extractor era long content that lacks any dimensions whatsoever",
            json!({ "merge_count": 0 }),
        );
        // v1 "PartialDimensionsLongContent" — long → ENQUEUE.
        insert_v1_row(
            &storage,
            "mem-partial",
            "content long enough that partial dimensions trigger a backfill now",
            json!({
                "dimensions": {
                    "participants": "alice"
                }
            }),
        );
        // v1 short content with no dimensions — UnparseableLegacy (SKIP).
        insert_v1_row(
            &storage,
            "mem-short",
            "short",
            json!({}),
        );
    }

    let mut mem = new_mem_at(&db);
    let report = mem
        .scan_and_enqueue_backfill(1000)
        .expect("scan succeeds");

    assert_eq!(report.scanned, 5, "every seeded row was inspected");
    assert_eq!(report.has_extractor_data, 1, "mem-has-data classified as HasExtractorData");
    assert_eq!(report.enqueued, 3, "three LowDimLegacy rows enqueued");
    assert_eq!(report.unparseable, 1, "short row skipped as unparseable");
    assert_eq!(report.already_v2, 0);

    assert_eq!(mem.count_backfill().unwrap(), 3);
}

/// scan is idempotent: running it twice doesn't double-enqueue.
#[test]
fn scan_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("scan_idem.db");
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        insert_v1_row(
            &storage,
            "mem-1",
            "long enough content to pass the backfill threshold for this test",
            json!({ "dimensions": {} }),
        );
    }

    let mut mem = new_mem_at(&db);
    mem.scan_and_enqueue_backfill(1000).unwrap();
    assert_eq!(mem.count_backfill().unwrap(), 1);

    // Second run: still one live row.
    mem.scan_and_enqueue_backfill(1000).unwrap();
    assert_eq!(mem.count_backfill().unwrap(), 1);
}

/// backfill_dimensions upgrades a v1 row to v2 by re-running the
/// extractor and merging the new dimensions into the existing row.
#[test]
fn backfill_dimensions_upgrades_v1_row_to_v2() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("backfill.db");

    let content = "long enough content to enqueue and then be backfilled here";
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        insert_v1_row(
            &storage,
            "mem-1",
            content,
            json!({ "dimensions": {} }),
        );
    }

    let mut mem = new_mem_at(&db);
    let (extractor, calls) = StubExtractor::new(0); // never fail
    mem.set_extractor(Box::new(extractor));

    // Pre-backfill: metadata is v1-shaped (no `engram` namespace).
    let meta_before: String = mem
        .connection()
        .query_row(
            "SELECT metadata FROM memories WHERE id = 'mem-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let meta_before_val: serde_json::Value = serde_json::from_str(&meta_before).unwrap();
    assert!(
        meta_before_val.get("engram").is_none(),
        "seeded row is v1 (no engram namespace)"
    );

    // Scan → enqueue.
    mem.scan_and_enqueue_backfill(1000).unwrap();
    assert_eq!(mem.count_backfill().unwrap(), 1);

    // Backfill.
    let report = mem
        .backfill_dimensions(10)
        .expect("backfill succeeds");
    assert_eq!(report.attempted, 1);
    assert_eq!(report.upgraded, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.permanently_rejected, 0);

    // Queue is empty.
    assert_eq!(mem.count_backfill().unwrap(), 0);

    // Extractor was called exactly once.
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Metadata is now v2-shaped.
    let meta_after: String = mem
        .connection()
        .query_row(
            "SELECT metadata FROM memories WHERE id = 'mem-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let meta_after_val: serde_json::Value = serde_json::from_str(&meta_after).unwrap();
    let engram = meta_after_val
        .get("engram")
        .expect("v2 engram namespace present");
    let dims = engram
        .get("dimensions")
        .and_then(|d| d.as_object())
        .expect("engram.dimensions object present");

    // Dimensions from the stub extractor propagated through merge.
    assert_eq!(
        dims.get("participants").and_then(|v| v.as_str()),
        Some("alice, bob"),
    );
    assert_eq!(
        dims.get("causation").and_then(|v| v.as_str()),
        Some("stub-extractor-backfill"),
    );
}

/// backfill_dimensions without an extractor returns an empty report
/// (re-extraction requires an extractor; this is not an error).
#[test]
fn backfill_dimensions_is_noop_without_extractor() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("backfill_no_ext.db");
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        insert_v1_row(
            &storage,
            "mem-1",
            "long enough content for the backfill threshold in this test",
            json!({ "dimensions": {} }),
        );
    }

    let mut mem = new_mem_at(&db);
    mem.scan_and_enqueue_backfill(1000).unwrap();
    assert_eq!(mem.count_backfill().unwrap(), 1);

    let report = mem.backfill_dimensions(10).unwrap();
    assert_eq!(report.attempted, 0);
    assert_eq!(report.upgraded, 0);
    // Queue still holds the row — nothing was done.
    assert_eq!(mem.count_backfill().unwrap(), 1);
}

/// Extractor failures bump attempts; repeated failures flip the row
/// to permanently_rejected once BACKFILL_MAX_ATTEMPTS is reached.
#[test]
fn backfill_permanent_rejection_after_max_attempts() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("backfill_reject.db");
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        insert_v1_row(
            &storage,
            "mem-1",
            "long enough content to qualify for the backfill queue here now",
            json!({ "dimensions": {} }),
        );
    }

    let mut mem = new_mem_at(&db);
    // Force failures on every extract() call.
    let (extractor, _calls) = StubExtractor::new(usize::MAX);
    mem.set_extractor(Box::new(extractor));

    mem.scan_and_enqueue_backfill(1000).unwrap();

    // Drive backfill up to (and past) the max-attempts threshold.
    for i in 0..Memory::BACKFILL_MAX_ATTEMPTS {
        let report = mem.backfill_dimensions(10).unwrap();
        assert_eq!(report.attempted, 1, "pass {i}: row still in queue");
        if i + 1 < Memory::BACKFILL_MAX_ATTEMPTS {
            assert_eq!(report.failed, 1);
            assert_eq!(report.permanently_rejected, 0);
        } else {
            // Final attempt crosses the threshold.
            assert_eq!(report.permanently_rejected, 1);
        }
    }

    // After the cap: no live rows, the row flipped to rejected.
    assert_eq!(mem.count_backfill().unwrap(), 0);
    let next = mem.backfill_dimensions(10).unwrap();
    assert_eq!(next.attempted, 0, "rejected rows are invisible to list_backfill_batch");
}

/// If a v1 row gets rewritten to v2 by some other path between scan
/// and backfill, backfill_dimensions recognises it and drops the queue
/// entry without re-running the extractor.
#[test]
fn backfill_drops_queue_for_already_v2_rows() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("backfill_noop.db");
    {
        let storage = Storage::new(db.to_str().unwrap()).unwrap();
        insert_v1_row(
            &storage,
            "mem-1",
            "long enough content for the backfill threshold in this case",
            json!({ "dimensions": {} }),
        );
    }

    let mut mem = new_mem_at(&db);
    let (extractor, calls) = StubExtractor::new(0);
    mem.set_extractor(Box::new(extractor));

    // Enqueue the v1 row directly (skip scan for test clarity).
    mem.storage()
        .enqueue_backfill("mem-1", "dimensions_empty", None)
        .unwrap();

    // Independently rewrite the row to v2-shaped metadata to simulate
    // another write path having already done the work.
    let v2_meta = json!({
        "engram": {
            "version": 2,
            "dimensions": {
                "core_fact": "already upgraded",
                "participants": "external",
                "domain": "general",
                "confidence": "likely",
                "valence": 0.0,
                "tags": []
            },
            "merge_count": 0,
            "merge_history": []
        },
        "user": {}
    });
    mem.connection()
        .execute(
            "UPDATE memories SET metadata = ? WHERE id = 'mem-1'",
            params![serde_json::to_string(&v2_meta).unwrap()],
        )
        .unwrap();

    let report = mem.backfill_dimensions(10).unwrap();
    assert_eq!(report.attempted, 1);
    assert_eq!(report.upgraded, 0);
    assert_eq!(report.unchanged, 1);
    // Extractor was never invoked.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // Queue drained.
    assert_eq!(mem.count_backfill().unwrap(), 0);
}
