//! Main Memory API — simplified interface to Engram's cognitive models.

use chrono::Utc;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Write;
use uuid::Uuid;

use crate::bus::EmpathyBus;
use crate::config::MemoryConfig;
use crate::embeddings::{EmbeddingConfig, EmbeddingProvider, EmbeddingError};
use crate::entities::EntityExtractor;
use crate::extractor::MemoryExtractor;
use crate::models::{effective_strength, retrieval_activation, run_consolidation_cycle};
use crate::storage::{Storage, EmbeddingStats};
use crate::bus::{SubscriptionManager, Subscription, Notification};
use crate::models::hebbian::{MemoryWithNamespace, record_cross_namespace_coactivation};
use crate::session_wm::{ActiveContext, SessionRecallResult};
use crate::synthesis::types::SynthesisEngine;
use crate::lifecycle::{DecayReport, ForgetReport, LifecycleError, ReconcileCandidate, ReconcileReport, PhaseReport};
use crate::types::{AclEntry, CrossLink, HebbianLink, LayerStats, MemoryLayer, MemoryRecord, MemoryStats, MemoryType, Permission, RecallResult, RecallWithAssociationsResult, TypeStats};

/// Report from a unified sleep cycle (consolidation + synthesis).
#[derive(Debug)]
pub struct SleepReport {
    /// Whether the consolidation phase completed successfully.
    pub consolidation_ok: bool,
    /// Synthesis report (None if synthesis not enabled or failed non-fatally).
    pub synthesis: Option<crate::synthesis::types::SynthesisReport>,
    /// Per-phase timing reports.
    pub phases: Vec<crate::lifecycle::PhaseReport>,
    /// Decay check report (if run).
    pub decay: Option<crate::lifecycle::DecayReport>,
    /// Forget report (if run).
    pub forget: Option<crate::lifecycle::ForgetReport>,
    /// Rebalance repair report (if run).
    pub rebalance: Option<crate::lifecycle::RebalanceReport>,
    /// Total sleep cycle duration in milliseconds.
    pub duration_ms: u64,
}

/// Check if a memory record is a synthesis insight.
pub fn is_insight(record: &MemoryRecord) -> bool {
    record
        .metadata
        .as_ref()
        .and_then(|m| m.get("is_synthesis"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Placeholder sink used only long enough for `Memory::new*`
/// constructors to return a value. Immediately replaced by
/// `install_default_counting_sink` before the `Memory` escapes the
/// constructor. Exists because Rust's partial-move rules make it
/// awkward to construct an `Arc<dyn EventSink>` and a concrete
/// `Arc<CountingSink>` that share storage *inside* a struct literal;
/// doing it in two steps (placeholder → real) keeps every
/// constructor readable.
fn default_event_sink_placeholder() -> crate::write_stats::SharedSink {
    std::sync::Arc::new(crate::write_stats::NoopSink)
}

/// Main interface to the Engram memory system.
///
/// Wraps the neuroscience math models behind a clean API.
/// All complexity is hidden — you just add, recall, and consolidate.
pub struct Memory {
    storage: Storage,
    config: MemoryConfig,
    created_at: chrono::DateTime<Utc>,
    /// Agent ID for this memory instance (used for ACL checks)
    agent_id: Option<String>,
    /// Optional Empathy Bus for drive alignment and emotional tracking
    empathy_bus: Option<EmpathyBus>,
    /// Embedding provider for semantic similarity (optional - falls back to FTS if unavailable)
    embedding: Option<EmbeddingProvider>,
    /// Optional LLM-based memory extractor for converting raw text to structured facts
    extractor: Option<Box<dyn MemoryExtractor>>,
    /// Entity extractor for identifying entities in memory content
    entity_extractor: EntityExtractor,
    /// Optional synthesis settings (None = synthesis disabled)
    synthesis_settings: Option<crate::synthesis::types::SynthesisSettings>,
    /// Optional LLM provider for synthesis insight generation
    synthesis_llm_provider: Option<Box<dyn crate::synthesis::types::SynthesisLlmProvider>>,
    /// Interoceptive hub for unified internal state monitoring
    interoceptive_hub: crate::interoceptive::InteroceptiveHub,
    /// Optional LLM-based triple extractor for knowledge graph enrichment
    triple_extractor: Option<Box<dyn crate::triple_extractor::TripleExtractor>>,
    /// Cached emotion data from last LLM extraction: Vec<(valence, domain)>.
    /// One-shot: `take_last_emotions()` clears it.
    last_extraction_emotions: std::sync::Mutex<Option<Vec<(f64, String)>>>,
    /// Recent recall timestamps for cross-recall co-occurrence detection (C8).
    /// Bounded ring buffer: last 50 recalls.
    recent_recalls: VecDeque<(String, std::time::Instant)>,
    /// Last add result for metrics access. Reset each sleep_cycle.
    last_add_result: Option<crate::lifecycle::AddResult>,
    /// Dedup merge counter (reset each sleep_cycle).
    dedup_merge_count: usize,
    /// Dedup new-write counter (reset each sleep_cycle).
    dedup_write_count: usize,
    /// Meta-cognition tracker for self-monitoring (lazy-init when enabled).
    metacognition: Option<crate::metacognition::MetaCognitionTracker>,
    /// Haiku-based intent classifier for Level 2 query-intent classification.
    /// Initialized via `auto_configure_intent_classifier()` when enabled in config.
    intent_classifier: Option<crate::query_classifier::HaikuIntentClassifier>,
    /// Write-path telemetry sink (ISS-019 Step 8). Default is a
    /// `CountingSink` so every instance gets structured counters
    /// for free. Callers that want a different sink (metrics
    /// exporter, capturing test sink) install it via
    /// [`Memory::set_event_sink`]. Held as `Arc<dyn EventSink>` so
    /// tests can keep a reference to the same sink after handing it
    /// to `Memory`.
    event_sink: crate::write_stats::SharedSink,
    /// Hot-path handle to the default `CountingSink`, if that's what
    /// we're holding. `write_stats()` uses it to snapshot without a
    /// downcast. `None` means the caller installed a custom sink
    /// and `write_stats()` returns `None` — callers with custom
    /// sinks read counters via their own handle.
    counting_sink: Option<std::sync::Arc<crate::write_stats::CountingSink>>,
    /// v0.3 §3.1 ingestion queue (Step C).
    ///
    /// `None` → v0.2-compat mode: `store_raw` runs the existing
    /// admission path and returns. No graph-extraction work is
    /// scheduled. This is the default for callers that haven't opted
    /// into v0.3 graph features.
    ///
    /// `Some(queue)` → after a successful `store_raw` admission, a
    /// `PipelineJob::initial(memory_id, episode_id)` is enqueued for
    /// each stored fact. Enqueue failures (`QueueFull`, `Closed`)
    /// are **non-fatal** to admission per GUARD-1: the L1/L2 write
    /// stays committed and the failure is recorded via
    /// `StoreEvent` telemetry. The recoverable memories are
    /// surfaceable via `extraction_status` once Step C-bis lands.
    ///
    /// Held as `Arc<dyn JobQueue>` so producers (this struct) and
    /// consumers (the worker pool, Step D) can share one instance.
    job_queue: Option<std::sync::Arc<dyn crate::resolution::JobQueue>>,

    /// Worker pool handle (ISS-037 Step 3). Owned by `Memory` so the
    /// pool's lifetime is tied to the memory instance — dropping `Memory`
    /// signals shutdown, and explicit `Memory::shutdown()` performs a
    /// graceful drain.
    ///
    /// Held inside an `Option<Mutex<Option<WorkerPool>>>` (rather than
    /// just `Option<WorkerPool>`) because shutdown takes the pool by
    /// value (`WorkerPool::shutdown(self, ...)`) but `Memory::shutdown`
    /// receives `&mut self` — `Mutex<Option<_>>::lock().take()` lets us
    /// extract the pool without consuming `Memory`.
    pipeline_pool: Option<std::sync::Mutex<Option<crate::resolution::worker::WorkerPool>>>,
}

impl Memory {
    // -----------------------------------------------------------------
    // ISS-019 Step 8: write-path telemetry wiring
    // -----------------------------------------------------------------

    /// Install a fresh `CountingSink` into both `event_sink` (as
    /// `dyn EventSink`) and `counting_sink` (concrete handle).
    ///
    /// Called immediately after every constructor so `write_stats()`
    /// works out of the box. The two fields share one `Arc` — the
    /// concrete handle is just a clone, so `snapshot()` on it sees
    /// every event the trait object records.
    fn install_default_counting_sink(&mut self) {
        let counting = std::sync::Arc::new(crate::write_stats::CountingSink::new());
        self.event_sink = counting.clone() as crate::write_stats::SharedSink;
        self.counting_sink = Some(counting);
    }

    /// Install a custom [`EventSink`]. Disables the default
    /// `CountingSink` fast path — `write_stats()` returns `None`
    /// from now on, since the caller owns their own counters.
    ///
    /// Use cases:
    /// - Tests that want to capture the full event stream.
    /// - Production that wires events to Prometheus / OTel exporters.
    pub fn set_event_sink(&mut self, sink: crate::write_stats::SharedSink) {
        self.event_sink = sink;
        self.counting_sink = None;
    }

    /// Restore the default `CountingSink`. Any events recorded by
    /// the previously-installed sink are lost (this is intentional —
    /// mixing event streams would be a correctness trap).
    pub fn install_default_write_stats(&mut self) {
        self.install_default_counting_sink();
    }

    // -----------------------------------------------------------------
    // v0.3 §3.1 ingestion queue (Step C)
    // -----------------------------------------------------------------

    /// Install the v0.3 resolution-pipeline job queue. After this is
    /// set, every successful `store_raw` admission enqueues a
    /// `PipelineJob::initial` so the (Step-D) worker pool can run
    /// graph extraction asynchronously.
    ///
    /// Calling this with the same queue arc the worker pool dequeues
    /// from is the only wiring required on the producer side.
    /// Re-installing replaces the previous queue; in-flight jobs on
    /// the old queue are unaffected.
    ///
    /// Per GUARD-1, enqueue failures (`QueueFull`, `Closed`) never
    /// abort `store_raw` — the L1/L2 write remains committed and the
    /// memory surfaces as `Pending(queue_full)` once Step C-bis
    /// adds the `Pending` run-status row at enqueue time.
    pub fn set_job_queue(&mut self, queue: std::sync::Arc<dyn crate::resolution::JobQueue>) {
        self.job_queue = Some(queue);
    }

    /// Builder-style variant of [`Memory::set_job_queue`]. Returns
    /// `self` so test/setup code can chain.
    pub fn with_job_queue(mut self, queue: std::sync::Arc<dyn crate::resolution::JobQueue>) -> Self {
        self.set_job_queue(queue);
        self
    }

    /// Borrow the currently-installed job queue, if any. Test-only
    /// hook so assertions can read queue depth without keeping a
    /// separate handle around.
    #[doc(hidden)]
    pub fn job_queue_ref(&self) -> Option<&std::sync::Arc<dyn crate::resolution::JobQueue>> {
        self.job_queue.as_ref()
    }

    /// Wire up the v0.3 resolution pipeline end-to-end (ISS-037 Step 3).
    ///
    /// Constructs the full pipeline machinery and attaches it to this
    /// `Memory` instance:
    ///
    /// 1. Opens a **second** SQLite [`Connection`] against `db_path`,
    ///    [`Box::leak`]ed to obtain `&'static mut Connection` so the
    ///    resulting [`SqliteGraphStore`] can satisfy the `'static`
    ///    bound that [`crate::resolution::worker::WorkerPool`]'s
    ///    `Arc<dyn JobProcessor>` requires (ISS-037 Blocker 1). The
    ///    leak is intentional: the connection's natural lifetime IS the
    ///    process lifetime, and the worker pool's shutdown drops all
    ///    references except this leaked one — which is correct.
    /// 2. Opens a third connection inside [`SqliteMemoryReader`] for
    ///    the cross-thread memory-row read path (ISS-037 Blocker 2).
    /// 3. Builds [`ResolutionPipeline`] with the supplied
    ///    `triple_extractor` (caller-injected so tests can pass mocks
    ///    and production can pass an LLM-backed impl).
    /// 4. Constructs a [`BoundedJobQueue`] of `queue_capacity` and
    ///    installs it via [`Memory::set_job_queue`].
    /// 5. Starts a [`WorkerPool`] with `worker_count` workers running
    ///    the pipeline as their [`JobProcessor`].
    ///
    /// After this call, every [`Memory::store_raw`] that successfully
    /// admits a fact also enqueues a `PipelineJob::initial`, and the
    /// worker pool drains the queue, populating the v0.3 graph.
    ///
    /// # Concurrency model
    ///
    /// - Foreground writes go through `self.storage.conn` (one
    ///   connection).
    /// - Pipeline graph writes go through the leaked connection
    ///   wrapped in `Arc<Mutex<SqliteGraphStore<'static>>>` (one
    ///   connection, serialized by the mutex).
    /// - Pipeline memory reads go through `SqliteMemoryReader`'s own
    ///   `Mutex<Connection>` (one connection).
    ///
    /// All three connections target the same DB file. SQLite WAL mode
    /// (set up in [`Storage::new`] and re-applied by
    /// `SqliteMemoryReader::open`) handles cross-connection
    /// concurrency without explicit coordination.
    ///
    /// # Errors
    ///
    /// Returns an error if the second/third connection fails to open
    /// or if [`WorkerPool::start`] fails (typically: invalid config).
    pub fn with_pipeline_pool(
        mut self,
        db_path: impl AsRef<std::path::Path>,
        triple_extractor: std::sync::Arc<dyn crate::triple_extractor::TripleExtractor>,
        config: crate::resolution::ResolutionConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use std::sync::{Arc, Mutex};
        use crate::graph::store::SqliteGraphStore;
        use crate::resolution::pipeline::{PipelineConfig, ResolutionPipeline};
        use crate::resolution::worker::{JobProcessor, WorkerPool};
        use crate::resolution::{BoundedJobQueue, JobQueue, SqliteMemoryReader};

        let db_path_ref = db_path.as_ref();

        // (1) Leaked connection for the graph store. See doc comment for
        // the rationale — this is correct semantics, not a leak in the
        // resource-bug sense.
        let graph_conn: &'static mut rusqlite::Connection = {
            let conn = rusqlite::Connection::open(db_path_ref)?;
            // Match Storage::new pragmas. Critical for WAL coexistence.
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
            )?;
            Box::leak(Box::new(conn))
        };
        let graph_store = SqliteGraphStore::new(graph_conn);
        let store_arc: Arc<Mutex<SqliteGraphStore<'static>>> = Arc::new(Mutex::new(graph_store));

        // (2) Memory reader (separate connection, internally Mutex-wrapped).
        let memory_reader: Arc<dyn crate::resolution::pipeline::MemoryReader> =
            Arc::new(SqliteMemoryReader::open(db_path_ref)?);

        // (3) Build the pipeline. Entity extractor is freshly constructed
        //     from this Memory's existing config so the pipeline sees
        //     identical extraction behavior. (EntityExtractor is not
        //     Clone; reconstruction from config is the canonical path.)
        let entity_extractor = Arc::new(crate::entities::EntityExtractor::new(
            &self.config.entity_config,
        ));
        let pipeline = ResolutionPipeline::new(
            memory_reader,
            entity_extractor,
            triple_extractor,
            store_arc,
            PipelineConfig::default(),
        );
        let processor: Arc<dyn JobProcessor> = Arc::new(pipeline);

        // (4) Queue + install on producer side.
        let queue: Arc<dyn JobQueue> = Arc::new(BoundedJobQueue::new(config.queue_cap));
        self.set_job_queue(Arc::clone(&queue));

        // (5) Start the worker pool.
        let pool = WorkerPool::start(&config, queue, processor)?;
        self.pipeline_pool = Some(std::sync::Mutex::new(Some(pool)));

        Ok(self)
    }

    /// Gracefully shut down the resolution pipeline pool, if any.
    ///
    /// Closes the queue, drains in-flight jobs (bounded by `deadline`),
    /// joins all worker threads, and returns the final pool stats. Safe
    /// to call multiple times — subsequent calls return `Ok(None)`.
    ///
    /// If the pool was never installed (no [`Memory::with_pipeline_pool`]
    /// call), returns `Ok(None)`.
    ///
    /// # Why this is on `&mut self`
    ///
    /// [`WorkerPool::shutdown`] takes `self` by value. The pool is held
    /// inside `Mutex<Option<WorkerPool>>`, so shutdown is `take()` →
    /// `pool.shutdown(deadline)`. This lets `Memory::shutdown` consume
    /// the pool without consuming the `Memory` itself.
    pub fn shutdown_pipeline(
        &mut self,
        deadline: std::time::Duration,
    ) -> Result<
        Option<crate::resolution::worker::WorkerPoolStatsSnapshot>,
        crate::resolution::worker::WorkerPoolError,
    > {
        let Some(slot) = self.pipeline_pool.as_ref() else {
            return Ok(None);
        };
        let mut guard = slot.lock().expect("pipeline_pool mutex poisoned");
        let Some(pool) = guard.take() else {
            return Ok(None);
        };
        pool.shutdown(deadline).map(Some)
    }

    /// §6.3 introspection: current `ExtractionStatus` for `memory_id`.
    ///
    /// Reads `graph_pipeline_runs` for the latest row scoped to the
    /// memory and maps it via [`ExtractionStatus::from_run_row`].
    /// Memories that have never been enqueued (legacy v0.2 rows or —
    /// pre-Step-C-bis — newly-enqueued ones whose pending row is not
    /// yet written) return [`ExtractionStatus::NotStarted`].
    ///
    /// Takes `&mut self` because the underlying `SqliteGraphStore`
    /// holds `&mut Connection`. The query itself is read-only — no
    /// rows are mutated — but the borrow signature follows the graph
    /// store's existing constructor shape.
    ///
    /// Errors are surfaced as `Box<dyn Error>` to match the
    /// established Memory error convention; callers that want the
    /// structured `GraphError` can downcast.
    pub fn extraction_status(
        &mut self,
        memory_id: &str,
    ) -> Result<crate::resolution::ExtractionStatus, Box<dyn std::error::Error>> {
        use crate::graph::store::{GraphStore, SqliteGraphStore};
        let conn = self.storage.connection_mut();
        let store = SqliteGraphStore::new(conn);
        let row = store.latest_pipeline_run_for_memory(memory_id)?;
        Ok(crate::resolution::ExtractionStatus::from_run_row(row))
    }

    /// Enqueue a `PipelineJob::initial` for `memory_id` after a
    /// successful `store_raw` admission. **Must not fail
    /// `store_raw`** per GUARD-1 — we record telemetry on enqueue
    /// rejection but always return `Ok(())` semantically (the L1/L2
    /// write is already committed by the time we get here).
    ///
    /// `episode_id` is generated fresh per call: every `store_raw`
    /// admission is a distinct episodic write event. Persisting the
    /// id back onto the `memories.episode_id` column is graph-layer
    /// integration (deferred); for Step C the id only flows through
    /// the queue to the (future) worker, which begins the pipeline
    /// run with it via `begin_pipeline_run_for_memory`.
    ///
    /// Returns the generated `episode_id` so callers can correlate
    /// follow-up reads with the enqueued job; `None` when no queue
    /// is installed (v0.2-compat mode).
    fn enqueue_pipeline_job(&self, memory_id: &str) -> Option<uuid::Uuid> {
        let queue = self.job_queue.as_ref()?;
        let episode_id = uuid::Uuid::new_v4();
        let job = crate::resolution::PipelineJob::initial(memory_id.to_string(), episode_id);
        match queue.try_enqueue(job) {
            Ok(()) => Some(episode_id),
            Err(err) => {
                // GUARD-1: never propagate. Log and let admission stand.
                // Step C-bis will write a `pending(queue_full)` row to
                // graph_pipeline_runs so this surfaces in
                // `extraction_status`; until then the memory reads as
                // `NotStarted` and is recoverable via `reextract`.
                log::warn!(
                    "store_raw: pipeline enqueue failed for memory {} ({}); admission preserved",
                    memory_id,
                    err
                );
                Some(episode_id)
            }
        }
    }


    /// Snapshot the current write-path counters.
    ///
    /// Returns `None` if the caller has installed a custom sink via
    /// [`Memory::set_event_sink`]. In that case, the caller owns
    /// their own handle and reads counters from there.
    ///
    /// Otherwise returns a fresh [`WriteStats`] reflecting every
    /// event recorded through `store_raw` since the last
    /// [`Memory::reset_write_stats`] (or since construction).
    pub fn write_stats(&self) -> Option<crate::write_stats::WriteStats> {
        self.counting_sink.as_ref().map(|s| s.snapshot())
    }

    /// Zero the default `CountingSink` counters. No effect if a
    /// custom sink is installed (returns `false`); the caller is
    /// expected to reset their own sink. Returns `true` when the
    /// default sink was reset.
    ///
    /// Used by rebuild-pilot code to bracket a batch with a clean
    /// window; used by tests to isolate scenarios.
    pub fn reset_write_stats(&mut self) -> bool {
        match self.counting_sink.as_ref() {
            Some(s) => {
                s.reset();
                true
            }
            None => false,
        }
    }

    /// Emit a [`StoreEvent`] through the installed sink. Internal
    /// helper so `store_raw` call sites stay concise. Never panics
    /// even if the sink's `record` is ill-behaved — the trait bound
    /// forbids panic but we don't rely on it.
    #[inline]
    fn emit_store_event(&self, event: crate::write_stats::StoreEvent) {
        self.event_sink.record(event);
    }

    /// Initialize Engram memory system.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file. Created if it doesn't exist.
    ///   Use `:memory:` for in-memory (non-persistent) operation.
    /// * `config` - MemoryConfig with tunable parameters. None = literature defaults.
    ///
    /// If Ollama is available, embeddings will be used for semantic search.
    /// Otherwise, falls back to FTS5 keyword matching.
    pub fn new(path: &str, config: Option<MemoryConfig>) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            log::info!("Ollama available at {}, embedding enabled", config.embedding.host);
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
            event_sink: default_event_sink_placeholder(),
            counting_sink: None,
            job_queue: None,
            pipeline_pool: None,
        };
        // Install the default CountingSink, sharing one Arc between
        // the trait-object slot and the fast-path `counting_sink`
        // handle. See `Memory::set_event_sink` for the custom-sink
        // alternative.
        mem.install_default_counting_sink();
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();

        // Initialize meta-cognition tracker if enabled
        mem.init_metacognition_if_enabled();
        
        Ok(mem)
    }
    
    /// Initialize Engram memory system requiring embeddings.
    ///
    /// Returns an error if Ollama is not available. Use this when embedding
    /// support is required for your use case.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `config` - MemoryConfig with tunable parameters
    pub fn new_with_required_embedding(
        path: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create embedding provider and validate Ollama is available
        let embedding = EmbeddingProvider::new(config.embedding.clone());
        if !embedding.is_available() {
            return Err(Box::new(EmbeddingError::OllamaNotAvailable(
                config.embedding.host.clone()
            )));
        }

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding: Some(embedding),
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
            event_sink: default_event_sink_placeholder(),
            counting_sink: None,
            job_queue: None,
            pipeline_pool: None,
        };
        // Install the default CountingSink, sharing one Arc between
        // the trait-object slot and the fast-path `counting_sink`
        // handle. See `Memory::set_event_sink` for the custom-sink
        // alternative.
        mem.install_default_counting_sink();
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }
    
    /// Initialize Engram memory system with custom embedding config.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `config` - MemoryConfig with tunable parameters
    /// * `embedding_config` - Custom embedding configuration (overrides config.embedding)
    pub fn with_embedding(
        path: &str,
        config: Option<MemoryConfig>,
        embedding_config: EmbeddingConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let mut config = config.unwrap_or_default();
        config.embedding = embedding_config;
        let created_at = Utc::now();
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };

        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus: None,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
            event_sink: default_event_sink_placeholder(),
            counting_sink: None,
            job_queue: None,
            pipeline_pool: None,
        };
        // Install the default CountingSink, sharing one Arc between
        // the trait-object slot and the fast-path `counting_sink`
        // handle. See `Memory::set_event_sink` for the custom-sink
        // alternative.
        mem.install_default_counting_sink();
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }
    
    /// Create a Memory instance with an Empathy Bus attached.
    ///
    /// The Empathy Bus connects memory to workspace files (SOUL.md, HEARTBEAT.md)
    /// for drive alignment and empathy feedback loops.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to SQLite database file
    /// * `workspace_dir` - Path to the agent workspace directory
    /// * `config` - Optional MemoryConfig
    pub fn with_empathy_bus(
        path: &str,
        workspace_dir: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let storage = Storage::new(path)?;
        let config = config.unwrap_or_default();
        let created_at = Utc::now();
        
        // Create Empathy Bus using storage's connection
        let empathy_bus = Some(EmpathyBus::new(workspace_dir, storage.connection())?);
        
        // Create embedding provider (optional - check if Ollama is available)
        let embedding_provider = EmbeddingProvider::new(config.embedding.clone());
        let embedding = if embedding_provider.is_available() {
            Some(embedding_provider)
        } else {
            log::warn!("Ollama not available at {}, falling back to FTS", config.embedding.host);
            None
        };
        
        let entity_extractor = EntityExtractor::new(&config.entity_config);

        let mut mem = Self {
            storage,
            config,
            created_at,
            agent_id: None,
            empathy_bus,
            embedding,
            extractor: None,
            entity_extractor,
            synthesis_settings: None,
            synthesis_llm_provider: None,
            interoceptive_hub: crate::interoceptive::InteroceptiveHub::new(),
            triple_extractor: None,
            last_extraction_emotions: std::sync::Mutex::new(None),
            recent_recalls: VecDeque::new(),
            last_add_result: None,
            dedup_merge_count: 0,
            dedup_write_count: 0,
            metacognition: None,
            intent_classifier: None,
            event_sink: default_event_sink_placeholder(),
            counting_sink: None,
            job_queue: None,
            pipeline_pool: None,
        };
        // Install the default CountingSink, sharing one Arc between
        // the trait-object slot and the fast-path `counting_sink`
        // handle. See `Memory::set_event_sink` for the custom-sink
        // alternative.
        mem.install_default_counting_sink();
        
        // Auto-configure extractor from environment/config
        mem.auto_configure_extractor();
        mem.auto_configure_intent_classifier();
        
        Ok(mem)
    }

    /// Backward-compat alias.
    pub fn with_emotional_bus(
        path: &str,
        workspace_dir: &str,
        config: Option<MemoryConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_empathy_bus(path, workspace_dir, config)
    }

    /// Initialize the meta-cognition tracker if enabled in config.
    fn init_metacognition_if_enabled(&mut self) {
        if self.config.metacognition_enabled && self.metacognition.is_none() {
            match crate::metacognition::MetaCognitionTracker::new(self.storage.conn()) {
                Ok(mut tracker) => {
                    if let Err(e) = tracker.load_history(self.storage.conn()) {
                        log::warn!("Failed to load metacognition history: {}", e);
                    }
                    self.metacognition = Some(tracker);
                }
                Err(e) => {
                    log::warn!("Failed to initialize metacognition tracker: {}", e);
                }
            }
        }
    }

    /// Get a meta-cognition report with current metrics.
    ///
    /// Returns None if metacognition is not enabled.
    pub fn metacognition_report(&self) -> Option<crate::metacognition::MetaCognitionReport> {
        self.metacognition.as_ref().map(|t| t.report())
    }

    /// Get parameter adjustment suggestions based on observed patterns.
    ///
    /// Returns None if metacognition is not enabled.
    pub fn parameter_suggestions(&self) -> Option<Vec<crate::metacognition::ParameterSuggestion>> {
        self.metacognition.as_ref().map(|t| t.parameter_suggestions(&self.config))
    }

    /// Submit external feedback for the most recent recall.
    ///
    /// `score`: 0.0 (useless) to 1.0 (perfect). Returns Ok(true) if feedback
    /// was attached, Ok(false) if no pending recall event, None if metacognition disabled.
    pub fn feedback_recall(&mut self, score: f64) -> Option<Result<bool, Box<dyn std::error::Error>>> {
        if let Some(ref mut tracker) = self.metacognition {
            Some(tracker.feedback_event(self.storage.conn(), score))
        } else {
            None
        }
    }
    
    /// Get a reference to the Empathy Bus, if attached.
    pub fn empathy_bus(&self) -> Option<&EmpathyBus> {
        self.empathy_bus.as_ref()
    }

    /// Backward-compat alias.
    #[inline]
    pub fn emotional_bus(&self) -> Option<&EmpathyBus> { self.empathy_bus() }
    
    /// Get a mutable reference to the Empathy Bus, if attached.
    pub fn empathy_bus_mut(&mut self) -> Option<&mut EmpathyBus> {
        self.empathy_bus.as_mut()
    }

    /// Backward-compat alias.
    #[inline]
    pub fn emotional_bus_mut(&mut self) -> Option<&mut EmpathyBus> { self.empathy_bus_mut() }

    // ── Interoceptive Hub API ─────────────────────────────────────────

    /// Get a reference to the interoceptive hub.
    pub fn interoceptive_hub(&self) -> &crate::interoceptive::InteroceptiveHub {
        &self.interoceptive_hub
    }

    /// Get a mutable reference to the interoceptive hub.
    pub fn interoceptive_hub_mut(&mut self) -> &mut crate::interoceptive::InteroceptiveHub {
        &mut self.interoceptive_hub
    }

    /// Take a snapshot of the current interoceptive state.
    ///
    /// Returns the integrated state across all domains — suitable for
    /// injection into system prompts or inspection.
    pub fn interoceptive_snapshot(&self) -> crate::interoceptive::InteroceptiveState {
        self.interoceptive_hub.current_state()
    }

    // ── Extraction Emotion Cache ───────────────────────────────────────

    /// Take the emotion data from the most recent LLM extraction.
    ///
    /// Returns `None` if no extraction has occurred since the last call.
    /// This is one-shot: calling it clears the cache.
    ///
    /// Each entry is `(valence, domain)` — one per extracted fact.
    /// A single user message may produce multiple facts with different
    /// valences and domains.
    pub fn take_last_emotions(&self) -> Option<Vec<(f64, String)>> {
        self.last_extraction_emotions.lock().unwrap().take()
    }

    /// Run an interoceptive tick: pull signals from all attached subsystems
    /// and feed them into the hub.
    ///
    /// Call this periodically (e.g., every heartbeat or every N messages)
    /// to keep the interoceptive state current.
    pub fn interoceptive_tick(&mut self) {
        use crate::bus::accumulator::EmpathyAccumulator;
        use crate::bus::feedback::BehaviorFeedback;
        use crate::interoceptive::InteroceptiveSignal;

        let mut signals: Vec<InteroceptiveSignal> = Vec::new();

        // Pull from DB-backed subsystems via the storage connection.
        let conn = self.storage.connection();

        // Accumulator: pull all empathy trends.
        if let Ok(acc) = EmpathyAccumulator::new(conn) {
            if let Ok(trends) = acc.get_all_trends() {
                for trend in &trends {
                    if let Ok(Some(sig)) = acc.to_signal(&trend.domain) {
                        signals.push(sig);
                    }
                }
            }
        }

        // Feedback: pull all action stats.
        if let Ok(fb) = BehaviorFeedback::new(conn) {
            if let Ok(stats) = fb.get_all_action_stats() {
                for stat in &stats {
                    if let Ok(Some(sig)) = fb.to_signal(&stat.action) {
                        signals.push(sig);
                    }
                }
            }
        }

        // Alignment: generate signal if emotional bus is attached (has drives).
        // Note: alignment is content-dependent, so we skip it in tick.
        // It's triggered per-interaction instead.

        // Feed all collected signals into the hub.
        if !signals.is_empty() {
            log::debug!("Interoceptive tick: processing {} signals", signals.len());
            self.interoceptive_hub.process_batch(signals);
        }
    }

    /// Feed an external interoceptive signal directly into the hub.
    ///
    /// Used by host agents (e.g., RustClaw) to inject runtime-sourced
    /// signals (OperationalLoad, ExecutionStress, CognitiveFlow, ResourcePressure)
    /// that originate outside of engram's own monitoring subsystems.
    pub fn feed_interoceptive_signal(&mut self, signal: crate::interoceptive::InteroceptiveSignal) {
        self.interoceptive_hub.process_signal(signal);
    }

    /// Broadcast memory admission to the interoceptive hub (GWT global workspace).
    ///
    /// When memories enter working memory (via recall), this broadcasts
    /// signals to the hub for integration. Implements Baars' Global Workspace
    /// Theory: working memory contents are "broadcast" to all cognitive modules.
    ///
    /// For each admitted memory:
    /// 1. Generate a confidence signal (metacognitive assessment)
    /// 2. Check drive alignment if emotional bus is attached
    /// 3. Spread activation to Hebbian neighbors (associative priming)
    ///
    /// Returns the IDs of Hebbian neighbors activated (for potential WM boosting).
    pub fn broadcast_admission(
        &mut self,
        memory_ids: &[String],
        session_wm: &mut ActiveContext,
    ) -> Vec<String> {
        use crate::interoceptive::InteroceptiveSignal;

        let mut signals: Vec<InteroceptiveSignal> = Vec::new();
        let mut neighbor_ids: Vec<String> = Vec::new();

        for memory_id in memory_ids {
            // Fetch the memory record for signal generation.
            let record = match self.storage.get(memory_id) {
                Ok(Some(r)) => r,
                _ => continue,
            };

            // 1. Confidence signal — metacognitive assessment of this memory.
            let conf_signal = crate::confidence::confidence_to_signal(&record, None, None);
            signals.push(conf_signal);

            // 2. Alignment signal — does this memory align with core drives?
            if let Some(ref bus) = self.empathy_bus {
                let drives = bus.drives();
                if !drives.is_empty() {
                    let align_signal =
                        crate::bus::alignment::alignment_to_signal(&record.content, drives);
                    signals.push(align_signal);
                }
            }

            // 3. Spreading activation — Hebbian neighbors get primed.
            if self.config.hebbian_enabled {
                if let Ok(neighbors) = self.storage.get_hebbian_links_weighted(memory_id) {
                    for (neighbor_id, weight) in &neighbors {
                        // Only spread to neighbors with meaningful link strength.
                        if *weight > 0.1 {
                            neighbor_ids.push(neighbor_id.clone());
                        }
                    }
                }
            }
        }

        // Feed all broadcast signals into the hub.
        if !signals.is_empty() {
            log::debug!(
                "GWT broadcast: {} memories → {} signals",
                memory_ids.len(),
                signals.len()
            );
            self.interoceptive_hub.process_batch(signals);
        }

        // Boost Hebbian neighbors in working memory (associative priming).
        // This implements spreading activation: memories connected to the
        // admitted ones get a small activation boost in WM.
        if !neighbor_ids.is_empty() {
            neighbor_ids.dedup();
            session_wm.activate(&neighbor_ids);
            log::debug!(
                "GWT spreading activation: {} Hebbian neighbors primed",
                neighbor_ids.len()
            );
        }

        neighbor_ids
    }
    
    /// Get the last add result (Created or Merged).
    pub fn last_add_result(&self) -> Option<&crate::lifecycle::AddResult> {
        self.last_add_result.as_ref()
    }

    /// Get a reference to the underlying storage connection.
    pub fn connection(&self) -> &rusqlite::Connection {
        self.storage.connection()
    }

    /// Get a reference to the underlying Storage.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Get a mutable reference to the underlying Storage.
    pub fn storage_mut(&mut self) -> &mut Storage {
        &mut self.storage
    }

    /// Access recent recalls ring buffer (for testing/inspection).
    pub fn recent_recalls(&self) -> &VecDeque<(String, std::time::Instant)> {
        &self.recent_recalls
    }

    /// Mutable access to recent recalls (for testing).
    pub fn recent_recalls_mut(&mut self) -> &mut VecDeque<(String, std::time::Instant)> {
        &mut self.recent_recalls
    }

    /// Scan namespace for duplicate pairs above similarity threshold.
    /// Returns candidates sorted by combined score (0.7 * embedding_sim + 0.3 * entity_jaccard).
    pub fn reconcile(
        &self,
        namespace: &str,
        max_scan: Option<usize>,
    ) -> Result<Vec<ReconcileCandidate>, LifecycleError> {
        let max_scan = max_scan.unwrap_or(1000);
        let model_id = self.config.embedding.model_id();

        // Load embeddings (bounded)
        let embeddings = self.storage.get_embeddings_in_namespace(
            Some(namespace), &model_id,
        ).map_err(LifecycleError::Storage)?;

        // Bound by max_scan
        let embeddings: Vec<_> = embeddings.into_iter().take(max_scan).collect();

        let mut candidates: Vec<ReconcileCandidate> = Vec::new();
        let mut seen_pairs: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

        for (id_a, emb_a) in &embeddings {
            let matches = self.storage.find_all_above_threshold(
                emb_a, &model_id, Some(namespace), 0.85,
            ).map_err(LifecycleError::Storage)?;

            for (id_b, score) in matches {
                if id_a == &id_b { continue; }
                let pair = if id_a < &id_b {
                    (id_a.clone(), id_b.clone())
                } else {
                    (id_b.clone(), id_a.clone())
                };
                if seen_pairs.contains(&pair) { continue; }
                seen_pairs.insert(pair);

                let entities_a = self.storage.get_entities_for_memory(id_a)
                    .map_err(LifecycleError::Storage)?;
                let entities_b = self.storage.get_entities_for_memory(&id_b)
                    .map_err(LifecycleError::Storage)?;
                let jaccard = jaccard_similarity_strings(&entities_a, &entities_b);

                let preview_a = self.storage.get_memory_content_preview(id_a, 100)
                    .map_err(LifecycleError::Storage)?;
                let preview_b = self.storage.get_memory_content_preview(&id_b, 100)
                    .map_err(LifecycleError::Storage)?;

                candidates.push(ReconcileCandidate {
                    id_a: id_a.clone(),
                    id_b,
                    similarity: score,
                    entity_overlap: jaccard,
                    content_preview_a: preview_a,
                    content_preview_b: preview_b,
                });
            }
        }

        // Sort by combined score: 0.7 * similarity + 0.3 * entity_overlap
        candidates.sort_by(|a, b| {
            let score_a = 0.7 * a.similarity as f64 + 0.3 * a.entity_overlap;
            let score_b = 0.7 * b.similarity as f64 + 0.3 * b.entity_overlap;
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(candidates)
    }

    /// Apply reconcile merges. If dry_run=true, just counts.
    pub fn reconcile_apply(
        &mut self,
        candidates: &[ReconcileCandidate],
        dry_run: bool,
    ) -> Result<ReconcileReport, LifecycleError> {
        let mut report = ReconcileReport {
            scanned: candidates.len(),
            candidates_found: candidates.len(),
            merges_applied: 0,
            dry_run,
        };

        if dry_run { return Ok(report); }

        let mut merged_away: std::collections::HashSet<String> = std::collections::HashSet::new();

        for candidate in candidates {
            if merged_away.contains(&candidate.id_a) || merged_away.contains(&candidate.id_b) {
                continue;
            }

            // Keep the older memory, merge newer into it
            let (keep_id, donor_id) = {
                let a = self.storage.get(&candidate.id_a).map_err(LifecycleError::Storage)?;
                let b = self.storage.get(&candidate.id_b).map_err(LifecycleError::Storage)?;
                match (a, b) {
                    (Some(a), Some(b)) => {
                        if a.created_at <= b.created_at {
                            (candidate.id_a.clone(), candidate.id_b.clone())
                        } else {
                            (candidate.id_b.clone(), candidate.id_a.clone())
                        }
                    }
                    _ => continue, // one already gone
                }
            };

            // Get donor content for merge
            let donor_content = self.storage.get_memory_content_preview(&donor_id, 10000)
                .map_err(LifecycleError::Storage)?;

            // ISS-019 Step 5.5: route through merge_enriched_into.
            // Lifecycle merge has no extractor-produced dimensions for
            // the donor side (we only hold its stored content). Build
            // a minimal EnrichedMemory — Dimensions::minimal populates
            // core_fact from content, scalars fall to defaults, and
            // Dimensions::union will preserve whatever the existing
            // (keep) side already has without loss.
            let donor_em = crate::enriched::EnrichedMemory::minimal(
                &donor_content,
                crate::dimensions::Importance::new(0.0),
                None,
                None,
            )
            .map_err(|e| LifecycleError::Storage(rusqlite::Error::InvalidColumnType(
                0,
                format!("EnrichedMemory::minimal failed in lifecycle merge: {}", e),
                rusqlite::types::Type::Text,
            )))?;
            self.storage
                .merge_enriched_into(&keep_id, &donor_em, candidate.similarity)
                .map_err(LifecycleError::Storage)?;

            // Merge Hebbian links
            let _ = self.storage.merge_hebbian_links(&donor_id, &keep_id);

            // Record provenance
            let _ = self.storage.append_merge_provenance(
                &keep_id, &donor_id, candidate.similarity, false,
            );

            // Delete donor
            self.storage.hard_delete_cascade(&donor_id)
                .map_err(LifecycleError::Storage)?;

            merged_away.insert(donor_id);
            report.merges_applied += 1;
        }

        Ok(report)
    }

    // === Lifecycle: Decay Detection + Safe Forget (FEAT-003 C5+C6) ===

    /// Check memories for decay using Ebbinghaus model and flag weak ones.
    /// Called during sleep_cycle's forget phase.
    pub fn check_decay_and_flag(
        &mut self,
        namespace: Option<&str>,
    ) -> Result<DecayReport, LifecycleError> {
        use crate::models::ebbinghaus;

        let memories = self.storage.all_in_namespace(namespace)
            .map_err(LifecycleError::Storage)?;
        let now = Utc::now();
        let mut report = DecayReport::default();

        for record in &memories {
            if record.pinned {
                continue;
            }
            let effective = ebbinghaus::effective_strength(record, now);
            if effective < 0.1 {
                report.below_threshold += 1;
                // Only auto-flag if rarely accessed (< 2 accesses)
                if record.access_times.len() < 2 {
                    self.storage.soft_delete(&record.id)
                        .map_err(LifecycleError::Storage)?;
                    report.flagged_for_forget += 1;
                    log::debug!(
                        "memory {} flagged for forget via Ebbinghaus decay (effective_strength={}, access_count={})",
                        record.id,
                        effective,
                        record.access_times.len(),
                    );
                }
            }
        }

        Ok(report)
    }

    /// Targeted forget: soft or hard delete a specific memory.
    pub fn forget_targeted(
        &mut self,
        memory_id: &str,
        soft: bool,
    ) -> Result<(), LifecycleError> {
        if soft {
            self.storage.soft_delete(memory_id)
                .map_err(LifecycleError::Storage)?;
            log::info!("soft-deleted memory {}", memory_id);
        } else {
            self.storage.hard_delete_cascade(memory_id)
                .map_err(LifecycleError::Storage)?;
            log::info!("hard-deleted memory {} with cascade", memory_id);
        }
        Ok(())
    }

    /// Bulk forget: soft-delete weak memories, hard-delete old soft-deleted ones.
    pub fn forget_bulk(&mut self) -> Result<ForgetReport, LifecycleError> {
        let mut report = ForgetReport::default();
        let now = Utc::now();

        // Phase A: soft-delete weak memories (that aren't already soft-deleted)
        let all = self.storage.all_in_namespace(None)  // active memories only
            .map_err(LifecycleError::Storage)?;
        report.scanned = all.len();

        for record in &all {
            if record.pinned { continue; }
            let effective = crate::models::ebbinghaus::effective_strength(record, now);
            if effective < self.config.forget_threshold {
                self.storage.soft_delete(&record.id)
                    .map_err(LifecycleError::Storage)?;
                report.soft_deleted += 1;
            }
        }

        // Phase B: hard-delete memories that were soft-deleted > 30 days ago
        let deleted = self.storage.list_deleted(Some("*"))
            .map_err(LifecycleError::Storage)?;
        for record in &deleted {
            // Parse deleted_at from DB column
            if let Some(deleted_at_str) = self.storage.get_deleted_at(&record.id)
                .map_err(LifecycleError::Storage)?
            {
                if let Ok(deleted_at) = chrono::DateTime::parse_from_rfc3339(&deleted_at_str) {
                    let days_deleted = (now - deleted_at.with_timezone(&Utc)).num_days();
                    if days_deleted > 30 {
                        self.storage.hard_delete_cascade(&record.id)
                            .map_err(LifecycleError::Storage)?;
                        report.hard_deleted += 1;
                    }
                }
            }
        }

        Ok(report)
    }

    /// Health check: inspect memory system integrity.
    pub fn health(&self) -> Result<crate::lifecycle::HealthReport, LifecycleError> {
        use crate::lifecycle::HealthReport;

        let total = self.storage.count_memories_in_namespace(None)
            .map_err(LifecycleError::Storage)?;
        let namespaces = self.storage.list_namespaces()
            .map_err(LifecycleError::Storage)?;
        let mut per_ns = std::collections::HashMap::new();
        for ns in &namespaces {
            let count = self.storage.count_memories_in_namespace(Some(ns))
                .map_err(LifecycleError::Storage)?;
            per_ns.insert(ns.clone(), count);
        }
        let orphans = self.storage.count_orphan_memories()
            .map_err(LifecycleError::Storage)?;
        let dangling = self.storage.count_dangling_hebbian()
            .map_err(LifecycleError::Storage)?;
        let soft_del = self.storage.count_soft_deleted()
            .map_err(LifecycleError::Storage)?;

        // Count memories below decay threshold
        let all = self.storage.all_in_namespace(None).map_err(LifecycleError::Storage)?;
        let now = Utc::now();
        let below = all.iter()
            .filter(|r| crate::models::ebbinghaus::effective_strength(r, now) < 0.1)
            .count();

        let stale = self.storage.count_stale_clusters()
            .map_err(LifecycleError::Storage)?;

        Ok(HealthReport {
            total_memories: total,
            per_namespace: per_ns,
            below_threshold: below,
            orphan_memories: orphans,
            stale_clusters: stale,
            dangling_hebbian_links: dangling,
            soft_deleted: soft_del,
        })
    }

    /// Rebalance: repair integrity issues.
    pub fn rebalance(&mut self) -> Result<crate::lifecycle::RebalanceReport, LifecycleError> {
        self.rebalance_internal()
    }

    fn rebalance_internal(&mut self) -> Result<crate::lifecycle::RebalanceReport, LifecycleError> {
        let mut report = crate::lifecycle::RebalanceReport::default();

        // 1. Remove orphaned access_log entries
        report.access_log_cleaned = self.storage.cleanup_orphaned_access_log()
            .map_err(LifecycleError::Storage)?;

        // 2. Repair dangling Hebbian links
        report.hebbian_repaired = self.storage.cleanup_dangling_hebbian()
            .map_err(LifecycleError::Storage)?;

        // 3. Cleanup entity_links for deleted memories
        report.entity_links_cleaned = self.storage.cleanup_orphaned_entity_links()
            .map_err(LifecycleError::Storage)?;

        // Note: embedding rebuilding requires EmbeddingProvider which is optional.
        // Skip for now — embeddings are rebuilt on next recall if missing.

        report.repairs = report.access_log_cleaned
            + report.hebbian_repaired
            + report.entity_links_cleaned;

        Ok(report)
    }

    // ── Supersession: High-Level API (Step 5) ──────────────────────────

    /// Correct a single memory: create a replacement and supersede the old one.
    ///
    /// This is the recommended way to fix wrong memories. It:
    /// 1. Fetches the old memory to inherit type/importance/namespace
    /// 2. Stores the new content as a fresh memory
    /// 3. Marks the old memory as superseded by the new one
    ///
    /// Returns the new memory's ID.
    pub fn correct(
        &mut self,
        old_id: &str,
        new_content: &str,
        importance_override: Option<f64>,
        memory_type_override: Option<MemoryType>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // 1. Fetch old memory
        let old = self.storage.get(old_id)?
            .ok_or_else(|| format!("Memory not found: {}", old_id))?;

        // 2. Determine type and importance
        let memory_type = memory_type_override.unwrap_or(old.memory_type);
        let importance = importance_override.unwrap_or_else(|| old.importance.max(0.5));

        // 3. Get namespace of old memory
        let namespace = self.storage.get_namespace(old_id)?;
        let ns_ref = namespace.as_deref();

        // 4. Store the new memory in the same namespace
        #[allow(deprecated)]
        let new_id = self.add_to_namespace(
            new_content,
            memory_type,
            Some(importance),
            Some("correction"),
            None,
            ns_ref,
        )?;

        // 5. Supersede old → new
        self.storage.supersede(old_id, &new_id)
            .map_err(|e| format!("Supersession failed after storing new memory: {}", e))?;

        log::info!("Corrected memory {} → {} in namespace {:?}", old_id, new_id, ns_ref);
        Ok(new_id)
    }

    /// Correct multiple memories matching a query: find them, create a replacement,
    /// and supersede all matches.
    ///
    /// The confirmation step (showing matches before applying) is handled at the
    /// CLI layer, not here. This method is unconditional.
    ///
    /// Returns a `BulkCorrectionResult` with the new ID and all superseded IDs.
    pub fn correct_bulk(
        &mut self,
        query: &str,
        new_content: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<crate::types::BulkCorrectionResult, Box<dyn std::error::Error>> {
        // 1. Find matching memories via recall
        let matches = self.recall_from_namespace(query, limit, None, None, namespace)?;
        if matches.is_empty() {
            return Err("No matching memories found for correction".into());
        }

        // 2. Determine memory type from the highest-scored match
        let best_match = &matches[0];
        let memory_type = best_match.record.memory_type;

        // 3. Store the new correction memory
        #[allow(deprecated)]
        let new_id = self.add_to_namespace(
            new_content,
            memory_type,
            Some(0.7), // Corrections get moderate-high importance
            Some("bulk_correction"),
            None,
            namespace,
        )?;

        // 4. Supersede all matching memories
        let ids: Vec<String> = matches.iter().map(|r| r.record.id.clone()).collect();
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let count = self.storage.supersede_bulk(&id_refs, &new_id)
            .map_err(|e| format!("Bulk supersession failed: {}", e))?;

        log::info!("Bulk corrected {} memories → {} (query: '{}')", count, new_id, query);

        Ok(crate::types::BulkCorrectionResult {
            new_id,
            superseded_count: count,
            superseded_ids: ids,
        })
    }

    /// List all superseded memories (observability).
    ///
    /// Returns superseded records with their replacement ID and chain head.
    pub fn list_superseded(
        &self,
        namespace: Option<&str>,
    ) -> Result<Vec<crate::types::SupersessionInfo>, Box<dyn std::error::Error>> {
        let pairs = self.storage.list_superseded(namespace)?;
        let mut results = Vec::with_capacity(pairs.len());
        for (record, replacement_id) in pairs {
            let chain_head = self.storage.resolve_chain_head(&replacement_id)?;
            results.push(crate::types::SupersessionInfo {
                superseded: record,
                superseded_by_id: replacement_id,
                chain_head,
            });
        }
        Ok(results)
    }

    /// Undo a supersession, restoring a memory to active recall.
    pub fn unsupersede(&mut self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.unsupersede(id)
            .map_err(|e| format!("Unsupersede failed: {}", e))?;
        log::info!("Restored memory {} to active recall", id);
        Ok(())
    }

    /// Set the agent ID for this memory instance.
    /// 
    /// This is used for ACL checks when storing and recalling memories.
    /// Each agent should identify itself before performing operations.
    pub fn set_agent_id(&mut self, id: &str) {
        self.agent_id = Some(id.to_string());
    }
    
    /// Get the current agent ID.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }
    
    /// Set a memory extractor for LLM-based fact extraction.
    ///
    /// When an extractor is set, `add()` and `add_to_namespace()` will
    /// pass the raw content through the LLM to extract structured facts,
    /// storing each fact as a separate memory. If extraction fails,
    /// the raw content is stored as a fallback.
    ///
    /// # Arguments
    ///
    /// * `extractor` - The extractor implementation (e.g., AnthropicExtractor, OllamaExtractor)
    pub fn set_extractor(&mut self, extractor: Box<dyn MemoryExtractor>) {
        self.extractor = Some(extractor);
    }

    /// Set the synthesis settings. Enables knowledge synthesis during consolidation.
    ///
    /// Synthesis is opt-in: it only runs when settings.enabled is true.
    /// This maintains backward compatibility (GUARD-3).
    pub fn set_synthesis_settings(&mut self, settings: crate::synthesis::types::SynthesisSettings) {
        self.synthesis_settings = Some(settings);
    }

    /// Set the LLM provider for synthesis insight generation.
    ///
    /// Without a provider, synthesis still discovers clusters and runs the gate check,
    /// but skips LLM insight generation (graceful degradation).
    pub fn set_synthesis_llm_provider(&mut self, provider: Box<dyn crate::synthesis::types::SynthesisLlmProvider>) {
        self.synthesis_llm_provider = Some(provider);
    }

    /// Set the LLM triple extractor for knowledge graph enrichment during consolidation.
    pub fn set_triple_extractor(&mut self, extractor: Box<dyn crate::triple_extractor::TripleExtractor>) {
        self.triple_extractor = Some(extractor);
    }
    
    /// Remove the memory extractor (revert to storing raw content).
    pub fn clear_extractor(&mut self) {
        self.extractor = None;
    }
    
    /// Check if an extractor is configured.
    pub fn has_extractor(&self) -> bool {
        self.extractor.is_some()
    }
    
    /// Auto-configure extractor from environment and config file.
    ///
    /// Called during Memory::new() if no extractor is explicitly set.
    /// This provides "just works" behavior for users who set env vars.
    ///
    /// Detection order (high → low priority):
    /// 1. ANTHROPIC_AUTH_TOKEN env var → AnthropicExtractor with OAuth
    /// 2. ANTHROPIC_API_KEY env var → AnthropicExtractor with API key
    /// 3. ~/.config/engram/config.json extractor section
    /// 4. None → no extraction (backward compatible)
    ///
    /// Model can be overridden via ENGRAM_EXTRACTOR_MODEL env var.
    pub fn auto_configure_extractor(&mut self) {
        use crate::extractor::{AnthropicExtractor, AnthropicExtractorConfig};
        
        // Check ANTHROPIC_AUTH_TOKEN first (OAuth mode)
        if let Ok(token) = std::env::var("ANTHROPIC_AUTH_TOKEN") {
            let model = std::env::var("ENGRAM_EXTRACTOR_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
            let config = AnthropicExtractorConfig {
                model,
                ..Default::default()
            };
            self.extractor = Some(Box::new(AnthropicExtractor::with_config(&token, true, config)));
            log::info!("Extractor: Anthropic (OAuth) from ANTHROPIC_AUTH_TOKEN");
            return;
        }
        
        // Check ANTHROPIC_API_KEY (API key mode)
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            let model = std::env::var("ENGRAM_EXTRACTOR_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
            let config = AnthropicExtractorConfig {
                model,
                ..Default::default()
            };
            self.extractor = Some(Box::new(AnthropicExtractor::with_config(&key, false, config)));
            log::info!("Extractor: Anthropic (API key) from ANTHROPIC_API_KEY");
            return;
        }
        
        // Check config file
        if let Some(extractor) = self.load_extractor_from_config() {
            self.extractor = Some(extractor);
            return;
        }
        
        // No extractor configured - that's fine, backward compatible
        log::debug!("No extractor configured, storing raw text");
    }

    /// Auto-configure Haiku intent classifier from environment and config.
    ///
    /// Reads `ANTHROPIC_AUTH_TOKEN` or `ANTHROPIC_API_KEY` from environment
    /// and creates a `HaikuIntentClassifier` if `haiku_l2_enabled` is true
    /// in `config.intent_classification`.
    pub fn auto_configure_intent_classifier(&mut self) {
        let config = &self.config.intent_classification;
        if !config.haiku_l2_enabled {
            return;
        }
        if let Ok(token) = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
        {
            let is_oauth = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
            self.intent_classifier = Some(crate::query_classifier::HaikuIntentClassifier::new(
                Box::new(crate::anthropic_client::StaticToken(token)),
                is_oauth,
                config.model.clone(),
                config.api_url.clone(),
                config.timeout_secs,
            ));
            log::info!("HaikuIntentClassifier: configured (oauth={})", is_oauth);
        }
    }
    
    /// Load extractor configuration from ~/.config/engram/config.json.
    ///
    /// The config file stores non-sensitive settings (provider, model, host).
    /// Auth tokens MUST come from environment variables or code.
    ///
    /// Config format:
    /// ```json
    /// {
    ///   "extractor": {
    ///     "provider": "anthropic",  // or "ollama"
    ///     "model": "claude-haiku-4-5-20251001"
    ///   }
    /// }
    /// ```
    fn load_extractor_from_config(&self) -> Option<Box<dyn MemoryExtractor>> {
        use crate::extractor::{AnthropicExtractor, AnthropicExtractorConfig, OllamaExtractor};
        
        // Get config directory
        let config_path = dirs::config_dir()?.join("engram/config.json");
        if !config_path.exists() {
            return None;
        }
        
        // Read and parse config file
        let content = std::fs::read_to_string(&config_path).ok()?;
        let config: serde_json::Value = serde_json::from_str(&content).ok()?;
        
        // Get extractor section
        let extractor_config = config.get("extractor")?;
        let provider = extractor_config.get("provider")?.as_str()?;
        
        match provider {
            "anthropic" => {
                // Still need env var for auth - config file NEVER stores tokens
                let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .ok()?;
                let is_oauth = std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok();
                
                let model = extractor_config
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("claude-haiku-4-5-20251001")
                    .to_string();
                
                let api_config = AnthropicExtractorConfig {
                    model: model.clone(),
                    ..Default::default()
                };
                
                log::info!("Extractor: Anthropic ({}) from config file", model);
                Some(Box::new(AnthropicExtractor::with_config(&token, is_oauth, api_config)))
            }
            "ollama" => {
                let model = extractor_config
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("llama3.2:3b")
                    .to_string();
                let host = extractor_config
                    .get("host")
                    .and_then(|v| v.as_str())
                    .unwrap_or("http://localhost:11434")
                    .to_string();
                
                log::info!("Extractor: Ollama ({}) from config file", model);
                Some(Box::new(OllamaExtractor::with_host(&model, &host)))
            }
            _ => {
                log::warn!("Unknown extractor provider in config: {}", provider);
                None
            }
        }
    }

    /// Store a new memory. Returns memory ID.
    ///
    /// The memory is encoded with initial working_strength=1.0 (strong
    /// hippocampal trace) and core_strength=0.0 (no neocortical trace yet).
    /// Consolidation cycles will gradually transfer it to core.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content (natural language)
    /// * `memory_type` - Memory type classification
    /// * `importance` - 0-1 importance score (None = auto from type)
    /// * `source` - Source identifier (e.g., filename, conversation ID)
    /// * `metadata` - Optional structured metadata (e.g., for causal memories)
    #[deprecated(
        since = "0.2.3",
        note = "Use `store_raw` (for text + extractor dispatch) or `store_enriched` (for pre-validated EnrichedMemory). This shim will be removed in v0.4."
    )]
    pub fn add(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        #[allow(deprecated)]
        self.add_to_namespace(content, memory_type, importance, source, metadata, None)
    }
    
    /// Store a new memory in a specific namespace. Returns memory ID.
    ///
    /// **Deprecated (ISS-019 Step 4.5):** use [`Memory::store_raw`] for
    /// the new typed write path. This method is now a thin shim that
    /// forwards to `store_raw`, preserving the return-type contract
    /// (a single `String` memory id) and the explicit-`memory_type`
    /// hint semantics through `StorageMeta::memory_type_hint`.
    ///
    /// Semantics preserved:
    /// - Extractor path: first fact's id is returned (warn log if N > 1).
    /// - Empty-facts path: returns a sentinel id `"skipped:<hash>"` and
    ///   a structured warn log, matching the previous empty-string
    ///   return (relaxed to a traceable sentinel).
    /// - Extractor failure: surfaces as `Err(StoreError::Quarantined)`.
    ///
    /// If an extractor is configured, the content is first passed through
    /// the LLM to extract structured facts. Each extracted fact is stored
    /// as a separate memory. If extraction fails or returns nothing,
    /// the raw content is stored as a fallback.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content (natural language)
    /// * `memory_type` - Memory type classification (used as fallback if extraction fails)
    /// * `importance` - 0-1 importance score (None = auto from type, or from extraction)
    /// * `source` - Source identifier (e.g., filename, conversation ID)
    /// * `metadata` - Optional structured metadata (e.g., for causal memories)
    /// * `namespace` - Namespace to store in (None = "default")
    #[deprecated(
        since = "0.2.3",
        note = "Use `store_raw` (for text + extractor dispatch) or `store_enriched` (for pre-validated EnrichedMemory). This shim will be removed in v0.4."
    )]
    pub fn add_to_namespace(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        use crate::store_api::{RawStoreOutcome, StorageMeta, StoreError, StoreOutcome};

        let meta = StorageMeta {
            importance_hint: importance,
            source: source.map(str::to_string),
            namespace: namespace.map(str::to_string),
            user_metadata: metadata.unwrap_or(serde_json::Value::Null),
            memory_type_hint: Some(memory_type),
        };

        let outcome = self.store_raw(content, meta).map_err(|e| match e {
            StoreError::Quarantined { id, reason } => {
                // Legacy callers had no quarantine concept — surface as Err
                // so silent success does not hide the new behavior.
                Box::<dyn std::error::Error>::from(format!(
                    "content quarantined: {reason:?} (id={})",
                    id.as_str()
                ))
            }
            other => Box::<dyn std::error::Error>::from(other.to_string()),
        })?;

        match outcome {
            RawStoreOutcome::Stored(outcomes) => {
                if outcomes.len() > 1 {
                    log::warn!(
                        "add_to_namespace shim: extractor produced {} facts; returning first id (legacy signature carries a single id)",
                        outcomes.len()
                    );
                }
                let id = outcomes
                    .first()
                    .map(|o| match o {
                        StoreOutcome::Inserted { id } => id.clone(),
                        StoreOutcome::Merged { id, .. } => id.clone(),
                    })
                    .unwrap_or_default();
                Ok(id)
            }
            RawStoreOutcome::Skipped { content_hash, reason } => {
                // ISS-019 Step 8: demoted to debug!. The shim's
                // caller sees "skipped:<hash>" in the return value,
                // and `Memory::write_stats()` already captures the
                // skip reason structurally.
                log::debug!(
                    "add_to_namespace shim: content skipped ({:?}) — returning sentinel id",
                    reason
                );
                Ok(format!("skipped:{}", content_hash.as_str()))
            }
            RawStoreOutcome::Quarantined { id, reason } => {
                // Shouldn't reach this branch because the map_err above
                // converts Quarantined StoreError into an Err, and the
                // Quarantined outcome variant is only produced via the
                // Ok path. Keep a safety net.
                Err(format!(
                    "content quarantined: {reason:?} (id={})",
                    id.as_str()
                )
                .into())
            }
        }
    }
    
    /// Parse a memory type string into MemoryType enum.
    #[allow(dead_code)] // Kept for potential future use (e.g., legacy migration)
    fn parse_memory_type(s: &str) -> Option<MemoryType> {
        match s.to_lowercase().as_str() {
            "factual" => Some(MemoryType::Factual),
            "episodic" => Some(MemoryType::Episodic),
            "relational" => Some(MemoryType::Relational),
            "emotional" => Some(MemoryType::Emotional),
            "procedural" => Some(MemoryType::Procedural),
            "opinion" => Some(MemoryType::Opinion),
            "causal" => Some(MemoryType::Causal),
            _ => None,
        }
    }
    
    /// Store a memory directly without extraction (internal method).
    ///
    /// This is the raw storage path used when no extractor is configured
    /// or when extractor fails.
    ///
    /// Flow (with dedup):
    /// 1. Generate embedding (if provider available)
    /// 2. If dedup enabled + have embedding → check for duplicates
    /// 3. If duplicate found → merge_memory_into + return existing_id
    /// 4. If no duplicate → storage.add() the new record
    /// 5. Store the pre-computed embedding (don't re-embed)
    /// 6. Extract entities
    fn add_raw(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let ns = namespace.unwrap_or("default");
        let id = format!("{}", Uuid::new_v4())[..8].to_string();
        let base_importance = importance.unwrap_or_else(|| memory_type.default_importance());
        
        // Apply drive alignment boost if Empathy Bus is attached
        let importance = if let Some(ref bus) = self.empathy_bus {
            let boost = bus.align_importance(content);
            (base_importance * boost).min(1.0) // Cap at 1.0
        } else {
            base_importance
        };

        // Step 1: Generate embedding up front (if provider available)
        // We need it before storage for dedup check, and reuse it after storage.
        let pre_embedding = if let Some(ref embedding_provider) = self.embedding {
            match embedding_provider.embed(content) {
                Ok(emb) => Some(emb),
                Err(e) => {
                    log::warn!("Failed to generate embedding for dedup/storage: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Step 2: Dedup check — entity Jaccard + embedding similarity
        if self.config.dedup_enabled {
            // Phase A: entity Jaccard pre-check (if entities are available)
            if self.config.entity_config.enabled {
                let entities = self.entity_extractor.extract(content);
                let entity_names: Vec<String> = entities.iter()
                    .map(|e| e.normalized.clone())
                    .collect();
                
                if !entity_names.is_empty() {
                    if let Ok(Some((candidate_id, _jaccard))) = 
                        self.storage.find_entity_overlap(&entity_names, ns, 0.5) 
                    {
                        // Entity overlap found — confirm with embedding similarity
                        if let Some(ref embedding) = pre_embedding {
                            // Check similarity specifically against the candidate
                            if let Ok(Some(candidate_emb)) = self.storage.get_embedding_for_memory(&candidate_id) {
                                let sim = crate::embeddings::EmbeddingProvider::cosine_similarity(embedding, &candidate_emb);
                                if sim as f64 >= self.config.dedup_threshold {
                                    log::info!(
                                        "Dedup (entity+embedding): merging into {} (jaccard={:.3}, sim={:.4})",
                                        candidate_id, _jaccard, sim
                                    );
                                    // ISS-019 Step 5.5: minimal EnrichedMemory
                                    // — extractor-less add_raw path has no
                                    // typed dimensions to merge. The keep
                                    // side's dimensions are preserved by
                                    // Dimensions::union (monotone).
                                    let incoming_em = crate::enriched::EnrichedMemory::minimal(
                                        content,
                                        crate::dimensions::Importance::new(importance),
                                        source.map(str::to_string),
                                        Some(ns.to_string()),
                                    )
                                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("EnrichedMemory::minimal failed in dedup entity path: {}", e))) })?;
                                    let outcome = self.storage.merge_enriched_into(
                                        &candidate_id, &incoming_em, sim,
                                    )?;
                                    if outcome.content_updated {
                                        log::info!(
                                            "Dedup: content updated for {} (merge_count={})",
                                            candidate_id, outcome.merge_count,
                                        );
                                    }
                                    // Update entity links for the merged memory
                                    for entity in &entities {
                                        if let Ok(eid) = self.storage.upsert_entity(
                                            &entity.normalized, entity.entity_type.as_str(), ns, None,
                                        ) {
                                            let _ = self.storage.link_memory_entity(&candidate_id, &eid, "mention");
                                        }
                                    }
                                    self.last_add_result = Some(crate::lifecycle::AddResult::Merged { 
                                        into: candidate_id.clone(), similarity: sim 
                                    });
                                    self.dedup_merge_count += 1;
                                    return Ok(candidate_id);
                                }
                            }
                        }
                    }
                }
            }
            
            // Phase B: embedding-only dedup (existing logic, catches cases without entities)
            if let Some(ref embedding) = pre_embedding {
                let model_id = self.config.embedding.model_id();
                if let Ok(Some((existing_id, similarity))) = self.storage.find_nearest_embedding(
                    embedding, &model_id, Some(ns), self.config.dedup_threshold,
                ) {
                    log::info!(
                        "Dedup: merging into existing memory {} (similarity: {:.4})",
                        existing_id, similarity
                    );
                    // ISS-019 Step 5.5: same minimal-EnrichedMemory
                    // construction as the entity-phase merge above.
                    let incoming_em = crate::enriched::EnrichedMemory::minimal(
                        content,
                        crate::dimensions::Importance::new(importance),
                        source.map(str::to_string),
                        Some(ns.to_string()),
                    )
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("EnrichedMemory::minimal failed in dedup embedding path: {}", e))) })?;
                    let outcome = self.storage.merge_enriched_into(
                        &existing_id, &incoming_em, similarity,
                    )?;
                    if outcome.content_updated {
                        log::info!(
                            "Dedup: content updated for {} (merge_count={})",
                            existing_id, outcome.merge_count,
                        );
                    }
                    if self.config.entity_config.enabled {
                        let entities = self.entity_extractor.extract(content);
                        for entity in &entities {
                            if let Ok(eid) = self.storage.upsert_entity(
                                &entity.normalized, entity.entity_type.as_str(), ns, None,
                            ) {
                                let _ = self.storage.link_memory_entity(&existing_id, &eid, "mention");
                            }
                        }
                    }
                    self.last_add_result = Some(crate::lifecycle::AddResult::Merged { 
                        into: existing_id.clone(), similarity 
                    });
                    self.dedup_merge_count += 1;
                    return Ok(existing_id);
                }
            }
        }

        // Step 3: No duplicate found — create new memory record
        let record = MemoryRecord {
            id: id.clone(),
            content: content.to_string(),
            memory_type,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.0,
            importance,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: source.unwrap_or("").to_string(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata,
        };

        self.storage.add(&record, ns)?;
        self.last_add_result = Some(crate::lifecycle::AddResult::Created { id: id.clone() });
        self.dedup_write_count += 1;
        
        // Step 4: Store pre-computed embedding (avoid double-embed)
        // Keep a reference for Step 6 (association discovery) before consuming
        let embedding_for_assoc: Option<Vec<f32>> = if self.config.association.enabled {
            pre_embedding.clone()
        } else {
            None
        };
        if let Some(embedding) = pre_embedding {
            self.storage.store_embedding(
                &id,
                &embedding,
                &self.config.embedding.model_id(),
                self.config.embedding.dimensions,
            )?;
        }
        
        // Step 5: Entity extraction
        if self.config.entity_config.enabled {
            let entities = self.entity_extractor.extract(content);
            let mut entity_ids = Vec::new();
            
            for entity in &entities {
                match self.storage.upsert_entity(
                    &entity.normalized,
                    entity.entity_type.as_str(),
                    ns,
                    None,
                ) {
                    Ok(eid) => {
                        if let Err(e) = self.storage.link_memory_entity(&id, &eid, "mention") {
                            log::warn!("Failed to link memory {} to entity {}: {}", id, eid, e);
                        }
                        entity_ids.push(eid);
                    }
                    Err(e) => {
                        log::warn!("Failed to upsert entity '{}': {}", entity.normalized, e);
                    }
                }
            }
            
            // Co-occurrence relations (capped at 10 to avoid O(n²))
            let cap = entity_ids.len().min(10);
            for i in 0..cap {
                for j in (i + 1)..cap {
                    if let Err(e) = self.storage.upsert_entity_relation(
                        &entity_ids[i],
                        &entity_ids[j],
                        "co_occurs",
                        ns,
                    ) {
                        log::warn!("Failed to upsert entity relation: {}", e);
                    }
                }
            }
        }

        // Step 6: Association discovery (multi-signal Hebbian)
        if self.config.association.enabled {
            let start = std::time::Instant::now();

            // Collect entity names for signal computation
            let entity_names: Vec<String> = if self.config.entity_config.enabled {
                self.entity_extractor.extract(content)
                    .into_iter()
                    .map(|e| e.normalized)
                    .collect()
            } else {
                Vec::new()
            };

            // created_at as f64 timestamp (same format as storage)
            let created_at_f64 = record.created_at.timestamp() as f64
                + record.created_at.timestamp_subsec_nanos() as f64 / 1_000_000_000.0;

            let selector = crate::association::CandidateSelector::new(&self.storage);
            match selector.select_candidates(
                &id,
                created_at_f64,
                &entity_names,
                embedding_for_assoc.as_deref(),
                &self.config.association,
            ) {
                Ok(candidates) if !candidates.is_empty() => {
                    let former = crate::association::LinkFormer::new(&self.storage);
                    match former.discover_associations(
                        &id,
                        candidates,
                        &entity_names,
                        embedding_for_assoc.as_deref(),
                        created_at_f64,
                        &self.config.association,
                        ns,
                    ) {
                        Ok(n) => {
                            if n > 0 {
                                log::debug!(
                                    "Association discovery: created {} links for memory {}",
                                    n, &id
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!("Association discovery failed: {}", e);
                        }
                    }
                }
                Ok(_) => {} // No candidates found
                Err(e) => {
                    log::warn!("Association candidate selection failed: {}", e);
                }
            }

            let elapsed = start.elapsed();
            if elapsed > std::time::Duration::from_millis(100) {
                log::warn!(
                    "Association discovery took {:?} — consider tuning candidate_limit",
                    elapsed
                );
            }
        }
        
        Ok(id)
    }
    
    /// Store a new memory with emotional tracking.
    ///
    /// This method both stores the memory and records the empathy valence
    /// in the Empathy Bus for trend tracking. Requires an Empathy Bus
    /// to be attached.
    ///
    /// # Arguments
    ///
    /// * `content` - The memory content
    /// * `memory_type` - Memory type classification
    /// * `importance` - 0-1 importance score (None = auto)
    /// * `source` - Source identifier
    /// * `metadata` - Optional metadata
    /// * `namespace` - Namespace to store in
    /// * `emotion` - Emotional valence (-1.0 to 1.0)
    /// * `domain` - Domain for emotional tracking
    #[allow(clippy::too_many_arguments)]
    #[deprecated(
        since = "0.2.3",
        note = "Use `store_raw` (or `store_enriched`) + `record_emotion` (or bus.process_interaction). This shim will be removed in v0.4."
    )]
    pub fn add_with_emotion(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        importance: Option<f64>,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        namespace: Option<&str>,
        emotion: f64,
        domain: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Store via the typed write path (the shim forwards to store_raw,
        // which gives us dedup + entity + association discovery identical
        // to the pre-ISS-019 behavior).
        #[allow(deprecated)]
        let id = self.add_to_namespace(content, memory_type, importance, source, metadata, namespace)?;

        // Record emotion if bus is attached — unchanged.
        if let Some(ref bus) = self.empathy_bus {
            bus.process_interaction(self.storage.connection(), content, emotion, domain)?;
        }

        Ok(id)
    }

    // =========================================================================
    // ISS-019 Step 4+: typed write-path API
    //
    // `store_enriched` — caller already has a validated `EnrichedMemory`.
    // `store_raw`      — caller has text only; engram runs the extractor
    //                    (or falls back to `Dimensions::minimal`) and
    //                    dispatches per-fact through `store_enriched`.
    //
    // These entry points are the canonical write surface going forward.
    // The legacy `add` / `add_to_namespace` / `add_with_emotion` stay as
    // `#[deprecated]` shims (Step 4.5) that forward to `store_raw` so
    // downstream callers migrate at their own pace.
    // =========================================================================

    /// Store a memory whose `Dimensions` are already validated.
    ///
    /// This is the primary write path. The returned `StoreOutcome` tells
    /// the caller whether a fresh row was inserted or the content was
    /// dedup-merged into an existing row.
    ///
    /// Metadata layout on disk today: we write the same legacy JSON shape
    /// the existing `add_raw` path produces, so dedup + merge history
    /// code keeps working untouched. Step 7 of the ISS-019 plan is where
    /// we namespace the blob under `engram.*` / `user.*`.
    pub fn store_enriched(
        &mut self,
        mem: crate::enriched::EnrichedMemory,
    ) -> Result<crate::store_api::StoreOutcome, crate::store_api::StoreError> {
        // Debug-time sanity: constructor guarantees this, but a caller
        // that built the struct literally could violate it.
        debug_assert!(mem.invariants_hold(), "EnrichedMemory::content must equal dimensions.core_fact");

        let memory_type = mem.dimensions.type_weights.primary_type();
        let importance_hint = Some(mem.importance.get());
        let source_opt = mem.source.clone();
        let namespace_opt = mem.namespace.clone();

        // Build the legacy metadata blob from the typed Dimensions.
        let metadata_json = build_legacy_metadata(&mem);

        let id = self
            .add_raw(
                &mem.content,
                memory_type,
                importance_hint,
                source_opt.as_deref(),
                Some(metadata_json),
                namespace_opt.as_deref(),
            )
            .map_err(boxed_err_to_store_error)?;

        // Translate `last_add_result` into the typed outcome.
        match self.last_add_result.clone() {
            Some(crate::lifecycle::AddResult::Merged { into, similarity }) => {
                Ok(crate::store_api::StoreOutcome::Merged { id: into, similarity })
            }
            Some(crate::lifecycle::AddResult::Created { id: created_id }) => {
                Ok(crate::store_api::StoreOutcome::Inserted { id: created_id })
            }
            None => {
                // add_raw always sets last_add_result; treat missing as Inserted
                // using the id we got back, keeping the contract intact.
                Ok(crate::store_api::StoreOutcome::Inserted { id })
            }
        }
    }

    /// Store raw text, running the configured extractor if present.
    ///
    /// Dispatch (see ISS-019 design §3.2):
    ///
    /// - no extractor configured      → `Dimensions::minimal` → `store_enriched`
    /// - extractor returns facts      → each fact → `EnrichedMemory::from_extracted` → `store_enriched`
    /// - extractor returns empty      → `Skipped { NoFactsExtracted }`
    /// - extractor runtime failure    → `Quarantined { ExtractorError }`
    ///
    /// Note: Step 4 ships with the quarantine path as an **in-memory**
    /// outcome (no dedicated SQLite table yet). The id returned in
    /// `Quarantined { id }` is a synthetic `q-*` hash of the content
    /// so callers can correlate retries. The persistent quarantine
    /// table lands in Step 6.
    pub fn store_raw(
        &mut self,
        content: &str,
        meta: crate::store_api::StorageMeta,
    ) -> Result<crate::store_api::RawStoreOutcome, crate::store_api::StoreError> {
        use crate::store_api::{ContentHash, QuarantineId, QuarantineReason, RawStoreOutcome, SkipReason};
        use crate::write_stats::{duration_to_ms, StoreEvent};

        // ISS-019 Step 8: wall-clock timer for the whole call. Every
        // return point emits exactly one `StoreEvent` carrying this
        // elapsed time. The timer lives in the call frame — no
        // allocation, Instant is a plain u64 under the hood.
        let t0 = std::time::Instant::now();

        let trimmed = content.trim();
        if trimmed.is_empty() {
            let content_hash = ContentHash::new(short_hash(content));
            self.emit_store_event(StoreEvent::Skipped {
                content_hash: content_hash.clone(),
                reason: SkipReason::TooShort,
                ms_elapsed: duration_to_ms(t0.elapsed()),
            });
            return Ok(RawStoreOutcome::Skipped {
                reason: SkipReason::TooShort,
                content_hash,
            });
        }

        // Path A: extractor present.
        if let Some(ref extractor) = self.extractor {
            match extractor.extract(content) {
                Ok(facts) if facts.is_empty() => {
                    // ISS-019 Step 8: demoted from info! — the
                    // structured `Skipped{NoFactsExtracted}` event is
                    // now the primary signal; the text log is only
                    // useful when tailing the process.
                    log::debug!(
                        "store_raw: extractor returned nothing for content ({}...)",
                        content.chars().take(50).collect::<String>()
                    );
                    let content_hash = ContentHash::new(short_hash(content));
                    self.emit_store_event(StoreEvent::Skipped {
                        content_hash: content_hash.clone(),
                        reason: SkipReason::NoFactsExtracted,
                        ms_elapsed: duration_to_ms(t0.elapsed()),
                    });
                    return Ok(RawStoreOutcome::Skipped {
                        reason: SkipReason::NoFactsExtracted,
                        content_hash,
                    });
                }
                Ok(facts) => {
                    // Cache emotion data for downstream consumers (parity
                    // with add_to_namespace).
                    let emotions: Vec<(f64, String)> = facts
                        .iter()
                        .map(|f| (f.valence, f.domain.clone()))
                        .collect();
                    *self.last_extraction_emotions.lock().unwrap() = Some(emotions);

                    let mut outcomes = Vec::with_capacity(facts.len());
                    let mut any_valid = false;
                    let mut first_err: Option<String> = None;

                    for fact in facts {
                        // Cap auto-extracted importance to prevent noise from
                        // dominating recall — same rule as the legacy path.
                        let capped = fact
                            .importance
                            .min(self.config.auto_extract_importance_cap);

                        let mut fact_adj = fact;
                        fact_adj.importance = capped;

                        match crate::enriched::EnrichedMemory::from_extracted(
                            fact_adj,
                            meta.source.clone(),
                            meta.namespace.clone(),
                            meta.user_metadata.clone(),
                        ) {
                            Ok(em) => {
                                any_valid = true;
                                let outcome = self.store_enriched(em)?;
                                outcomes.push(outcome);
                            }
                            Err(e) => {
                                // Don't abort the whole batch for a single
                                // empty-core_fact; skip that one fact and
                                // record the first error for the
                                // `AllFactsInvalid` branch below.
                                log::warn!(
                                    "store_raw: skipping invalid fact ({}); continuing batch",
                                    e
                                );
                                if first_err.is_none() {
                                    first_err = Some(e.to_string());
                                }
                            }
                        }
                    }

                    if any_valid {
                        // ISS-019 Step 8: Stored event carries the
                        // batch size (fact_count) and merge count so
                        // `WriteStats.merged_count` tracks dedup rate
                        // without parsing outcome vectors.
                        let first_id = outcomes
                            .first()
                            .map(|o| o.id().clone())
                            .unwrap_or_default();
                        let merged_count = outcomes
                            .iter()
                            .filter(|o| matches!(
                                o,
                                crate::store_api::StoreOutcome::Merged { .. }
                            ))
                            .count();
                        self.emit_store_event(StoreEvent::Stored {
                            id: first_id,
                            fact_count: outcomes.len(),
                            merged_count,
                            ms_elapsed: duration_to_ms(t0.elapsed()),
                        });

                        // v0.3 §3.1 Step C: enqueue resolution job for
                        // each newly-inserted memory. Merged outcomes
                        // skip — the underlying memory was already
                        // extracted on its first ingest; re-extraction
                        // is operator-driven via `reextract` (§6.2).
                        // GUARD-1: enqueue failures NEVER abort
                        // admission; `enqueue_pipeline_job` swallows
                        // and logs.
                        for outcome in &outcomes {
                            if let crate::store_api::StoreOutcome::Inserted { id } = outcome {
                                let _ = self.enqueue_pipeline_job(id);
                            }
                        }

                        return Ok(RawStoreOutcome::Stored(outcomes));
                    }

                    // Extractor produced facts but every one failed validation.
                    let qid_str = format!("q-{}", short_hash(content));
                    let reason = QuarantineReason::AllFactsInvalid(
                        first_err.unwrap_or_else(|| "no valid facts".to_string()),
                    );
                    log::warn!(
                        "store_raw: quarantining content ({}...): {:?}",
                        content.chars().take(50).collect::<String>(),
                        reason
                    );
                    // ISS-019 Step 6: persist quarantine row.
                    let persisted_id = self.persist_quarantine_row(
                        &qid_str, content, &reason, &meta,
                    ).map_err(crate::store_api::StoreError::DbError)?;
                    let qid = QuarantineId::new(persisted_id);
                    self.emit_store_event(StoreEvent::Quarantined {
                        id: qid.clone(),
                        reason: reason.clone(),
                        ms_elapsed: duration_to_ms(t0.elapsed()),
                    });
                    return Ok(RawStoreOutcome::Quarantined {
                        id: qid,
                        reason,
                    });
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    log::warn!("store_raw: extractor error: {}", err_msg);
                    let qid_str = format!("q-{}", short_hash(content));
                    let reason = QuarantineReason::ExtractorError(err_msg);
                    // ISS-019 Step 6: persist quarantine row.
                    let persisted_id = self.persist_quarantine_row(
                        &qid_str, content, &reason, &meta,
                    ).map_err(crate::store_api::StoreError::DbError)?;
                    let qid = QuarantineId::new(persisted_id);
                    self.emit_store_event(StoreEvent::Quarantined {
                        id: qid.clone(),
                        reason: reason.clone(),
                        ms_elapsed: duration_to_ms(t0.elapsed()),
                    });
                    return Ok(RawStoreOutcome::Quarantined {
                        id: qid,
                        reason,
                    });
                }
            }
        }

        // Path B: no extractor — fall back to minimal dimensions.
        let importance_val = meta
            .importance_hint
            .map(crate::dimensions::Importance::new)
            .unwrap_or_else(|| {
                // Use the legacy memory_type default if the caller
                // threaded one through.
                let base = meta
                    .memory_type_hint
                    .map(|mt| mt.default_importance())
                    .unwrap_or(0.5);
                crate::dimensions::Importance::new(base)
            });

        let mut em = crate::enriched::EnrichedMemory::minimal(
            content,
            importance_val,
            meta.source.clone(),
            meta.namespace.clone(),
        )
        .map_err(|e| crate::store_api::StoreError::InvalidInput(e.to_string()))?;

        // If the caller supplied an explicit MemoryType hint (legacy
        // `add()` contract), bias type_weights so `primary_type()`
        // returns the hinted variant. Without this the minimal path
        // would always degrade to `MemoryType::Factual` (default of a
        // tie-break), regressing behavior tests rely on.
        if let Some(mt) = meta.memory_type_hint {
            em.dimensions.type_weights = type_weights_favoring(mt);
        }

        em.user_metadata = meta.user_metadata.clone();

        let outcome = self.store_enriched(em)?;
        // ISS-019 Step 8: minimal path always produces exactly one
        // outcome (see `Ok(RawStoreOutcome::Stored(vec![outcome]))`
        // below) so fact_count = 1.
        let merged_count = match &outcome {
            crate::store_api::StoreOutcome::Merged { .. } => 1,
            crate::store_api::StoreOutcome::Inserted { .. } => 0,
        };
        self.emit_store_event(StoreEvent::Stored {
            id: outcome.id().clone(),
            fact_count: 1,
            merged_count,
            ms_elapsed: duration_to_ms(t0.elapsed()),
        });

        // v0.3 §3.1 Step C: enqueue resolution job on Inserted
        // outcomes only — Merged means the underlying memory was
        // extracted on its first ingest. Same GUARD-1 contract as
        // Path A: enqueue failures don't abort admission.
        if let crate::store_api::StoreOutcome::Inserted { id } = &outcome {
            let _ = self.enqueue_pipeline_job(id);
        }

        Ok(RawStoreOutcome::Stored(vec![outcome]))
    }

    // ---- ISS-019 Step 6: quarantine persistence + retry ------------------

    /// Max retry attempts before a quarantine row is marked
    /// `permanently_rejected`. Design §4: "After max_attempts = 5
    /// (configurable), the record is marked permanently_rejected".
    pub const QUARANTINE_MAX_ATTEMPTS: u32 = 5;

    /// Default TTL for purging permanently-rejected quarantine rows (30 days).
    /// Design §10 R2. Purge is explicit — never automatic.
    pub const QUARANTINE_PURGE_TTL_SECS: i64 = 30 * 24 * 3600;

    /// Persist a quarantine row via the storage layer.
    ///
    /// Extracts the reason tag/detail pair from `QuarantineReason`'s
    /// serde representation (kept in sync with `store_api.rs`) and
    /// threads caller-supplied `StorageMeta` fields through so a
    /// later retry can reconstruct the full write context.
    fn persist_quarantine_row(
        &self,
        id: &str,
        content: &str,
        reason: &crate::store_api::QuarantineReason,
        meta: &crate::store_api::StorageMeta,
    ) -> Result<String, rusqlite::Error> {
        use crate::store_api::QuarantineReason as QR;
        let (kind, detail): (&str, Option<String>) = match reason {
            QR::ExtractorTimeout          => ("extractor_timeout", None),
            QR::ExtractorError(s)         => ("extractor_error", Some(s.clone())),
            QR::ExtractorPanic            => ("extractor_panic", None),
            QR::AllFactsInvalid(s)        => ("all_facts_invalid", Some(s.clone())),
            QR::PipelineError(s)          => ("pipeline_error", Some(s.clone())),
        };
        let content_hash = short_hash(content);
        let memory_type_hint = meta
            .memory_type_hint
            .map(|mt| mt.to_string());
        let user_meta_json = if meta.user_metadata.is_null() {
            None
        } else {
            Some(meta.user_metadata.to_string())
        };

        self.storage.insert_quarantine_row(
            id,
            content,
            &content_hash,
            kind,
            detail.as_deref(),
            meta.source.as_deref(),
            meta.namespace.as_deref(),
            meta.importance_hint,
            memory_type_hint.as_deref(),
            user_meta_json.as_deref(),
        )
    }

    /// Retry quarantined rows.
    ///
    /// Fetches up to `max_items` live (non-rejected) quarantine rows
    /// ordered by `received_at ASC` (oldest first — fairness) and
    /// re-runs `store_raw` on each one using the preserved
    /// `StorageMeta`. Three outcomes per row:
    ///
    /// - `Stored`: extractor succeeded this time. The quarantine row
    ///   is deleted; the new memory id(s) are collected in
    ///   `RetryReport::recovered`. Multi-fact outcomes use the first
    ///   outcome's id (same convention as the legacy shim).
    /// - `Quarantined`: extractor failed again. Attempts counter is
    ///   bumped; if it crosses `QUARANTINE_MAX_ATTEMPTS` the row is
    ///   flipped to `permanently_rejected = 1` and appears in
    ///   `RetryReport::permanently_rejected`. Otherwise it appears
    ///   in `RetryReport::still_failing` and is eligible for the
    ///   next retry pass.
    /// - `Skipped`: extractor returned zero facts. The content is
    ///   not memory-worthy, so we treat it as a resolution — delete
    ///   the quarantine row. (Rationale: if a previous failure was
    ///   a transient outage and the extractor now cleanly decides
    ///   "nothing to extract here", the row is no longer useful.)
    ///   Counted as `recovered` with an empty id list is not
    ///   meaningful — we count it under `recovered` with a synthetic
    ///   "skipped:<hash>" marker so the caller can distinguish from
    ///   memory-promotion. TODO: a dedicated `Resolved` bucket would
    ///   be cleaner; deferred to Step 8 alongside WriteStats.
    ///
    /// Preserves the "store_raw latency-bounded" property: retry is
    /// a separate operation, owned by the caller. Callers who want
    /// background retries run this on a schedule.
    pub fn retry_quarantined(
        &mut self,
        max_items: usize,
    ) -> Result<crate::store_api::RetryReport, crate::store_api::StoreError> {
        use crate::store_api::{RawStoreOutcome, RetryReport, StorageMeta, StoreError, QuarantineId};

        let rows = self.storage
            .list_quarantine_for_retry_batch(max_items)
            .map_err(StoreError::DbError)?;

        let mut report = RetryReport {
            attempted: rows.len(),
            ..RetryReport::default()
        };

        for row in rows {
            // Reconstruct StorageMeta from the preserved hints.
            let memory_type_hint = row.memory_type_hint
                .as_deref()
                .and_then(|s| serde_json::from_str::<crate::types::MemoryType>(
                    &format!("\"{}\"", s)
                ).ok());
            let user_metadata = match row.user_metadata.as_deref() {
                Some(s) => serde_json::from_str::<serde_json::Value>(s)
                    .unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::Null,
            };
            let meta = StorageMeta {
                importance_hint: row.importance_hint,
                source: row.source.clone(),
                namespace: row.namespace.clone(),
                user_metadata,
                memory_type_hint,
            };

            // Re-run store_raw on the preserved content.
            let retry_outcome = self.store_raw(&row.content, meta);

            match retry_outcome {
                Ok(RawStoreOutcome::Stored(outcomes)) => {
                    // Promoted — delete quarantine row.
                    let _ = self.storage.delete_quarantine_row(&row.id);
                    if let Some(first) = outcomes.first() {
                        report.recovered.push(first.id().clone());
                    }
                }
                Ok(RawStoreOutcome::Skipped { .. }) => {
                    // Extractor now cleanly decided "not memory-worthy".
                    // Remove from quarantine — it's resolved.
                    let _ = self.storage.delete_quarantine_row(&row.id);
                    // Record as recovered with a synthetic marker so
                    // callers can distinguish in logs. No main-table
                    // id exists, so we use a `skipped:<hash>` form.
                    report.recovered.push(format!("skipped:{}", row.content_hash));
                }
                Ok(RawStoreOutcome::Quarantined { reason, .. }) => {
                    // Another failure — increment attempts and, if
                    // we've crossed the threshold, flip to permanently
                    // rejected. Note: store_raw INSERTed a new row
                    // via its own path (dedup matched on content_hash
                    // so the row we're looking at has already been
                    // updated or deduplicated — either way, we update
                    // our current row's attempts for bookkeeping).
                    let err_msg = format!("{:?}", reason);
                    let _ = self.storage.record_quarantine_attempt(
                        &row.id, Some(&err_msg),
                    );
                    let new_attempts = row.attempts + 1;
                    if new_attempts >= Self::QUARANTINE_MAX_ATTEMPTS {
                        let _ = self.storage
                            .mark_quarantine_permanently_rejected(&row.id);
                        report.permanently_rejected
                            .push(QuarantineId::new(row.id.clone()));
                    } else {
                        report.still_failing
                            .push((QuarantineId::new(row.id.clone()), err_msg));
                    }
                }
                Err(e) => {
                    // Programmer-level error during retry. Bump
                    // attempts with the error message; do not mark
                    // rejected (the error is not about the content).
                    let err_msg = e.to_string();
                    let _ = self.storage.record_quarantine_attempt(
                        &row.id, Some(&err_msg),
                    );
                    report.still_failing
                        .push((QuarantineId::new(row.id.clone()), err_msg));
                }
            }
        }

        Ok(report)
    }

    /// Purge permanently-rejected quarantine rows older than
    /// `ttl_seconds`. Never deletes live rows. Returns count of
    /// rows removed. Pass `None` to use the default 30-day TTL.
    pub fn purge_rejected_quarantine(
        &self,
        ttl_seconds: Option<i64>,
    ) -> Result<usize, crate::store_api::StoreError> {
        let ttl = ttl_seconds.unwrap_or(Self::QUARANTINE_PURGE_TTL_SECS);
        self.storage
            .purge_rejected_quarantine(ttl)
            .map_err(crate::store_api::StoreError::DbError)
    }

    /// Count live (non-rejected) quarantine rows. For stats / tests.
    pub fn count_quarantine(&self) -> Result<usize, crate::store_api::StoreError> {
        self.storage
            .count_quarantine_live()
            .map_err(crate::store_api::StoreError::DbError)
    }

    // ---- ISS-019 Step 7b: dimensional backfill ---------------------------

    /// Max retry attempts before a backfill_queue row is marked
    /// `permanently_rejected`. Mirrors the quarantine policy.
    pub const BACKFILL_MAX_ATTEMPTS: u32 = 5;

    /// Scan the `memories` table for v1 (flat) metadata rows and enqueue
    /// those that would benefit from re-extraction into `backfill_queue`.
    ///
    /// Pure read-side operation on `memories`; writes only to
    /// `backfill_queue`. Never rewrites a memory's metadata here — that
    /// is the job of `backfill_dimensions`, which runs the extractor.
    ///
    /// This is an **explicit** operation, never triggered as a read-path
    /// side effect (design §6: "No eager migration"). Callers (rebuild
    /// pilot, CLI, scheduled job) invoke it on demand.
    ///
    /// Returns a [`ScanReport`] summarising what was found. Progress is
    /// cursor-driven via `page_size`; each call scans at most
    /// `max_rows` memories and returns, letting callers budget their
    /// time explicitly.
    pub fn scan_and_enqueue_backfill(
        &mut self,
        max_rows: usize,
    ) -> Result<crate::migration_types::ScanReport, crate::store_api::StoreError> {
        use crate::migration_types::{
            classify_stored_metadata, BackfillReason, LegacyClassification, ScanReport,
        };
        use crate::store_api::StoreError;

        const PAGE_SIZE: usize = 200;
        let mut report = ScanReport::default();
        let mut after_id: Option<String> = None;
        let mut budget = max_rows;

        while budget > 0 {
            let page_size = budget.min(PAGE_SIZE);
            let rows = self
                .storage
                .list_v1_candidates_page(after_id.as_deref(), page_size)
                .map_err(StoreError::DbError)?;
            if rows.is_empty() {
                break;
            }

            for (id, content, metadata_str) in &rows {
                report.scanned += 1;
                let metadata_val = match metadata_str.as_deref() {
                    Some(s) => serde_json::from_str::<serde_json::Value>(s)
                        .unwrap_or(serde_json::Value::Null),
                    None => serde_json::Value::Null,
                };
                match classify_stored_metadata(&metadata_val, content.len()) {
                    None => {
                        // Already v2 (the LIKE filter missed it — possible
                        // if metadata is a non-object value). No-op.
                        report.already_v2 += 1;
                    }
                    Some(LegacyClassification::HasExtractorData) => {
                        // Lossless upgrade possible on next write; not
                        // a backfill candidate (extractor doesn't need
                        // to re-run). Skip.
                        report.has_extractor_data += 1;
                    }
                    Some(LegacyClassification::LowDimLegacy { reason }) => {
                        let reason_kind = match reason {
                            BackfillReason::MissingCoreDimensions => "missing_core_dimensions",
                            BackfillReason::DimensionsEmpty => "dimensions_empty",
                            BackfillReason::PartialDimensionsLongContent => {
                                "partial_dimensions_long_content"
                            }
                        };
                        self.storage
                            .enqueue_backfill(id, reason_kind, None)
                            .map_err(StoreError::DbError)?;
                        report.enqueued += 1;
                    }
                    Some(LegacyClassification::UnparseableLegacy { error }) => {
                        // Short content / non-object metadata — not
                        // worth re-extracting. Skip, record for stats.
                        log::debug!(
                            "scan_and_enqueue_backfill: skipping id={} ({})",
                            id, error
                        );
                        report.unparseable += 1;
                    }
                }
            }

            // Advance cursor to last id seen.
            after_id = rows.last().map(|(id, _, _)| id.clone());
            budget = budget.saturating_sub(rows.len());
        }

        Ok(report)
    }

    /// Drain up to `max_items` rows from `backfill_queue` and re-run
    /// the extractor on each, merging new dimensions into the existing
    /// `memories` row.
    ///
    /// Requires an extractor to be configured. Without one, returns a
    /// report with `attempted = 0` (nothing to do — backfill is about
    /// recovering dimensional signal, which requires an extractor).
    ///
    /// Per row:
    /// - Load existing `MemoryRecord` via `Storage::get`.
    /// - If the row is no longer v1 (already rewritten by another
    ///   write), delete the queue row and record as `unchanged`.
    /// - Otherwise run the extractor on the row's content. For each
    ///   extracted fact, construct an `EnrichedMemory` and merge into
    ///   the existing id via `Storage::merge_enriched_into` (similarity
    ///   1.0 since we know it's the same content).
    /// - On success → delete queue row, increment `upgraded`.
    /// - On extractor failure → `record_backfill_attempt`; if attempts
    ///   cross `BACKFILL_MAX_ATTEMPTS` flip to `permanently_rejected`.
    pub fn backfill_dimensions(
        &mut self,
        max_items: usize,
    ) -> Result<crate::migration_types::BackfillReport, crate::store_api::StoreError> {
        use crate::migration_types::{classify_stored_metadata, BackfillReport};
        use crate::store_api::StoreError;

        let mut report = BackfillReport::default();
        if self.extractor.is_none() {
            // Nothing to do — re-extraction requires an extractor.
            return Ok(report);
        }

        let rows = self
            .storage
            .list_backfill_batch(max_items)
            .map_err(StoreError::DbError)?;
        report.attempted = rows.len() as u64;

        for row in rows {
            // Load the memory row.
            let record = match self
                .storage
                .get(&row.memory_id)
                .map_err(StoreError::DbError)?
            {
                Some(r) => r,
                None => {
                    // Memory deleted/superseded — drop queue entry.
                    let _ = self.storage.delete_backfill_row(&row.memory_id);
                    report.unchanged += 1;
                    continue;
                }
            };

            // If the row has already been rewritten to v2, nothing to do.
            let metadata_val = record
                .metadata
                .clone()
                .unwrap_or(serde_json::Value::Null);
            if classify_stored_metadata(&metadata_val, record.content.len()).is_none() {
                let _ = self.storage.delete_backfill_row(&row.memory_id);
                report.unchanged += 1;
                continue;
            }

            // Re-run extractor. (Safe unwrap — guarded by the None-check
            // at the top of the function.)
            let extractor = self.extractor.as_ref().unwrap();
            let facts_result = extractor.extract(&record.content);

            match facts_result {
                Ok(facts) if facts.is_empty() => {
                    // Extractor now says "nothing to extract". Keep row
                    // in memories as-is (minimal dimensions) but
                    // consider the backfill resolved — removing the
                    // queue entry prevents endless re-attempts.
                    let _ = self.storage.delete_backfill_row(&row.memory_id);
                    report.unchanged += 1;
                }
                Ok(facts) => {
                    let cap = self.config.auto_extract_importance_cap;
                    let mut merged_any = false;
                    let mut first_err: Option<String> = None;

                    for fact in facts {
                        let capped = fact.importance.min(cap);
                        let mut fact_adj = fact;
                        fact_adj.importance = capped;

                        let source_opt = if record.source.is_empty() {
                            None
                        } else {
                            Some(record.source.clone())
                        };
                        // namespace not present on MemoryRecord — preserved
                        // by the existing row and by merge_enriched_into.
                        match crate::enriched::EnrichedMemory::from_extracted(
                            fact_adj,
                            source_opt,
                            None,
                            serde_json::Value::Null, // preserve user metadata via merge
                        ) {
                            Ok(em) => {
                                match self.storage.merge_enriched_into(
                                    &row.memory_id,
                                    &em,
                                    1.0,
                                ) {
                                    Ok(_) => {
                                        merged_any = true;
                                    }
                                    Err(e) => {
                                        if first_err.is_none() {
                                            first_err = Some(e.to_string());
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if first_err.is_none() {
                                    first_err = Some(e.to_string());
                                }
                            }
                        }
                    }

                    if merged_any {
                        let _ = self.storage.delete_backfill_row(&row.memory_id);
                        report.upgraded += 1;
                    } else {
                        let msg = first_err.unwrap_or_else(|| "no valid facts".into());
                        self.bump_backfill_attempts_or_reject(&row, &msg, &mut report)?;
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    self.bump_backfill_attempts_or_reject(&row, &msg, &mut report)?;
                }
            }
        }

        Ok(report)
    }

    fn bump_backfill_attempts_or_reject(
        &self,
        row: &crate::storage::BackfillRow,
        err: &str,
        report: &mut crate::migration_types::BackfillReport,
    ) -> Result<(), crate::store_api::StoreError> {
        use crate::store_api::StoreError;

        self.storage
            .record_backfill_attempt(&row.memory_id, Some(err))
            .map_err(StoreError::DbError)?;
        let new_attempts = row.attempts + 1;
        if new_attempts >= Self::BACKFILL_MAX_ATTEMPTS {
            self.storage
                .mark_backfill_permanently_rejected(&row.memory_id)
                .map_err(StoreError::DbError)?;
            report.permanently_rejected += 1;
        } else {
            report.failed += 1;
        }
        Ok(())
    }

    /// Count live (non-rejected) backfill-queue rows.
    pub fn count_backfill(&self) -> Result<usize, crate::store_api::StoreError> {
        self.storage
            .count_backfill_live()
            .map_err(crate::store_api::StoreError::DbError)
    }


    ///
    /// Unlike simple cosine similarity, this uses:
    /// - Base-level activation (frequency × recency, power law)
    /// - Spreading activation from context keywords
    /// - Importance modulation (emotional memories are more accessible)
    ///
    /// Results include a confidence score (metacognitive monitoring)
    /// that tells you how "trustworthy" each retrieval is.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `limit` - Maximum number of results
    /// * `context` - Additional context keywords to boost relevant memories
    /// * `min_confidence` - Minimum confidence threshold (0-1)
    pub fn recall(
        &mut self,
        query: &str,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        self.recall_from_namespace(query, limit, context, min_confidence, None)
    }
    
    /// Retrieve relevant memories from a specific namespace.
    ///
    /// Uses hybrid search: FTS + embedding + ACT-R activation.
    /// FTS catches exact term matches, embedding catches semantic similarity,
    /// ACT-R boosts frequently/recently accessed memories.
    ///
    /// When embedding is not available, uses FTS + ACT-R only.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `limit` - Maximum number of results
    /// * `context` - Additional context keywords to boost relevant memories
    /// * `min_confidence` - Minimum confidence threshold (0-1)
    /// * `namespace` - Namespace to search (None = "default", Some("*") = all namespaces)
    pub fn recall_from_namespace(
        &mut self,
        query: &str,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let now = Utc::now();
        let recall_start = std::time::Instant::now();
        let context = context.unwrap_or_default();
        let min_conf = min_confidence.unwrap_or(0.0);
        let ns = namespace.unwrap_or("default");
        
        // If embedding provider is available, use embedding-based recall
        if let Some(ref embedding_provider) = self.embedding {
            // Generate embedding for query
            let query_embedding = match embedding_provider.embed(query) {
                Ok(emb) => emb,
                Err(e) => {
                    log::warn!("Failed to generate query embedding, falling back to FTS: {}", e);
                    return self.recall_fts(query, limit, &context, min_conf, ns, now);
                }
            };
            
            // Get all embeddings for namespace and current model
            let model_id = self.config.embedding.model_id();
            let stored_embeddings = self.storage.get_embeddings_in_namespace(Some(ns), &model_id)?;
            
            // If no embedded memories, fall back to FTS
            if stored_embeddings.is_empty() {
                log::debug!("No embedded memories in namespace '{}', falling back to FTS", ns);
                return self.recall_fts(query, limit, &context, min_conf, ns, now);
            }
            
            // Build a map of memory_id -> embedding similarity
            let mut similarity_map: HashMap<String, f32> = HashMap::new();
            for (memory_id, stored_emb) in &stored_embeddings {
                let sim = EmbeddingProvider::cosine_similarity(&query_embedding, stored_emb);
                similarity_map.insert(memory_id.clone(), sim);
            }
            
            // Also get FTS results for exact term matching
            let fts_results = self.storage.search_fts_ns(query, limit * 3, Some(ns))
                .unwrap_or_default();
            let fts_count = fts_results.len();
            
            // Build FTS score map (rank-based normalization)
            let mut fts_score_map: HashMap<String, f64> = HashMap::new();
            for (rank, record) in fts_results.iter().enumerate() {
                let score = 1.0 - (rank as f64 / (fts_count.max(1)) as f64);
                fts_score_map.insert(record.id.clone(), score);
            }
            
            // Entity-based recall (4th channel)
            let (entity_scores, entity_w) = if self.config.entity_config.enabled {
                let es = self.entity_recall(query, Some(ns), limit * 3)?;
                (es, self.config.entity_weight)
            } else {
                (HashMap::new(), 0.0)
            };
            
            // Query-type adaptive weight adjustment (C7: Multi-Retrieval Fusion)
            // + Query intent classification (Level 1 regex + Level 2 Haiku LLM)
            let query_analysis = if self.config.adaptive_weights {
                crate::query_classifier::classify_query_with_l2(
                    query,
                    self.intent_classifier.as_ref(),
                )
            } else {
                crate::query_classifier::QueryAnalysis::neutral()
            };
            
            // Merge candidate IDs from embedding, FTS, and entity recall
            let mut all_ids: std::collections::HashSet<String> = similarity_map.keys().cloned().collect();
            for record in &fts_results {
                all_ids.insert(record.id.clone());
            }
            for id in entity_scores.keys() {
                all_ids.insert(id.clone());
            }
            
            // Get candidate memories
            let mut candidates: Vec<MemoryRecord> = Vec::new();
            for id in &all_ids {
                if let Some(record) = self.storage.get(id)? {
                    candidates.push(record);
                }
            }
            
            // Defense-in-depth: filter out any superseded memories that slipped through SQL filter
            candidates.retain(|r| r.superseded_by.is_none());
            
            // Hebbian channel (6th channel): score candidates by Hebbian connectivity
            let candidate_ids: Vec<String> = candidates.iter().map(|r| r.id.clone()).collect();
            let hebbian_scores = if self.config.hebbian_enabled {
                Self::hebbian_channel_scores(&self.storage, &candidate_ids)?
            } else {
                HashMap::new()
            };
            let hebbian_w = if self.config.hebbian_enabled {
                self.config.hebbian_recall_weight
            } else {
                0.0
            };
            
            // Somatic channel (7th) — emotional memory bias
            let somatic_scores = self.somatic_scores(query, &candidates);
            let raw_somatic_weight = self.config.somatic_weight;
            
            // Base weights (from config)
            let raw_fts_weight = self.config.fts_weight;
            let raw_emb_weight = self.config.embedding_weight;
            let raw_actr_weight = self.config.actr_weight;
            let raw_entity_weight = entity_w;
            let raw_temporal_weight = self.config.temporal_weight;
            let raw_hebbian_weight = hebbian_w;
            
            // Apply query-type modifiers (C7 adaptive weights)
            let adj_fts = raw_fts_weight * query_analysis.weight_modifiers.fts;
            let adj_emb = raw_emb_weight * query_analysis.weight_modifiers.embedding;
            let adj_actr = raw_actr_weight * query_analysis.weight_modifiers.actr;
            let adj_entity = raw_entity_weight * query_analysis.weight_modifiers.entity;
            let adj_temporal = raw_temporal_weight * query_analysis.weight_modifiers.temporal;
            let adj_hebbian = raw_hebbian_weight * query_analysis.weight_modifiers.hebbian;
            let adj_somatic = raw_somatic_weight * query_analysis.weight_modifiers.somatic;
            
            // Runtime normalization — always divide by sum
            let total_weight = adj_fts + adj_emb + adj_actr + adj_entity + adj_temporal + adj_hebbian + adj_somatic;
            let (fts_weight, emb_weight, actr_weight, ent_weight, temp_weight, hebb_weight, som_weight) = if total_weight > 0.0 {
                (
                    adj_fts / total_weight,
                    adj_emb / total_weight,
                    adj_actr / total_weight,
                    adj_entity / total_weight,
                    adj_temporal / total_weight,
                    adj_hebbian / total_weight,
                    adj_somatic / total_weight,
                )
            } else {
                let n = 1.0 / 7.0;
                (n, n, n, n, n, n, n)
            };
            
            log::debug!(
                "C7 recall weights: fts={:.3} emb={:.3} actr={:.3} entity={:.3} temporal={:.3} hebbian={:.3} somatic={:.3} (query_type={:?})",
                fts_weight, emb_weight, actr_weight, ent_weight, temp_weight, hebb_weight, som_weight,
                query_analysis.query_type,
            );
            
            let mut scored: Vec<_> = candidates
                .into_iter()
                .map(|record| {
                    // Get embedding similarity (already normalized to -1..1, convert to 0..1)
                    let embedding_sim = similarity_map
                        .get(&record.id)
                        .copied()
                        .unwrap_or(0.0);
                    let embedding_score = (embedding_sim + 1.0) / 2.0; // Normalize to 0..1
                    
                    // Get FTS score (0 if not in FTS results)
                    let fts_score = fts_score_map.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Get entity score (0 if not in entity results)
                    let entity_score = entity_scores.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Get ACT-R activation
                    let activation = retrieval_activation(
                        &record,
                        &context,
                        now,
                        self.config.actr_decay,
                        self.config.context_weight,
                        self.config.importance_weight,
                        self.config.contradiction_penalty,
                    );
                    
                    // Normalize activation to 0..1 range (sigmoid — much better discrimination than linear)
                    let activation_normalized = crate::models::actr::normalize_activation(
                        activation,
                        self.config.actr_sigmoid_center,
                        self.config.actr_sigmoid_scale,
                    );
                    
                    // Temporal channel (5th) — time-range proximity
                    let temporal_score = Self::temporal_score(&record, &query_analysis.time_range, now);
                    
                    // Hebbian channel (6th) — graph connectivity
                    let hebbian_score = hebbian_scores.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Somatic channel (7th) — emotional memory bias (Damasio)
                    let somatic_score = somatic_scores.get(&record.id).copied().unwrap_or(0.0);
                    
                    // Combined: 7-channel fusion
                    let combined_score = (fts_weight * fts_score)
                        + (emb_weight * embedding_score as f64)
                        + (actr_weight * activation_normalized)
                        + (ent_weight * entity_score)
                        + (temp_weight * temporal_score)
                        + (hebb_weight * hebbian_score)
                        + (som_weight * somatic_score);
                    
                    // Type-affinity modulation: weighted max over type_weights × affinity.
                    // For old memories (no type_weights in metadata), TypeWeights::default() (all 1.0)
                    // gives: max(1.0 × affinity_i) = max(affinity_i), equivalent to old behavior.
                    let type_weights = crate::type_weights::TypeWeights::from_metadata(&record.metadata);
                    let affinity_multiplier = [
                        type_weights.factual    * query_analysis.type_affinity.factual,
                        type_weights.episodic   * query_analysis.type_affinity.episodic,
                        type_weights.relational * query_analysis.type_affinity.relational,
                        type_weights.emotional  * query_analysis.type_affinity.emotional,
                        type_weights.procedural * query_analysis.type_affinity.procedural,
                        type_weights.opinion    * query_analysis.type_affinity.opinion,
                        type_weights.causal     * query_analysis.type_affinity.causal,
                    ]
                    .iter()
                    .cloned()
                    .fold(f64::NEG_INFINITY, f64::max);
                    let final_score = combined_score * affinity_multiplier;
                    
                    (record, final_score, activation)
                })
                .collect();

            // Sort by combined score descending
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            // Take expanded candidate pool for dedup backfilling + type-affinity reranking.
            // 3x expansion applied unconditionally: dedup may drop duplicates, affinity may reorder.
            let expanded_limit = limit * 3;
            let top_candidates: Vec<_> = scored.into_iter().take(expanded_limit).collect();

            // Build pairwise embedding lookup for dedup
            let embedding_lookup: HashMap<&str, &Vec<f32>> = stored_embeddings.iter()
                .map(|(id, emb)| (id.as_str(), emb))
                .collect();

            // Convert to RecallResults with confidence
            let mut all_results: Vec<RecallResult> = top_candidates.iter()
                .map(|(record, _combined_score, activation)| {
                    // Compute confidence from individual signals (not combined_score)
                    let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                    let confidence = compute_query_confidence(
                        similarity_map.get(&record.id).copied(),
                        fts_score_map.contains_key(&record.id),
                        entity_scores.get(&record.id).copied().unwrap_or(0.0),
                        age_hours,
                    );
                    let confidence_label = confidence_label(confidence);

                    RecallResult {
                        record: record.clone(),
                        activation: *activation,
                        confidence,
                        confidence_label,
                    }
                })
                .filter(|r| r.confidence >= min_conf)
                .collect();

            // Dedup by embedding similarity
            if self.config.recall_dedup_enabled {
                all_results = Self::dedup_recall_results_by_embedding(
                    all_results,
                    &embedding_lookup,
                    self.config.recall_dedup_threshold,
                    limit,
                );
            } else {
                all_results.truncate(limit);
            }

            let results = all_results;

            // Record access for all retrieved memories (ACT-R learning)
            for result in &results {
                self.storage.record_access(&result.record.id)?;
            }

            // Hebbian learning: record co-activation (namespace-aware)
            if self.config.hebbian_enabled && results.len() >= 2 {
                let memory_ids: Vec<_> = results.iter().map(|r| r.record.id.clone()).collect();
                crate::models::record_coactivation_ns(
                    &mut self.storage,
                    &memory_ids,
                    self.config.hebbian_threshold,
                    ns,
                )?;
            }

            // Cross-recall co-occurrence detection (C8 / GOAL-17)
            // Track which memories are recalled across separate queries.
            // If two memories are recalled within 30 seconds (across different queries),
            // record their co-activation to strengthen Hebbian links.
            {
                let now_instant = std::time::Instant::now();
                for result in &results {
                    // Check against previously recalled memories within 30s window
                    for (prev_id, prev_time) in &self.recent_recalls {
                        if prev_id == &result.record.id { continue; }
                        if now_instant.duration_since(*prev_time) <= std::time::Duration::from_secs(30) {
                            // Within 30s window → record co-activation
                            let _ = self.storage.record_coactivation_ns(
                                prev_id,
                                &result.record.id,
                                self.config.hebbian_threshold,
                                ns,
                            );
                        }
                    }
                    // Add this result to the ring buffer
                    self.recent_recalls.push_back((result.record.id.clone(), now_instant));
                    if self.recent_recalls.len() > 50 {
                        self.recent_recalls.pop_front();
                    }
                }
            }

            // Update somatic markers with emotional feedback from recall results
            self.update_somatic_after_recall(query, &results);

            // Meta-cognition: record this recall event
            self.record_metacognition_recall(query, &results, recall_start, true);

            Ok(results)
        } else {
            // No embedding provider, use FTS fallback
            let results = self.recall_fts(query, limit, &context, min_conf, ns, now)?;
            self.record_metacognition_recall(query, &results, recall_start, false);
            Ok(results)
        }
    }

    /// Fetch the N most recently created memories, ordered newest-first.
    ///
    /// No query string needed — pure chronological retrieval.
    /// Designed for session bootstrap: after restart, inject recent context
    /// so the agent doesn't start from zero.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of memories to return
    /// * `namespace` - Namespace filter (None = "default", Some("*") = all)
    pub fn recall_recent(
        &self,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let mut records = self.storage.fetch_recent(limit, namespace)?;
        // Defense-in-depth: filter out any superseded memories that slipped through SQL filter
        records.retain(|r| r.superseded_by.is_none());
        Ok(records)
    }
    
    /// Record a recall event to the meta-cognition tracker (if enabled).
    fn record_metacognition_recall(
        &mut self,
        query: &str,
        results: &[RecallResult],
        start: std::time::Instant,
        used_embedding: bool,
    ) {
        if let Some(ref mut tracker) = self.metacognition {
            let mean_conf = if results.is_empty() {
                0.0
            } else {
                results.iter().map(|r| r.confidence).sum::<f64>() / results.len() as f64
            };
            let max_conf = results
                .iter()
                .map(|r| r.confidence)
                .fold(0.0_f64, f64::max);
            let event = crate::metacognition::RecallEvent {
                timestamp: chrono::Utc::now().timestamp(),
                query: query.to_string(),
                result_count: results.len(),
                mean_confidence: mean_conf,
                max_confidence: max_conf,
                latency_ms: start.elapsed().as_millis() as u64,
                used_embedding,
                feedback_score: None,
            };
            if let Err(e) = tracker.record_recall(self.storage.conn(), event) {
                log::warn!("Metacognition recall record failed: {}", e);
            }
        }
    }

    /// Entity-based recall: extract entities from query, look up matching entities,
    /// return memory→score mapping (0.0-1.0 normalized).
    ///
    /// Direct entity matches get full score, 1-hop related entities get half score.
    /// Scores are normalized to 0.0-1.0 by dividing by the maximum score.
    fn entity_recall(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<HashMap<String, f64>, Box<dyn std::error::Error>> {
        let _ = limit; // limit is implicit via the number of entity matches
        let query_entities = self.entity_extractor.extract(query);
        let mut memory_scores: HashMap<String, f64> = HashMap::new();
        
        for qe in &query_entities {
            // Direct entity match (exact name)
            let matches = self.storage.find_entities(&qe.normalized, namespace, 5)?;
            for entity in &matches {
                let memory_ids = self.storage.get_entity_memories(&entity.id)?;
                for mid in memory_ids {
                    *memory_scores.entry(mid).or_insert(0.0) += 1.0;
                }
                
                // 1-hop related entities (weaker signal)
                let related = self.storage.get_related_entities(&entity.id, 10)?;
                for (rel_id, _relation) in related {
                    let rel_memories = self.storage.get_entity_memories(&rel_id)?;
                    for mid in rel_memories {
                        *memory_scores.entry(mid).or_insert(0.0) += 0.5;
                    }
                }
            }
        }
        
        // Normalize scores to 0.0-1.0
        if let Some(&max_score) = memory_scores.values().max_by(|a, b| a.partial_cmp(b).unwrap()) {
            if max_score > 0.0 {
                for score in memory_scores.values_mut() {
                    *score /= max_score;
                }
            }
        }
        
        Ok(memory_scores)
    }
    
    /// Temporal channel scoring for C7 Multi-Retrieval Fusion.
    ///
    /// When a time range is detected in the query, memories within that range
    /// are scored by proximity to the range center (1.0 at center, 0.5 at edges).
    /// Memories outside the range get 0.0.
    ///
    /// When no time range is detected, returns a neutral 0.5 for all memories
    /// (so the temporal channel doesn't distort results when there's no temporal signal).
    fn temporal_score(
        record: &MemoryRecord,
        time_range: &Option<crate::query_classifier::TimeRange>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> f64 {
        match time_range {
            Some(range) => {
                if record.created_at >= range.start && record.created_at <= range.end {
                    // Within range: score by proximity to center of range
                    let range_duration = (range.end - range.start).num_seconds() as f64;
                    let range_center = range.start + (range.end - range.start) / 2;
                    let distance = (record.created_at - range_center).num_seconds().abs() as f64;
                    let half_range = range_duration / 2.0;
                    if half_range > 0.0 {
                        // 1.0 at center, 0.5 at edges
                        1.0 - (distance / half_range) * 0.5
                    } else {
                        1.0
                    }
                } else {
                    0.0 // Outside range
                }
            }
            None => {
                // No temporal query: gentle recency signal (complement ACT-R)
                // Recent memories get slight boost, but not enough to dominate
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                // Sigmoid centered at 72 hours (3 days), scale 48
                let recency = 1.0 / (1.0 + (age_hours - 72.0).exp() / 48.0_f64.exp());
                // Map to 0.3-0.7 range (narrow band so it's a weak signal)
                0.3 + recency * 0.4
            }
        }
    }
    
    /// Somatic marker channel scoring (7th channel — Damasio's somatic marker hypothesis).
    ///
    /// For each candidate memory, computes how much the current situation's emotional
    /// "gut feeling" should bias its recall ranking. The somatic marker for the query
    /// situation provides an intensity signal: strongly emotional situations (positive
    /// or negative) boost emotionally relevant memories.
    ///
    /// Score composition:
    /// - Base = abs(marker_valence) — emotional intensity of the situation
    /// - Emotional type memories get full intensity
    /// - High-importance memories (≥0.7) get 70% intensity
    /// - Other memories get 30% intensity (faint background signal)
    /// - Encounter count adds confidence: min(encounter_count / 5, 1.0) scaling
    ///
    /// Returns scores normalized to 0.0-1.0 for each candidate.
    fn somatic_scores(
        &mut self,
        query: &str,
        candidates: &[MemoryRecord],
    ) -> HashMap<String, f64> {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        
        // Hash the query to create a situation identifier
        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        let situation_hash = hasher.finish();
        
        // Look up or create the somatic marker for this query situation.
        // We use 0.0 as current valence for new situations — neutral until
        // emotional memories are actually recalled and processed.
        let marker = self.interoceptive_hub.somatic_lookup(situation_hash, 0.0);
        let marker_intensity = marker.valence.abs();
        let encounter_confidence = (marker.encounter_count as f64 / 5.0).min(1.0);
        
        // If the marker has no emotional charge (new situation with 0 valence),
        // return empty scores — somatic channel is silent for novel situations.
        if marker_intensity < 0.01 {
            return HashMap::new();
        }
        
        let mut scores = HashMap::new();
        
        for record in candidates {
            // Determine emotional relevance of this memory
            let emotional_relevance = match record.memory_type {
                crate::types::MemoryType::Emotional => 1.0,  // Full somatic boost
                _ if record.importance >= 0.7 => 0.7,        // High-importance: notable
                _ => 0.3,                                      // Faint background signal
            };
            
            // Somatic score: intensity × relevance × confidence
            let score = marker_intensity * emotional_relevance * encounter_confidence;
            scores.insert(record.id.clone(), score.min(1.0));
        }
        
        scores
    }
    
    /// Update somatic markers after recall completes.
    ///
    /// When emotional memories are successfully recalled, their valence
    /// feeds back into the somatic marker for this situation — reinforcing
    /// the emotional association (Damasio's "as-if body loop").
    fn update_somatic_after_recall(
        &mut self,
        query: &str,
        results: &[RecallResult],
    ) {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        
        // Only update if any recalled memories are emotional
        let emotional_valences: Vec<f64> = results.iter()
            .filter(|r| r.record.memory_type == crate::types::MemoryType::Emotional)
            .map(|r| {
                // Use importance as a proxy for valence direction:
                // High importance (>0.5) → positive valence, low → negative
                // This is a heuristic — memories don't store explicit valence yet
                (r.record.importance - 0.5) * 2.0  // Maps [0,1] → [-1,1]
            })
            .collect();
        
        if emotional_valences.is_empty() {
            return;
        }
        
        // Average emotional valence from recalled memories
        let avg_valence = emotional_valences.iter().sum::<f64>() / emotional_valences.len() as f64;
        
        let mut hasher = DefaultHasher::new();
        query.hash(&mut hasher);
        let situation_hash = hasher.finish();
        
        // Feed this emotional signal back into the somatic marker
        self.interoceptive_hub.somatic_lookup(situation_hash, avg_valence);
    }

    /// Hebbian channel scoring for C7 Multi-Retrieval Fusion.
    ///
    /// For each candidate, checks how many other candidates it's Hebbian-linked to.
    /// Memories that are well-connected to other recall results get boosted —
    /// they form coherent clusters of associated knowledge.
    ///
    /// Scores are normalized to 0.0-1.0.
    fn hebbian_channel_scores(
        storage: &crate::storage::Storage,
        candidate_ids: &[String],
    ) -> Result<HashMap<String, f64>, Box<dyn std::error::Error>> {
        let mut scores: HashMap<String, f64> = HashMap::new();
        let candidate_set: std::collections::HashSet<&String> = candidate_ids.iter().collect();
        
        for id in candidate_ids {
            let links = storage.get_hebbian_links_weighted(id)?;
            let mut link_score = 0.0;
            for (linked_id, strength) in &links {
                if candidate_set.contains(linked_id) {
                    // This candidate is Hebbian-linked to another candidate
                    link_score += strength;
                }
            }
            if link_score > 0.0 {
                scores.insert(id.clone(), link_score);
            }
        }
        
        // Normalize to 0.0-1.0
        if let Some(&max) = scores.values().max_by(|a, b| a.partial_cmp(b).unwrap()) {
            if max > 0.0 {
                for v in scores.values_mut() {
                    *v /= max;
                }
            }
        }
        
        Ok(scores)
    }
    
    /// Remove near-duplicate recall results based on pairwise embedding similarity.
    /// Greedy: iterate in score order, skip any result too similar to an already-kept one.
    /// Backfills from the expanded candidate pool to maintain the requested limit.
    fn dedup_recall_results_by_embedding(
        candidates: Vec<RecallResult>,
        embeddings: &HashMap<&str, &Vec<f32>>,
        threshold: f64,
        limit: usize,
    ) -> Vec<RecallResult> {
        let mut kept: Vec<RecallResult> = Vec::with_capacity(limit);
        let mut kept_embeddings: Vec<&Vec<f32>> = Vec::with_capacity(limit);

        for candidate in candidates {
            if kept.len() >= limit {
                break;
            }

            let candidate_emb = match embeddings.get(candidate.record.id.as_str()) {
                Some(emb) => *emb,
                None => {
                    // No embedding available, keep by default
                    kept.push(candidate);
                    continue;
                }
            };

            // Check against all kept results
            let is_dup = kept_embeddings.iter().any(|kept_emb| {
                let sim = EmbeddingProvider::cosine_similarity(candidate_emb, kept_emb);
                sim as f64 > threshold
            });

            if !is_dup {
                kept_embeddings.push(candidate_emb);
                kept.push(candidate);
            }
        }

        kept
    }

    /// FTS-based recall fallback when embeddings are not available.
    fn recall_fts(
        &mut self,
        query: &str,
        limit: usize,
        context: &[String],
        min_conf: f64,
        ns: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let fts_candidates = self.storage.search_fts_ns(query, limit * 3, Some(ns))?;
        
        // Defense-in-depth: filter out any superseded memories that slipped through SQL filter
        let fts_candidates: Vec<_> = fts_candidates.into_iter()
            .filter(|r| r.superseded_by.is_none())
            .collect();
        
        // Somatic channel scoring (also active in FTS-only path)
        let somatic_scores = self.somatic_scores(query, &fts_candidates);
        let somatic_w = self.config.somatic_weight;
        
        let mut scored: Vec<_> = fts_candidates
            .into_iter()
            .map(|record| {
                let activation = retrieval_activation(
                    &record,
                    context,
                    now,
                    self.config.actr_decay,
                    self.config.context_weight,
                    self.config.importance_weight,
                    self.config.contradiction_penalty,
                );
                
                // Add somatic boost to activation score
                let somatic_boost = somatic_scores.get(&record.id).copied().unwrap_or(0.0) * somatic_w;
                let boosted_activation = activation + somatic_boost;
                
                (record, boosted_activation)
            })
            .filter(|(_, act)| *act > f64::NEG_INFINITY)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let results: Vec<_> = scored
            .into_iter()
            .take(limit)
            .map(|(record, activation)| {
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                let confidence = compute_query_confidence(
                    None,       // no embedding available in FTS path
                    true,       // it's an FTS result by definition
                    0.0,        // no entity score in FTS path
                    age_hours,
                );
                let confidence_label = confidence_label(confidence);

                RecallResult {
                    record,
                    activation,
                    confidence,
                    confidence_label,
                }
            })
            .filter(|r| r.confidence >= min_conf)
            .collect();

        for result in &results {
            self.storage.record_access(&result.record.id)?;
        }

        // Hebbian learning: record co-activation (namespace-aware)
        if self.config.hebbian_enabled && results.len() >= 2 {
            let memory_ids: Vec<_> = results.iter().map(|r| r.record.id.clone()).collect();
            crate::models::record_coactivation_ns(
                &mut self.storage,
                &memory_ids,
                self.config.hebbian_threshold,
                ns,
            )?;
        }

        // Cross-recall co-occurrence detection (C8 / GOAL-17)
        // Track which memories are recalled across separate queries.
        // If two memories are recalled within 30 seconds (across different queries),
        // record their co-activation to strengthen Hebbian links.
        {
            let now_instant = std::time::Instant::now();
            for result in &results {
                // Check against previously recalled memories within 30s window
                for (prev_id, prev_time) in &self.recent_recalls {
                    if prev_id == &result.record.id { continue; }
                    if now_instant.duration_since(*prev_time) <= std::time::Duration::from_secs(30) {
                        // Within 30s window → record co-activation
                        let _ = self.storage.record_coactivation_ns(
                            prev_id,
                            &result.record.id,
                            self.config.hebbian_threshold,
                            ns,
                        );
                    }
                }
                // Add this result to the ring buffer
                self.recent_recalls.push_back((result.record.id.clone(), now_instant));
                if self.recent_recalls.len() > 50 {
                    self.recent_recalls.pop_front();
                }
            }
        }

        // Update somatic markers with emotional feedback from FTS recall results
        self.update_somatic_after_recall(query, &results);

        Ok(results)
    }

    /// Run a consolidation cycle ("sleep replay").
    ///
    /// This is the core of memory maintenance. Based on Murre & Chessa's
    /// Memory Chain Model, it:
    ///
    /// 1. Decays working_strength (hippocampal traces fade)
    /// 2. Transfers knowledge to core_strength (neocortical consolidation)
    /// 3. Replays archived memories (prevents catastrophic forgetting)
    /// 4. Rebalances layers (promote strong → core, demote weak → archive)
    ///
    /// Call this periodically — once per "day" of agent operation,
    /// or after significant learning sessions.
    ///
    /// # Arguments
    ///
    /// * `days` - Simulated time step in days (1.0 = one day of consolidation)
    pub fn consolidate(&mut self, days: f64) -> Result<(), Box<dyn std::error::Error>> {
        self.consolidate_namespace(days, None)
    }
    
    /// Run a consolidation cycle for a specific namespace.
    ///
    /// # Arguments
    ///
    /// * `days` - Simulated time step in days (1.0 = one day of consolidation)
    /// * `namespace` - Namespace to consolidate (None = all namespaces)
    pub fn consolidate_namespace(
        &mut self,
        days: f64,
        namespace: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        run_consolidation_cycle(&mut self.storage, days, &self.config, namespace)?;

        // Decay Hebbian links
        if self.config.hebbian_enabled {
            if self.config.association.enabled {
                // Differential decay: co-recall links decay slowest, multi medium, single fastest
                self.storage.decay_hebbian_links_differential(
                    self.config.association.decay_corecall,
                    self.config.association.decay_multi,
                    self.config.association.decay_single,
                )?;
            } else {
                self.storage.decay_hebbian_links(self.config.hebbian_decay)?;
            }
        }

        // Run synthesis if enabled (GUARD-3: opt-in, backward compatible)
        let synth_settings = self.synthesis_settings.clone();
        if let Some(ref settings) = synth_settings {
            if settings.enabled {
                let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
                let engine = self.build_synthesis_engine(embedding_model);
                match engine.synthesize(&mut self.storage, settings) {
                    Ok(report) => {
                        if report.clusters_found > 0 {
                            log::info!(
                                "Synthesis: {} clusters found, {} synthesized, {} skipped, {} deferred",
                                report.clusters_found,
                                report.clusters_synthesized,
                                report.clusters_skipped,
                                report.clusters_deferred,
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("Synthesis failed (non-fatal): {e}");
                    }
                }
                self.restore_llm_provider(engine.into_provider());
            }
        }

        // [ISS-016] Triple extraction phase (cold path, no DB lock during LLM calls)
        if self.config.triple.enabled
            && self.triple_extractor.is_some() {
                if let Err(e) = self.run_triple_extraction() {
                    log::warn!("Triple extraction failed (non-fatal): {e}");
                }
            }

        // [ISS-008] Promotion detection phase
        if self.config.promotion.enabled {
            match self.detect_promotion_candidates() {
                Ok(candidates) if !candidates.is_empty() => {
                    log::info!("Promotion: {} candidates found", candidates.len());
                    for c in &candidates {
                        if let Err(e) = self.storage.store_promotion_candidate(c) {
                            log::warn!("Failed to store promotion candidate: {}", e);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => log::warn!("Promotion detection failed (non-fatal): {e}"),
            }
        }

        Ok(())
    }

    /// Run triple extraction on un-enriched memories.
    /// Called during consolidation when triple extraction is enabled.
    /// Uses lock-release-lock pattern to avoid holding DB lock during LLM calls.
    fn run_triple_extraction(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let batch_size = self.config.triple.batch_size;
        let max_retries = self.config.triple.max_retries;

        // Step 1: Query un-enriched memories
        let memory_ids = self.storage.get_unenriched_memory_ids(batch_size, max_retries)?;
        if memory_ids.is_empty() {
            return Ok(());
        }

        // Step 2: Read memory content (batch read)
        let mut memory_texts: Vec<(String, String)> = Vec::new();
        for id in &memory_ids {
            if let Ok(Some(record)) = self.storage.get(id) {
                memory_texts.push((id.clone(), record.content.clone()));
            }
        }

        // Step 3: LLM extraction — NO DB lock held
        let extractor = self.triple_extractor.as_ref().unwrap(); // caller checks this
        type TripleResult = Vec<(String, Result<Vec<crate::triple::Triple>, Box<dyn std::error::Error + Send + Sync>>)>;
        let mut results: TripleResult = Vec::new();
        for (id, content) in &memory_texts {
            let result = extractor.extract_triples(content);
            results.push((id.clone(), result));
        }

        // Step 4: Store results in a transaction
        self.storage.begin_transaction()?;
        let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            for (id, result) in &results {
                match result {
                    Ok(triples) if !triples.is_empty() => {
                        self.storage.store_triples(id, triples)?;
                        log::debug!("Extracted {} triples for memory {}", triples.len(), id);
                    }
                    Ok(_) => {
                        // Empty triples — mark as attempted
                        self.storage.increment_extraction_attempts(id)?;
                    }
                    Err(e) => {
                        log::warn!("Triple extraction failed for {}: {}", id, e);
                        self.storage.increment_extraction_attempts(id)?;
                    }
                }
            }
            Ok(())
        })();

        match write_result {
            Ok(()) => {
                self.storage.commit_transaction()?;
            }
            Err(e) => {
                let _ = self.storage.rollback_transaction();
                log::warn!("Triple storage failed (non-fatal): {}", e);
            }
        }

        Ok(())
    }

    // === [ISS-008] Knowledge Promotion API ===

    /// Detect knowledge clusters ready for promotion to persistent documents.
    pub fn detect_promotion_candidates(&self) -> Result<Vec<crate::promotion::PromotionCandidate>, Box<dyn std::error::Error>> {
        crate::promotion::detect_promotable_clusters(&self.storage, &self.config.promotion)
    }

    /// Get pending promotion suggestions.
    pub fn pending_promotions(&self) -> Result<Vec<crate::promotion::PromotionCandidate>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_pending_promotions()?)
    }

    /// Approve or dismiss a promotion candidate.
    pub fn resolve_promotion(&self, id: &str, status: &str) -> Result<(), Box<dyn std::error::Error>> {
        Ok(self.storage.resolve_promotion(id, status)?)
    }

    /// Forget a specific memory or prune all below threshold.
    ///
    /// If memory_id is given, removes that specific memory.
    /// Otherwise, prunes all memories whose effective_strength
    /// is below threshold (moves them to archive).
    pub fn forget(
        &mut self,
        memory_id: Option<&str>,
        threshold: Option<f64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let threshold = threshold.unwrap_or(self.config.forget_threshold);

        if let Some(id) = memory_id {
            self.storage.delete(id)?;
        } else {
            // Prune all weak memories
            let now = Utc::now();
            let all = self.storage.all()?;
            for record in all {
                if !record.pinned && effective_strength(&record, now) < threshold
                    && record.layer != MemoryLayer::Archive {
                        let mut updated = record;
                        updated.layer = MemoryLayer::Archive;
                        self.storage.update(&updated)?;
                    }
            }
        }

        Ok(())
    }

    /// Process user feedback as a dopaminergic reward signal.
    ///
    /// Detects positive/negative sentiment and applies reward modulation
    /// to recently accessed memories.
    pub fn reward(&mut self, feedback: &str, recent_n: usize) -> Result<(), Box<dyn std::error::Error>> {
        let polarity = detect_feedback_polarity(feedback);

        if polarity == 0.0 {
            return Ok(()); // Neutral feedback
        }

        // Get recently accessed memories
        let all = self.storage.all()?;
        let _now = Utc::now();
        let mut recent: Vec<_> = all
            .into_iter()
            .filter(|r| !r.access_times.is_empty())
            .collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.access_times.last().cloned()));

        // Apply reward to top-N recent
        for mut record in recent.into_iter().take(recent_n) {
            if polarity > 0.0 {
                // Positive feedback: boost working strength
                record.working_strength += self.config.reward_magnitude * polarity;
                record.working_strength = record.working_strength.min(2.0);
            } else {
                // Negative feedback: suppress working strength
                record.working_strength *= 1.0 + polarity * 0.1; // polarity is negative
                record.working_strength = record.working_strength.max(0.0);
            }
            self.storage.update(&record)?;
        }

        Ok(())
    }

    /// Global synaptic downscaling — normalize all memory weights.
    ///
    /// Based on Tononi & Cirelli's Synaptic Homeostasis Hypothesis.
    pub fn downscale(&mut self, factor: Option<f64>) -> Result<usize, Box<dyn std::error::Error>> {
        let factor = factor.unwrap_or(self.config.downscale_factor);
        let all = self.storage.all()?;
        let mut count = 0;

        for mut record in all {
            if !record.pinned {
                record.working_strength *= factor;
                record.core_strength *= factor;
                self.storage.update(&record)?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Memory system statistics.
    pub fn stats(&self) -> Result<MemoryStats, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        let now = Utc::now();

        let mut by_type: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut by_layer: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut pinned = 0;

        for record in &all {
            by_type
                .entry(record.memory_type.to_string())
                .or_default()
                .push(record);
            by_layer
                .entry(record.layer.to_string())
                .or_default()
                .push(record);
            if record.pinned {
                pinned += 1;
            }
        }

        let type_stats: HashMap<String, TypeStats> = by_type
            .into_iter()
            .map(|(type_name, records)| {
                let count = records.len();
                let avg_strength = records
                    .iter()
                    .map(|r| effective_strength(r, now))
                    .sum::<f64>()
                    / count as f64;
                let avg_importance = records.iter().map(|r| r.importance).sum::<f64>() / count as f64;

                (
                    type_name,
                    TypeStats {
                        count,
                        avg_strength,
                        avg_importance,
                    },
                )
            })
            .collect();

        let layer_stats: HashMap<String, LayerStats> = by_layer
            .into_iter()
            .map(|(layer_name, records)| {
                let count = records.len();
                let avg_working = records.iter().map(|r| r.working_strength).sum::<f64>() / count as f64;
                let avg_core = records.iter().map(|r| r.core_strength).sum::<f64>() / count as f64;

                (
                    layer_name,
                    LayerStats {
                        count,
                        avg_working,
                        avg_core,
                    },
                )
            })
            .collect();

        let uptime_hours = (now - self.created_at).num_seconds() as f64 / 3600.0;

        Ok(MemoryStats {
            total_memories: all.len(),
            by_type: type_stats,
            by_layer: layer_stats,
            pinned,
            uptime_hours,
        })
    }

    /// Pin a memory — it won't decay or be pruned.
    pub fn pin(&mut self, memory_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut record) = self.storage.get(memory_id)? {
            record.pinned = true;
            self.storage.update(&record)?;
        }
        Ok(())
    }

    /// Unpin a memory — it will resume normal decay.
    pub fn unpin(&mut self, memory_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut record) = self.storage.get(memory_id)? {
            record.pinned = false;
            self.storage.update(&record)?;
        }
        Ok(())
    }
    
    // === Update Memory ===
    
    /// Update an existing memory's content.
    ///
    /// Stores the old content in metadata for audit trail and regenerates
    /// the embedding if embedding support is enabled.
    ///
    /// ## v1 → v2 upgrade side-effect (ISS-019 design §6)
    ///
    /// When the target row is still in the v1 flat metadata layout, this
    /// method rewrites it to the v2 `{engram, user}` layout as a side
    /// effect — same contract as `merge_enriched_into`. The audit trail
    /// (previous content, reason, timestamp) is appended to
    /// `user.update_audit[]` as a chronological list, never mixed into
    /// the `engram.*` namespace or the v1 flat fields.
    ///
    /// If the row is already v2, the layout is preserved and a new
    /// `update_audit` entry is appended.
    ///
    /// # Arguments
    ///
    /// * `memory_id` - ID of the memory to update
    /// * `new_content` - New content to replace the existing content
    /// * `reason` - Reason for the update (stored in audit trail)
    ///
    /// # Returns
    ///
    /// The memory ID on success, or an error if the memory doesn't exist.
    pub fn update_memory(
        &mut self,
        memory_id: &str,
        new_content: &str,
        reason: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        use crate::enriched::EnrichedMemory;

        // Fetch existing record (v1 or v2 layout, both accepted).
        let record = self.storage.get(memory_id)?
            .ok_or_else(|| format!("Memory {} not found", memory_id))?;

        let previous_content = record.content.clone();

        // Decode into typed EnrichedMemory. This is the dual-path read
        // (`Dimensions::from_stored_metadata` handles v2 first, v1
        // fallback). user_metadata captures any caller-supplied keys
        // outside the engram-reserved set — including a v1 row's
        // `update_audit` if one was written by an earlier call.
        let em = EnrichedMemory::from_memory_record(&record).map_err(|e| {
            format!(
                "update_memory: failed to decode row {} into EnrichedMemory: {}",
                memory_id, e
            )
        })?;

        // Swap in new content. Per the EnrichedMemory invariant,
        // core_fact must track content — rebuild Dimensions with the
        // new fact. All other dimensional fields (participants,
        // temporal, causation, …) are preserved verbatim because an
        // update is a content rewrite, not a re-extraction.
        let new_core = crate::dimensions::NonEmptyString::new(new_content)
            .map_err(|_| "update_memory: new_content is empty or whitespace-only")?;
        let mut new_dims = em.dimensions.clone();
        new_dims.core_fact = new_core;

        let new_em = EnrichedMemory::from_dimensions(
            new_dims,
            em.importance,
            em.source.clone(),
            em.namespace.clone(),
            em.user_metadata.clone(),
        );

        // Serialize to v2 layout. `to_legacy_metadata()` (misnamed for
        // historical reasons) produces the v2 `{engram, user}` shape.
        let mut metadata = new_em.to_legacy_metadata();

        // Append audit entry under `user.update_audit` — a chronological
        // Vec of {previous_content, reason, updated_at}. Keeps audit
        // data out of the engram.* namespace so reads stay clean.
        let audit_entry = serde_json::json!({
            "previous_content": previous_content,
            "reason": reason,
            "updated_at": Utc::now().to_rfc3339(),
        });
        if let Some(top) = metadata.as_object_mut() {
            let user_entry = top
                .entry("user".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !user_entry.is_object() {
                *user_entry = serde_json::json!({});
            }
            if let Some(user_obj) = user_entry.as_object_mut() {
                let history_entry = user_obj
                    .entry("update_audit".to_string())
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                if !history_entry.is_array() {
                    *history_entry = serde_json::Value::Array(Vec::new());
                }
                if let Some(arr) = history_entry.as_array_mut() {
                    arr.push(audit_entry);
                    // Cap to last 10 entries — matches merge_history cap.
                    if arr.len() > 10 {
                        let start = arr.len() - 10;
                        *arr = arr[start..].to_vec();
                    }
                }
            }
        }

        // Persist — single UPDATE on content + metadata.
        self.storage.update_content(memory_id, new_content, Some(metadata))?;
        
        // Regenerate embedding if provider is available
        if let Some(ref embedding_provider) = self.embedding {
            match embedding_provider.embed(new_content) {
                Ok(embedding) => {
                    // Delete old embedding for this model and store new one
                    self.storage.delete_embedding(memory_id, &self.config.embedding.model_id())?;
                    self.storage.store_embedding(
                        memory_id,
                        &embedding,
                        &self.config.embedding.model_id(),
                        self.config.embedding.dimensions,
                    )?;
                }
                Err(e) => {
                    log::warn!("Failed to regenerate embedding for {}: {}", memory_id, e);
                }
            }
        }
        
        Ok(memory_id.to_string())
    }
    
    // === Export ===
    
    /// Export all memories to a JSON file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the output JSON file
    ///
    /// # Returns
    ///
    /// Number of memories exported.
    pub fn export(&self, path: &str) -> Result<usize, Box<dyn std::error::Error>> {
        self.export_namespace(path, None)
    }
    
    /// Export memories from a specific namespace to a JSON file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the output JSON file
    /// * `namespace` - Namespace to export (None = all)
    ///
    /// # Returns
    ///
    /// Number of memories exported.
    pub fn export_namespace(
        &self,
        path: &str,
        namespace: Option<&str>,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let memories = self.storage.all_in_namespace(namespace)?;
        let count = memories.len();
        
        // Serialize to JSON
        let json = serde_json::to_string_pretty(&memories)?;
        
        // Write to file
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        
        log::info!("Exported {} memories to {}", count, path);
        
        Ok(count)
    }
    
    // === Recall Causal ===
    
    /// Recall associated memories (type=causal, created by STDP during consolidation).
    ///
    /// # Arguments
    ///
    /// * `cause_query` - Optional query to filter causal memories
    /// * `limit` - Maximum number of results
    /// * `min_confidence` - Minimum confidence threshold
    ///
    /// # Returns
    ///
    /// Matching associated memories sorted by importance.
    /// Hybrid recall combining FTS + embedding + ACT-R activation.
    /// 
    /// Unlike `recall()` which uses embedding+ACT-R only, this also includes
    /// FTS exact matching. Better for queries with specific names/terms.
    pub fn hybrid_recall(
        &mut self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        use crate::hybrid_search::{hybrid_search, HybridSearchOpts};
        
        // Generate query embedding if available
        let query_vector = if let Some(ref embedding) = self.embedding {
            embedding.embed(query).ok()
        } else {
            None
        };
        
        let opts = HybridSearchOpts {
            limit,
            namespace: namespace.map(String::from),
            ..Default::default()
        };
        
        let results = hybrid_search(
            &self.storage,
            query_vector.as_deref(),
            query,
            opts,
            &self.config.embedding.model_id(),
        )?;
        
        // Convert HybridSearchResult to RecallResult
        let recall_results: Vec<RecallResult> = results.into_iter()
            .filter_map(|hr| {
                let record = hr.record?; // Skip if no record
                let score = hr.score;
                let label = if score > 0.7 {
                    "confident".to_string()
                } else if score > 0.4 {
                    "likely".to_string()
                } else {
                    "uncertain".to_string()
                };
                Some(RecallResult {
                    record,
                    activation: score,
                    confidence: score,
                    confidence_label: label,
                })
            })
            .collect();
        
        // Defense-in-depth: filter out any superseded memories that slipped through
        let recall_results: Vec<_> = recall_results.into_iter()
            .filter(|r| r.record.superseded_by.is_none())
            .collect();
        
        Ok(recall_results)
    }    
    /// Uses Hebbian links to find memories that frequently co-occur.
    /// Note: this finds *associations*, not true causal relationships.
    /// LLMs can infer causality from the associated context.
    pub fn recall_associated(
        &mut self,
        cause_query: Option<&str>,
        limit: usize,
        min_confidence: f64,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        self.recall_associated_ns(cause_query, limit, min_confidence, None)
    }
    
    /// Recall associated memories from a specific namespace.
    pub fn recall_associated_ns(
        &mut self,
        cause_query: Option<&str>,
        limit: usize,
        min_confidence: f64,
        namespace: Option<&str>,
    ) -> Result<Vec<RecallResult>, Box<dyn std::error::Error>> {
        let now = Utc::now();
        
        if let Some(query) = cause_query {
            // Do normal recall but filter to causal type
            let results = self.recall_from_namespace(
                query,
                limit * 2, // Fetch more to filter
                None,
                Some(min_confidence),
                namespace,
            )?;
            
            // Filter to causal type
            let filtered: Vec<_> = results
                .into_iter()
                .filter(|r| r.record.memory_type == MemoryType::Causal)
                .take(limit)
                .collect();
            
            Ok(filtered)
        } else {
            // Get all causal memories sorted by importance
            let causal_memories = self.storage.search_by_type_ns(
                MemoryType::Causal,
                namespace,
                limit * 2,
            )?;
            
            // Score and filter
            let mut scored: Vec<_> = causal_memories
                .into_iter()
                .filter(|r| r.superseded_by.is_none()) // Defense-in-depth
                .map(|record| {
                    let activation = retrieval_activation(
                        &record,
                        &[],
                        now,
                        self.config.actr_decay,
                        self.config.context_weight,
                        self.config.importance_weight,
                        self.config.contradiction_penalty,
                    );
                    let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                    let confidence = compute_query_confidence(
                        None,   // no embedding in causal recall path
                        false,  // not an FTS query match
                        0.0,    // no entity score
                        age_hours,
                    );
                    (record, activation, confidence)
                })
                .filter(|(_, _, conf)| *conf >= min_confidence)
                .collect();
            
            // Sort by importance then activation
            scored.sort_by(|a, b| {
                b.0.importance.partial_cmp(&a.0.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            
            // Take top-k
            let results: Vec<_> = scored
                .into_iter()
                .take(limit)
                .map(|(record, activation, confidence)| {
                    RecallResult {
                        record,
                        activation,
                        confidence,
                        confidence_label: confidence_label(confidence),
                    }
                })
                .collect();
            
            Ok(results)
        }
    }
    
    // === Session Recall ===
    
    /// Session-aware recall using working memory to avoid redundant searches.
    ///
    /// If the topic is continuous (high overlap with previous recall), returns
    /// cached working memory items. If topic changed or WM is empty, does full recall.
    ///
    /// # Arguments
    ///
    /// * `query` - Search query
    /// * `session_wm` - Mutable reference to session's working memory
    /// * `limit` - Maximum number of results
    /// * `context` - Optional context keywords
    /// * `min_confidence` - Minimum confidence threshold
    ///
    /// # Returns
    ///
    /// SessionRecallResult with memories and metadata about the recall.
    pub fn session_recall(
        &mut self,
        query: &str,
        session_wm: &mut ActiveContext,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
    ) -> Result<SessionRecallResult, Box<dyn std::error::Error>> {
        self.session_recall_ns(query, session_wm, limit, context, min_confidence, None)
    }
    
    /// Session-aware recall from a specific namespace.
    pub fn session_recall_ns(
        &mut self,
        query: &str,
        session_wm: &mut ActiveContext,
        limit: usize,
        context: Option<Vec<String>>,
        min_confidence: Option<f64>,
        namespace: Option<&str>,
    ) -> Result<SessionRecallResult, Box<dyn std::error::Error>> {
        const CONTINUITY_THRESHOLD: f64 = 0.6;
        
        // Get active IDs from working memory
        let active_ids = session_wm.get_active_ids();
        
        // Check if we need full recall, capturing the initial probe ratio
        let (need_full_recall, initial_ratio) = if active_ids.is_empty() {
            (true, 0.0)
        } else {
            // Do lightweight probe (3 results) to check topic continuity
            let probe = self.recall_from_namespace(
                query,
                3,
                context.clone(),
                min_confidence,
                namespace,
            )?;
            
            let probe_ids: Vec<String> = probe.iter().map(|r| r.record.id.clone()).collect();
            let (_, ratio) = session_wm.overlap(&probe_ids);
            if ratio >= CONTINUITY_THRESHOLD {
                (false, ratio)  // topic continuous
            } else {
                (true, ratio)   // topic changed
            }
        };
        
        if need_full_recall {
            // Topic changed or WM empty → full recall
            let results = self.recall_from_namespace(
                query,
                limit,
                context,
                min_confidence,
                namespace,
            )?;
            
            // Update working memory with scores for future cached path
            let entries: Vec<(String, f64, f64)> = results.iter()
                .map(|r| (r.record.id.clone(), r.confidence, r.activation))
                .collect();
            session_wm.activate_with_scores(&entries);
            session_wm.set_query(query);

            // GWT broadcast: admitted memories → interoceptive hub + Hebbian spreading.
            let admitted_ids: Vec<String> = results.iter().map(|r| r.record.id.clone()).collect();
            self.broadcast_admission(&admitted_ids, session_wm);
            
            Ok(SessionRecallResult {
                results,
                full_recall: true,
                wm_size: session_wm.len(),
                continuity_ratio: initial_ratio,
            })
        } else {
            // Topic continuous → return cached WM items with preserved scores
            let mut cached_results = Vec::new();
            let now = Utc::now();
            
            for id in &active_ids {
                if let Some(record) = self.storage.get(id)? {
                    // Reuse cached scores from the original full recall
                    let (activation, confidence) = if let Some(cached) = session_wm.get_score(id) {
                        (cached.activation, cached.confidence)
                    } else {
                        // Fallback: memory activated by ID only (legacy/no cached scores)
                        let activation = retrieval_activation(
                            &record,
                            &context.clone().unwrap_or_default(),
                            now,
                            self.config.actr_decay,
                            self.config.context_weight,
                            self.config.importance_weight,
                            self.config.contradiction_penalty,
                        );
                        let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                        let confidence = compute_query_confidence(
                            None,
                            false,
                            0.0,
                            age_hours,
                        );
                        (activation, confidence)
                    };
                    
                    if min_confidence.map(|mc| confidence >= mc).unwrap_or(true) {
                        cached_results.push(RecallResult {
                            record,
                            activation,
                            confidence,
                            confidence_label: confidence_label(confidence),
                        });
                    }
                }
            }
            
            // Sort by activation
            cached_results.sort_by(|a, b| {
                b.activation.partial_cmp(&a.activation).unwrap_or(std::cmp::Ordering::Equal)
            });
            cached_results.truncate(limit);
            
            // Refresh activation timestamps (reuse existing scores)
            let result_ids: Vec<String> = cached_results.iter().map(|r| r.record.id.clone()).collect();
            session_wm.activate(&result_ids);
            session_wm.set_query(query);

            // GWT broadcast on cached path too — re-activation reinforces the hub state.
            // (Lighter than full-recall broadcast: same memories, but keeps hub current.)
            self.broadcast_admission(&result_ids, session_wm);
            
            Ok(SessionRecallResult {
                results: cached_results,
                full_recall: false,
                wm_size: session_wm.len(),
                continuity_ratio: initial_ratio,  // reuse from initial probe
            })
        }
    }
    
    /// Get a memory by ID.
    pub fn get(&self, memory_id: &str) -> Result<Option<MemoryRecord>, Box<dyn std::error::Error>> {
        Ok(self.storage.get(memory_id)?)
    }
    
    /// List all memories (with optional limit).
    pub fn list(&self, limit: Option<usize>) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        match limit {
            Some(l) => Ok(all.into_iter().take(l).collect()),
            None => Ok(all),
        }
    }
    
    /// List all memories in a namespace.
    pub fn list_ns(
        &self,
        namespace: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all_in_namespace(namespace)?;
        match limit {
            Some(l) => Ok(all.into_iter().take(l).collect()),
            None => Ok(all),
        }
    }

    /// Get Hebbian links for a specific memory.
    pub fn hebbian_links(&self, memory_id: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_hebbian_neighbors(memory_id)?)
    }
    
    /// Get Hebbian links for a specific memory, filtered by namespace.
    pub fn hebbian_links_ns(
        &self,
        memory_id: &str,
        namespace: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_hebbian_neighbors_ns(memory_id, namespace)?)
    }
    
    // === Embedding Methods ===
    
    /// Get embedding statistics.
    pub fn embedding_stats(&self) -> Result<EmbeddingStats, Box<dyn std::error::Error>> {
        Ok(self.storage.embedding_stats()?)
    }
    
    /// Get the embedding configuration.
    pub fn embedding_config(&self) -> &EmbeddingConfig {
        &self.config.embedding
    }
    
    /// Check if the embedding provider is available.
    pub fn is_embedding_available(&self) -> bool {
        self.embedding.as_ref().map(|e| e.is_available()).unwrap_or(false)
    }

    /// Get a reference to the embedding provider (if available).
    pub fn embedding_provider(&self) -> Option<&EmbeddingProvider> {
        self.embedding.as_ref()
    }
    
    /// Check if embedding support is enabled (provider was created).
    pub fn has_embedding_support(&self) -> bool {
        self.embedding.is_some()
    }
    
    /// Reindex embeddings for all memories without embeddings.
    ///
    /// Useful after migration or model change. Iterates all memories
    /// that don't have embeddings and generates them.
    ///
    /// Returns the number of memories reindexed.
    /// Returns an error if embedding provider is not available.
    pub fn reindex_embeddings(&mut self) -> Result<usize, Box<dyn std::error::Error>> {
        let embedding_provider = self.embedding.as_ref().ok_or_else(|| {
            Box::new(EmbeddingError::OllamaNotAvailable(self.config.embedding.host.clone()))
                as Box<dyn std::error::Error>
        })?;
        
        let model_id = self.config.embedding.model_id();
        let missing_ids = self.storage.get_memories_without_embeddings(&model_id)?;
        let total = missing_ids.len();
        
        if total == 0 {
            return Ok(0);
        }
        
        log::info!("Reindexing {} memories without embeddings for model {}", total, model_id);
        
        let mut reindexed = 0;
        for id in missing_ids {
            if let Some(record) = self.storage.get(&id)? {
                match embedding_provider.embed(&record.content) {
                    Ok(embedding) => {
                        self.storage.store_embedding(
                            &id,
                            &embedding,
                            &model_id,
                            self.config.embedding.dimensions,
                        )?;
                        reindexed += 1;
                    }
                    Err(e) => {
                        log::warn!("Failed to generate embedding for {}: {}", id, e);
                    }
                }
            }
        }
        
        log::info!("Reindexed {}/{} memories", reindexed, total);
        Ok(reindexed)
    }
    
    /// Reindex embeddings with progress callback.
    ///
    /// The callback receives (current, total) progress updates.
    /// Returns an error if embedding provider is not available.
    pub fn reindex_embeddings_with_progress<F>(
        &mut self,
        mut progress: F,
    ) -> Result<usize, Box<dyn std::error::Error>>
    where
        F: FnMut(usize, usize),
    {
        let embedding_provider = self.embedding.as_ref().ok_or_else(|| {
            Box::new(EmbeddingError::OllamaNotAvailable(self.config.embedding.host.clone()))
                as Box<dyn std::error::Error>
        })?;
        
        let model_id = self.config.embedding.model_id();
        let missing_ids = self.storage.get_memories_without_embeddings(&model_id)?;
        let total = missing_ids.len();
        
        if total == 0 {
            return Ok(0);
        }
        
        let mut reindexed = 0;
        for (i, id) in missing_ids.into_iter().enumerate() {
            progress(i + 1, total);
            
            if let Some(record) = self.storage.get(&id)? {
                match embedding_provider.embed(&record.content) {
                    Ok(embedding) => {
                        self.storage.store_embedding(
                            &id,
                            &embedding,
                            &model_id,
                            self.config.embedding.dimensions,
                        )?;
                        reindexed += 1;
                    }
                    Err(e) => {
                        log::warn!("Failed to generate embedding for {}: {}", id, e);
                    }
                }
            }
        }
        
        Ok(reindexed)
    }
    
    // === ACL Methods ===
    
    /// Grant a permission to an agent for a namespace.
    /// 
    /// Only agents with admin permission on the namespace (or wildcard admin)
    /// can grant permissions. If no agent_id is set, uses "system" as grantor.
    pub fn grant(
        &mut self,
        agent_id: &str,
        namespace: &str,
        permission: Permission,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let grantor = self.agent_id.clone().unwrap_or_else(|| "system".to_string());
        self.storage.grant_permission(agent_id, namespace, permission, &grantor)?;
        Ok(())
    }
    
    /// Revoke a permission from an agent for a namespace.
    pub fn revoke(&mut self, agent_id: &str, namespace: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.revoke_permission(agent_id, namespace)?;
        Ok(())
    }
    
    /// Check if an agent has a specific permission for a namespace.
    pub fn check_permission(
        &self,
        agent_id: &str,
        namespace: &str,
        permission: Permission,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(self.storage.check_permission(agent_id, namespace, permission)?)
    }
    
    /// List all permissions for an agent.
    pub fn list_permissions(&self, agent_id: &str) -> Result<Vec<AclEntry>, Box<dyn std::error::Error>> {
        Ok(self.storage.list_permissions(agent_id)?)
    }
    
    /// Get statistics for a specific namespace.
    pub fn stats_ns(&self, namespace: Option<&str>) -> Result<MemoryStats, Box<dyn std::error::Error>> {
        let all = self.storage.all_in_namespace(namespace)?;
        let now = Utc::now();

        let mut by_type: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut by_layer: HashMap<String, Vec<&MemoryRecord>> = HashMap::new();
        let mut pinned = 0;

        for record in &all {
            by_type
                .entry(record.memory_type.to_string())
                .or_default()
                .push(record);
            by_layer
                .entry(record.layer.to_string())
                .or_default()
                .push(record);
            if record.pinned {
                pinned += 1;
            }
        }

        let type_stats: HashMap<String, TypeStats> = by_type
            .into_iter()
            .map(|(type_name, records)| {
                let count = records.len();
                let avg_strength = records
                    .iter()
                    .map(|r| effective_strength(r, now))
                    .sum::<f64>()
                    / count as f64;
                let avg_importance = records.iter().map(|r| r.importance).sum::<f64>() / count as f64;

                (
                    type_name,
                    TypeStats {
                        count,
                        avg_strength,
                        avg_importance,
                    },
                )
            })
            .collect();

        let layer_stats: HashMap<String, LayerStats> = by_layer
            .into_iter()
            .map(|(layer_name, records)| {
                let count = records.len();
                let avg_working = records.iter().map(|r| r.working_strength).sum::<f64>() / count as f64;
                let avg_core = records.iter().map(|r| r.core_strength).sum::<f64>() / count as f64;

                (
                    layer_name,
                    LayerStats {
                        count,
                        avg_working,
                        avg_core,
                    },
                )
            })
            .collect();

        let uptime_hours = (now - self.created_at).num_seconds() as f64 / 3600.0;

        Ok(MemoryStats {
            total_memories: all.len(),
            by_type: type_stats,
            by_layer: layer_stats,
            pinned,
            uptime_hours,
        })
    }
    
    // === Phase 3: Cross-Agent Intelligence ===
    
    /// Recall memories with cross-namespace associations.
    ///
    /// When using namespace="*", this also returns Hebbian links that span
    /// across different namespaces, enabling cross-domain intelligence.
    ///
    /// # Arguments
    ///
    /// * `query` - Natural language query
    /// * `namespace` - Namespace to search ("*" for all)
    /// * `limit` - Maximum number of results
    pub fn recall_with_associations(
        &mut self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<RecallWithAssociationsResult, Box<dyn std::error::Error>> {
        let now = Utc::now();
        let ns = namespace.unwrap_or("default");
        
        // Get candidate memories via FTS
        let candidates = self.storage.search_fts_ns(query, limit * 3, Some(ns))?;
        
        // Score each candidate with ACT-R activation
        let mut scored: Vec<_> = candidates
            .into_iter()
            .filter(|r| r.superseded_by.is_none()) // Defense-in-depth
            .map(|record| {
                let activation = retrieval_activation(
                    &record,
                    &[],
                    now,
                    self.config.actr_decay,
                    self.config.context_weight,
                    self.config.importance_weight,
                    self.config.contradiction_penalty,
                );
                (record, activation)
            })
            .filter(|(_, act)| *act > f64::NEG_INFINITY)
            .collect();
        
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        // Take top-k
        let results: Vec<_> = scored
            .into_iter()
            .take(limit)
            .map(|(record, activation)| {
                let age_hours = (now - record.created_at).num_seconds() as f64 / 3600.0;
                let confidence = compute_query_confidence(
                    None,   // no embedding in associations path
                    false,  // not an FTS query match
                    0.0,    // no entity score
                    age_hours,
                );
                let confidence_label = confidence_label(confidence);
                
                RecallResult {
                    record,
                    activation,
                    confidence,
                    confidence_label,
                }
            })
            .collect();
        
        // Record access for all retrieved memories
        for result in &results {
            self.storage.record_access(&result.record.id)?;
        }
        
        // Collect cross-namespace associations
        let mut cross_links = Vec::new();
        
        // For wildcard namespace queries, also collect cross-namespace Hebbian neighbors
        if ns == "*" && results.len() >= 2 {
            // Get namespaces for all retrieved memories
            let mut memories_with_ns: Vec<MemoryWithNamespace> = Vec::new();
            
            for result in &results {
                if let Some(mem_ns) = self.storage.get_namespace(&result.record.id)? {
                    memories_with_ns.push(MemoryWithNamespace {
                        id: result.record.id.clone(),
                        namespace: mem_ns,
                    });
                }
            }
            
            // Record cross-namespace co-activation
            if self.config.hebbian_enabled {
                record_cross_namespace_coactivation(
                    &mut self.storage,
                    &memories_with_ns,
                    self.config.hebbian_threshold,
                )?;
            }
            
            // Collect cross-links from all retrieved memories
            for result in &results {
                let links = self.storage.get_cross_namespace_neighbors(&result.record.id)?;
                cross_links.extend(links);
            }
            
            // Deduplicate by (source_id, target_id)
            cross_links.sort_by(|a, b| {
                (&a.source_id, &a.target_id).cmp(&(&b.source_id, &b.target_id))
            });
            cross_links.dedup_by(|a, b| a.source_id == b.source_id && a.target_id == b.target_id);
            
            // Sort by strength descending
            cross_links.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());
        }
        
        Ok(RecallWithAssociationsResult {
            memories: results,
            cross_links,
        })
    }
    
    /// Discover cross-namespace Hebbian links between two namespaces.
    ///
    /// Returns all Hebbian associations that span across the given namespaces.
    /// ACL-aware: only returns links between namespaces the agent can read.
    pub fn discover_cross_links(
        &self,
        namespace_a: &str,
        namespace_b: &str,
    ) -> Result<Vec<HebbianLink>, Box<dyn std::error::Error>> {
        // ACL check if agent_id is set
        if let Some(ref agent_id) = self.agent_id {
            let can_read_a = self.storage.check_permission(agent_id, namespace_a, Permission::Read)?;
            let can_read_b = self.storage.check_permission(agent_id, namespace_b, Permission::Read)?;
            
            if !can_read_a || !can_read_b {
                return Ok(vec![]); // No access to one or both namespaces
            }
        }
        
        Ok(self.storage.discover_cross_links(namespace_a, namespace_b)?)
    }
    
    /// Get all cross-namespace associations for a memory.
    pub fn get_cross_associations(
        &self,
        memory_id: &str,
    ) -> Result<Vec<CrossLink>, Box<dyn std::error::Error>> {
        Ok(self.storage.get_cross_namespace_neighbors(memory_id)?)
    }
    
    // === Subscription/Notification Methods ===
    
    /// Subscribe to notifications for a namespace.
    ///
    /// The agent will receive notifications when new memories are stored
    /// with importance >= min_importance in the specified namespace.
    pub fn subscribe(
        &self,
        agent_id: &str,
        namespace: &str,
        min_importance: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.subscribe(agent_id, namespace, min_importance)?;
        Ok(())
    }
    
    /// Unsubscribe from a namespace.
    pub fn unsubscribe(
        &self,
        agent_id: &str,
        namespace: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.unsubscribe(agent_id, namespace)
    }
    
    /// List subscriptions for an agent.
    pub fn list_subscriptions(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Subscription>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.list_subscriptions(agent_id)
    }
    
    /// Check for notifications since last check.
    ///
    /// Returns new memories that exceed the subscription thresholds.
    /// Updates the cursor so the same notifications aren't returned twice.
    pub fn check_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.check_notifications(agent_id)
    }
    
    /// Peek at notifications without updating cursor.
    pub fn peek_notifications(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Notification>, Box<dyn std::error::Error>> {
        let mgr = SubscriptionManager::new(self.storage.connection())?;
        mgr.peek_notifications(agent_id)
    }

    /// Extract entities from existing memories that don't have entity links yet.
    /// Returns (processed_count, entity_count, relation_count).
    pub fn backfill_entities(&self, batch_size: usize) -> Result<(usize, usize, usize), Box<dyn std::error::Error>> {
        let unlinked = self.storage.get_memories_without_entities(batch_size)?;
        let mut entity_count = 0;
        let mut relation_count = 0;
        let processed = unlinked.len();

        for (memory_id, content, ns) in &unlinked {
            let entities = self.entity_extractor.extract(content);
            let mut entity_ids = Vec::new();

            for entity in &entities {
                match self.storage.upsert_entity(
                    &entity.normalized,
                    entity.entity_type.as_str(),
                    ns,
                    None,
                ) {
                    Ok(eid) => {
                        let _ = self.storage.link_memory_entity(memory_id, &eid, "mention");
                        entity_ids.push(eid);
                        entity_count += 1;
                    }
                    Err(e) => {
                        log::warn!("Entity upsert failed during backfill: {}", e);
                    }
                }
            }

            // Co-occurrence (capped at 10)
            let cap = entity_ids.len().min(10);
            for i in 0..cap {
                for j in (i + 1)..cap {
                    if self
                        .storage
                        .upsert_entity_relation(&entity_ids[i], &entity_ids[j], "co_occurs", ns)
                        .is_ok()
                    {
                        relation_count += 1;
                    }
                }
            }
        }

        Ok((processed, entity_count, relation_count))
    }

    /// Get entity statistics: (entity_count, relation_count, link_count).
    pub fn entity_stats(&self) -> Result<(usize, usize, usize), Box<dyn std::error::Error>> {
        Ok(self.storage.entity_stats()?)
    }
    
    /// Purge garbage entities created by regex false positives.
    /// Removes:
    /// - Person entities that are 1-2 chars or pure digits (e.g., "0", "1", "types")
    /// - Orphaned entities with no memory links
    ///
    /// Returns count of entities deleted.
    pub fn purge_garbage_entities(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let mut total_deleted = 0;
        
        // Phase 1: Delete short/numeric person entities that are clearly false positives
        let garbage_persons: Vec<String> = {
            let conn = self.storage.connection();
            let mut stmt = conn.prepare(
                "SELECT id, name FROM entities WHERE entity_type = 'person'"
            )?;
            let rows: Vec<(String, String)> = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?.filter_map(|r| r.ok()).collect();
            drop(stmt);
            
            rows.into_iter()
                .filter(|(_, name)| {
                    let n = name.trim();
                    // Pure digit, single char, or known false positive
                    n.len() <= 2
                        || n.chars().all(|c| c.is_ascii_digit())
                        || matches!(n, "types" | "user" | "mac" | "sigma" | "github")
                })
                .map(|(id, _)| id)
                .collect()
        };
        
        for id in &garbage_persons {
            if self.storage.delete_entity(id)? {
                total_deleted += 1;
            }
        }
        
        // Phase 2: Delete orphaned entities (no memory_entities links)
        let orphans: Vec<String> = {
            let conn = self.storage.connection();
            let mut stmt = conn.prepare(
                "SELECT e.id FROM entities e
                 LEFT JOIN memory_entities me ON e.id = me.entity_id
                 WHERE me.entity_id IS NULL"
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        
        for id in &orphans {
            if self.storage.delete_entity(id)? {
                total_deleted += 1;
            }
        }
        
        if total_deleted > 0 {
            log::info!("Purged {} garbage entities ({} false-positive persons, {} orphans)",
                total_deleted, garbage_persons.len(), orphans.len());
        }
        
        Ok(total_deleted)
    }

    /// List entities, optionally filtered by type and namespace.
    /// Returns (EntityRecord, mention_count) pairs ordered by mention count descending.
    pub fn list_entities(
        &self,
        entity_type: Option<&str>,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(crate::storage::EntityRecord, usize)>, Box<dyn std::error::Error>> {
        Ok(self.storage.list_entities(entity_type, namespace, limit)?)
    }

    // ===================================================================
    // Knowledge Synthesis API (ISS-005)
    // ===================================================================

    /// Run a full synthesis cycle: discover clusters → gate check → generate insights → store.
    ///
    /// Requires `set_synthesis_settings()` to have been called with `enabled: true`.
    /// Without an LLM provider (`set_synthesis_llm_provider()`), synthesis still
    /// discovers clusters and runs gate checks but skips insight text generation
    /// (graceful degradation).
    pub fn synthesize(
        &mut self,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        let settings = self.synthesis_settings.clone().unwrap_or_default();
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.synthesize(&mut self.storage, &settings);
        // Restore LLM provider (consumed by build_synthesis_engine via take())
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Run a full synthesis cycle with custom settings (overrides stored settings).
    pub fn synthesize_with(
        &mut self,
        settings: &crate::synthesis::types::SynthesisSettings,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.synthesize(&mut self.storage, settings);
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Dry-run synthesis: discover clusters and gate decisions without making any changes.
    ///
    /// Returns the report with clusters_found and gate_results populated,
    /// but insights_created and sources_demoted will be empty.
    pub fn synthesize_dry_run(
        &mut self,
    ) -> Result<crate::synthesis::types::SynthesisReport, Box<dyn std::error::Error>> {
        use crate::synthesis::{cluster, gate};
        let settings = self.synthesis_settings.clone().unwrap_or_default();
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());

        let clusters = cluster::discover_clusters(
            &self.storage,
            &settings.cluster_discovery,
            embedding_model.as_deref(),
        )?;

        let mut gate_results = Vec::new();

        // Pre-load all memories ONCE and build a HashMap index for O(1) lookups.
        // Previously storage.all() was called inside the per-cluster loop — O(C×N).
        let all_memories = self.storage.all()?;
        let memory_index: std::collections::HashMap<String, MemoryRecord> = all_memories
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();

        for cluster_data in &clusters {
            let members: Vec<MemoryRecord> = cluster_data
                .members
                .iter()
                .filter_map(|id| memory_index.get(id).cloned())
                .collect();

            let covered_pct = self.storage.check_coverage(&cluster_data.members)?;
            let gate_result = gate::check_gate(
                cluster_data,
                &members,
                &settings.gate,
                covered_pct,
                true,  // assume changed
                false, // not all pairs similar
            );
            gate_results.push(gate_result);
        }

        Ok(crate::synthesis::types::SynthesisReport {
            clusters_found: clusters.len(),
            clusters_synthesized: 0,
            clusters_auto_updated: 0,
            clusters_deferred: gate_results.iter().filter(|g| matches!(g.decision, crate::synthesis::types::GateDecision::Defer { .. })).count(),
            clusters_skipped: gate_results.iter().filter(|g| matches!(g.decision, crate::synthesis::types::GateDecision::Skip { .. })).count(),
            synthesis_runs_full: 0,
            synthesis_runs_incremental: 0,
            insights_created: Vec::new(),
            sources_demoted: Vec::new(),
            errors: Vec::new(),
            duration: std::time::Duration::ZERO,
            gate_results,
        })
    }

    /// Unified sleep cycle: consolidate, synthesize, decay, forget, rebalance.
    ///
    /// This is the recommended way to run the full memory maintenance pipeline.
    /// Consolidation always runs; synthesis only runs if enabled via settings.
    pub fn sleep_cycle(
        &mut self,
        days: f64,
        namespace: Option<&str>,
    ) -> Result<SleepReport, Box<dyn std::error::Error>> {
        use std::time::Instant;
        let cycle_start = Instant::now();
        let mut phases = Vec::new();

        // Phase 1: Synaptic consolidation (existing)
        let t = Instant::now();
        self.consolidate_namespace(days, namespace)?;
        phases.push(PhaseReport {
            name: "consolidate".to_string(),
            duration_ms: t.elapsed().as_millis() as u64,
            count: 0,
        });

        // Phase 2: Knowledge synthesis (if enabled, now incremental via C4)
        let t = Instant::now();
        let synthesis = if self.synthesis_settings.as_ref().is_some_and(|s| s.enabled) {
            match self.synthesize() {
                Ok(report) => {
                    phases.push(PhaseReport {
                        name: "synthesis".to_string(),
                        duration_ms: t.elapsed().as_millis() as u64,
                        count: report.insights_created.len(),
                    });
                    Some(report)
                }
                Err(e) => {
                    log::warn!("Synthesis in sleep cycle failed (non-fatal): {e}");
                    phases.push(PhaseReport {
                        name: "synthesis".to_string(),
                        duration_ms: t.elapsed().as_millis() as u64,
                        count: 0,
                    });
                    None
                }
            }
        } else {
            None
        };

        // Phase 3: Decay check (C5) — flag weak memories
        let t = Instant::now();
        let decay = self.check_decay_and_flag(namespace)?;
        phases.push(PhaseReport {
            name: "decay".to_string(),
            duration_ms: t.elapsed().as_millis() as u64,
            count: decay.flagged_for_forget,
        });

        // Phase 4: Forget (C6) — soft-delete + hard-delete old
        let t = Instant::now();
        let forget = self.forget_bulk()?;
        phases.push(PhaseReport {
            name: "forget".to_string(),
            duration_ms: t.elapsed().as_millis() as u64,
            count: forget.soft_deleted + forget.hard_deleted,
        });

        // Phase 5: Rebalance (C9) — repair integrity
        let t = Instant::now();
        let rebalance = self.rebalance_internal()?;
        phases.push(PhaseReport {
            name: "rebalance".to_string(),
            duration_ms: t.elapsed().as_millis() as u64,
            count: rebalance.repairs,
        });

        // Reset per-cycle counters
        self.dedup_merge_count = 0;
        self.dedup_write_count = 0;
        self.last_add_result = None;

        // Meta-cognition: record synthesis event
        if let Some(ref mut tracker) = self.metacognition {
            let synth_ref = &synthesis;
            let event = crate::metacognition::SynthesisEvent {
                timestamp: chrono::Utc::now().timestamp(),
                clusters_found: synth_ref.as_ref().map(|s| s.clusters_found).unwrap_or(0),
                insights_created: synth_ref.as_ref().map(|s| s.insights_created.len()).unwrap_or(0),
                duration_ms: cycle_start.elapsed().as_millis() as u64,
                error_count: synth_ref.as_ref().map(|s| s.errors.len()).unwrap_or(0),
            };
            if let Err(e) = tracker.record_synthesis(self.storage.conn(), event) {
                log::warn!("Metacognition synthesis record failed: {}", e);
            }
        }

        Ok(SleepReport {
            consolidation_ok: true,
            synthesis,
            phases,
            decay: Some(decay),
            forget: Some(forget),
            rebalance: Some(rebalance),
            duration_ms: cycle_start.elapsed().as_millis() as u64,
        })
    }

    /// List all insight memories (memories with `is_synthesis: true` metadata).
    ///
    /// Returns insight MemoryRecords sorted by creation time (newest first).
    pub fn list_insights(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<MemoryRecord>, Box<dyn std::error::Error>> {
        let all = self.storage.all()?;
        let mut insights: Vec<MemoryRecord> = all
            .into_iter()
            .filter(is_insight)
            .collect();
        insights.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        if let Some(l) = limit {
            insights.truncate(l);
        }
        Ok(insights)
    }

    /// Get source provenance records for a given insight.
    pub fn insight_sources(
        &self,
        insight_id: &str,
    ) -> Result<Vec<crate::synthesis::types::ProvenanceRecord>, Box<dyn std::error::Error>> {
        self.storage.get_insight_sources(insight_id)
    }

    /// Get insights derived from a specific source memory.
    pub fn insights_for_memory(
        &self,
        memory_id: &str,
    ) -> Result<Vec<crate::synthesis::types::ProvenanceRecord>, Box<dyn std::error::Error>> {
        self.storage.get_memory_insights(memory_id)
    }

    /// Reverse a synthesis: archive the insight and restore source importances.
    ///
    /// Does NOT delete the insight (GUARD-1: No Data Loss). Instead, archives it
    /// with importance 0.0 and restores all source memories to their pre-demotion state.
    pub fn reverse_synthesis(
        &mut self,
        insight_id: &str,
    ) -> Result<crate::synthesis::types::UndoSynthesis, Box<dyn std::error::Error>> {
        let embedding_model = self.embedding.as_ref().map(|e| e.config().model_id());
        let engine = self.build_synthesis_engine(embedding_model);
        let result = engine.undo_synthesis(&mut self.storage, insight_id);
        self.restore_llm_provider(engine.into_provider());
        result
    }

    /// Trace the provenance chain of a memory/insight back through its sources.
    pub fn get_provenance(
        &self,
        memory_id: &str,
        max_depth: usize,
    ) -> Result<crate::synthesis::types::ProvenanceChain, Box<dyn std::error::Error>> {
        // get_provenance only needs &Storage, no need to take LLM provider
        crate::synthesis::provenance::get_provenance_chain(&self.storage, memory_id, max_depth)
    }

    /// Build a DefaultSynthesisEngine, temporarily borrowing the LLM provider.
    ///
    /// Uses `Option::take` to move the provider into the engine. Callers must
    /// call `restore_llm_provider` after using the engine to put it back.
    fn build_synthesis_engine(
        &mut self,
        embedding_model: Option<String>,
    ) -> crate::synthesis::engine::DefaultSynthesisEngine {
        let llm = self.synthesis_llm_provider.take();
        crate::synthesis::engine::DefaultSynthesisEngine::new(llm, embedding_model)
    }

    /// Restore the LLM provider after engine use (engine returns it via `into_provider()`).
    fn restore_llm_provider(&mut self, provider: Option<Box<dyn crate::synthesis::types::SynthesisLlmProvider>>) {
        self.synthesis_llm_provider = provider;
    }

}

/// Best-effort graceful shutdown of the resolution pipeline pool when
/// `Memory` is dropped without an explicit [`Memory::shutdown_pipeline`]
/// call (ISS-037 Step 4).
///
/// Uses a small fixed deadline (1s) — long enough for in-flight jobs to
/// finish their current commit, short enough not to block process exit.
/// Production code should prefer explicit `shutdown_pipeline` so failures
/// surface as `Result` rather than being swallowed in the destructor.
impl Drop for Memory {
    fn drop(&mut self) {
        if self.pipeline_pool.is_some() {
            // Ignore errors — Drop cannot return them. The explicit
            // `shutdown_pipeline` API exists for callers that need
            // structured errors.
            let _ = self.shutdown_pipeline(std::time::Duration::from_secs(1));
        }
    }
}

/// Compute confidence score (0.0-1.0) for a recall result.
///
/// Unlike the ranking score (combined_score), confidence measures how certain
/// we are that this memory is genuinely relevant to the query.
///
/// Signals:
/// - embedding_similarity: cosine sim of query vs memory embedding (strongest signal)
/// - in_fts_results: whether FTS found this memory (keyword overlap = confidence boost)
/// - entity_score: entity overlap score 0-1 (topical relevance)
/// - age_hours: memory age in hours (mild recency boost for very recent memories)
fn compute_query_confidence(
    embedding_similarity: Option<f32>,  // None if no embedding available
    in_fts_results: bool,
    entity_score: f64,  // 0.0 if no entity match or entity disabled
    age_hours: f64,
) -> f64 {
    let mut confidence = 0.0;
    let mut max_possible = 0.0;

    // Signal 1: Embedding similarity (weight: 0.55)
    if let Some(sim) = embedding_similarity {
        let sim = sim as f64;
        // Apply sigmoid-like curve to sharpen discrimination
        // sim > 0.7 → strong confidence, sim < 0.3 → near zero
        let emb_conf = if sim > 0.0 {
            // Logistic function centered at 0.5 with steepness 10
            1.0 / (1.0 + (-10.0 * (sim - 0.5)).exp())
        } else {
            0.0
        };
        confidence += 0.55 * emb_conf;
        max_possible += 0.55;
    }

    // Signal 2: FTS match (weight: 0.20)
    if in_fts_results {
        confidence += 0.20;
    }
    max_possible += 0.20;

    // Signal 3: Entity overlap (weight: 0.20)
    confidence += 0.20 * entity_score;
    max_possible += 0.20;

    // Signal 4: Recency boost (weight: 0.05)
    // Very recent memories get a small confidence boost
    // 1 hour ago → ~1.0, 24 hours → ~0.5, 7 days → ~0.1
    let recency = 1.0 / (1.0 + (age_hours / 24.0).ln().max(0.0));
    confidence += 0.05 * recency.clamp(0.0, 1.0);
    max_possible += 0.05;

    // Normalize by max possible (handles case where embedding is unavailable)
    if max_possible > 0.0 {
        (confidence / max_possible).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn confidence_label(confidence: f64) -> String {
    match confidence {
        c if c >= 0.8 => "high".to_string(),
        c if c >= 0.5 => "medium".to_string(),
        c if c >= 0.2 => "low".to_string(),
        _ => "very low".to_string(),
    }
}

fn detect_feedback_polarity(feedback: &str) -> f64 {
    let lower = feedback.to_lowercase();
    let positive = ["good", "great", "excellent", "correct", "right", "yes", "nice", "perfect"];
    let negative = ["bad", "wrong", "incorrect", "no", "error", "mistake", "poor"];

    let pos_count = positive.iter().filter(|&w| lower.contains(w)).count();
    let neg_count = negative.iter().filter(|&w| lower.contains(w)).count();

    if pos_count > neg_count {
        1.0
    } else if neg_count > pos_count {
        -1.0
    } else {
        0.0
    }
}

// =========================================================================
// ISS-019 Step 4 helpers (module-level — re-used by the shim layer).
// =========================================================================

/// Build the v2 namespaced `metadata` JSON blob from a validated
/// `EnrichedMemory`.
///
/// v2 on-disk layout (ISS-019 Step 7a):
/// ```json
/// {
///   "engram": {
///     "version": 2,
///     "dimensions": { ..., "type_weights": { ... } },
///     "merge_count": 0,
///     "merge_history": []
///   },
///   "user": { /* caller-supplied keys */ }
/// }
/// ```
///
/// Reads remain backward-compatible with the v1 flat layout via
/// `Dimensions::from_stored_metadata`. Step 7b will introduce the
/// explicit backfill job for pre-v2 rows.
fn build_legacy_metadata(mem: &crate::enriched::EnrichedMemory) -> serde_json::Value {
    let d = &mem.dimensions;

    // Serialize Dimensions directly — this produces the complete
    // dimensional object including nested type_weights.
    let mut dims_val = serde_json::to_value(d).unwrap_or_else(|_| serde_json::json!({}));

    // Normalize domain to the legacy loose-str representation so
    // downstream consumers (KC, clustering) keep working unchanged.
    if let Some(obj) = dims_val.as_object_mut() {
        obj.insert(
            "domain".into(),
            serde_json::Value::String(domain_to_loose_str(&d.domain)),
        );
        // Drop tags key if empty to match prior behavior (no empty arrays).
        if d.tags.is_empty() {
            obj.remove("tags");
        }
    }

    let engram = serde_json::json!({
        "version": 2,
        "dimensions": dims_val,
        "merge_count": 0,
        "merge_history": [],
    });

    let user = if let serde_json::Value::Object(_) = &mem.user_metadata {
        mem.user_metadata.clone()
    } else {
        serde_json::json!({})
    };

    serde_json::json!({
        "engram": engram,
        "user": user,
    })
}

fn domain_to_loose_str(d: &crate::dimensions::Domain) -> String {
    match d {
        crate::dimensions::Domain::Coding => "coding".into(),
        crate::dimensions::Domain::Trading => "trading".into(),
        crate::dimensions::Domain::Research => "research".into(),
        crate::dimensions::Domain::Communication => "communication".into(),
        crate::dimensions::Domain::General => "general".into(),
        crate::dimensions::Domain::Other(s) => s.clone(),
    }
}

/// Map `add_raw`'s `Box<dyn Error>` into the typed `StoreError`.
///
/// `add_raw` today returns a boxed trait object, which is too loose for
/// the new API. The only error kind we actually expect through this
/// path is a `rusqlite::Error` (DB write failure); anything else is
/// treated as a pipeline error and surfaces as `InvalidState`.
fn boxed_err_to_store_error(
    e: Box<dyn std::error::Error>,
) -> crate::store_api::StoreError {
    // Attempt to downcast to rusqlite::Error first — the common case.
    match e.downcast::<rusqlite::Error>() {
        Ok(db_err) => crate::store_api::StoreError::DbError(*db_err),
        Err(other) => crate::store_api::StoreError::InvalidState(other.to_string()),
    }
}

/// Short, stable hex digest of content (first 16 hex chars of SHA-256).
/// Used for skip / quarantine content_hash; cheap, deterministic, not
/// cryptographically strong (not a security boundary here).
fn short_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Build a `TypeWeights` that favors the given `MemoryType`.
///
/// Used by the `store_raw` no-extractor branch (FINDING-4 / legacy
/// shim) so an explicit `memory_type_hint` survives round-tripping
/// through `Dimensions::type_weights.primary_type()`. Without this
/// helper, a minimal `TypeWeights::default()` (all 1.0) would
/// degenerate `primary_type()` to a tie-break winner (factual) and
/// silently discard the caller's explicit classification.
fn type_weights_favoring(mt: crate::types::MemoryType) -> crate::type_weights::TypeWeights {
    use crate::types::MemoryType;
    // Baseline 1.0 preserves neutral recall behavior for other types;
    // the hinted type gets 2.0 so `primary_type()` picks it unambiguously.
    let mut w = crate::type_weights::TypeWeights::default();
    let favored = 2.0_f64;
    match mt {
        MemoryType::Factual => w.factual = favored,
        MemoryType::Episodic => w.episodic = favored,
        MemoryType::Procedural => w.procedural = favored,
        MemoryType::Relational => w.relational = favored,
        MemoryType::Emotional => w.emotional = favored,
        MemoryType::Opinion => w.opinion = favored,
        MemoryType::Causal => w.causal = favored,
    }
    w
}

#[cfg(test)]
#[allow(deprecated, clippy::field_reassign_with_default, clippy::manual_range_contains, clippy::cloned_ref_to_slice_refs, clippy::items_after_test_module)]
mod confidence_tests {
    use super::*;

    #[test]
    fn test_confidence_high_embedding_sim() {
        // High embedding sim + FTS + entity → high confidence
        let c = compute_query_confidence(Some(0.85), true, 0.8, 1.0);
        assert!(c > 0.8, "expected high confidence, got {c}");
        assert_eq!(confidence_label(c), "high");
    }

    #[test]
    fn test_confidence_low_embedding_sim() {
        // Low embedding sim, no FTS, no entity → low confidence
        let c = compute_query_confidence(Some(0.2), false, 0.0, 720.0);
        assert!(c < 0.3, "expected low confidence, got {c}");
    }

    #[test]
    fn test_confidence_medium_embedding_sim() {
        // Medium embedding sim + FTS → medium range
        let c = compute_query_confidence(Some(0.55), true, 0.0, 24.0);
        assert!(c > 0.3 && c < 0.8, "expected medium confidence, got {c}");
    }

    #[test]
    fn test_confidence_no_embedding() {
        // No embedding available, FTS=true, some entity match
        let c = compute_query_confidence(None, true, 0.5, 2.0);
        // Without embedding, max_possible = 0.45, confidence comes from FTS + entity + recency
        assert!(c > 0.3, "expected reasonable confidence without embedding, got {c}");
        assert!(c < 1.0);
    }

    #[test]
    fn test_confidence_fts_only_boost() {
        // No embedding, FTS only, no entity
        let c_fts = compute_query_confidence(None, true, 0.0, 24.0);
        let c_none = compute_query_confidence(None, false, 0.0, 24.0);
        assert!(c_fts > c_none, "FTS should boost confidence: {c_fts} > {c_none}");
    }

    #[test]
    fn test_confidence_entity_boost() {
        // Same embedding sim, but different entity scores
        let c_entity = compute_query_confidence(Some(0.5), false, 0.9, 48.0);
        let c_no_entity = compute_query_confidence(Some(0.5), false, 0.0, 48.0);
        assert!(c_entity > c_no_entity, "entity should boost confidence: {c_entity} > {c_no_entity}");
    }

    #[test]
    fn test_confidence_recency_boost() {
        // Recent (1h) vs old (30 days = 720h) — same other signals
        let c_recent = compute_query_confidence(Some(0.6), true, 0.0, 1.0);
        let c_old = compute_query_confidence(Some(0.6), true, 0.0, 720.0);
        assert!(c_recent > c_old, "recent should have slightly higher confidence: {c_recent} > {c_old}");
        // But the difference should be small (recency weight is only 0.05)
        assert!((c_recent - c_old).abs() < 0.1, "recency difference should be small");
    }

    #[test]
    fn test_confidence_all_zero() {
        // All signals at worst values
        let c = compute_query_confidence(Some(0.0), false, 0.0, 8760.0); // 1 year old
        assert!(c < 0.1, "all-zero signals should give near-zero confidence, got {c}");
    }

    #[test]
    fn test_confidence_all_max() {
        // All signals at best values
        let c = compute_query_confidence(Some(0.99), true, 1.0, 0.1);
        assert!(c > 0.9, "all-max signals should give high confidence, got {c}");
    }

    #[test]
    fn test_confidence_label_thresholds() {
        assert_eq!(confidence_label(0.9), "high");
        assert_eq!(confidence_label(0.8), "high");
        assert_eq!(confidence_label(0.79), "medium");
        assert_eq!(confidence_label(0.5), "medium");
        assert_eq!(confidence_label(0.49), "low");
        assert_eq!(confidence_label(0.2), "low");
        assert_eq!(confidence_label(0.19), "very low");
        assert_eq!(confidence_label(0.0), "very low");
    }

    #[test]
    fn test_confidence_sigmoid_discrimination() {
        // The sigmoid should create clear separation between high/low sim
        let c_high = compute_query_confidence(Some(0.8), false, 0.0, 100.0);
        let c_low = compute_query_confidence(Some(0.3), false, 0.0, 100.0);
        let gap = c_high - c_low;
        assert!(gap > 0.3, "sigmoid should create large gap between 0.8 and 0.3 sim: gap={gap}");
    }

    #[test]
    fn test_auto_extract_importance_cap() {
        let mut config = MemoryConfig::default();
        config.auto_extract_importance_cap = 0.7;
        
        // Test capping logic directly
        let extracted_importance: f64 = 0.95;
        let capped = extracted_importance.min(config.auto_extract_importance_cap);
        assert_eq!(capped, 0.7);
        
        // Below cap — no change
        let low_importance: f64 = 0.3;
        let not_capped = low_importance.min(config.auto_extract_importance_cap);
        assert_eq!(not_capped, 0.3);
        
        // Exactly at cap — no change
        let at_cap: f64 = 0.7;
        let stays = at_cap.min(config.auto_extract_importance_cap);
        assert_eq!(stays, 0.7);
    }

    #[test]
    fn test_auto_extract_importance_cap_default() {
        let config = MemoryConfig::default();
        assert_eq!(config.auto_extract_importance_cap, 0.7);
    }

    #[test]
    fn test_dedup_recall_results_by_embedding() {
        // Create mock embeddings - two near-identical and one different
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.99, 0.1, 0.0]; // Very similar to A
        let emb_c: Vec<f32> = vec![0.0, 1.0, 0.0]; // Different

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
            superseded_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.8), // Near-dup of A
            make_result("id-c", 0.7),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85, // threshold
            3,    // limit
        );

        // Should keep A and C, skip B (too similar to A)
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-c");
    }

    #[test]
    fn test_dedup_recall_no_duplicates() {
        // All embeddings are orthogonal — nothing should be deduped
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.0, 1.0, 0.0];
        let emb_c: Vec<f32> = vec![0.0, 0.0, 1.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
            superseded_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.8),
            make_result("id-c", 0.7),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            3,
        );

        // All three should be kept
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_dedup_recall_respects_limit() {
        // No dups, but limit is 2
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.0, 1.0, 0.0];
        let emb_c: Vec<f32> = vec![0.0, 0.0, 1.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
            superseded_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence: 0.8,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a"),
            make_result("id-b"),
            make_result("id-c"),
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            2, // limit of 2
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-b");
    }

    #[test]
    fn test_dedup_recall_missing_embedding_kept() {
        // If a candidate has no embedding in the lookup, it should be kept
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        // id-b has no embedding

        let make_result = |id: &str| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
            superseded_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence: 0.8,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a"),
            make_result("id-b"), // No embedding
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            5,
        );

        // Both should be kept (id-b has no embedding, so can't be deduped)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_dedup_recall_backfills_from_candidates() {
        // A, B (dup of A), C (different) — with limit=2, should get A and C
        let emb_a: Vec<f32> = vec![1.0, 0.0, 0.0];
        let emb_b: Vec<f32> = vec![0.98, 0.05, 0.0]; // Near-duplicate of A
        let emb_c: Vec<f32> = vec![0.0, 1.0, 0.0]; // Different

        let mut embeddings_map: HashMap<&str, &Vec<f32>> = HashMap::new();
        embeddings_map.insert("id-a", &emb_a);
        embeddings_map.insert("id-b", &emb_b);
        embeddings_map.insert("id-c", &emb_c);

        let make_result = |id: &str, confidence: f64| RecallResult {
            record: MemoryRecord {
                id: id.to_string(),
                content: format!("content-{}", id),
                memory_type: MemoryType::Factual,
                layer: MemoryLayer::Working,
                created_at: Utc::now(),
                access_times: vec![Utc::now()],
                working_strength: 1.0,
                core_strength: 0.0,
                importance: 0.5,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: "test".to_string(),
                contradicts: None,
                contradicted_by: None,
            superseded_by: None,
                metadata: None,
            },
            activation: 0.5,
            confidence,
            confidence_label: "high".to_string(),
        };

        let candidates = vec![
            make_result("id-a", 0.9),
            make_result("id-b", 0.85), // Dup of A, would normally be #2
            make_result("id-c", 0.7),  // #3 backfills into slot #2
        ];

        let result = Memory::dedup_recall_results_by_embedding(
            candidates,
            &embeddings_map,
            0.85,
            2, // limit=2, B gets deduped, C backfills
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].record.id, "id-a");
        assert_eq!(result[1].record.id, "id-c"); // Backfilled
    }

    #[test]
    fn test_recall_dedup_config_defaults() {
        let config = MemoryConfig::default();
        assert!(config.recall_dedup_enabled);
        assert!((config.recall_dedup_threshold - 0.85).abs() < f64::EPSILON);
    }

    // ── C7 Multi-Retrieval Fusion tests ────────────────────────────

    fn make_test_record(id: &str, content: &str, created_at: chrono::DateTime<Utc>) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: crate::types::MemoryLayer::Working,
            created_at,
            access_times: vec![created_at],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn test_temporal_score_within_range() {
        use crate::query_classifier::TimeRange;
        let now = Utc::now();
        
        // Memory at the center of a 24-hour range
        let range = TimeRange {
            start: now - chrono::Duration::hours(24),
            end: now,
        };
        
        let record = make_test_record("t1", "test", now - chrono::Duration::hours(12));
        
        let score = Memory::temporal_score(&record, &Some(range), now);
        assert!(score > 0.9, "Center of range should score high: {}", score);
    }

    #[test]
    fn test_temporal_score_outside_range() {
        use crate::query_classifier::TimeRange;
        let now = Utc::now();
        
        let range = TimeRange {
            start: now - chrono::Duration::hours(24),
            end: now - chrono::Duration::hours(12),
        };
        
        let record = make_test_record("t2", "test", now - chrono::Duration::hours(1));
        
        let score = Memory::temporal_score(&record, &Some(range), now);
        assert!(score < 0.01, "Outside range should score ~0: {}", score);
    }

    #[test]
    fn test_temporal_score_no_range() {
        let now = Utc::now();
        
        let record = make_test_record("t3", "test", now - chrono::Duration::hours(1));
        
        let score = Memory::temporal_score(&record, &None, now);
        // Should be in neutral range (0.25-0.75)
        assert!(score >= 0.25 && score <= 0.75,
            "No range: score should be neutral-ish: {}", score);
    }

    #[test]
    fn test_hebbian_channel_scores_basic() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let mut storage = crate::storage::Storage::new(db.to_str().unwrap()).unwrap();
        
        let now = Utc::now();
        let rec_a = make_test_record("a", "memory A", now);
        let rec_b = make_test_record("b", "memory B", now);
        let rec_c = make_test_record("c", "memory C", now);
        storage.add(&rec_a, "default").unwrap();
        storage.add(&rec_b, "default").unwrap();
        storage.add(&rec_c, "default").unwrap();
        
        // Create Hebbian link between A and B
        // First call creates tracking record, second call forms the link (threshold=1)
        storage.record_coactivation("a", "b", 1).unwrap();
        storage.record_coactivation("a", "b", 1).unwrap();
        
        let candidate_ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let scores = Memory::hebbian_channel_scores(&storage, &candidate_ids).unwrap();
        
        // A and B should have scores (linked to each other), C should not
        assert!(scores.get("a").copied().unwrap_or(0.0) > 0.0, "A should have hebbian score");
        assert!(scores.get("b").copied().unwrap_or(0.0) > 0.0, "B should have hebbian score");
        assert!(scores.get("c").copied().unwrap_or(0.0) < 0.01, "C should have no hebbian score");
    }

    #[test]
    fn test_c7_config_defaults() {
        let config = MemoryConfig::default();
        assert!((config.temporal_weight - 0.10).abs() < f64::EPSILON);
        assert!((config.hebbian_recall_weight - 0.10).abs() < f64::EPSILON);
        assert!(config.adaptive_weights);
    }

    #[test]
    fn test_adaptive_weights_disabled_preserves_behavior() {
        // When adaptive_weights = false, query classifier should return neutral
        let analysis = crate::query_classifier::QueryAnalysis::neutral();
        assert_eq!(analysis.weight_modifiers.fts, 1.0);
        assert_eq!(analysis.weight_modifiers.embedding, 1.0);
        assert_eq!(analysis.weight_modifiers.actr, 1.0);
        assert_eq!(analysis.weight_modifiers.temporal, 1.0);
        assert_eq!(analysis.weight_modifiers.hebbian, 1.0);
    }

    // ── GWT Broadcast Tests ───────────────────────────────────────

    #[test]
    fn test_broadcast_admission_generates_confidence_signals() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = ActiveContext::default();

        // Store a memory so we have something to broadcast.
        let id = mem
            .add("Rust is a systems programming language", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();

        // Hub should be empty before broadcast.
        assert_eq!(mem.interoceptive_hub().buffer_len(), 0);

        // Broadcast the memory admission.
        mem.broadcast_admission(&[id], &mut wm);

        // Hub should now have at least one signal (confidence).
        // No emotional bus → no alignment signal, but confidence is always generated.
        assert!(
            mem.interoceptive_hub().buffer_len() >= 1,
            "expected ≥1 signal in hub, got {}",
            mem.interoceptive_hub().buffer_len()
        );
    }

    #[test]
    fn test_broadcast_admission_multiple_memories() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = ActiveContext::default();

        let id1 = mem
            .add("First memory about coding", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();
        let id2 = mem
            .add("Second memory about trading", MemoryType::Factual, Some(0.6), None, None)
            .unwrap();
        let id3 = mem
            .add("Third memory about research", MemoryType::Factual, Some(0.7), None, None)
            .unwrap();

        mem.broadcast_admission(&[id1, id2, id3], &mut wm);

        // Each memory generates at least 1 confidence signal → ≥3.
        assert!(
            mem.interoceptive_hub().buffer_len() >= 3,
            "expected ≥3 signals, got {}",
            mem.interoceptive_hub().buffer_len()
        );
    }

    #[test]
    fn test_broadcast_with_nonexistent_memory_is_safe() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = ActiveContext::default();

        // Broadcast a memory that doesn't exist — should not panic.
        let neighbors = mem.broadcast_admission(&["nonexistent-id".to_string()], &mut wm);
        assert!(neighbors.is_empty());
        assert_eq!(mem.interoceptive_hub().buffer_len(), 0);
    }

    #[test]
    fn test_broadcast_updates_hub_domain_state() {
        let mut mem = Memory::new(":memory:", None).unwrap();
        let mut wm = ActiveContext::default();

        // Store multiple memories and broadcast them.
        let id1 = mem
            .add("Important fact about Rust", MemoryType::Factual, Some(0.8), None, None)
            .unwrap();
        let id2 = mem
            .add("Another fact about memory", MemoryType::Factual, Some(0.3), None, None)
            .unwrap();

        mem.broadcast_admission(&[id1, id2], &mut wm);

        // Hub should have processed signals and have some state.
        let state = mem.interoceptive_snapshot();
        assert!(state.buffer_size > 0, "hub should have buffered signals");
    }

    #[test]
    fn test_broadcast_hebbian_spreading() {
        // Use raw storage to set up Hebbian links, then verify
        // broadcast_admission spreads activation.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("broadcast_hebb.db");
        let mut mem = Memory::new(db.to_str().unwrap(), None).unwrap();
        let mut wm = ActiveContext::default();

        // Store two memories.
        let id_a = mem
            .add("Memory A about Rust", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();
        let id_b = mem
            .add("Memory B about Rust compilers", MemoryType::Factual, Some(0.5), None, None)
            .unwrap();

        // Create a Hebbian link by simulating co-recall via the storage layer.
        // We need to strengthen it enough (weight > 0.1) for spreading to activate.
        // record_coactivation with threshold=1 → second call forms the link.
        {
            // Use recall to trigger co-activation recording (indirect approach).
            // Or directly use the underlying record_coactivation on Memory.
            // Since Memory doesn't expose &mut Storage, we'll use a workaround:
            // call recall with both IDs to trigger Hebbian learning.
            //
            // Actually, the cleanest approach: use rusqlite directly on the
            // connection to insert a Hebbian link for testing purposes.
            let conn = mem.connection();
            conn.execute(
                "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?1, ?2, 0.5, 5, ?3)",
                rusqlite::params![&id_a, &id_b, Utc::now().timestamp() as f64],
            ).unwrap();
        }

        // Now broadcast only memory A → should spread to B via Hebbian link.
        let neighbors = mem.broadcast_admission(&[id_a.clone()], &mut wm);

        // B should appear as a primed neighbor.
        assert!(
            neighbors.contains(&id_b),
            "expected id_b in neighbors, got {:?}",
            neighbors
        );

        // B should now be in working memory (primed by spreading activation).
        assert!(wm.contains(&id_b), "id_b should be in WM after spreading");
    }

    /// ISS-020 P0.0: valence, confidence, and domain must be persisted into
    /// `metadata.dimensions` on every `remember()` path that calls the extractor.
    ///
    /// This test simulates the serialization step in isolation — it mirrors the
    /// exact code inside `Memory::remember()` at src/memory.rs:~1325, but drives
    /// it with a hand-built ExtractedFact so we don't need a live LLM.
    ///
    /// The invariant: for ANY ExtractedFact, `metadata.dimensions` always
    /// contains `valence` (number), `confidence` (string), and `domain` (string).
    #[test]
    fn test_iss020_p0_0_dimensions_persist_valence_confidence_domain() {
        use crate::extractor::ExtractedFact;

        let fact = ExtractedFact {
            core_fact: "potato prefers Rust for systems work".into(),
            participants: Some("potato".into()),
            temporal: Some("2026-04-22".into()),
            location: None,
            context: None,
            causation: None,
            outcome: None,
            method: None,
            relations: None,
            sentiment: Some("positive".into()),
            stance: Some("prefers Rust over Go".into()),
            importance: 0.7,
            tags: vec!["coding".into()],
            confidence: "confident".into(),
            valence: 0.6,
            domain: "coding".into(),
        };

        // Mirror the exact write-path logic from Memory::remember().
        let mut dimensions = serde_json::Map::new();
        if let Some(ref v) = fact.participants {
            dimensions.insert(
                "participants".into(),
                serde_json::Value::String(v.clone()),
            );
        }
        if let Some(ref v) = fact.temporal {
            dimensions.insert("temporal".into(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = fact.sentiment {
            dimensions.insert("sentiment".into(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = fact.stance {
            dimensions.insert("stance".into(), serde_json::Value::String(v.clone()));
        }
        // P0.0 additions:
        dimensions.insert("valence".into(), serde_json::json!(fact.valence));
        dimensions.insert(
            "confidence".into(),
            serde_json::Value::String(fact.confidence.clone()),
        );
        dimensions.insert(
            "domain".into(),
            serde_json::Value::String(fact.domain.clone()),
        );

        // Serialize → deserialize (SQLite stores as JSON text; this is the round-trip).
        let json = serde_json::Value::Object(dimensions);
        let serialized = serde_json::to_string(&json).expect("serialize");
        let roundtripped: serde_json::Value =
            serde_json::from_str(&serialized).expect("deserialize");

        // valence: must be present as a number in [-1.0, 1.0].
        let v = roundtripped
            .get("valence")
            .expect("valence must be persisted");
        let v_num = v.as_f64().expect("valence must be a number");
        assert!(
            (-1.0..=1.0).contains(&v_num),
            "valence {v_num} out of range [-1, 1]"
        );
        assert!((v_num - 0.6).abs() < 1e-9, "valence round-trip mismatch");

        // confidence: must be a string in {"confident", "likely", "uncertain"}.
        let c = roundtripped
            .get("confidence")
            .and_then(|v| v.as_str())
            .expect("confidence must be persisted as string");
        assert!(
            matches!(c, "confident" | "likely" | "uncertain"),
            "confidence {c} not a recognized variant"
        );

        // domain: must be a non-empty string.
        let d = roundtripped
            .get("domain")
            .and_then(|v| v.as_str())
            .expect("domain must be persisted as string");
        assert!(!d.is_empty(), "domain must not be empty");
        assert_eq!(d, "coding");
    }

    /// P0.0 edge case: extractor may produce valence = 0.0 (neutral) — still persist.
    #[test]
    fn test_iss020_p0_0_neutral_valence_still_persisted() {
        use crate::extractor::ExtractedFact;

        let fact = ExtractedFact {
            core_fact: "neutral fact".into(),
            confidence: "likely".into(),
            valence: 0.0,
            domain: "general".into(),
            ..Default::default()
        };

        let mut dimensions = serde_json::Map::new();
        dimensions.insert("valence".into(), serde_json::json!(fact.valence));
        dimensions.insert(
            "confidence".into(),
            serde_json::Value::String(fact.confidence.clone()),
        );
        dimensions.insert(
            "domain".into(),
            serde_json::Value::String(fact.domain.clone()),
        );

        let json = serde_json::Value::Object(dimensions);
        assert_eq!(json["valence"].as_f64(), Some(0.0));
        assert_eq!(json["confidence"].as_str(), Some("likely"));
        assert_eq!(json["domain"].as_str(), Some("general"));
    }
}

/// Jaccard similarity between two string sets.
fn jaccard_similarity_strings(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() { return 0.0; }
    let set_a: std::collections::HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: std::collections::HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}
