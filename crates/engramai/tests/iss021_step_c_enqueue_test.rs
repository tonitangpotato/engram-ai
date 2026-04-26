//! v0.3 §3.1 Step C integration tests — `store_raw` enqueue hook +
//! `extraction_status` read path.
//!
//! Covers:
//! - Storing a memory with a `JobQueue` installed enqueues exactly one
//!   `PipelineJob::initial` per `Inserted` outcome.
//! - No queue installed → no enqueue, admission unaffected (v0.2 compat).
//! - `Skipped` outcomes (empty content, no facts) do not enqueue.
//! - `QueueFull` errors at enqueue time do NOT abort `store_raw`
//!   (GUARD-1: admission write stays committed).
//! - `extraction_status` returns `NotStarted` for an unknown id and
//!   for a freshly-stored id (since Step C-bis hasn't landed the
//!   `Pending`-row write).
//!
//! See `.gid/features/v03-resolution/design.md` §3.1, §6.3.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use engramai::memory::Memory;
use engramai::resolution::{
    BoundedJobQueue, EnqueueError, ExtractionStatus, JobMode, JobQueue, PipelineJob,
};
use engramai::store_api::{RawStoreOutcome, StorageMeta, StoreOutcome};

fn new_mem() -> Memory {
    Memory::new(":memory:", None).expect("memory engine boots")
}

// -------------------------------------------------------------------
// Test queue: counts try_enqueue calls and lets the test inject a
// `QueueFull` reject deterministically.
// -------------------------------------------------------------------

struct CountingQueue {
    inner: BoundedJobQueue,
    enqueue_count: AtomicUsize,
    reject_next: AtomicUsize, // if > 0, next N enqueues fail with QueueFull
}

impl CountingQueue {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: BoundedJobQueue::new(capacity),
            enqueue_count: AtomicUsize::new(0),
            reject_next: AtomicUsize::new(0),
        })
    }
}

impl JobQueue for CountingQueue {
    fn try_enqueue(&self, job: PipelineJob) -> Result<(), EnqueueError> {
        self.enqueue_count.fetch_add(1, Ordering::SeqCst);
        if self.reject_next.load(Ordering::SeqCst) > 0 {
            self.reject_next.fetch_sub(1, Ordering::SeqCst);
            return Err(EnqueueError::QueueFull);
        }
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
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

#[test]
fn store_raw_with_no_queue_does_not_panic_and_returns_stored() {
    // v0.2-compat: no queue installed → store_raw works exactly as
    // before. This is the regression guard against accidentally making
    // the queue mandatory.
    let mut mem = new_mem();
    let meta = StorageMeta::default();
    let out = mem
        .store_raw("step C should not require a queue", meta)
        .expect("store_raw ok");
    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert_eq!(outcomes.len(), 1);
            assert!(matches!(outcomes[0], StoreOutcome::Inserted { .. }));
        }
        other => panic!("expected Stored, got {other:?}"),
    }
}

#[test]
fn store_raw_with_queue_enqueues_one_job_per_inserted_outcome() {
    let queue = CountingQueue::new(16);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let out = mem
        .store_raw("first ingest, fresh memory", meta)
        .expect("store_raw ok");

    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert_eq!(outcomes.len(), 1);
            assert!(matches!(outcomes[0], StoreOutcome::Inserted { .. }));
        }
        other => panic!("expected Stored, got {other:?}"),
    }

    // Exactly one Inserted → exactly one enqueue.
    assert_eq!(queue.enqueue_count.load(Ordering::SeqCst), 1);
    assert_eq!(queue.len(), 1);

    // Job shape: Initial mode, memory_id is non-empty.
    let job = queue.try_dequeue().expect("queued job");
    assert_eq!(job.mode, JobMode::Initial);
    assert!(!job.memory_id.is_empty());
}

#[test]
fn store_raw_skipped_too_short_does_not_enqueue() {
    let queue = CountingQueue::new(16);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let out = mem
        .store_raw("   ", meta)
        .expect("store_raw ok with whitespace");

    assert!(matches!(out, RawStoreOutcome::Skipped { .. }));
    // No enqueue on a Skipped outcome — admission produced no row.
    assert_eq!(queue.enqueue_count.load(Ordering::SeqCst), 0);
}

#[test]
fn store_raw_merged_outcome_does_not_enqueue() {
    // Merged means the underlying memory was extracted on its first
    // ingest; re-extraction is operator-driven (`reextract`), not
    // automatic. Same content twice should produce one Inserted +
    // one Merged, and exactly one enqueue.
    let queue = CountingQueue::new(16);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let _ = mem
        .store_raw("the same memory, twice", meta.clone())
        .expect("first store ok");
    let count_after_first = queue.enqueue_count.load(Ordering::SeqCst);
    assert_eq!(count_after_first, 1, "first store enqueues");

    let out2 = mem
        .store_raw("the same memory, twice", meta)
        .expect("second store ok");

    // Either Stored([Merged]) or Skipped(DuplicateContent) is
    // acceptable depending on dedup mode; in both cases the enqueue
    // count must NOT increment.
    match out2 {
        RawStoreOutcome::Stored(outcomes) => {
            // If we got back outcomes, none of them should be Inserted.
            for o in outcomes {
                assert!(
                    !matches!(o, StoreOutcome::Inserted { .. }),
                    "second store should not produce a fresh Inserted"
                );
            }
        }
        RawStoreOutcome::Skipped { .. } => { /* fine */ }
        other => panic!("unexpected second-store outcome: {other:?}"),
    }

    assert_eq!(
        queue.enqueue_count.load(Ordering::SeqCst),
        count_after_first,
        "second store must not enqueue"
    );
}

#[test]
fn store_raw_survives_queue_full_per_guard_1() {
    // GUARD-1: enqueue rejection MUST NOT abort admission. The
    // memory is committed and recoverable via reextract (Step C-bis).
    let queue = CountingQueue::new(16);
    queue.reject_next.store(1, Ordering::SeqCst);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let out = mem
        .store_raw("admission must survive queue rejection", meta)
        .expect("store_raw still returns Ok despite QueueFull");

    match out {
        RawStoreOutcome::Stored(outcomes) => {
            assert_eq!(outcomes.len(), 1);
            assert!(matches!(outcomes[0], StoreOutcome::Inserted { .. }));
        }
        other => panic!("expected Stored, got {other:?}"),
    }

    // Enqueue was attempted (count incremented) but the queue is empty
    // because it rejected.
    assert_eq!(queue.enqueue_count.load(Ordering::SeqCst), 1);
    assert_eq!(queue.len(), 0);
}

#[test]
fn extraction_status_returns_not_started_for_unknown_id() {
    let mut mem = new_mem();
    let st = mem
        .extraction_status("unknown-memory-id")
        .expect("read ok");
    assert!(matches!(st, ExtractionStatus::NotStarted));
}

#[test]
fn extraction_status_returns_not_started_for_freshly_stored_id() {
    // Step C: enqueue does NOT yet write a `pending` row to
    // graph_pipeline_runs (that's Step C-bis). So a freshly-stored
    // memory reads as `NotStarted` until the worker pool picks it up.
    // This test pins that current contract — if it starts failing,
    // either Step C-bis landed (good — update the assertion to
    // `Pending`) or something regressed.
    let queue = CountingQueue::new(16);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let out = mem
        .store_raw("fresh memory, never extracted", meta)
        .expect("store_raw ok");
    let id = match out {
        RawStoreOutcome::Stored(o) => o[0].id().clone(),
        other => panic!("expected Stored, got {other:?}"),
    };

    // Sanity: enqueue happened.
    assert_eq!(queue.enqueue_count.load(Ordering::SeqCst), 1);

    let st = mem.extraction_status(&id).expect("read ok");
    assert!(
        matches!(st, ExtractionStatus::NotStarted),
        "Step C contract: enqueued-but-not-running reads as NotStarted; got {st:?}"
    );
}

#[test]
fn store_raw_with_queue_full_still_admits_and_status_is_not_started() {
    // Compose: queue rejects, admission survives, extraction_status
    // reports NotStarted. After Step C-bis lands the `Pending` write
    // at enqueue, this should switch to Pending(queue_full=true).
    let queue = CountingQueue::new(16);
    queue.reject_next.store(1, Ordering::SeqCst);
    let mut mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);

    let meta = StorageMeta::default();
    let out = mem
        .store_raw("rejected at enqueue but still committed", meta)
        .expect("admission preserved");
    let id = match out {
        RawStoreOutcome::Stored(o) => o[0].id().clone(),
        other => panic!("expected Stored, got {other:?}"),
    };

    let st = mem.extraction_status(&id).expect("read ok");
    assert!(matches!(st, ExtractionStatus::NotStarted));
}

#[test]
fn job_queue_ref_returns_installed_queue() {
    let queue = CountingQueue::new(8);
    let mem = new_mem().with_job_queue(queue.clone() as Arc<dyn JobQueue>);
    assert!(mem.job_queue_ref().is_some());

    // Capacity round-trip — sanity check the trait dispatch is wired.
    let cap = mem.job_queue_ref().unwrap().capacity();
    assert_eq!(cap, Some(8));
}
