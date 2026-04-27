//! §5.1 worker pool — concurrency scheduling layer.
//!
//! This module is **only** the worker-pool concurrency scheduling layer.
//! It does NOT orchestrate the extract→resolve→persist pipeline; that lives
//! in `pipeline.rs` (upper layer) and is invoked via the [`JobProcessor`]
//! trait.
//!
//! Responsibilities:
//!   1. Manage N worker threads (config: `ResolutionConfig::worker_count`).
//!   2. Session-affinity dispatch: a single dispatcher thread reads from the
//!      shared [`JobQueue`] and routes each job to worker
//!      `hash(memory_id) % N` via a per-worker bounded inbox. This guarantees
//!      jobs for the same memory are serialized on the same worker
//!      (GUARD-1: single-writer per memory).
//!   3. Each worker pulls jobs from its inbox, calls
//!      [`JobProcessor::process`], and updates pool counters.
//!   4. Graceful shutdown: `shutdown(deadline)` closes the queue, drains
//!      inboxes up to the deadline, then signals workers to stop. Crash
//!      recovery (re-enqueue of in-flight jobs on abnormal termination)
//!      is **not** the worker's responsibility — it is handled by the
//!      durable-status layer (`status.rs`) on next startup.
//!
//! Non-goals:
//!   - No knowledge of `Episode`, `EntityMention`, fusion, persist, or any
//!     pipeline stage. Those are behind the [`JobProcessor`] trait.
//!   - No direct `MemoryGraph` / `store_raw` calls.
//!   - No retry policy. The processor decides whether to retry; the worker
//!     just records success/failure.
//!
//! See `.gid/features/v03-resolution/design.md` §5.1 (worker pool),
//! §5.2 (shutdown drain), GUARD-1 (single-writer per memory).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::queue::{JobQueue, PipelineJob};
use super::stats::ResolutionConfig;

// ---------------------------------------------------------------------------
// JobProcessor — the seam between the worker pool and the pipeline.
// ---------------------------------------------------------------------------

/// Errors a [`JobProcessor`] may return.
///
/// The worker pool treats every variant identically (increments
/// `jobs_failed`); semantic distinctions matter only to the upper layer
/// (e.g., `pipeline.rs` deciding retry vs. quarantine).
#[derive(Debug)]
pub enum ProcessError {
    /// Stage-level failure (extract / resolve / persist returned `Err`).
    Stage(String),
    /// Job referenced state that no longer exists (e.g., memory deleted
    /// between enqueue and dispatch).
    NotFound(String),
    /// Catch-all for processor-internal errors.
    Other(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessError::Stage(m) => write!(f, "stage failure: {m}"),
            ProcessError::NotFound(m) => write!(f, "not found: {m}"),
            ProcessError::Other(m) => write!(f, "processor error: {m}"),
        }
    }
}

impl std::error::Error for ProcessError {}

/// Trait implemented by the upper-layer pipeline. The worker pool calls
/// `process` for each job; everything inside is opaque to the worker.
///
/// Implementations **must** be `Send + Sync` because a single processor
/// instance is shared across all workers (wrapped in `Arc`).
pub trait JobProcessor: Send + Sync {
    /// Process one job to completion (success or terminal failure).
    /// Long-running stages should respect cancellation via their own
    /// mechanisms; the worker pool does not preempt.
    fn process(&self, job: PipelineJob) -> Result<(), ProcessError>;
}

// ---------------------------------------------------------------------------
// WorkerPoolStats — pool-wide atomic counters (separate from per-job
// `ResolutionStats`). Cheap to read at any time for observability.
// ---------------------------------------------------------------------------

/// Atomic counters maintained by the pool itself. Per-job extraction /
/// merge counters live in [`super::stats::ResolutionStats`] and are
/// produced by the processor.
#[derive(Debug, Default)]
pub struct WorkerPoolStats {
    /// Jobs successfully processed (processor returned `Ok`).
    pub jobs_processed: AtomicU64,
    /// Jobs that ended in `ProcessError` (any variant).
    pub jobs_failed: AtomicU64,
    /// Jobs dispatched but not yet completed. Incremented on dispatch,
    /// decremented on processor return. A non-zero value at shutdown
    /// indicates work was abandoned past the drain deadline.
    pub jobs_in_flight: AtomicU64,
    /// Jobs the dispatcher could not route because the target worker's
    /// inbox was full and the job was droppable. Non-droppable jobs
    /// block the dispatcher instead.
    pub jobs_dropped_inbox_full: AtomicU64,
}

impl WorkerPoolStats {
    /// Snapshot the current values. Each field is read independently with
    /// `Relaxed` ordering — values are eventually consistent, not a
    /// transactional snapshot.
    pub fn snapshot(&self) -> WorkerPoolStatsSnapshot {
        WorkerPoolStatsSnapshot {
            jobs_processed: self.jobs_processed.load(Ordering::Relaxed),
            jobs_failed: self.jobs_failed.load(Ordering::Relaxed),
            jobs_in_flight: self.jobs_in_flight.load(Ordering::Relaxed),
            jobs_dropped_inbox_full: self.jobs_dropped_inbox_full.load(Ordering::Relaxed),
        }
    }
}

/// Plain-data snapshot of [`WorkerPoolStats`] for logging / metrics export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPoolStatsSnapshot {
    pub jobs_processed: u64,
    pub jobs_failed: u64,
    pub jobs_in_flight: u64,
    pub jobs_dropped_inbox_full: u64,
}

// ---------------------------------------------------------------------------
// Internal control messages routed through per-worker inboxes.
// ---------------------------------------------------------------------------

enum WorkerMsg {
    Job(PipelineJob),
    /// Sent during shutdown after drain to wake idle workers so they exit.
    Stop,
}

// ---------------------------------------------------------------------------
// WorkerPool — public handle.
// ---------------------------------------------------------------------------

/// A pool of N worker threads plus one dispatcher thread, fronted by a
/// shared [`JobQueue`]. Construct with [`WorkerPool::start`] and stop with
/// [`WorkerPool::shutdown`].
pub struct WorkerPool {
    stats: Arc<WorkerPoolStats>,
    /// Per-worker inbox senders. The dispatcher owns the read side via a
    /// parallel `Vec<Receiver<WorkerMsg>>` it consumed at startup.
    inboxes: Vec<SyncSender<WorkerMsg>>,
    /// Cooperative stop flag. Set by `shutdown` after queue drain to make
    /// workers exit their idle-poll loops promptly.
    stop_flag: Arc<AtomicBool>,
    /// Shared queue handle — used by `shutdown` to call `close()` so no
    /// new jobs are admitted during drain.
    queue: Arc<dyn JobQueue>,
    /// Worker thread handles, joined on shutdown.
    worker_handles: Vec<JoinHandle<()>>,
    /// Dispatcher thread handle.
    dispatcher_handle: Option<JoinHandle<()>>,
    /// Configured idle-poll interval (cached from config).
    idle_poll: Duration,
    /// `true` once `shutdown` has run; guards against double-shutdown.
    shut_down: Mutex<bool>,
}

impl WorkerPool {
    /// Start the dispatcher and `config.worker_count` worker threads.
    ///
    /// The pool takes shared ownership of `queue` and `processor`. The
    /// queue's `try_dequeue` is polled by the dispatcher only.
    pub fn start(
        config: &ResolutionConfig,
        queue: Arc<dyn JobQueue>,
        processor: Arc<dyn JobProcessor>,
    ) -> Result<Self, WorkerPoolError> {
        config
            .validate()
            .map_err(|e| WorkerPoolError::InvalidConfig(format!("{e}")))?;

        let n = config.worker_count;
        let stats = Arc::new(WorkerPoolStats::default());
        let stop_flag = Arc::new(AtomicBool::new(false));
        let idle_poll = config.worker_idle_poll;

        // Per-worker inbox capacity. Sized small (4) on purpose: the shared
        // queue is the buffer; per-worker inboxes are just dispatch
        // hand-off slots. Larger inboxes would let one slow worker
        // accumulate a backlog while peers idle.
        const INBOX_CAP: usize = 4;

        let mut inboxes = Vec::with_capacity(n);
        let mut receivers = Vec::with_capacity(n);
        for _ in 0..n {
            let (tx, rx) = mpsc::sync_channel::<WorkerMsg>(INBOX_CAP);
            inboxes.push(tx);
            receivers.push(rx);
        }

        // Spawn workers. Each owns its receiver.
        let mut worker_handles = Vec::with_capacity(n);
        for (idx, rx) in receivers.into_iter().enumerate() {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop_flag);
            let proc = Arc::clone(&processor);
            let handle = thread::Builder::new()
                .name(format!("resolution-worker-{idx}"))
                .spawn(move || worker_loop(idx, rx, proc, stats, stop))
                .map_err(|e| WorkerPoolError::SpawnFailed(format!("worker-{idx}: {e}")))?;
            worker_handles.push(handle);
        }

        // Spawn dispatcher.
        let dispatcher_handle = {
            let queue = Arc::clone(&queue);
            let inboxes_clone = inboxes.clone();
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop_flag);
            thread::Builder::new()
                .name("resolution-dispatcher".to_string())
                .spawn(move || dispatcher_loop(queue, inboxes_clone, stats, stop, idle_poll))
                .map_err(|e| WorkerPoolError::SpawnFailed(format!("dispatcher: {e}")))?
        };

        Ok(Self {
            stats,
            inboxes,
            stop_flag,
            queue,
            worker_handles,
            dispatcher_handle: Some(dispatcher_handle),
            idle_poll,
            shut_down: Mutex::new(false),
        })
    }

    /// Read-only access to the pool's atomic counters.
    pub fn stats(&self) -> Arc<WorkerPoolStats> {
        Arc::clone(&self.stats)
    }

    /// Graceful shutdown.
    ///
    /// 1. Close the shared queue (no new enqueues admitted).
    /// 2. Wait up to `deadline` for `jobs_in_flight` to reach 0 AND the
    ///    queue to be drained.
    /// 3. Signal stop, send `WorkerMsg::Stop` to every worker, join all
    ///    threads.
    ///
    /// Returns `Ok(())` even if the deadline expires — abandoned jobs are
    /// recoverable on next startup via the durable-status layer
    /// (§5.2, GUARD-2).
    pub fn shutdown(mut self, deadline: Duration) -> Result<WorkerPoolStatsSnapshot, WorkerPoolError> {
        {
            let mut flag = self
                .shut_down
                .lock()
                .map_err(|_| WorkerPoolError::ShutdownPoisoned)?;
            if *flag {
                return Err(WorkerPoolError::AlreadyShutDown);
            }
            *flag = true;
        }

        // Step 1: stop accepting new work.
        self.queue.close();

        // Step 2: drain. Poll until either deadline expires or both
        // queue is empty AND no jobs are in flight.
        let drain_start = Instant::now();
        let drain_poll = self.idle_poll.max(Duration::from_millis(1));
        loop {
            let in_flight = self.stats.jobs_in_flight.load(Ordering::Relaxed);
            let queued = self.queue.len();
            if in_flight == 0 && queued == 0 {
                break;
            }
            if drain_start.elapsed() >= deadline {
                break;
            }
            thread::sleep(drain_poll);
        }

        // Step 3: signal stop and notify workers.
        self.stop_flag.store(true, Ordering::SeqCst);
        for tx in &self.inboxes {
            // Best-effort: a full inbox means the worker will see stop_flag
            // on its next poll regardless. We don't block.
            let _ = tx.try_send(WorkerMsg::Stop);
        }

        // Drop senders so any worker still in `recv_timeout` sees a
        // disconnect and exits.
        self.inboxes.clear();

        // Step 4: join dispatcher first (it stops pulling once stop_flag
        // is set), then workers.
        if let Some(h) = self.dispatcher_handle.take() {
            let _ = h.join();
        }
        let handles = std::mem::take(&mut self.worker_handles);
        for h in handles {
            let _ = h.join();
        }

        Ok(self.stats.snapshot())
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        // If the user dropped without calling `shutdown`, do a best-effort
        // stop. We cannot return errors from Drop, so failures are silent.
        let already = self
            .shut_down
            .lock()
            .map(|g| *g)
            .unwrap_or(true);
        if already {
            return;
        }
        self.queue.close();
        self.stop_flag.store(true, Ordering::SeqCst);
        // Drop senders to break worker recv loops.
        self.inboxes.clear();
        if let Some(h) = self.dispatcher_handle.take() {
            let _ = h.join();
        }
        for h in std::mem::take(&mut self.worker_handles) {
            let _ = h.join();
        }
    }
}

/// Errors returned by [`WorkerPool::start`] / [`WorkerPool::shutdown`].
#[derive(Debug)]
pub enum WorkerPoolError {
    InvalidConfig(String),
    SpawnFailed(String),
    AlreadyShutDown,
    ShutdownPoisoned,
}

impl std::fmt::Display for WorkerPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerPoolError::InvalidConfig(m) => write!(f, "invalid config: {m}"),
            WorkerPoolError::SpawnFailed(m) => write!(f, "thread spawn failed: {m}"),
            WorkerPoolError::AlreadyShutDown => write!(f, "pool already shut down"),
            WorkerPoolError::ShutdownPoisoned => write!(f, "shutdown lock poisoned"),
        }
    }
}

impl std::error::Error for WorkerPoolError {}

// ---------------------------------------------------------------------------
// Internal: dispatcher and worker loops.
// ---------------------------------------------------------------------------

/// Hash a memory_id to a worker index in `[0, n)`.
///
/// Uses `DefaultHasher` — stable within a process run, which is all we
/// need for session affinity. Across restarts a different mapping is fine
/// because the durable status layer re-enqueues in-flight jobs.
fn route(memory_id: &str, n: usize) -> usize {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    memory_id.hash(&mut h);
    (h.finish() % (n as u64)) as usize
}

fn dispatcher_loop(
    queue: Arc<dyn JobQueue>,
    inboxes: Vec<SyncSender<WorkerMsg>>,
    stats: Arc<WorkerPoolStats>,
    stop: Arc<AtomicBool>,
    idle_poll: Duration,
) {
    let n = inboxes.len();
    debug_assert!(n > 0, "dispatcher started with zero workers");

    // Buffer of jobs we couldn't immediately dispatch (target inbox full
    // *and* job is non-droppable). Keyed by worker index so we retry in
    // FIFO order per worker. Bounded retry: we hold them and try again
    // each tick; we never drop non-droppable jobs.
    let mut pending: HashMap<usize, Vec<PipelineJob>> = HashMap::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            // On stop, abandon any pending non-droppable jobs — they
            // remain in their durable status and will be re-enqueued on
            // next startup (GUARD-2). We do NOT push them back to the
            // shared queue because it's already closed.
            return;
        }

        // First, try to flush previously-stuck jobs. These were already
        // counted in `jobs_in_flight` when first dequeued from the shared
        // queue, so we do NOT re-increment here.
        let mut still_stuck: HashMap<usize, Vec<PipelineJob>> = HashMap::new();
        for (idx, jobs) in pending.drain() {
            let tx = &inboxes[idx];
            let mut leftovers = Vec::new();
            for job in jobs {
                match tx.try_send(WorkerMsg::Job(job)) {
                    Ok(()) => { /* already counted as in-flight */ }
                    Err(TrySendError::Full(WorkerMsg::Job(j))) => leftovers.push(j),
                    Err(TrySendError::Full(WorkerMsg::Stop))
                    | Err(TrySendError::Disconnected(_)) => {
                        // Worker gone — abandon. Stop loop will exit
                        // shortly when stop_flag flips.
                        return;
                    }
                }
            }
            if !leftovers.is_empty() {
                still_stuck.insert(idx, leftovers);
            }
        }
        pending = still_stuck;

        // Then, pull a new job (only if no stuck backlog for the routed
        // worker — we want FIFO per worker).
        match queue.try_dequeue() {
            Some(job) => {
                // Count as in-flight at the moment we take ownership from
                // the shared queue. This way the count covers buffered
                // pending jobs too, so shutdown drain waits for them.
                stats.jobs_in_flight.fetch_add(1, Ordering::Relaxed);
                let idx = route(&job.memory_id, n);
                if let Some(buf) = pending.get_mut(&idx) {
                    // Backlog exists for this worker — append to preserve order.
                    buf.push(job);
                    continue;
                }
                let tx = &inboxes[idx];
                match tx.try_send(WorkerMsg::Job(job)) {
                    Ok(()) => { /* already counted as in-flight */ }
                    Err(TrySendError::Full(WorkerMsg::Job(j))) => {
                        // Once a job has been dequeued from the shared
                        // queue, it has crossed the durability gate; we
                        // never drop it here — droppability is enforced
                        // upstream at `try_enqueue`. Buffer until the
                        // target worker frees up.
                        pending.entry(idx).or_default().push(j);
                    }
                    Err(TrySendError::Full(WorkerMsg::Stop))
                    | Err(TrySendError::Disconnected(_)) => return,
                }
            }
            None => {
                // Queue empty. Park briefly. If we have stuck pending jobs,
                // poll faster so they flush as soon as worker frees up.
                let park = if pending.is_empty() {
                    idle_poll
                } else {
                    idle_poll.min(Duration::from_millis(1))
                };
                thread::sleep(park);
            }
        }
    }
}

fn worker_loop(
    _idx: usize,
    rx: Receiver<WorkerMsg>,
    processor: Arc<dyn JobProcessor>,
    stats: Arc<WorkerPoolStats>,
    stop: Arc<AtomicBool>,
) {
    loop {
        // `recv` blocks until a message arrives or the channel is
        // disconnected (all senders dropped during shutdown).
        let msg = match rx.recv() {
            Ok(m) => m,
            Err(_) => return, // dispatcher gone → exit
        };
        match msg {
            WorkerMsg::Stop => return,
            WorkerMsg::Job(job) => {
                let result = processor.process(job);
                match result {
                    Ok(()) => {
                        stats.jobs_processed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        stats.jobs_failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                stats.jobs_in_flight.fetch_sub(1, Ordering::Relaxed);
                if stop.load(Ordering::Relaxed) {
                    // After finishing the in-flight job, exit if asked.
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::queue::{BoundedJobQueue, JobMode};
    use std::sync::atomic::AtomicUsize;
    use uuid::Uuid;

    /// Test processor that records every memory_id it sees, in order,
    /// per worker thread. Lets tests assert session affinity.
    struct RecordingProcessor {
        seen: Mutex<Vec<(String, String)>>, // (thread_name, memory_id)
        delay: Duration,
        fail_on: Option<String>,
        processed: AtomicUsize,
    }

    impl RecordingProcessor {
        fn new() -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                delay: Duration::from_millis(0),
                fail_on: None,
                processed: AtomicUsize::new(0),
            }
        }
        fn with_delay(mut self, d: Duration) -> Self {
            self.delay = d;
            self
        }
        fn fail_on(mut self, mid: &str) -> Self {
            self.fail_on = Some(mid.to_string());
            self
        }
    }

    impl JobProcessor for RecordingProcessor {
        fn process(&self, job: PipelineJob) -> Result<(), ProcessError> {
            if !self.delay.is_zero() {
                thread::sleep(self.delay);
            }
            let tname = thread::current().name().unwrap_or("?").to_string();
            self.seen.lock().unwrap().push((tname, job.memory_id.clone()));
            self.processed.fetch_add(1, Ordering::Relaxed);
            if self.fail_on.as_deref() == Some(job.memory_id.as_str()) {
                return Err(ProcessError::Stage("forced".into()));
            }
            Ok(())
        }
    }

    fn cfg(workers: usize) -> ResolutionConfig {
        ResolutionConfig {
            worker_count: workers,
            queue_cap: 64,
            shutdown_drain: Duration::from_secs(2),
            worker_idle_poll: Duration::from_millis(1),
        }
    }

    fn enqueue_initial(q: &Arc<BoundedJobQueue>, id: &str) {
        q.try_enqueue(PipelineJob::initial(id.into(), Uuid::new_v4()))
            .expect("enqueue");
    }

    #[test]
    fn rejects_invalid_config() {
        let bad = ResolutionConfig {
            worker_count: 0,
            ..cfg(1)
        };
        let queue: Arc<dyn JobQueue> = Arc::new(BoundedJobQueue::new(8));
        let proc: Arc<dyn JobProcessor> = Arc::new(RecordingProcessor::new());
        let r = WorkerPool::start(&bad, queue, proc);
        assert!(matches!(r, Err(WorkerPoolError::InvalidConfig(_))));
    }

    #[test]
    fn processes_single_job() {
        let q = Arc::new(BoundedJobQueue::new(8));
        let proc = Arc::new(RecordingProcessor::new());
        let pool = WorkerPool::start(
            &cfg(1),
            q.clone() as Arc<dyn JobQueue>,
            proc.clone() as Arc<dyn JobProcessor>,
        )
        .unwrap();

        enqueue_initial(&q, "mem-1");

        let snap = pool.shutdown(Duration::from_secs(2)).unwrap();
        assert_eq!(snap.jobs_processed, 1);
        assert_eq!(snap.jobs_failed, 0);
        assert_eq!(snap.jobs_in_flight, 0);
        assert_eq!(proc.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn counts_failures() {
        let q = Arc::new(BoundedJobQueue::new(8));
        let proc = Arc::new(RecordingProcessor::new().fail_on("bad"));
        let pool = WorkerPool::start(
            &cfg(1),
            q.clone() as Arc<dyn JobQueue>,
            proc.clone() as Arc<dyn JobProcessor>,
        )
        .unwrap();

        enqueue_initial(&q, "ok");
        enqueue_initial(&q, "bad");
        enqueue_initial(&q, "ok2");

        let snap = pool.shutdown(Duration::from_secs(2)).unwrap();
        assert_eq!(snap.jobs_processed, 2);
        assert_eq!(snap.jobs_failed, 1);
        assert_eq!(snap.jobs_in_flight, 0);
    }

    #[test]
    fn session_affinity_same_memory_same_worker() {
        // 4 workers, 20 jobs across 3 distinct memory_ids. Every job for
        // the same memory_id must execute on the same thread.
        let q = Arc::new(BoundedJobQueue::new(64));
        let proc = Arc::new(RecordingProcessor::new().with_delay(Duration::from_millis(2)));
        let pool = WorkerPool::start(
            &cfg(4),
            q.clone() as Arc<dyn JobQueue>,
            proc.clone() as Arc<dyn JobProcessor>,
        )
        .unwrap();

        let ids = ["alpha", "beta", "gamma"];
        for i in 0..20 {
            enqueue_initial(&q, ids[i % 3]);
        }

        let _ = pool.shutdown(Duration::from_secs(5)).unwrap();

        // Group thread names by memory_id; each group must be size 1.
        let seen = proc.seen.lock().unwrap().clone();
        let mut by_mem: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for (tn, mid) in seen {
            by_mem.entry(mid).or_default().insert(tn);
        }
        for (mid, threads) in by_mem {
            assert_eq!(
                threads.len(),
                1,
                "memory_id {mid} executed on multiple threads: {threads:?}"
            );
        }
    }

    #[test]
    fn drains_on_shutdown() {
        let q = Arc::new(BoundedJobQueue::new(64));
        let proc = Arc::new(RecordingProcessor::new().with_delay(Duration::from_millis(5)));
        let pool = WorkerPool::start(
            &cfg(2),
            q.clone() as Arc<dyn JobQueue>,
            proc.clone() as Arc<dyn JobProcessor>,
        )
        .unwrap();

        for i in 0..30 {
            enqueue_initial(&q, &format!("m-{i}"));
        }

        // Generous deadline — all should complete.
        let snap = pool.shutdown(Duration::from_secs(5)).unwrap();
        assert_eq!(snap.jobs_processed, 30, "all jobs should drain within deadline");
        assert_eq!(snap.jobs_in_flight, 0);
    }

    #[test]
    fn shutdown_close_rejects_new_enqueues() {
        let q = Arc::new(BoundedJobQueue::new(8));
        let proc = Arc::new(RecordingProcessor::new());
        let pool = WorkerPool::start(
            &cfg(1),
            q.clone() as Arc<dyn JobQueue>,
            proc as Arc<dyn JobProcessor>,
        )
        .unwrap();

        enqueue_initial(&q, "m-1");
        let _ = pool.shutdown(Duration::from_secs(2)).unwrap();

        // Queue should now be closed.
        let r = q.try_enqueue(PipelineJob::initial("late".into(), Uuid::new_v4()));
        assert!(r.is_err(), "post-shutdown enqueue must be rejected");
    }

    #[test]
    fn route_is_deterministic_and_in_range() {
        for n in [1usize, 2, 4, 8] {
            for id in ["a", "memory-42", "long-id-with-stuff-zzz"] {
                let r1 = route(id, n);
                let r2 = route(id, n);
                assert_eq!(r1, r2, "route must be deterministic");
                assert!(r1 < n, "route must be in [0, n)");
            }
        }
    }

    #[test]
    fn non_droppable_job_not_dropped_under_pressure() {
        // 1 worker, slow processor. Flood with droppable jobs to fill
        // the inbox, then send a non-droppable ReExtract — it must
        // eventually be processed (not dropped).
        let q = Arc::new(BoundedJobQueue::new(128));
        let proc = Arc::new(RecordingProcessor::new().with_delay(Duration::from_millis(3)));
        let pool = WorkerPool::start(
            &cfg(1),
            q.clone() as Arc<dyn JobQueue>,
            proc.clone() as Arc<dyn JobProcessor>,
        )
        .unwrap();

        for i in 0..50 {
            enqueue_initial(&q, &format!("m-{i}"));
        }
        // ReExtract is non-droppable (per design §5.2).
        q.try_enqueue(PipelineJob {
            memory_id: "important".into(),
            episode_id: Uuid::new_v4(),
            mode: JobMode::ReExtract,
            enqueued_at: chrono::Utc::now(),
        })
        .unwrap();

        let snap = pool.shutdown(Duration::from_secs(10)).unwrap();
        // The non-droppable must have been processed.
        let seen_important = proc
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|(_, mid)| mid == "important");
        assert!(seen_important, "non-droppable ReExtract was not processed");
        assert!(snap.jobs_processed >= 1);
    }
}
