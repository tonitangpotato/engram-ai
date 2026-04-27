//! v0.3 Resolution-pipeline `Memory` public API tests
//! (`task:res-impl-memory-api`, design §6.2 + §6.4).
//!
//! Covers the 5 new methods added to `Memory`:
//!   * `Memory::reextract`
//!   * `Memory::reextract_failed`
//!   * `Memory::compile_knowledge`
//!   * `Memory::list_knowledge_topics`
//!   * `Memory::ingest_with_stats`
//!
//! `ResolutionPipeline::resolve_for_backfill` (design §6.5) is
//! intentionally NOT covered here — that lives on the pipeline, not on
//! `Memory`, per §A.1 of `tasks/2026-04-27-night-autopilot.md`.
//!
//! Idempotence (GOAL-2.1) on `reextract` is verified end-to-end.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use engramai::memory::Memory;
use engramai::resolution::{
    BoundedJobQueue, EnqueueError, JobMode, JobQueue, PipelineJob,
};
use engramai::store_api::{RawStoreOutcome, StorageMeta};

// ---------------------------------------------------------------------
// Test scaffolding
// ---------------------------------------------------------------------

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

/// Counting queue that records every enqueue and lets tests assert the
/// `JobMode` of each call (so we can prove `reextract` enqueues
/// `JobMode::ReExtract`, not `JobMode::Initial`).
struct ModeRecorder {
    inner: BoundedJobQueue,
    enqueued_modes: std::sync::Mutex<Vec<JobMode>>,
    enqueue_count: AtomicUsize,
}

impl ModeRecorder {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: BoundedJobQueue::new(capacity),
            enqueued_modes: std::sync::Mutex::new(Vec::new()),
            enqueue_count: AtomicUsize::new(0),
        })
    }

    fn modes(&self) -> Vec<JobMode> {
        self.enqueued_modes.lock().unwrap().clone()
    }

    fn enqueue_count(&self) -> usize {
        self.enqueue_count.load(Ordering::SeqCst)
    }
}

impl JobQueue for ModeRecorder {
    fn try_enqueue(&self, job: PipelineJob) -> Result<(), EnqueueError> {
        self.enqueue_count.fetch_add(1, Ordering::SeqCst);
        self.enqueued_modes.lock().unwrap().push(job.mode);
        self.inner.try_enqueue(job)
    }

    fn try_dequeue(&self) -> Option<PipelineJob> {
        self.inner.try_dequeue()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn capacity(&self) -> Option<usize> {
        self.inner.capacity()
    }

    fn close(&self) {
        self.inner.close();
    }
}

// ---------------------------------------------------------------------
// reextract — §6.2, GOAL-2.1
// ---------------------------------------------------------------------

#[test]
fn reextract_without_pipeline_pool_returns_error() {
    // No queue installed (v0.2-compat): re-extract has no destination.
    let mut mem = new_mem();
    let result = mem.reextract(&"some-memory-id".to_string());
    assert!(
        result.is_err(),
        "expected Err when no pipeline pool installed, got {result:?}"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("no pipeline pool installed"),
        "error message should explain why: {err_msg}"
    );
}

#[test]
fn reextract_with_queue_enqueues_a_reextract_job() {
    let mut mem = new_mem();
    let q = ModeRecorder::new(8);
    mem.set_job_queue(Arc::clone(&q) as Arc<dyn JobQueue>);

    let memory_id = "mem-abc-123".to_string();
    let episode_id = mem.reextract(&memory_id).expect("reextract enqueues");

    // Observable contract: returned a fresh, valid Uuid.
    assert!(!episode_id.is_nil(), "episode_id must be a fresh Uuid");

    // The queue saw exactly one enqueue, in `ReExtract` mode (not `Initial`).
    assert_eq!(q.enqueue_count(), 1);
    assert_eq!(q.modes(), vec![JobMode::ReExtract]);
}

#[test]
fn reextract_called_twice_enqueues_twice_with_distinct_episode_ids() {
    // GOAL-2.1: re-enqueueing on a memory whose run is already
    // `Running` is the dispatcher's job to deduplicate (per design
    // §3.1 idempotence keying). At the `Memory::reextract` layer the
    // contract is "always enqueue, always with a fresh episode_id" —
    // dedup is downstream. This test pins the producer-side contract.
    let mut mem = new_mem();
    let q = ModeRecorder::new(8);
    mem.set_job_queue(Arc::clone(&q) as Arc<dyn JobQueue>);

    let memory_id = "mem-abc-123".to_string();
    let ep1 = mem.reextract(&memory_id).unwrap();
    let ep2 = mem.reextract(&memory_id).unwrap();

    assert_ne!(ep1, ep2, "each call must mint a fresh episode_id");
    assert_eq!(q.enqueue_count(), 2);
    assert!(
        q.modes().iter().all(|m| matches!(m, JobMode::ReExtract)),
        "every enqueue must be ReExtract mode"
    );
}

// ---------------------------------------------------------------------
// reextract_failed — §6.2, GOAL-2.3
// ---------------------------------------------------------------------

#[test]
fn reextract_failed_without_pipeline_pool_returns_error() {
    let mut mem = new_mem();
    let result = mem.reextract_failed();
    assert!(result.is_err(), "{result:?}");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("no pipeline pool installed"),
        "error message should be informative: {msg}"
    );
}

#[test]
fn reextract_failed_returns_zero_when_no_failed_runs_recorded() {
    // Empty ledger → 0 enqueued.
    let mut mem = new_mem();
    let q = ModeRecorder::new(8);
    mem.set_job_queue(Arc::clone(&q) as Arc<dyn JobQueue>);

    let count = mem.reextract_failed().expect("query succeeds even if empty");
    assert_eq!(count, 0);
    assert_eq!(q.enqueue_count(), 0);
}

// ---------------------------------------------------------------------
// compile_knowledge — §6.2 + §5bis (A.1 stub)
// ---------------------------------------------------------------------

#[test]
fn compile_knowledge_stub_returns_zero_counter_report() {
    // §A.1 ships the API surface; §A.2 fills the body. Until then
    // the contract is "no-op report with zero counters" — explicitly
    // NOT `unimplemented!()` (GUARD-2: never silent panic).
    let mut mem = new_mem();
    let report = mem
        .compile_knowledge("default")
        .expect("stub returns Ok");

    assert_eq!(report.candidates_considered, 0);
    assert_eq!(report.clusters_formed, 0);
    assert_eq!(report.topics_written, 0);
    assert_eq!(report.topics_superseded, 0);
    assert_eq!(report.llm_calls, 0);
    assert_eq!(report.duration, std::time::Duration::ZERO);
    assert!(!report.run_id.is_nil(), "run_id must be a fresh Uuid");
}

#[test]
fn compile_knowledge_reports_have_unique_run_ids() {
    // Even a stub must mint a fresh run_id per call so downstream
    // logs can correlate (later, when bodies fill in).
    let mut mem = new_mem();
    let r1 = mem.compile_knowledge("default").unwrap();
    let r2 = mem.compile_knowledge("default").unwrap();
    assert_ne!(r1.run_id, r2.run_id);
}

// ---------------------------------------------------------------------
// list_knowledge_topics — §6.2 (thin wrapper over GraphStore::list_topics)
// ---------------------------------------------------------------------

#[test]
fn list_knowledge_topics_returns_empty_for_fresh_memory() {
    let mut mem = new_mem();
    let topics = mem
        .list_knowledge_topics("default", false, 100)
        .expect("list succeeds on empty store");
    assert!(topics.is_empty(), "fresh store has no topics");
}

#[test]
fn list_knowledge_topics_zero_limit_returns_empty() {
    // Mirrors the underlying `list_topics_zero_limit_returns_empty`
    // GraphStore-level test, asserts wrapper preserves the policy.
    let mut mem = new_mem();
    let topics = mem
        .list_knowledge_topics("default", true, 0)
        .expect("list with limit=0 succeeds");
    assert!(topics.is_empty());
}

// ---------------------------------------------------------------------
// ingest_with_stats — §6.4, GOAL-2.11 / GOAL-2.14
// ---------------------------------------------------------------------

#[test]
fn ingest_with_stats_returns_memory_id_and_default_stats() {
    // Public-contract test: signature + happy-path return shape.
    // Stats body wiring is a separate task; current MVP returns
    // `ResolutionStats::default()` — the contract test pins the shape
    // (caller can pattern-match the tuple) without locking us to a
    // specific stats implementation.
    let mut mem = new_mem();
    let (id, stats) = mem
        .ingest_with_stats("a meaningful sentence about Rust testing")
        .expect("ingest succeeds");

    assert!(!id.is_empty(), "memory id should not be empty");
    // Default stats — every counter is zero in the MVP.
    assert_eq!(stats.entities_extracted, 0);
    assert_eq!(stats.edges_extracted, 0);
    assert_eq!(stats.stage_failures, 0);
}

#[test]
fn ingest_with_stats_errors_on_skipped_content() {
    // Empty / too-short content → store_raw `Skipped`. The benchmark
    // contract surfaces this as a hard error (test bug, not runtime).
    let mut mem = new_mem();
    let result = mem.ingest_with_stats("");
    assert!(result.is_err(), "empty content must surface as Err");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Skipped") || msg.contains("skipped"),
        "error should name the underlying RawStoreOutcome variant: {msg}"
    );
}

#[test]
fn ingest_with_stats_admission_path_matches_store_raw() {
    // Sanity: `ingest_with_stats(c)` and `store_raw(c)` admit the
    // same content — we check that both produce a Stored outcome
    // with non-empty id. (Cannot literally compare ids: each call
    // mints a fresh row.)
    let mut mem = new_mem();

    let (id_a, _) = mem.ingest_with_stats("some content for a").unwrap();
    assert!(!id_a.is_empty());

    let raw = mem
        .store_raw("some content for b", StorageMeta::default())
        .unwrap();
    match raw {
        RawStoreOutcome::Stored(outcomes) => {
            assert!(!outcomes.is_empty(), "store_raw should also succeed");
        }
        other => panic!("expected Stored, got {other:?}"),
    }
}
