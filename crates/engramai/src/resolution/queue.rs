//! §3.1 ingestion — `PipelineJob` type and bounded job queue.
//!
//! Step A of §3.1: pure types and a storage-agnostic queue trait + an
//! in-memory bounded implementation. No DB, no `store_raw` integration,
//! no worker pool. Those are subsequent steps.
//!
//! # Design references
//!
//! - `.gid/features/v03-resolution/design.md` §3.1 (ingestion responsibilities,
//!   idempotence key)
//! - `.gid/features/v03-resolution/design.md` §5.1 (execution model — bounded
//!   channel + worker pool)
//! - `.gid/features/v03-resolution/design.md` §5.2 (backpressure & queue
//!   semantics — bounded with non-droppable ReExtract)
//!
//! # Boundary rules
//!
//! - `JobQueue` is **storage-agnostic**: a real impl may wire a crossbeam
//!   channel, a tokio mpsc, or — in tests — the in-memory `BoundedJobQueue`
//!   here. The trait surface is intentionally tiny so swapping the backing
//!   channel is a one-file change.
//! - `try_enqueue` is **non-blocking**. It either accepts or returns
//!   `EnqueueError::QueueFull` immediately. GUARD-1 forbids backpressure on
//!   the ingest path.
//! - `JobMode::ReExtract` jobs bypass the capacity check (§5.2 non-droppable).
//! - The queue is FIFO. There is no priority lane: ReExtract jobs are
//!   appended to the same queue but are exempt from the capacity reject.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use uuid::Uuid;

use crate::store_api::MemoryId;

/// Why was this job enqueued? Drives capacity-reject behavior (§5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobMode {
    /// Normal post-`store_raw` enqueue. Subject to `queue_cap` rejection
    /// when full (§5.2 — failure surfaces as `Pending(queue_full)` on the
    /// memory's `extraction_status`).
    Initial,
    /// Operator-triggered re-extract via `reextract` (§4 / §6.2). Bypasses
    /// the capacity limit because the operator already accepted the cost
    /// of the queue depth.
    ReExtract,
}

impl JobMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::ReExtract => "reextract",
        }
    }

    /// True iff this job is allowed to grow the queue past `capacity`.
    /// See `.gid/features/v03-resolution/design.md` §5.2 paragraph
    /// "non-droppable".
    pub fn is_non_droppable(&self) -> bool {
        matches!(self, Self::ReExtract)
    }
}

/// One unit of work for the resolution pipeline.
///
/// Carries the idempotence key `(memory_id, episode_id)` per GOAL-2.1 — the
/// dispatcher dedupes by querying the `graph_pipeline_runs` ledger before
/// starting work (Step B). This struct is the *queue payload*; the ledger
/// row is a separate concept written by the dispatcher.
///
/// `enqueued_at` is captured at construction time so queue depth + age
/// telemetry (`resolution_pending_memory_oldest_age_seconds`, §5.2.1) can
/// be computed without a separate timestamp column.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineJob {
    pub memory_id: MemoryId,
    pub episode_id: Uuid,
    pub mode: JobMode,
    pub enqueued_at: DateTime<Utc>,
}

impl PipelineJob {
    /// Build a fresh `Initial`-mode job for a memory just admitted by
    /// `store_raw`.
    pub fn initial(memory_id: MemoryId, episode_id: Uuid) -> Self {
        Self {
            memory_id,
            episode_id,
            mode: JobMode::Initial,
            enqueued_at: Utc::now(),
        }
    }

    /// Build a `ReExtract` job for an operator-triggered replay.
    pub fn reextract(memory_id: MemoryId, episode_id: Uuid) -> Self {
        Self {
            memory_id,
            episode_id,
            mode: JobMode::ReExtract,
            enqueued_at: Utc::now(),
        }
    }
}

/// Why the queue rejected an enqueue.
///
/// Today there is exactly one variant; the enum exists so future failure
/// modes (e.g. `ShuttingDown`, `RateLimited`) can be added without changing
/// the `try_enqueue` signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueError {
    /// Queue is at capacity and the job is droppable (`mode == Initial`).
    /// Surface as `StageFailure { stage: Ingest, kind: "queue_full" }`
    /// per §5.2.
    QueueFull,
    /// Queue is shut down — no more enqueues accepted. The pipeline is
    /// draining or has stopped. The L1/L2 admission write is unaffected
    /// (GUARD-1) — caller should record the failure and return.
    Closed,
}

impl std::fmt::Display for EnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => f.write_str("pipeline queue is full"),
            Self::Closed => f.write_str("pipeline queue is closed"),
        }
    }
}

impl std::error::Error for EnqueueError {}

/// Storage-agnostic queue surface for the resolution pipeline.
///
/// Implementations must be `Send + Sync` so producers (calling threads of
/// `store_raw`) and consumers (worker pool threads) can share one instance.
///
/// All methods are **non-blocking** by contract. A blocking variant would
/// violate GUARD-1 (`store_raw` must never wait on graph extraction).
pub trait JobQueue: Send + Sync {
    /// Enqueue a job. Returns `Err(QueueFull)` immediately if the queue is
    /// at capacity and the job is droppable (`Initial`); `ReExtract` jobs
    /// always succeed unless the queue is `Closed`.
    fn try_enqueue(&self, job: PipelineJob) -> Result<(), EnqueueError>;

    /// Pop the front job, or `None` if empty. Workers spin/park on this.
    fn try_dequeue(&self) -> Option<PipelineJob>;

    /// Current number of queued jobs.
    fn len(&self) -> usize;

    /// True iff `len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Configured capacity for droppable jobs. `None` means unbounded
    /// (unusual; reserved for tests / specialized backends).
    fn capacity(&self) -> Option<usize>;
}

/// In-memory bounded FIFO queue. Used in tests and as the default
/// production backend until a higher-throughput channel is wired in
/// Step D (worker pool).
///
/// # Concurrency
///
/// Single `Mutex<VecDeque>` — adequate at the throughput v0.3 targets
/// (sub-1k ingests/sec). When `worker_count > 1` and contention shows
/// up, swap in a crossbeam-channel-backed impl behind the same trait.
pub struct BoundedJobQueue {
    inner: Mutex<BoundedJobQueueInner>,
    capacity: usize,
}

struct BoundedJobQueueInner {
    queue: VecDeque<PipelineJob>,
    closed: bool,
}

impl BoundedJobQueue {
    /// Build a new queue with the given capacity for droppable jobs.
    /// Per §5.2 default capacity in production is 10_000.
    ///
    /// `capacity == 0` is permitted but pathological — every `Initial`
    /// enqueue will fail. Useful only for tests of the queue-full path.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(BoundedJobQueueInner {
                queue: VecDeque::with_capacity(capacity.min(1024)),
                closed: false,
            }),
            capacity,
        }
    }

    /// Stop accepting new enqueues. Existing queued jobs remain dequeueable.
    /// Used during shutdown drain (§5.2 — "shutdown drains the queue up to
    /// a configurable deadline").
    pub fn close(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.closed = true;
        }
    }

    /// Test-only inspector. Production code must not snapshot internal
    /// state — use the trait's `len()` / `is_empty()`.
    #[cfg(test)]
    fn snapshot_modes(&self) -> Vec<JobMode> {
        self.inner
            .lock()
            .unwrap()
            .queue
            .iter()
            .map(|j| j.mode)
            .collect()
    }
}

impl JobQueue for BoundedJobQueue {
    fn try_enqueue(&self, job: PipelineJob) -> Result<(), EnqueueError> {
        let mut g = self
            .inner
            .lock()
            .map_err(|_| EnqueueError::Closed)?; // poisoned == effectively closed
        if g.closed {
            return Err(EnqueueError::Closed);
        }
        // §5.2 — only droppable jobs are subject to the capacity reject.
        if !job.mode.is_non_droppable() && g.queue.len() >= self.capacity {
            return Err(EnqueueError::QueueFull);
        }
        g.queue.push_back(job);
        Ok(())
    }

    fn try_dequeue(&self) -> Option<PipelineJob> {
        self.inner.lock().ok()?.queue.pop_front()
    }

    fn len(&self) -> usize {
        self.inner.lock().map(|g| g.queue.len()).unwrap_or(0)
    }

    fn capacity(&self) -> Option<usize> {
        Some(self.capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_initial(id: &str) -> PipelineJob {
        PipelineJob::initial(id.to_string(), Uuid::new_v4())
    }

    fn job_reextract(id: &str) -> PipelineJob {
        PipelineJob::reextract(id.to_string(), Uuid::new_v4())
    }

    // ---- PipelineJob / JobMode ---------------------------------------

    #[test]
    fn job_mode_string_form_is_stable() {
        // These strings are persisted in graph_pipeline_runs.kind and used
        // in metric labels — changing them is a breaking audit-trail change.
        assert_eq!(JobMode::Initial.as_str(), "initial");
        assert_eq!(JobMode::ReExtract.as_str(), "reextract");
    }

    #[test]
    fn only_reextract_is_non_droppable() {
        assert!(!JobMode::Initial.is_non_droppable());
        assert!(JobMode::ReExtract.is_non_droppable());
    }

    #[test]
    fn pipeline_job_initial_constructor_sets_mode() {
        let j = job_initial("mem-1");
        assert_eq!(j.mode, JobMode::Initial);
        assert_eq!(j.memory_id, "mem-1");
    }

    #[test]
    fn pipeline_job_reextract_constructor_sets_mode() {
        let j = job_reextract("mem-2");
        assert_eq!(j.mode, JobMode::ReExtract);
        assert_eq!(j.memory_id, "mem-2");
    }

    #[test]
    fn pipeline_job_serde_roundtrip() {
        let j = job_initial("mem-3");
        let s = serde_json::to_string(&j).unwrap();
        let back: PipelineJob = serde_json::from_str(&s).unwrap();
        assert_eq!(back, j);
    }

    // ---- BoundedJobQueue: basic FIFO ---------------------------------

    #[test]
    fn empty_queue_has_zero_len() {
        let q = BoundedJobQueue::new(8);
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
        assert_eq!(q.capacity(), Some(8));
        assert!(q.try_dequeue().is_none());
    }

    #[test]
    fn enqueue_then_dequeue_preserves_order() {
        let q = BoundedJobQueue::new(8);
        q.try_enqueue(job_initial("a")).unwrap();
        q.try_enqueue(job_initial("b")).unwrap();
        q.try_enqueue(job_initial("c")).unwrap();
        assert_eq!(q.len(), 3);
        assert_eq!(q.try_dequeue().unwrap().memory_id, "a");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "b");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "c");
        assert!(q.try_dequeue().is_none());
    }

    // ---- BoundedJobQueue: capacity / queue-full ----------------------

    #[test]
    fn initial_jobs_rejected_when_at_capacity() {
        let q = BoundedJobQueue::new(2);
        q.try_enqueue(job_initial("a")).unwrap();
        q.try_enqueue(job_initial("b")).unwrap();
        let err = q.try_enqueue(job_initial("c")).unwrap_err();
        assert_eq!(err, EnqueueError::QueueFull);
        // The reject does not consume capacity from the queue.
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn reextract_jobs_bypass_capacity() {
        // §5.2 — non-droppable. Even at cap, ReExtract is admitted.
        let q = BoundedJobQueue::new(1);
        q.try_enqueue(job_initial("a")).unwrap();
        // Initial would be rejected here.
        assert_eq!(q.try_enqueue(job_initial("b")), Err(EnqueueError::QueueFull));
        // But ReExtract pushes through.
        q.try_enqueue(job_reextract("r1")).unwrap();
        q.try_enqueue(job_reextract("r2")).unwrap();
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn capacity_zero_rejects_every_initial() {
        let q = BoundedJobQueue::new(0);
        assert_eq!(
            q.try_enqueue(job_initial("a")),
            Err(EnqueueError::QueueFull)
        );
        assert_eq!(q.len(), 0);
        // ReExtract still works (non-droppable).
        q.try_enqueue(job_reextract("r")).unwrap();
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn capacity_check_uses_total_len_not_initial_count() {
        // ReExtract jobs grow the queue past capacity — subsequent Initial
        // enqueues see a `len() >= capacity` queue and are rejected. This
        // is intentional per §5.2: when the queue is already over capacity
        // due to operator replays, new ingest is degraded *visibly* rather
        // than silently dropping the operator's replay batch.
        let q = BoundedJobQueue::new(1);
        q.try_enqueue(job_reextract("r1")).unwrap();
        q.try_enqueue(job_reextract("r2")).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(
            q.try_enqueue(job_initial("a")),
            Err(EnqueueError::QueueFull)
        );
    }

    // ---- BoundedJobQueue: closed --------------------------------------

    #[test]
    fn close_rejects_new_enqueues_but_dequeue_drains() {
        let q = BoundedJobQueue::new(8);
        q.try_enqueue(job_initial("a")).unwrap();
        q.try_enqueue(job_initial("b")).unwrap();
        q.close();
        assert_eq!(q.try_enqueue(job_initial("c")), Err(EnqueueError::Closed));
        // ReExtract also rejected when closed — closed wins over
        // non-droppable. Shutdown means *no new work*, period.
        assert_eq!(
            q.try_enqueue(job_reextract("r")),
            Err(EnqueueError::Closed)
        );
        // But the pre-close queued items are still dequeueable.
        assert_eq!(q.try_dequeue().unwrap().memory_id, "a");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "b");
        assert!(q.try_dequeue().is_none());
    }

    // ---- BoundedJobQueue: ordering with mixed modes ------------------

    #[test]
    fn mixed_mode_dequeue_is_strict_fifo() {
        // §5.2: "ReExtract jobs are dispatched at normal priority". They
        // are appended to the same queue; no priority lane.
        let q = BoundedJobQueue::new(8);
        q.try_enqueue(job_initial("a")).unwrap();
        q.try_enqueue(job_reextract("r1")).unwrap();
        q.try_enqueue(job_initial("b")).unwrap();
        q.try_enqueue(job_reextract("r2")).unwrap();
        assert_eq!(
            q.snapshot_modes(),
            vec![
                JobMode::Initial,
                JobMode::ReExtract,
                JobMode::Initial,
                JobMode::ReExtract,
            ]
        );
        assert_eq!(q.try_dequeue().unwrap().memory_id, "a");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "r1");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "b");
        assert_eq!(q.try_dequeue().unwrap().memory_id, "r2");
    }

    // ---- Send + Sync (compile-time) ----------------------------------

    #[test]
    fn bounded_queue_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BoundedJobQueue>();
        // Trait object too — workers will hold `Arc<dyn JobQueue>`.
        assert_send_sync::<std::sync::Arc<dyn JobQueue>>();
    }

    // ---- Concurrent access (smoke) -----------------------------------

    #[test]
    fn concurrent_producers_preserve_count() {
        use std::sync::Arc;
        use std::thread;

        let q: Arc<dyn JobQueue> = Arc::new(BoundedJobQueue::new(10_000));
        let mut handles = Vec::new();
        for t in 0..8 {
            let q2 = q.clone();
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let id = format!("t{}-{}", t, i);
                    q2.try_enqueue(PipelineJob::initial(id, Uuid::new_v4()))
                        .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(q.len(), 800);
    }

    #[test]
    fn concurrent_full_queue_some_rejected_none_lost() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        let cap = 50;
        let total = 8 * 100; // 800 attempts, only 50 should fit
        let q: Arc<dyn JobQueue> = Arc::new(BoundedJobQueue::new(cap));
        let accepted = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for t in 0..8 {
            let q2 = q.clone();
            let acc = accepted.clone();
            let rej = rejected.clone();
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let id = format!("t{}-{}", t, i);
                    match q2.try_enqueue(PipelineJob::initial(id, Uuid::new_v4())) {
                        Ok(()) => acc.fetch_add(1, Ordering::Relaxed),
                        Err(EnqueueError::QueueFull) => rej.fetch_add(1, Ordering::Relaxed),
                        Err(other) => panic!("unexpected error: {other:?}"),
                    };
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let acc_n = accepted.load(Ordering::Relaxed);
        let rej_n = rejected.load(Ordering::Relaxed);
        // No enqueue is silently dropped: every attempt is either accepted
        // or rejected.
        assert_eq!(acc_n + rej_n, total);
        // Capacity is honored: at most `cap` jobs are queued.
        assert_eq!(acc_n, cap);
        assert_eq!(q.len(), cap);
    }
}
