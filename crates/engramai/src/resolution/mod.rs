//! # v0.3 Resolution Pipeline
//!
//! Write-path pipeline that converts an ingested `MemoryRecord` into a
//! populated slice of the v0.3 semantic graph (canonical `Entity` nodes and
//! typed bi-temporal `Edge` rows), atomically, with structured failure
//! surfacing.
//!
//! See `.gid/features/v03-resolution/design.md` for the full spec.
//!
//! ## Stages
//!
//! ```text
//! MemoryRecord (raw)
//!        │
//!        ▼
//! ┌──────────────────┐
//! │ 3.1 Ingestion    │  v0.2-compat: store_raw returns here (sync path)
//! │ memory row draft │
//! └────────┬─────────┘
//!          │ enqueue for background
//!          ▼
//! ┌──────────────────┐
//! │ 3.2 Entity extract│  reuses EntityExtractor; emits ExtractedEntity[]
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │ 3.3 Edge extract  │  reuses TripleExtractor; emits Triple[]
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │ 3.4 Resolve / dedup
//! │  a. candidate retrieve
//! │  b. multi-signal fusion (s1–s8)
//! │  c. entity decision
//! │  d. edge decision (ADD / UPDATE / NONE)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │ 3.5 Persist       │  one SQLite transaction: memory+entities+edges
//! └────────┬─────────┘
//!          │
//!          ▼
//!    ResolutionTrace (persisted + observable, §7)
//! ```
//!
//! ## Implementation status (incremental)
//!
//! - [x] adapters: v0.2 → v0.3 type mapping (this file's `adapters` module)
//! - [x] context: `PipelineContext`, `DraftEntity`, `DraftEdge`, `StageFailure`
//! - [x] §3.4.2 multi-signal fusion (signals s1–s8 + proportional weight
//!   redistribution)
//! - [x] §3.4.3 entity decision (MergeInto / DeferToLlm / CreateNew)
//! - [x] resolution trace (per-candidate score breakdown for §7)
//! - [x] §3.4.1 candidate retrieval driver (`candidate_retrieval` module —
//!   bridges `GraphStore::search_candidates` to fusion `Measurement`s)
//! - [~] §3.1 ingestion driver — Steps A, B, C done. Step A: `PipelineJob`
//!   + `JobQueue` trait + `BoundedJobQueue` (in-memory bounded FIFO,
//!   non-droppable `ReExtract` per §5.2). Step B: `extraction_status`
//!   reader (`GraphStore::latest_pipeline_run_for_memory` + indexed
//!   `memory_id` / `episode_id` columns on `graph_pipeline_runs`).
//!   Step C: `ExtractionStatus` enum + `Memory::store_raw` enqueue
//!   hook (this commit). Pending: Step C-bis (`Pending` RunStatus
//!   variant for queue-full tracking) and Step D (worker pool).
//! - [x] §3.2/§3.3 stage drivers (`stage_extract`, `stage_edge_extract`)
//! - [x] §3.4.4 edge decision (ADD / UPDATE / Preserve / Supersede)
//! - [x] §3.5 atomic persist (pure `build_delta` + `drive_persist`
//!   wired to `GraphStore::apply_graph_delta` via the narrow `ApplyDelta`
//!   trait — production impl awaits v03-graph-layer Phase 4)
//! - [ ] §4 preserve-plus-resynthesize (pending)
//!
//! ## Boundary rules
//!
//! - This module is a **caller** of `crate::graph::GraphStore`. It never
//!   writes to graph tables directly via SQL.
//! - Adapter functions (v0.2 → v0.3 type maps) are pure: no IO, no panics,
//!   total mappings. Subtype loss is documented in the mapping tables.
//! - Stage functions take `&mut PipelineContext` and either advance it or
//!   record a `StageFailure`. They do not return early via panic.
//! - Fusion / decision modules are **pure arithmetic**: testable without a
//!   database, deterministic, no IO. Real candidate retrieval and embedding
//!   fetch live in the (future) driver layer that calls into them.

pub mod adapters;
pub mod candidate_retrieval;
pub mod context;
pub mod decision;
pub mod edge_decision;
pub mod fusion;
pub mod pipeline;
pub mod queue;
pub mod signals;
pub mod stage_edge_extract;
pub mod stage_extract;
pub mod stage_persist;
pub mod stats;
pub mod status;
pub mod trace;
pub mod worker;

pub use adapters::{
    draft_entity_from_mention, map_entity_kind, map_predicate, normalize_predicate_str,
};
pub use candidate_retrieval::{retrieve_candidates, RetrievalParams, ScoredCandidate};
pub use context::{DraftEdge, DraftEntity, PipelineContext, PipelineStage, StageFailure};
pub use decision::{decide, Decision, DecisionThresholds, ResolutionOutcome};
pub use fusion::{fuse, FusionResult, Measurement, SignalWeights};
pub use queue::{BoundedJobQueue, EnqueueError, JobMode, JobQueue, PipelineJob};
pub use signals::{
    affective_continuity, cooccurrence, graph_context, identity_hint, name_match, recency,
    semantic_similarity, somatic_match, Signal, DEFAULT_RECENCY_HALF_LIFE,
};
pub use stage_persist::{
    build_delta, drive_persist, ApplyDelta, EdgeResolution, EntityResolution, PersistOutcome,
};
pub use stats::{
    ConfigError as ResolutionConfigError, ResolutionConfig, ResolutionStats, DEFAULT_QUEUE_CAP,
    DEFAULT_SHUTDOWN_DRAIN, DEFAULT_WORKER_COUNT, DEFAULT_WORKER_IDLE_POLL, MAX_WORKER_COUNT,
};
pub use status::{ExtractionStatus, FailureKind, ResolutionTraceSummary};
pub use trace::{CandidateScore, SignalContribution};
