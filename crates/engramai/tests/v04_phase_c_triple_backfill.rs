//! T26a — Phase C resumable triple-extraction backfill driver.
//!
//! These tests exercise the **infrastructure** behaviours of the
//! driver: idempotency, resumability via checkpoint, rate limiting,
//! retry, namespace filter, and the audit row. **No live API calls
//! are made** — every test injects either `NoopTripleExtractor` or
//! the in-test `CountingMockExtractor`.
//!
//! Design ref: `.gid/features/v04-unified-substrate/design.md` §8.4 T26a.

use std::error::Error;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::substrate::triple_backfill::{
    backfill_triples_from_memories, TripleBackfillOpts,
};
use engramai::triple::{Predicate, Triple, TripleSource};
use engramai::triple_extractor::{NoopTripleExtractor, TripleExtractor};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

// ===================================================================
// Fixtures
// ===================================================================

fn sample_record(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 14, 12, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "t26a-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

/// In-test extractor that:
///   - Returns one canned triple per call (deterministic),
///   - Counts call attempts,
///   - Can be programmed to fail the first N calls on each memory
///     for retry-path coverage.
struct CountingMockExtractor {
    calls: Mutex<u32>,
    fail_first_n: Mutex<u32>,
}

impl CountingMockExtractor {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
            fail_first_n: Mutex::new(0),
        }
    }
    fn with_failures(n: u32) -> Self {
        Self {
            calls: Mutex::new(0),
            fail_first_n: Mutex::new(n),
        }
    }
    fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

impl TripleExtractor for CountingMockExtractor {
    fn extract_triples(
        &self,
        content: &str,
    ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        *self.calls.lock().unwrap() += 1;
        let mut remaining = self.fail_first_n.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
            return Err("simulated upstream failure".into());
        }
        // Single deterministic triple derived from content prefix.
        let subject = format!("subj_of_{}", content.chars().take(4).collect::<String>());
        let object = format!("obj_of_{}",  content.chars().take(4).collect::<String>());
        Ok(vec![Triple {
            subject,
            predicate: Predicate::RelatedTo,
            object,
            confidence: 0.9,
            source: TripleSource::Llm,
            subject_kind_hint: None,
            object_kind_hint: None,
        }])
    }
}

fn seed_memory(storage: &mut Storage, id: &str, content: &str, ns: &str) {
    let rec = sample_record(id, content);
    storage.add(&rec, ns).expect("seed memory");
}

// ===================================================================
// Tests
// ===================================================================

#[test]
fn t26a_noop_extractor_zero_inserts_clean_audit() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "alpha content", "default");
    seed_memory(&mut storage, "m-2", "beta content",  "default");

    let opts = TripleBackfillOpts::default();
    let run = backfill_triples_from_memories(&storage, &NoopTripleExtractor::new(), &opts)
        .expect("backfill");

    assert_eq!(run.legacy_table, "triples");
    assert_eq!(run.rows_read, 2);
    assert_eq!(run.rows_inserted, 0, "noop produces no triples");
    assert_eq!(run.rows_skipped_existing, 0);
    assert_eq!(run.rows_failed, 0);

    // Checkpoint flipped to completed.
    let status: String = storage.conn().query_row(
        "SELECT status FROM triple_backfill_checkpoint WHERE run_id = ?",
        params![run.run_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(status, "completed");

    // Audit row finished_at set.
    let finished: Option<f64> = storage.conn().query_row(
        "SELECT finished_at FROM backfill_runs WHERE run_id = ?",
        params![run.run_id],
        |r| r.get(0),
    ).unwrap();
    assert!(finished.is_some(), "finished_at populated");
}

#[test]
fn t26a_mock_extractor_inserts_triples_and_counts_correctly() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "first memory content", "default");
    seed_memory(&mut storage, "m-2", "second memory content", "default");
    seed_memory(&mut storage, "m-3", "third memory content", "default");

    let mock = CountingMockExtractor::new();
    let opts = TripleBackfillOpts::default();
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");

    assert_eq!(run.rows_read, 3);
    assert_eq!(run.rows_inserted, 3, "one triple per memory");
    assert_eq!(run.rows_failed, 0);
    assert_eq!(mock.call_count(), 3);

    // Triples landed in the legacy table.
    let triple_count: i64 = storage.conn().query_row(
        "SELECT COUNT(*) FROM triples", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(triple_count, 3);
}

#[test]
fn t26a_skips_memories_that_already_have_triples() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "content one", "default");
    seed_memory(&mut storage, "m-2", "content two", "default");

    // First run extracts both.
    let mock1 = CountingMockExtractor::new();
    let run1 = backfill_triples_from_memories(&storage, &mock1, &TripleBackfillOpts::default())
        .expect("run1");
    assert_eq!(run1.rows_inserted, 2);

    // Second run: both memories now have triples → all skipped, no
    // extractor calls.
    let mock2 = CountingMockExtractor::new();
    let run2 = backfill_triples_from_memories(&storage, &mock2, &TripleBackfillOpts::default())
        .expect("run2");
    assert_eq!(run2.rows_read, 2);
    assert_eq!(run2.rows_inserted, 0);
    assert_eq!(run2.rows_skipped_existing, 2);
    assert_eq!(mock2.call_count(), 0, "extractor not called for already-extracted memories");
}

#[test]
fn t26a_retry_succeeds_within_budget() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "retry content", "default");

    // Extractor fails twice then succeeds; max_retries=3 covers it.
    let mock = CountingMockExtractor::with_failures(2);
    let opts = TripleBackfillOpts {
        max_retries: 3,
        retry_backoff_ms: 1, // fast for tests
        ..TripleBackfillOpts::default()
    };
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");

    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1, "succeeded after retries");
    assert_eq!(run.rows_failed, 0);
    assert_eq!(mock.call_count(), 3, "2 failures + 1 success");
}

#[test]
fn t26a_retry_exhausted_counts_as_failed() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "always-fails content", "default");

    // 5 programmed failures, retry budget only 2 → exhausted.
    let mock = CountingMockExtractor::with_failures(5);
    let opts = TripleBackfillOpts {
        max_retries: 2,
        retry_backoff_ms: 1,
        ..TripleBackfillOpts::default()
    };
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");

    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_failed, 1);
    assert_eq!(mock.call_count(), 3, "1 initial + 2 retries = 3 attempts");
}

#[test]
fn t26a_resume_picks_up_after_crashed_run() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "a", "default");
    seed_memory(&mut storage, "m-2", "b", "default");
    seed_memory(&mut storage, "m-3", "c", "default");

    // Simulate a previous in-progress run that processed up through m-1.
    storage.conn().execute(
        r#"
        INSERT INTO triple_backfill_checkpoint (
            run_id, last_memory_id, memories_processed, triples_inserted,
            memories_failed, status, started_at, updated_at,
            namespace_filter, notes
        ) VALUES ('ckpt-prior', 'm-1', 1, 1, 0, 'in_progress',
                  1747000000.0, 1747000000.0, NULL, '{}')
        "#,
        [],
    ).unwrap();
    // Seed the triple row that the prior run "wrote" for m-1.
    storage.conn().execute(
        r#"INSERT INTO triples (memory_id, subject, predicate, object,
              confidence, source, created_at)
            VALUES ('m-1', 's', 'related_to', 'o', 0.9, 'llm',
                    '2026-05-14T12:00:00Z')"#,
        [],
    ).unwrap();

    // New run: should resume past m-1 (m-1 also has triple row → would
    // skip anyway, but the cursor handoff is the contract under test).
    let mock = CountingMockExtractor::new();
    let run = backfill_triples_from_memories(&storage, &mock, &TripleBackfillOpts::default())
        .expect("resume");

    // The new run iterates m-2 and m-3 (m-1 already-extracted →
    // either skipped-via-cursor or skipped-via-existing-triples).
    assert!(run.rows_inserted >= 2, "at least m-2 and m-3 freshly extracted");
    // m-1 should NOT be re-extracted by the mock.
    let m1_extracted = mock.call_count() < 3;
    assert!(m1_extracted, "mock should have been called <3 times — m-1 skipped");
}

#[test]
fn t26a_namespace_filter_restricts_scope() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-a1", "ns-a content 1", "ns-a");
    seed_memory(&mut storage, "m-a2", "ns-a content 2", "ns-a");
    seed_memory(&mut storage, "m-b1", "ns-b content 1", "ns-b");

    let mock = CountingMockExtractor::new();
    let opts = TripleBackfillOpts {
        namespace_filter: Some("ns-a".to_string()),
        ..TripleBackfillOpts::default()
    };
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");

    assert_eq!(run.rows_read, 2, "only ns-a memories iterated");
    assert_eq!(mock.call_count(), 2);

    // ns-b memory got no triples.
    let ns_b_triples: i64 = storage.conn().query_row(
        "SELECT COUNT(*) FROM triples WHERE memory_id = 'm-b1'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(ns_b_triples, 0);
}

#[test]
fn t26a_rate_limit_enforces_lower_bound_interval() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "x", "default");
    seed_memory(&mut storage, "m-2", "y", "default");
    seed_memory(&mut storage, "m-3", "z", "default");

    // 10 rps → 100ms minimum between calls. 3 calls → ≥200ms total
    // (first call is immediate; only the second and third are gated).
    let mock = CountingMockExtractor::new();
    let opts = TripleBackfillOpts {
        rate_limit_per_sec: 10.0,
        ..TripleBackfillOpts::default()
    };
    let start = Instant::now();
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");
    let elapsed = start.elapsed();

    assert_eq!(run.rows_inserted, 3);
    assert!(
        elapsed.as_millis() >= 200,
        "rate limit not enforced: elapsed={}ms (expected ≥200ms for 3 calls @10rps)",
        elapsed.as_millis()
    );
    // Generous upper bound to keep CI from flaking.
    assert!(elapsed.as_millis() < 2_000, "elapsed too high: {}ms", elapsed.as_millis());
}

#[test]
fn t26a_max_memories_caps_invocation() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    for i in 0..10 {
        seed_memory(&mut storage, &format!("m-{i:02}"), "content", "default");
    }
    let mock = CountingMockExtractor::new();
    let opts = TripleBackfillOpts {
        max_memories: Some(3),
        ..TripleBackfillOpts::default()
    };
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");
    assert_eq!(run.rows_read, 3, "cap honored");
    assert_eq!(mock.call_count(), 3);
}

#[test]
fn t26a_counter_invariant_holds() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_memory(&mut storage, "m-1", "always-fails", "default");
    seed_memory(&mut storage, "m-2", "succeeds-after-retry", "default");
    seed_memory(&mut storage, "m-3", "fresh", "default");

    // m-1 already has triples → skipped path.
    storage.conn().execute(
        r#"INSERT INTO triples (memory_id, subject, predicate, object,
              confidence, source, created_at)
            VALUES ('m-1', 's', 'related_to', 'o', 0.9, 'llm',
                    '2026-05-14T00:00:00Z')"#,
        [],
    ).unwrap();

    // Programmed: first call fails, then succeeds. With max_retries=1
    // → m-2 succeeds on 2nd attempt; m-3 succeeds on 1st attempt
    // (no further failures programmed).
    let mock = CountingMockExtractor::with_failures(1);
    let opts = TripleBackfillOpts {
        max_retries: 1,
        retry_backoff_ms: 1,
        ..TripleBackfillOpts::default()
    };
    let run = backfill_triples_from_memories(&storage, &mock, &opts).expect("backfill");

    assert_eq!(run.rows_read, 3);
    assert_eq!(run.rows_skipped_existing, 1, "m-1");
    assert_eq!(run.rows_failed, 0);
    assert_eq!(run.rows_inserted, 2, "m-2 and m-3");
    // Per-memory: 1 skipped + 0 failed + 2 inserted = 3 read ✓
    assert_eq!(
        run.rows_skipped_existing + run.rows_failed + 2, // memories that produced triples
        run.rows_read
    );
}
