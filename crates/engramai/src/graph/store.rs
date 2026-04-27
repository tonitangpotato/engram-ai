//! v0.3 GraphStore trait + SqliteGraphStore impl. See design §4.2.
//!
//! ## Module organization
//!
//! - **Trait `GraphStore`** — abstract CRUD/query surface for the graph
//!   layer; allows test stubs and future alternative backends.
//! - **`SqliteGraphStore<'a>`** — production impl over `&'a mut
//!   rusqlite::Connection` borrowed from `Storage`. Constructed by
//!   `Storage::graph()` (see `crate::storage_graph`).
//! - **Helper types** — `MergeReport`, `EntityMentions`,
//!   `ProposedPredicateStats` (defined here; trait-public).
//!
//! ## Deviations from §4.2
//!
//! 1. **Time types: `chrono::DateTime<Utc>` at the API; `REAL` (unix
//!    seconds, f64) on disk.** §4.2's pseudo-Rust used `f64` for `at`
//!    parameters; the canonical Rust types (`Entity`, `Edge`, audit) use
//!    `DateTime<Utc>`. The SQLite schema (§4.1, see `storage_graph.rs`)
//!    persists every temporal field as `REAL NOT NULL` unix-seconds.
//!    Conversion happens at the storage boundary via the
//!    `dt_to_unix` / `unix_to_dt` helpers below.
//!
//! 2. **`record_extraction_failure` / `record_resolution_trace` /
//!    `begin_pipeline_run` / `finish_pipeline_run`** — these reference
//!    the audit types from `graph::audit`, not duplicates redefined
//!    inline. The §4.2 listing redefined them; we re-use the canonical
//!    versions to avoid type drift.
//!
//! 3. **`KnowledgeTopic` is from `graph::topic`.** Same rationale.
//!
//! 4. **`ExtractionFailure.stage` and `error_category`** are owned-`String`
//!    on the canonical `audit::ExtractionFailure` (the §4.2 redefined
//!    version used `&'static str`). We use the owned-string canonical.
//!
//! 5. **`Edge.confidence_source` is not yet persisted.** The §4.1 schema
//!    (see `storage_graph.rs`) does not currently carry a
//!    `confidence_source` column. The Phase 2 edge slice writes every
//!    other field round-trip; on read, `confidence_source` is
//!    re-defaulted to `Recovered` (the constructor's default for
//!    non-`Migrated` edges). Tests cover this explicitly. Adding the
//!    column is a schema change and is left for the slice that lands
//!    `apply_graph_delta` (Phase 4) or the v03-migration import path,
//!    whichever needs it first — they are the only writers that
//!    currently produce non-`Recovered` values.
//!
//! ## Transaction model
//!
//! All multi-statement methods open their own SQLite transaction unless
//! noted. `apply_graph_delta` and `merge_entities` are the only trait methods
//! that span tables; both commit atomically. Single-row reads/writes
//! piggyback on the implicit autocommit.
//!
//! ## Telemetry
//!
//! Every successful CRUD call emits an `OperationalLoad` signal via the
//! sink configured at construction. Failed calls do **not** emit
//! `OperationalLoad`; they may emit `Invariant` or `ResourcePressure`
//! depending on the failure (§6).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::graph::{
    audit::{ExtractionFailure, PipelineKind, ResolutionTrace, RunStatus},
    delta::{ApplyReport, GraphDelta, GRAPH_DELTA_SCHEMA_VERSION},
    edge::{Edge, EdgeEnd, ResolutionMethod},
    affect::SomaticFingerprint,
    entity::{normalize_alias, validate_attributes, Entity, EntityKind, HistoryEntry},
    error::GraphError,
    schema::Predicate,
    telemetry::{NoopSink, TelemetrySink, WatermarkTracker},
    topic::KnowledgeTopic,
};

// ---------------------------------------------------------------------------
// Helper types (trait-public, defined here)
// ---------------------------------------------------------------------------

/// Result of `merge_entities`: counts of mutations applied during the merge.
/// Returned to callers (typically v03-resolution) so they can record
/// `ResolutionTrace` rows without re-counting from disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Edges whose subject or object were repointed from loser to winner.
    pub edges_superseded: u64,
    /// Alias rows updated to point at the winner.
    pub aliases_repointed: u64,
}

/// Bidirectional provenance result from `mentions_of_entity` (§4.2,
/// GOAL-1.3 / GOAL-1.7). Episodes and memories are de-duplicated and
/// ordered by ascending `recorded_at` (oldest mention first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityMentions {
    pub episode_ids: Vec<Uuid>,
    pub memory_ids: Vec<String>,
}

/// Stats about a proposed (non-canonical) predicate, for operator drift
/// monitoring (§4.2, GOAL-1.10). Returned by `list_proposed_predicates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedPredicateStats {
    pub label: String,
    pub usage_count: u64,
}

/// Projection of one `graph_pipeline_runs` row, returned by
/// [`GraphStore::latest_pipeline_run_for_memory`] (§3.1 / §6.3 introspection).
///
/// All fields are decoded from SQLite into their canonical Rust types:
/// `started_at` / `finished_at` are converted from REAL unix-seconds back to
/// `DateTime<Utc>`, and `kind` / `status` are parsed back into the `audit`
/// enums. Decoding errors surface as `GraphError::Storage`.
///
/// `error_detail` is the raw text the writer passed to
/// `finish_pipeline_run`. The resolution layer wraps a `StageFailure` JSON
/// blob in there for `Failed` runs; callers that don't care about the
/// structured form (e.g. operator dashboards) can ignore the JSON shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineRunRow {
    pub run_id: Uuid,
    pub kind: PipelineKind,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub memory_id: Option<String>,
    pub episode_id: Option<Uuid>,
    pub error_detail: Option<String>,
}

/// Hard ceiling enforced by [`GraphStore::search_candidates`] regardless of
/// the caller's `top_k`. Bounds memory and latency for the v0 brute-force
/// scan over `graph_entities.embedding` (§4.2, ISS-033). The limit applies
/// after the alias and embedding signals are unioned, before recency scoring.
pub const MAX_TOP_K: usize = 50;

/// Input to [`GraphStore::search_candidates`] (§3.4.1 driver, ISS-033).
///
/// **Construction.** `mention_text` and `namespace` are mandatory; everything
/// else is optional. The store does NOT re-embed `mention_text` if
/// `mention_embedding` is `None` — embedding is the caller's responsibility
/// (see contract on `search_candidates`).
#[derive(Debug, Clone)]
pub struct CandidateQuery {
    /// The mention text as it appears in the source memory. Normalized
    /// internally for the alias-exact lookup (`normalize_alias`); the raw
    /// form is kept for diagnostics in returned `CandidateMatch` rows.
    pub mention_text: String,
    /// Optional pre-computed embedding of the mention (in the system-wide
    /// embedding dim). When `None`, the embedding signal is omitted — no
    /// implicit re-embedding inside the store.
    pub mention_embedding: Option<Vec<f32>>,
    /// When `Some`, restricts both alias and embedding scans to this kind.
    pub kind_filter: Option<EntityKind>,
    /// Hard filter; cross-namespace candidates are never returned.
    pub namespace: String,
    /// Caller's requested cap; the store enforces a hard ceiling
    /// ([`MAX_TOP_K`]) regardless of this value.
    pub top_k: usize,
    /// Window for the linear recency-decay score. `None` ⇒ unbounded window
    /// (all entities get a positive recency score, scaled against the oldest
    /// `last_seen` in the candidate set).
    pub recency_window: Option<std::time::Duration>,
    /// "Now" reference for recency scoring (unix seconds). Passed in (not
    /// read from the system clock) so tests are deterministic.
    pub now: f64,
}

/// One row of [`GraphStore::search_candidates`] output.
///
/// **Signal semantics — `None` vs `Some(0.0)`.**
/// - `embedding_score = None`  ⇒ signal *missing* (no embedding to compare).
///   Fusion redistributes this signal's weight across present signals.
/// - `embedding_score = Some(0.0)` ⇒ signal *present, value zero*. Fusion
///   keeps the embedding weight allocated; the candidate just scored 0.
///
/// Collapsing both into `0.0` would systematically miscalibrate the
/// fusion module's weight redistribution — load-bearing distinction.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateMatch {
    pub entity_id: Uuid,
    pub kind: EntityKind,
    pub canonical_name: String,
    /// True iff reached via exact alias hit on the normalized
    /// `mention_text`. Mutually independent of the embedding signal — a
    /// single candidate may have `alias_match = true` AND a non-`None`
    /// `embedding_score`.
    pub alias_match: bool,
    /// Cosine similarity in `[-1.0, 1.0]`, or `None` when the signal is
    /// missing (caller had no `mention_embedding` *or* candidate has no
    /// `embedding` blob).
    pub embedding_score: Option<f32>,
    /// Linear decay in `[0.0, 1.0]` over `recency_window`; 0 outside window.
    pub recency_score: f32,
    /// Last-seen unix seconds — projected so fusion doesn't need a follow-up
    /// `get_entity` round-trip.
    pub last_seen: f64,
    /// Identity confidence at the time of the read — projected for the same
    /// reason.
    pub identity_confidence: f64,
}

// ---------------------------------------------------------------------------
// GraphStore trait — full §4.2 surface
// ---------------------------------------------------------------------------

/// Hard cap on rows returned by [`GraphStore::find_edges`] (ISS-034, design
/// §4.2). Bounds work for pathological histories where one (subject,
/// predicate, object) triple has hundreds of supersession versions; the
/// resolution driver (v03-resolution §3.4.4) only ever inspects the head
/// of the list so silently truncating is safe.
pub const MAX_FIND_EDGES_RESULTS: usize = 64;

/// Persistence and query surface for the v0.3 graph layer.
///
/// Object-safe; callers may hold `&dyn GraphStore` for test injection.
/// The production impl is [`SqliteGraphStore`].
pub trait GraphStore {
    // ------------------------------------------------------------ Entity
    fn insert_entity(&mut self, e: &Entity) -> Result<(), GraphError>;
    fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError>;
    fn update_entity_cognitive(
        &mut self,
        id: Uuid,
        activation: f64,
        importance: f64,
        identity_confidence: f64,
        agent_affect: Option<Json>,
    ) -> Result<(), GraphError>;
    fn touch_entity_last_seen(
        &mut self,
        id: Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GraphError>;
    fn list_entities_by_kind(
        &self,
        kind: &EntityKind,
        limit: usize,
    ) -> Result<Vec<Entity>, GraphError>;

    // ------------------------------------ Candidate retrieval (ISS-033)
    /// Update the embedding blob for an existing entity (ISS-033).
    /// Separated from `update_entity_cognitive` because embedding is not a
    /// cognitive scalar and validation rules differ — length-validated,
    /// not range-validated.
    ///
    /// On dim mismatch returns `GraphError::Invariant("entity embedding
    /// dim mismatch")`. Passing `None` clears the embedding (used by
    /// v03-migration during cross-dim rewrites; not a normal operational
    /// path). Returns `GraphError::EntityNotFound(id)` if the row does not
    /// exist in the current namespace.
    fn update_entity_embedding(
        &mut self,
        id: Uuid,
        embedding: Option<&[f32]>,
    ) -> Result<(), GraphError>;

    /// Raw multi-signal candidate lookup for v03-resolution §3.4.1.
    ///
    /// **Contract.** Returns *unranked* raw signals — the caller (fusion
    /// module) combines them into a final score. The store does NOT rank;
    /// putting ranking here would duplicate the fusion module's
    /// missing-signal weight redistribution logic in two places. This
    /// method is a pure retrieval primitive.
    ///
    /// **Inputs.** See [`CandidateQuery`] field docs. Highlights:
    /// - `mention_embedding = None` ⇒ no implicit re-embedding; the
    ///   embedding signal is simply omitted from results.
    /// - `namespace` is a hard filter; cross-namespace candidates are
    ///   never returned.
    /// - `top_k` is capped at [`MAX_TOP_K`] regardless of caller value.
    /// - `recency_window = None` ⇒ unbounded window (recency scaled
    ///   against the oldest `last_seen` in the candidate set).
    /// - `now` is passed in (not read from system clock) for deterministic
    ///   tests.
    ///
    /// **Outputs.** A `Vec<CandidateMatch>` ordered by ascending
    /// `entity_id` only — callers MUST NOT rely on the order being
    /// meaningful for ranking. See [`CandidateMatch`] for field semantics
    /// (in particular the `None` vs `Some(0.0)` distinction on
    /// `embedding_score`).
    ///
    /// **Performance.** v0 uses brute-force scan over the partial index
    /// `idx_graph_entities_embed_scan`. Acceptable while entity counts
    /// per namespace stay below ~10⁵. ANN / sqlite-vec integration is a
    /// future ISS — the trait signature does not change when that lands.
    fn search_candidates(
        &self,
        query: &CandidateQuery,
    ) -> Result<Vec<CandidateMatch>, GraphError>;

    // -------------------------------------------------- Alias / identity
    fn upsert_alias(
        &mut self,
        normalized: &str,
        alias_raw: &str,
        canonical_id: Uuid,
        source_episode: Option<Uuid>,
    ) -> Result<(), GraphError>;
    fn resolve_alias(&self, normalized: &str) -> Result<Option<Uuid>, GraphError>;
    fn merge_entities(
        &mut self,
        winner: Uuid,
        loser: Uuid,
        batch_size: usize,
    ) -> Result<MergeReport, GraphError>;

    // -------------------------------------------------------------- Edge
    fn insert_edge(&mut self, edge: &Edge) -> Result<(), GraphError>;
    fn get_edge(&self, id: Uuid) -> Result<Option<Edge>, GraphError>;
    /// Edge lookup with two query modes (ISS-034 + ISS-035, design §4.2):
    ///
    /// - **Slot lookup** (`object = None`): returns all live edges
    ///   matching `(subject, predicate)` — i.e. every active object the
    ///   subject is attached to via this predicate. Used by the
    ///   resolution pipeline (v03-resolution §3.4.4) to compute
    ///   `EdgeDecision`. **No row cap** — slot semantics requires the
    ///   complete set; truncation would cause spurious `Add` decisions
    ///   for objects already known. Uses `idx_graph_edges_live` (partial
    ///   index over active edges) when `valid_only=true`, falling back
    ///   to `idx_graph_edges_spo` otherwise.
    ///
    /// - **Triple lookup** (`object = Some(_)`): returns existing edges
    ///   matching the full `(subject, predicate, object)` triple. Used
    ///   for exact-match queries and consolidation. Hard-capped at
    ///   [`MAX_FIND_EDGES_RESULTS`] (logically 0-1 live edges; the cap
    ///   is a safety net against degenerate state). Uses
    ///   `idx_graph_edges_spo`.
    ///
    /// In both modes results are ordered by `recorded_at DESC`. When
    /// `valid_only=true` only edges with `invalidated_at IS NULL` are
    /// returned.
    fn find_edges(
        &self,
        subject_id: Uuid,
        predicate: &Predicate,
        object: Option<&EdgeEnd>,
        valid_only: bool,
    ) -> Result<Vec<Edge>, GraphError>;
    /// Mark `prior_id` as invalidated by `successor_id` at `now` (ISS-034,
    /// design §4.2). Sets `prior.invalidated_at = now` and
    /// `prior.invalidated_by = Some(successor_id)`. GUARD-3 primitive: never
    /// deletes, never touches any other column. Idempotent when called
    /// twice with the same `successor_id`; returns `Invariant` if the
    /// prior is already closed by a *different* successor.
    fn invalidate_edge(
        &mut self,
        prior_id: Uuid,
        successor_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), GraphError>;
    /// Invalidate `old` pointing at `successor`; mark `successor.supersedes
    /// = old`. Atomic; rolls back on any invariant break. Convenience
    /// wrapper composing `insert_edge` + `invalidate_edge` (design §4.2).
    fn supersede_edge(
        &mut self,
        old: Uuid,
        successor: &Edge,
        at: DateTime<Utc>,
    ) -> Result<(), GraphError>;
    fn edges_of(
        &self,
        subject: Uuid,
        predicate: Option<&Predicate>,
        include_invalidated: bool,
    ) -> Result<Vec<Edge>, GraphError>;
    /// As-of query (GOAL-1.5). Edges "believed true" for `subject` at
    /// real-world time `at`. See design §4.2.
    fn edges_as_of(
        &self,
        subject: Uuid,
        at: DateTime<Utc>,
    ) -> Result<Vec<Edge>, GraphError>;
    /// BFS over canonical predicates only (GOAL-1.9). See design §4.2 for
    /// the contract (bounded output, cycle handling, ordering).
    fn traverse(
        &self,
        start: Uuid,
        max_depth: usize,
        max_results: usize,
        predicate_filter: &[Predicate],
    ) -> Result<Vec<(Uuid, Edge)>, GraphError>;

    // ----------------------------------------------- Provenance (entity)
    fn entities_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError>;
    fn edges_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError>;
    fn mentions_of_entity(&self, entity: Uuid) -> Result<EntityMentions, GraphError>;

    // ---------------------------------------- Memory ↔ Entity provenance
    fn link_memory_to_entities(
        &mut self,
        memory_id: &str,
        entity_ids: &[(Uuid, f64, Option<String>)],
        at: DateTime<Utc>,
    ) -> Result<(), GraphError>;
    fn entities_linked_to_memory(
        &self,
        memory_id: &str,
    ) -> Result<Vec<Uuid>, GraphError>;
    fn memories_mentioning_entity(
        &self,
        entity: Uuid,
        limit: usize,
    ) -> Result<Vec<String>, GraphError>;
    fn edges_sourced_from_memory(
        &self,
        memory_id: &str,
    ) -> Result<Vec<Edge>, GraphError>;

    // ------------------------------------------------- L5 Knowledge Topics
    fn upsert_topic(&mut self, t: &KnowledgeTopic) -> Result<(), GraphError>;
    fn get_topic(&self, id: Uuid) -> Result<Option<KnowledgeTopic>, GraphError>;
    fn list_topics(
        &self,
        namespace: &str,
        include_superseded: bool,
        limit: usize,
    ) -> Result<Vec<KnowledgeTopic>, GraphError>;
    fn supersede_topic(
        &mut self,
        old: Uuid,
        successor: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), GraphError>;

    // ---------------------------------------------- Pipeline-run ledger
    fn begin_pipeline_run(
        &mut self,
        kind: PipelineKind,
        input_summary: Json,
    ) -> Result<Uuid, GraphError>;
    /// §3.1 ingestion variant: same as `begin_pipeline_run` but additionally
    /// stores `(memory_id, episode_id)` in indexed columns so the latest run
    /// can be looked up directly via `latest_pipeline_run_for_memory`. Use
    /// this for `Resolution` and `Reextract` runs; use `begin_pipeline_run`
    /// for memory-scope-less runs (`KnowledgeCompile`).
    fn begin_pipeline_run_for_memory(
        &mut self,
        kind: PipelineKind,
        memory_id: &str,
        episode_id: Uuid,
        input_summary: Json,
    ) -> Result<Uuid, GraphError>;
    fn finish_pipeline_run(
        &mut self,
        run_id: Uuid,
        status: RunStatus,
        output_summary: Option<Json>,
        error_detail: Option<&str>,
    ) -> Result<(), GraphError>;
    /// §6.3: latest pipeline run for `memory_id` (any kind, by `started_at`
    /// DESC). Returns `None` if no run has ever been recorded for that
    /// memory. The row is the projection consumed by `extraction_status`.
    fn latest_pipeline_run_for_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<PipelineRunRow>, GraphError>;
    fn record_resolution_trace(
        &mut self,
        t: &ResolutionTrace,
    ) -> Result<(), GraphError>;

    // -------------------------------------- Schema registry (GOAL-1.10)
    fn record_predicate_use(
        &mut self,
        p: &Predicate,
        raw: &str,
        at: DateTime<Utc>,
    ) -> Result<(), GraphError>;
    fn list_proposed_predicates(
        &self,
        min_usage: u64,
    ) -> Result<Vec<ProposedPredicateStats>, GraphError>;

    // ----------------------------------- Visible failures (GOAL-1.12)
    fn record_extraction_failure(
        &mut self,
        f: &ExtractionFailure,
    ) -> Result<(), GraphError>;
    fn list_failed_episodes(
        &self,
        unresolved_only: bool,
    ) -> Result<Vec<Uuid>, GraphError>;
    fn mark_failure_resolved(
        &mut self,
        failure_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), GraphError>;

    // -------------------------------------- Namespaces (§4.1 lifecycle)
    fn list_namespaces(&self) -> Result<Vec<String>, GraphError>;

    // ------------------------------------------- Transaction escape hatch
    /// **Warning:** raw `&Transaction` access; bypasses GUARDs. See §4.2.
    fn with_transaction(
        &mut self,
        f: &mut dyn FnMut(&Transaction<'_>) -> Result<(), GraphError>,
    ) -> Result<(), GraphError>;

    // ------------------------------------------- Atomic batched apply (§4.2)
    /// Apply a [`GraphDelta`] atomically: short-circuits on idempotence-key
    /// hit (replays return [`ApplyReport::already_applied_marker`]); otherwise
    /// runs entity upserts, merges, edge inserts/invalidations, mention
    /// inserts, predicate counter flush, and failure-row commits inside a
    /// single transaction (§4.2 batched-counter clause; §4.3 Rule A separates
    /// `stage_failures` into its own transaction so they survive the main
    /// rollback path).
    ///
    /// This is the **only** intended writer for v03-resolution's
    /// `stage_persist`. Direct CRUD methods on this trait remain available
    /// for tests, migrations, and v03-migration backfill.
    fn apply_graph_delta(
        &mut self,
        delta: &GraphDelta,
    ) -> Result<ApplyReport, GraphError>;
}

// ---------------------------------------------------------------------------
// SqliteGraphStore — production impl
// ---------------------------------------------------------------------------

/// Production [`GraphStore`] impl over a borrowed SQLite connection.
///
/// Lifetime `'a` is the borrow on the underlying `rusqlite::Connection`
/// owned by `Storage`. Constructed via `Storage::graph()` (see
/// `crate::storage_graph`).
///
/// The store carries a [`TelemetrySink`] (default: [`NoopSink`]) and a
/// [`WatermarkTracker`] for emitting `OperationalLoad` /
/// `ResourcePressure` signals on each successful mutation (§6).
#[allow(dead_code)] // Phase 1: fields used in Phase 2/3 method bodies.
pub struct SqliteGraphStore<'a> {
    pub(crate) conn: &'a mut rusqlite::Connection,
    pub(crate) namespace: String,
    pub(crate) sink: Box<dyn TelemetrySink>,
    pub(crate) watermark: WatermarkTracker,
    /// In-transaction predicate-use counter. Flushed by
    /// `apply_graph_delta` at commit time (§4.2 batched-counter clause).
    /// Empty outside `apply_graph_delta`.
    pub(crate) predicate_use_buffer: HashMap<String, u64>,
    /// System-wide embedding dim (ISS-033). All `graph_entities.embedding`
    /// blobs MUST decode to a vector of this length; mismatch is a hard
    /// `GraphError::Invariant("entity embedding dim mismatch")`. Same value
    /// as `EmbeddingClient::config.dimensions` and `KnowledgeTopic` write
    /// path — single source of truth lives in the embedding provider config,
    /// not duplicated here. Defaulted to [`DEFAULT_ENTITY_EMBEDDING_DIM`] for
    /// fresh stores; override via [`Self::with_embedding_dim`].
    pub(crate) embedding_dim: usize,
}

/// Default embedding dim for `SqliteGraphStore` (ISS-033). 768 matches the
/// nomic-embed-text Ollama default declared in
/// `crate::embeddings::EmbeddingConfig::default()`. Production code that
/// uses a different provider (OpenAI 1536, etc.) MUST call
/// [`SqliteGraphStore::with_embedding_dim`] with the matching value before
/// any write touches `graph_entities.embedding`.
pub const DEFAULT_ENTITY_EMBEDDING_DIM: usize = 768;

impl<'a> SqliteGraphStore<'a> {
    /// Construct a new store with default namespace `"default"` and a
    /// `NoopSink` telemetry sink. Use [`Self::with_namespace`] /
    /// [`Self::with_sink`] to override.
    pub fn new(conn: &'a mut rusqlite::Connection) -> Self {
        Self {
            conn,
            namespace: "default".to_string(),
            sink: Box::new(NoopSink),
            watermark: WatermarkTracker::new(1000),
            predicate_use_buffer: HashMap::new(),
            embedding_dim: DEFAULT_ENTITY_EMBEDDING_DIM,
        }
    }

    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    pub fn with_sink(mut self, sink: Box<dyn TelemetrySink>) -> Self {
        self.sink = sink;
        self
    }

    /// Override the system-wide embedding dim (ISS-033). Must match the dim
    /// of the embedding provider that produces `Entity::embedding` vectors;
    /// see [`DEFAULT_ENTITY_EMBEDDING_DIM`].
    pub fn with_embedding_dim(mut self, dim: usize) -> Self {
        self.embedding_dim = dim;
        self
    }

    /// Borrow the underlying connection. Tests use this to inspect rows
    /// directly; production code should not.
    #[cfg(test)]
    #[allow(dead_code)] // Used by Phase 2 CRUD tests.
    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        self.conn
    }

    // ---------------------------------- ISS-033 internal helpers
    //
    // Kept on the impl rather than free functions so they can borrow
    // `&self` if a future refactor needs configurable scoring (e.g.
    // weight-blending tuned per-store). Today they only take primitive
    // refs but the impl placement is forward-compatible.

    /// Row mapper used by `search_candidates` for the alias-hit point
    /// lookup. Returns the raw projected columns; semantic conversion
    /// (text→`EntityKind`, blob→`Vec<f32>`, etc.) happens after the
    /// rusqlite closure to keep the closure infallible w.r.t.
    /// `GraphError`.
    fn map_candidate_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<CandidateRowProjection> {
        Ok((
            row.get::<_, String>(1)?,                      // kind
            row.get::<_, String>(2)?,                      // canonical_name
            row.get::<_, f64>(3)?,                         // last_seen
            row.get::<_, f64>(4)?,                         // identity_confidence
            row.get::<_, Option<Vec<u8>>>(5)?,             // embedding blob
        ))
    }

    /// L2 norm of a vector. Used to amortize the mention embedding's norm
    /// across the per-candidate cosine computation. Pure function; no
    /// state. Returns `1.0` for an all-zero input to avoid divide-by-zero
    /// downstream — a zero-vector mention embedding is meaningless input
    /// but not an error worth halting on (cosine becomes 0/1 = 0, which
    /// is the right "no signal" semantics).
    fn l2_norm(v: &[f32]) -> f32 {
        let s: f32 = v.iter().map(|x| x * x).sum();
        let n = s.sqrt();
        if n == 0.0 {
            1.0
        } else {
            n
        }
    }

    /// Cosine similarity between `a` and `b`, given a precomputed `a_norm`.
    /// Caller MUST ensure `a.len() == b.len()` (we don't re-check here —
    /// `search_candidates` validates dim before this hot loop). Returns a
    /// value in `[-1.0, 1.0]`; an all-zero `b` produces `0.0` (no signal).
    fn cosine(a: &[f32], b: &[f32], a_norm: f32) -> f32 {
        let mut dot: f32 = 0.0;
        let mut b_sq: f32 = 0.0;
        for i in 0..a.len() {
            dot += a[i] * b[i];
            b_sq += b[i] * b[i];
        }
        let b_norm = b_sq.sqrt();
        if b_norm == 0.0 {
            return 0.0;
        }
        let cos = dot / (a_norm * b_norm);
        // Clamp against floating-point drift just outside [-1, 1].
        cos.clamp(-1.0, 1.0)
    }

    // ---- §3.1 / §6.3 — pipeline-run-row writer (private) ----------------
    //
    // Single source of truth for every `INSERT INTO graph_pipeline_runs`
    // path. The two trait dispatchers (`begin_pipeline_run` for non-memory-
    // scoped runs, `begin_pipeline_run_for_memory` for ingestion / re-extract)
    // both delegate here so the kind/status string encoding, the
    // `audit::PipelineRun::start` invariant, and the column ordering stay
    // exactly aligned with `finish_pipeline_run`'s atomic-update guard.
    fn begin_pipeline_run_inner(
        &mut self,
        kind: PipelineKind,
        memory_id: Option<&str>,
        episode_id: Option<Uuid>,
        input_summary: serde_json::Value,
    ) -> Result<Uuid, GraphError> {
        // Construct a fresh PipelineRun in `Running` state via the canonical
        // helper (`audit::PipelineRun::start`). This guarantees the id, status
        // string, and started_at all use the same source of truth as the
        // pure-Rust state machine — when `finish_pipeline_run` later validates
        // a transition it reads what `start` wrote, not a divergent encoding.
        let run = crate::graph::audit::PipelineRun::start(
            kind,
            input_summary.clone(),
            dt_to_unix(Utc::now()),
        );
        let kind_str = serde_json::to_string(&kind)?;
        let kind_label = kind_str.trim_matches('"').to_string();
        let status_str = serde_json::to_string(&run.status)?;
        let status_label = status_str.trim_matches('"').to_string();
        let input_json = serde_json::to_string(&input_summary)?;
        self.conn.execute(
            "INSERT INTO graph_pipeline_runs (
                run_id, kind, started_at, finished_at, status,
                input_summary, output_summary, error_detail,
                namespace, memory_id, episode_id
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, NULL, NULL, ?6, ?7, ?8)",
            rusqlite::params![
                run.id.as_bytes().to_vec(),
                kind_label,
                run.started_at,
                status_label,
                input_json,
                self.namespace,
                memory_id,
                episode_id.map(|u| u.as_bytes().to_vec()),
            ],
        )?;
        Ok(run.id)
    }
}

// ---------------------------------------------------------------------------
// SQL constants — kept here so storage_graph migrations can reference them
// when introspecting which tables the trait expects.
// ---------------------------------------------------------------------------

/// Table names owned by the graph layer (§4.1). Listed for storage_graph
/// migration cross-checking and for the `list_namespaces` query.
pub const GRAPH_TABLES: &[&str] = &[
    "graph_entities",
    "graph_entity_aliases",
    "graph_edges",
    "graph_predicates",
    "graph_extraction_failures",
    "graph_memory_entity_mentions",
    "knowledge_topics",
    "graph_pipeline_runs",
    "graph_resolution_traces",
    "graph_applied_deltas",
];

// ---------------------------------------------------------------------------
// Internal helpers — time/uuid/serde codecs at the storage boundary
// ---------------------------------------------------------------------------

/// Encode a `DateTime<Utc>` as the unix-seconds `f64` used by §4.1 schema.
/// Sub-second precision: nanoseconds folded into the fractional part.
fn dt_to_unix(t: DateTime<Utc>) -> f64 {
    t.timestamp() as f64 + (t.timestamp_subsec_nanos() as f64) / 1e9
}

/// Decode a unix-seconds `f64` (as stored in §4.1 REAL columns) back to
/// `DateTime<Utc>`. Returns `Invariant("timestamp out of range")` for values
/// chrono cannot represent.
fn unix_to_dt(secs: f64) -> Result<DateTime<Utc>, GraphError> {
    let whole = secs.trunc() as i64;
    let nanos = ((secs - whole as f64) * 1e9).round() as u32;
    DateTime::<Utc>::from_timestamp(whole, nanos)
        .ok_or(GraphError::Invariant("timestamp out of range"))
}

/// Encode `EntityKind` as the TEXT column value. Uses serde_json so that
/// `Other("foo")` and the canonical variants both round-trip exactly the
/// way they do over the wire (single source of truth: the serde derive on
/// `EntityKind`). For canonical variants this yields a JSON string like
/// `"\"person\""`; for `Other(s)` it yields `{"other":"<s>"}`.
fn kind_to_text(kind: &EntityKind) -> Result<String, GraphError> {
    Ok(serde_json::to_string(kind)?)
}

/// Decode a TEXT column back to `EntityKind`. Companion to [`kind_to_text`].
fn text_to_kind(s: &str) -> Result<EntityKind, GraphError> {
    Ok(serde_json::from_str(s)?)
}

/// Decode a BLOB column carrying a 16-byte UUID. `None` if the SQL value
/// was NULL; `Invariant` if the BLOB is the wrong length.
fn opt_blob_to_uuid(blob: Option<Vec<u8>>) -> Result<Option<Uuid>, GraphError> {
    match blob {
        None => Ok(None),
        Some(b) if b.len() == 16 => Ok(Some(Uuid::from_slice(&b).unwrap())),
        Some(_) => Err(GraphError::Invariant("uuid blob length != 16")),
    }
}

/// Encode an `Entity::embedding` (or any `&[f32]` of the system embedding
/// dim) as the SQLite blob format used in `graph_entities.embedding` and
/// `knowledge_topics.embedding` (§4.1, ISS-033): `dim * f32` little-endian,
/// `4 * dim` bytes total.
///
/// Validates `embedding.len() == expected_dim` first; on mismatch returns
/// `GraphError::Invariant("entity embedding dim mismatch")` (verbatim, locks
/// with the read-path message in [`entity_embedding_from_blob`]).
///
/// `None` ⇒ `None` blob (NULL column), trivially valid: an entity that has
/// not been embedded yet is a normal lifecycle state, not an error.
pub(crate) fn entity_embedding_to_blob(
    embedding: Option<&[f32]>,
    expected_dim: usize,
) -> Result<Option<Vec<u8>>, GraphError> {
    let Some(v) = embedding else { return Ok(None) };
    if v.len() != expected_dim {
        return Err(GraphError::Invariant("entity embedding dim mismatch"));
    }
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    Ok(Some(out))
}

/// Inverse of [`entity_embedding_to_blob`]. Decodes a SQLite blob into a
/// `Vec<f32>` of length `expected_dim`. NULL ⇒ `None`. A non-NULL blob whose
/// length is not exactly `4 * expected_dim` is rejected as
/// `GraphError::Invariant("entity embedding dim mismatch")` — same message
/// as the writer, so callers can pattern-match a single string regardless of
/// which side detected the inconsistency.
///
/// (No "shorter blob is fine, longer is error" tolerance: a stale dim is a
/// migration bug, and silently truncating would corrupt similarity scores.)
pub(crate) fn entity_embedding_from_blob(
    blob: Option<Vec<u8>>,
    expected_dim: usize,
) -> Result<Option<Vec<f32>>, GraphError> {
    let Some(b) = blob else { return Ok(None) };
    if b.len() != expected_dim * 4 {
        return Err(GraphError::Invariant("entity embedding dim mismatch"));
    }
    let mut out = Vec::with_capacity(expected_dim);
    let mut buf = [0u8; 4];
    for chunk in b.chunks_exact(4) {
        buf.copy_from_slice(chunk);
        out.push(f32::from_le_bytes(buf));
    }
    Ok(Some(out))
}

/// Optional DateTime → REAL unix-seconds. NULL when `None`.
fn opt_dt_to_unix(t: Option<DateTime<Utc>>) -> Option<f64> {
    t.map(dt_to_unix)
}

/// Optional REAL unix-seconds → DateTime. Errors if the value is non-NULL
/// but out of chrono's range.
fn opt_unix_to_dt(secs: Option<f64>) -> Result<Option<DateTime<Utc>>, GraphError> {
    match secs {
        None => Ok(None),
        Some(s) => unix_to_dt(s).map(Some),
    }
}

/// Split a [`Predicate`] into the `(predicate_kind, predicate_label)`
/// column pair used by `graph_edges` (§4.1). For canonical predicates the
/// label is the serde `rename_all = "snake_case"` form (e.g.
/// `WorksAt` → `"works_at"`); for proposed predicates the already-normalized
/// inner string is reused verbatim — `Predicate::proposed` is the only
/// sanctioned constructor and it normalizes at construction time (§3.3).
fn predicate_to_columns(p: &Predicate) -> Result<(&'static str, String), GraphError> {
    match p {
        Predicate::Canonical(c) => {
            // serde_json renders the snake_case enum tag as `"\"works_at\""`;
            // strip the surrounding quotes to get the bare label.
            let json = serde_json::to_string(c)?;
            let label = json
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or(GraphError::Invariant("canonical predicate label not a JSON string"))?
                .to_string();
            Ok(("canonical", label))
        }
        Predicate::Proposed(s) => Ok(("proposed", s.clone())),
    }
}

/// Inverse of [`predicate_to_columns`]. Re-builds a [`Predicate`] from the
/// `(predicate_kind, predicate_label)` pair stored in `graph_edges`. For
/// `kind = 'canonical'` the label is parsed back through serde so an
/// unknown label (e.g. a future variant present on disk but not in this
/// build) yields a clean `GraphError`, not a panic. For `kind = 'proposed'`
/// the label is wrapped directly — no re-normalization, since the column
/// already holds the normalized form by construction.
fn columns_to_predicate(kind: &str, label: &str) -> Result<Predicate, GraphError> {
    match kind {
        "canonical" => {
            let json = format!("\"{}\"", label);
            let c = serde_json::from_str(&json)?;
            Ok(Predicate::Canonical(c))
        }
        "proposed" => Ok(Predicate::Proposed(label.to_string())),
        _ => Err(GraphError::Invariant(
            "graph_edges.predicate_kind not in ('canonical','proposed')",
        )),
    }
}

/// Validate `(stage, error_category)` against the closed sets defined in
/// `crate::graph::audit` (`STAGE_*` / `CATEGORY_*` constants). Surfaces a
/// clean invariant string if either is unknown — same pattern as
/// `predicate_to_columns` (closure enforced in code, not in the schema, so
/// extending the set in v0.4 is a no-migration change).
fn validate_failure_closed_sets(stage: &str, category: &str) -> Result<(), GraphError> {
    use crate::graph::audit::{
        CATEGORY_BUDGET_EXHAUSTED, CATEGORY_DB_ERROR, CATEGORY_INTERNAL,
        CATEGORY_LLM_INVALID_OUTPUT, CATEGORY_LLM_TIMEOUT, STAGE_DEDUP, STAGE_EDGE_EXTRACT,
        STAGE_ENTITY_EXTRACT, STAGE_PERSIST,
    };
    const STAGES: &[&str] = &[
        STAGE_ENTITY_EXTRACT,
        STAGE_EDGE_EXTRACT,
        STAGE_DEDUP,
        STAGE_PERSIST,
    ];
    const CATEGORIES: &[&str] = &[
        CATEGORY_LLM_TIMEOUT,
        CATEGORY_LLM_INVALID_OUTPUT,
        CATEGORY_BUDGET_EXHAUSTED,
        CATEGORY_DB_ERROR,
        CATEGORY_INTERNAL,
    ];
    if !STAGES.contains(&stage) {
        return Err(GraphError::Invariant(
            "record_extraction_failure: unknown stage label (see audit::STAGE_*)",
        ));
    }
    if !CATEGORIES.contains(&category) {
        return Err(GraphError::Invariant(
            "record_extraction_failure: unknown error_category (see audit::CATEGORY_*)",
        ));
    }
    Ok(())
}

/// Encode `ResolutionMethod` for the `graph_edges.resolution_method` TEXT
/// column. Re-uses serde to keep the wire/disk forms in lockstep.
fn resolution_method_to_text(m: &ResolutionMethod) -> Result<String, GraphError> {
    let json = serde_json::to_string(m)?;
    Ok(json
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(GraphError::Invariant("resolution_method not a JSON string"))?
        .to_string())
}

/// Decode TEXT → `ResolutionMethod`.
fn text_to_resolution_method(s: &str) -> Result<ResolutionMethod, GraphError> {
    let json = format!("\"{}\"", s);
    Ok(serde_json::from_str(&json)?)
}

/// Encode the object side of an edge (`object_kind`, `object_entity_id`,
/// `object_literal`) — exactly the §4.1 XOR shape.
#[allow(clippy::type_complexity)]
fn edge_end_to_columns(
    end: &EdgeEnd,
) -> Result<(&'static str, Option<Vec<u8>>, Option<String>), GraphError> {
    match end {
        EdgeEnd::Entity { id } => Ok(("entity", Some(id.as_bytes().to_vec()), None)),
        EdgeEnd::Literal { value } => {
            let s = serde_json::to_string(value)?;
            Ok(("literal", None, Some(s)))
        }
    }
}

/// Decode `(object_kind, object_entity_id, object_literal)` back to
/// `EdgeEnd`. Enforces the §4.1 XOR at the read boundary so a corrupted
/// row never silently constructs a degenerate `EdgeEnd`.
fn columns_to_edge_end(
    kind: &str,
    entity_blob: Option<Vec<u8>>,
    literal_text: Option<String>,
) -> Result<EdgeEnd, GraphError> {
    match (kind, entity_blob, literal_text) {
        ("entity", Some(b), None) => {
            let id = Uuid::from_slice(&b)
                .map_err(|_| GraphError::Invariant("edge object_entity_id blob length != 16"))?;
            Ok(EdgeEnd::Entity { id })
        }
        ("literal", None, Some(s)) => {
            let value: serde_json::Value = serde_json::from_str(&s)?;
            Ok(EdgeEnd::Literal { value })
        }
        _ => Err(GraphError::Invariant("edge object kind/columns mismatch")),
    }
}

/// Tuple type for raw `graph_edges` row reads. Centralized here so each
/// `SELECT … FROM graph_edges` site uses the same closure
/// ([`row_to_edge_columns`]) and the same decoder ([`decode_edge_row`]).
/// Column order matches the canonical SELECT list used in this module.
#[allow(clippy::type_complexity)]
type EdgeRowColumns = (
    Vec<u8>,         // 0: id
    Vec<u8>,         // 1: subject_id
    String,          // 2: predicate_kind
    String,          // 3: predicate_label
    String,          // 4: object_kind
    Option<Vec<u8>>, // 5: object_entity_id
    Option<String>,  // 6: object_literal
    String,          // 7: summary
    Option<f64>,     // 8: valid_from
    Option<f64>,     // 9: valid_to
    f64,             // 10: recorded_at
    Option<f64>,     // 11: invalidated_at
    Option<Vec<u8>>, // 12: invalidated_by
    Option<Vec<u8>>, // 13: supersedes
    Option<Vec<u8>>, // 14: episode_id
    Option<String>,  // 15: memory_id
    String,          // 16: resolution_method
    f64,             // 17: activation
    f64,             // 18: confidence
    Option<String>,  // 19: agent_affect (JSON)
    f64,             // 20: created_at
);

/// Row layout for the embedding-cohort scan inside `search_candidates`
/// (ISS-033 §3.4.1). Mirrors the `SELECT id, kind, canonical_name,
/// last_seen, identity_confidence, embedding FROM graph_entities` shape.
/// Lifted to a type alias to keep the scan loop body readable and to
/// silence `clippy::type_complexity`.
#[allow(clippy::type_complexity)]
type CandidateScanRow = (
    Vec<u8>,         // 0: id
    String,          // 1: kind
    String,          // 2: canonical_name
    f64,             // 3: last_seen
    f64,             // 4: identity_confidence
    Option<Vec<u8>>, // 5: embedding
);

/// Row layout for the alias-hit single-row lookup inside
/// `search_candidates` — same columns as [`CandidateScanRow`] minus the
/// id (already known from the alias resolution). Returned by
/// [`SqliteGraphStore::map_candidate_row`].
#[allow(clippy::type_complexity)]
type CandidateRowProjection = (
    String,          // 0: kind
    String,          // 1: canonical_name
    f64,             // 2: last_seen
    f64,             // 3: identity_confidence
    Option<Vec<u8>>, // 4: embedding
);

/// Row mapper closure body: pulls every column straight off the row in
/// the canonical order. Infallible at the storage layer (any
/// type-coercion error bubbles as `rusqlite::Error`); semantic decoding
/// is done by [`decode_edge_row`] afterward.
fn row_to_edge_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<EdgeRowColumns> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
        row.get(20)?,
    ))
}

/// Decode a raw `graph_edges` row into an [`Edge`]. Inverse of the
/// `INSERT` in `insert_edge`. `confidence_source` defaults to `Recovered`
/// because §4.1 has no column for it yet (DevNote #5); migrated edges
/// will set it explicitly once the migration path lands.
fn decode_edge_row(c: EdgeRowColumns) -> Result<Edge, GraphError> {
    let id = Uuid::from_slice(&c.0)
        .map_err(|_| GraphError::Invariant("edge id blob length != 16"))?;
    let subject_id = Uuid::from_slice(&c.1)
        .map_err(|_| GraphError::Invariant("edge subject_id blob length != 16"))?;
    let predicate = columns_to_predicate(&c.2, &c.3)?;
    let object = columns_to_edge_end(&c.4, c.5, c.6)?;
    let invalidated_by = opt_blob_to_uuid(c.12)?;
    let supersedes = opt_blob_to_uuid(c.13)?;
    let episode_id = opt_blob_to_uuid(c.14)?;
    let resolution_method = text_to_resolution_method(&c.16)?;
    let agent_affect: Option<Json> = match c.19 {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };

    Ok(Edge {
        id,
        subject_id,
        predicate,
        object,
        summary: c.7,
        valid_from: opt_unix_to_dt(c.8)?,
        valid_to: opt_unix_to_dt(c.9)?,
        recorded_at: unix_to_dt(c.10)?,
        invalidated_at: opt_unix_to_dt(c.11)?,
        invalidated_by,
        supersedes,
        episode_id,
        memory_id: c.15,
        resolution_method,
        activation: c.17,
        confidence: c.18,
        // §4.1 has no `confidence_source` column today. Re-defaulting to
        // `Recovered` matches the constructor's default for non-`Migrated`
        // edges; `Migrated` edges set this explicitly via the migration
        // path (which has its own write site, not insert_edge). See
        // DevNote #5.
        confidence_source: crate::graph::edge::ConfidenceSource::Recovered,
        agent_affect,
        created_at: unix_to_dt(c.20)?,
    })
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// GraphStore impl for SqliteGraphStore
//
// Implementation note: methods are organized into impl blocks below, grouped
// by concern (entity / edge / alias / provenance / topic / audit / schema /
// failure / namespace / traverse / merge). Each block is preceded by a
// section header comment.
//
// Phase 1 (this commit): trait surface + helper types + SqliteGraphStore
// constructor. Method bodies are `unimplemented!()` and will be filled in
// Phase 2 (CRUD) and Phase 3 (transactional / multi-table).
// ---------------------------------------------------------------------------

impl<'a> GraphStore for SqliteGraphStore<'a> {
    // -------------------------------------------------- Entity (Phase 2)
    fn insert_entity(&mut self, e: &Entity) -> Result<(), GraphError> {
        // Reserved-key gate (§3.1) — refuse to write `attributes` containing
        // promoted-to-typed-field keys. Loud failure beats silent shadowing.
        validate_attributes(&e.attributes)?;

        let kind_text = kind_to_text(&e.kind)?;
        let attributes_json = serde_json::to_string(&e.attributes)?;
        let history_json = serde_json::to_string(&e.history)?;
        let agent_affect_json = match &e.agent_affect {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        let fp_blob: Option<Vec<u8>> = e
            .somatic_fingerprint
            .as_ref()
            .map(|fp| fp.to_le_bytes().to_vec());
        let merged_into_blob: Option<Vec<u8>> =
            e.merged_into.map(|u| u.as_bytes().to_vec());
        // ISS-033: validate + encode embedding before INSERT. Dim mismatch is
        // a hard error; see `entity_embedding_to_blob` for the contract.
        let embedding_blob: Option<Vec<u8>> =
            entity_embedding_to_blob(e.embedding.as_deref(), self.embedding_dim)?;

        // Single-statement INSERT runs in SQLite autocommit; FK + CHECK
        // failures bubble up as `GraphError::Sqlite`.
        self.conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, summary, attributes,
                first_seen, last_seen, created_at, updated_at,
                activation, agent_affect, arousal, importance,
                identity_confidence, somatic_fingerprint, namespace,
                history, merged_into, embedding
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11, ?12, ?13,
                ?14, ?15, ?16,
                ?17, ?18, ?19
            )",
            rusqlite::params![
                e.id.as_bytes().to_vec(),
                e.canonical_name,
                kind_text,
                e.summary,
                attributes_json,
                dt_to_unix(e.first_seen),
                dt_to_unix(e.last_seen),
                dt_to_unix(e.created_at),
                dt_to_unix(e.updated_at),
                e.activation,
                agent_affect_json,
                e.arousal,
                e.importance,
                e.identity_confidence,
                fp_blob,
                self.namespace,
                history_json,
                merged_into_blob,
                embedding_blob,
            ],
        )?;
        Ok(())
    }

    fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, canonical_name, kind, summary, attributes,
                    first_seen, last_seen, created_at, updated_at,
                    activation, agent_affect, arousal, importance,
                    identity_confidence, somatic_fingerprint,
                    history, merged_into, embedding
             FROM graph_entities
             WHERE id = ?1 AND namespace = ?2",
        )?;
        let row = stmt
            .query_row(
                rusqlite::params![id.as_bytes().to_vec(), self.namespace],
                |row| {
                    // Decode primitives in the row mapper; semantic conversion
                    // (kind, fingerprint, uuid blob) happens after to keep the
                    // mapper closure infallible w.r.t. graph errors.
                    let id_blob: Vec<u8> = row.get(0)?;
                    let canonical_name: String = row.get(1)?;
                    let kind_text: String = row.get(2)?;
                    let summary: String = row.get(3)?;
                    let attributes_json: String = row.get(4)?;
                    let first_seen: f64 = row.get(5)?;
                    let last_seen: f64 = row.get(6)?;
                    let created_at: f64 = row.get(7)?;
                    let updated_at: f64 = row.get(8)?;
                    let activation: f64 = row.get(9)?;
                    let agent_affect_json: Option<String> = row.get(10)?;
                    let arousal: f64 = row.get(11)?;
                    let importance: f64 = row.get(12)?;
                    let identity_confidence: f64 = row.get(13)?;
                    let fp_blob: Option<Vec<u8>> = row.get(14)?;
                    let history_json: String = row.get(15)?;
                    let merged_into_blob: Option<Vec<u8>> = row.get(16)?;
                    let embedding_blob: Option<Vec<u8>> = row.get(17)?;
                    Ok((
                        id_blob,
                        canonical_name,
                        kind_text,
                        summary,
                        attributes_json,
                        first_seen,
                        last_seen,
                        created_at,
                        updated_at,
                        activation,
                        agent_affect_json,
                        arousal,
                        importance,
                        identity_confidence,
                        fp_blob,
                        history_json,
                        merged_into_blob,
                        embedding_blob,
                    ))
                },
            )
            .optional()?;

        let Some(r) = row else { return Ok(None) };
        let id_decoded = Uuid::from_slice(&r.0)
            .map_err(|_| GraphError::Invariant("uuid blob length != 16"))?;
        let kind = text_to_kind(&r.2)?;
        let attributes: Json = serde_json::from_str(&r.4)?;
        let agent_affect: Option<Json> = match r.10 {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        };
        let somatic_fingerprint = match r.14 {
            Some(b) => Some(SomaticFingerprint::from_le_bytes(&b)?),
            None => None,
        };
        let history: Vec<HistoryEntry> = serde_json::from_str(&r.15)?;
        let merged_into = opt_blob_to_uuid(r.16)?;
        // ISS-033: decode embedding blob if present. Dim mismatch on read =
        // stale data from a dim-change that wasn't migrated; surface as
        // `Invariant` so the caller can decide (re-embed vs. abort).
        let embedding = entity_embedding_from_blob(r.17, self.embedding_dim)?;

        Ok(Some(Entity {
            id: id_decoded,
            canonical_name: r.1,
            kind,
            summary: r.3,
            attributes,
            history,
            merged_into,
            first_seen: unix_to_dt(r.5)?,
            last_seen: unix_to_dt(r.6)?,
            created_at: unix_to_dt(r.7)?,
            updated_at: unix_to_dt(r.8)?,
            // Provenance columns live in join tables (graph_memory_entity_mentions
            // and the episode-mention table). Phase 3 will materialize these on
            // read; Phase 2 returns empty vectors so callers see a consistent
            // shape and can already exercise the typed fields.
            episode_mentions: vec![],
            memory_mentions: vec![],
            activation: r.9,
            agent_affect,
            arousal: r.11,
            importance: r.12,
            identity_confidence: r.13,
            somatic_fingerprint,
            embedding,
        }))
    }

    fn update_entity_cognitive(
        &mut self,
        id: Uuid,
        activation: f64,
        importance: f64,
        identity_confidence: f64,
        agent_affect: Option<Json>,
    ) -> Result<(), GraphError> {
        // Bounds: schema CHECK enforces [0,1] but we surface the error as
        // `Invariant` rather than a raw rusqlite CHECK failure for caller
        // ergonomics (and because NaN slips past CHECK on some SQLite builds).
        for (name, v) in [
            ("activation", activation),
            ("importance", importance),
            ("identity_confidence", identity_confidence),
        ] {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                let _ = name; // name kept for future log-on-failure hook
                return Err(GraphError::Invariant(
                    "cognitive scalar out of [0.0, 1.0]",
                ));
            }
        }
        let agent_affect_json = match agent_affect {
            Some(v) => Some(serde_json::to_string(&v)?),
            None => None,
        };
        let now = dt_to_unix(Utc::now());
        let rows = self.conn.execute(
            "UPDATE graph_entities
             SET activation = ?1,
                 importance = ?2,
                 identity_confidence = ?3,
                 agent_affect = ?4,
                 updated_at = ?5
             WHERE id = ?6 AND namespace = ?7",
            rusqlite::params![
                activation,
                importance,
                identity_confidence,
                agent_affect_json,
                now,
                id.as_bytes().to_vec(),
                self.namespace,
            ],
        )?;
        if rows == 0 {
            return Err(GraphError::EntityNotFound(id));
        }
        Ok(())
    }

    fn touch_entity_last_seen(
        &mut self,
        id: Uuid,
        ts: DateTime<Utc>,
    ) -> Result<(), GraphError> {
        // Monotonic last_seen: never roll backwards. The CHECK
        // (first_seen <= last_seen) at the schema layer would also reject
        // a write that violates the row invariant, but we want a clear
        // EntityNotFound vs. a no-op-because-stale-ts distinction.
        let new_ts = dt_to_unix(ts);
        let rows = self.conn.execute(
            "UPDATE graph_entities
             SET last_seen = MAX(last_seen, ?1),
                 updated_at = ?1
             WHERE id = ?2 AND namespace = ?3",
            rusqlite::params![
                new_ts,
                id.as_bytes().to_vec(),
                self.namespace,
            ],
        )?;
        if rows == 0 {
            return Err(GraphError::EntityNotFound(id));
        }
        Ok(())
    }

    fn list_entities_by_kind(
        &self,
        kind: &EntityKind,
        limit: usize,
    ) -> Result<Vec<Entity>, GraphError> {
        // The `kind` column stores serde_json output (see kind_to_text).
        // We compare exact strings — `EntityKind::other` already normalizes
        // its input, so two `Other` values that should match collapse here too.
        let kind_text = kind_to_text(kind)?;

        // Reuse the get_entity row-decoding path: query ids, then fetch each
        // through `get_entity`. This costs N+1 round-trips but keeps
        // serialization logic in exactly one place. For the bulk-read paths
        // (Phase 3 traverse / edges_of) we'll hand-write a wider SELECT.
        let mut id_stmt = self.conn.prepare_cached(
            "SELECT id FROM graph_entities
             WHERE namespace = ?1 AND kind = ?2
             ORDER BY last_seen DESC
             LIMIT ?3",
        )?;
        let ids: Vec<Vec<u8>> = id_stmt
            .query_map(
                rusqlite::params![self.namespace, kind_text, limit as i64],
                |row| row.get::<_, Vec<u8>>(0),
            )?
            .collect::<Result<_, _>>()?;
        drop(id_stmt);

        let mut out = Vec::with_capacity(ids.len());
        for id_blob in ids {
            let id = Uuid::from_slice(&id_blob)
                .map_err(|_| GraphError::Invariant("uuid blob length != 16"))?;
            if let Some(e) = self.get_entity(id)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    // ---------------------------------- Candidate retrieval (ISS-033)

    fn update_entity_embedding(
        &mut self,
        id: Uuid,
        embedding: Option<&[f32]>,
    ) -> Result<(), GraphError> {
        // Validate + encode through the same codec as `insert_entity` so the
        // dim mismatch error message ("entity embedding dim mismatch") is
        // identical regardless of code path. `None` clears the column,
        // which is a legitimate path used by v03-migration cross-dim
        // rewrites — not an operational anti-pattern.
        let blob: Option<Vec<u8>> =
            entity_embedding_to_blob(embedding, self.embedding_dim)?;
        let now = dt_to_unix(Utc::now());

        let rows = self.conn.execute(
            "UPDATE graph_entities
             SET embedding = ?1,
                 updated_at = ?2
             WHERE id = ?3 AND namespace = ?4",
            rusqlite::params![
                blob,
                now,
                id.as_bytes().to_vec(),
                self.namespace,
            ],
        )?;
        if rows == 0 {
            return Err(GraphError::EntityNotFound(id));
        }
        Ok(())
    }

    fn search_candidates(
        &self,
        query: &CandidateQuery,
    ) -> Result<Vec<CandidateMatch>, GraphError> {
        // ---- Stage 1: hard cap on top_k. Caller-side `top_k` may be huge;
        // we always bound at MAX_TOP_K to keep memory and latency bounded
        // (§4.2 contract). top_k = 0 is a degenerate but legal request →
        // return empty quickly.
        let cap = query.top_k.min(MAX_TOP_K);
        if cap == 0 {
            return Ok(vec![]);
        }

        // ---- Stage 2: validate caller-supplied embedding dim before any
        // SQL work. Mismatch is the same `Invariant` everywhere — the
        // caller cannot ever supply a wrong-dim mention embedding without
        // a system-wide misconfiguration.
        if let Some(emb) = query.mention_embedding.as_deref() {
            if emb.len() != self.embedding_dim {
                return Err(GraphError::Invariant(
                    "entity embedding dim mismatch",
                ));
            }
        }

        // ---- Stage 3: candidate-id collection.
        //
        // We perform a single namespace-scoped SELECT that pulls every
        // entity (matching kind_filter, if any) along with the columns we
        // need for scoring. This is intentionally a brute-force scan — the
        // partial index `idx_graph_entities_embed_scan` (namespace,
        // last_seen DESC WHERE embedding IS NOT NULL) is the v0 strategy;
        // ANN / sqlite-vec is deferred (see trait doc).
        //
        // Why a single scan instead of two queries (alias-exact UNION
        // embedding-scan)?
        //   1. The two signal sets overlap heavily — an alias-matched
        //      entity often has an embedding too. UNION-ing forces a
        //      post-merge dedup pass.
        //   2. Recency is independent of both — we'd re-fetch `last_seen`
        //      for either path anyway.
        //   3. SQLite's planner picks the right index automatically; the
        //      WHERE-clause filter on namespace + (optional) kind is
        //      cheap.
        //
        // Stale-dim safety: if a row's embedding blob length doesn't match
        // `self.embedding_dim`, the codec returns `Invariant`. We
        // deliberately propagate that rather than skipping the row —
        // silently dropping stale rows would mask a migration bug. v03
        // migration's job is to sweep stale blobs before they reach this
        // path.

        let kind_filter_text: Option<String> = match &query.kind_filter {
            Some(k) => Some(kind_to_text(k)?),
            None => None,
        };

        // Normalized mention for the alias-exact path. We resolve aliases
        // in a second, indexed query rather than joining in SQL so the
        // single-row alias hit doesn't force the planner into a join over
        // the full entity scan.
        let alias_norm = normalize_alias(&query.mention_text);

        // Step 3a: alias-exact lookup — at most one entity per call (in v0
        // `resolve_alias` returns a single canonical_id). Multi-canonical
        // ambiguity is left to v03-resolution to detect (a future signal).
        let alias_hit_id: Option<Uuid> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT canonical_id FROM graph_entity_aliases
                 WHERE namespace = ?1 AND normalized = ?2
                 ORDER BY canonical_id ASC
                 LIMIT 1",
            )?;
            let blob: Option<Vec<u8>> = stmt
                .query_row(
                    rusqlite::params![&query.namespace, &alias_norm],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?;
            match blob {
                Some(b) => Some(
                    Uuid::from_slice(&b).map_err(|_| {
                        GraphError::Invariant("uuid blob length != 16")
                    })?,
                ),
                None => None,
            }
        };

        // Step 3b: scan candidate entities. We pull two cohorts:
        //   - The alias hit (if any) — always included regardless of
        //     embedding presence.
        //   - All entities in the same namespace (+ kind filter) that
        //     carry an embedding, IF the caller supplied
        //     `mention_embedding`. Without a mention embedding the
        //     embedding cohort is empty (no signal to compute).
        //
        // Skipping the embedding scan when `mention_embedding = None` is
        // an important optimization: it reduces a query that would
        // otherwise scan every entity to a single point lookup for the
        // alias hit.

        struct RawCandidate {
            id: Uuid,
            kind: EntityKind,
            canonical_name: String,
            last_seen: f64,
            identity_confidence: f64,
            embedding: Option<Vec<f32>>,
        }

        let mut raw: Vec<RawCandidate> = Vec::new();
        let mut seen_ids: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();

        // Alias hit fetch (if any).
        if let Some(aid) = alias_hit_id {
            let sql = match &kind_filter_text {
                Some(_) => {
                    "SELECT id, kind, canonical_name, last_seen,
                            identity_confidence, embedding
                     FROM graph_entities
                     WHERE id = ?1 AND namespace = ?2 AND kind = ?3"
                }
                None => {
                    "SELECT id, kind, canonical_name, last_seen,
                            identity_confidence, embedding
                     FROM graph_entities
                     WHERE id = ?1 AND namespace = ?2"
                }
            };
            let mut stmt = self.conn.prepare_cached(sql)?;
            let row_opt = if let Some(kt) = &kind_filter_text {
                stmt.query_row(
                    rusqlite::params![
                        aid.as_bytes().to_vec(),
                        &query.namespace,
                        kt
                    ],
                    Self::map_candidate_row,
                )
                .optional()?
            } else {
                stmt.query_row(
                    rusqlite::params![
                        aid.as_bytes().to_vec(),
                        &query.namespace
                    ],
                    Self::map_candidate_row,
                )
                .optional()?
            };
            if let Some((kind_text, name, ls, ic, eb)) = row_opt {
                let kind = text_to_kind(&kind_text)?;
                let embedding =
                    entity_embedding_from_blob(eb, self.embedding_dim)?;
                raw.push(RawCandidate {
                    id: aid,
                    kind,
                    canonical_name: name,
                    last_seen: ls,
                    identity_confidence: ic,
                    embedding,
                });
                seen_ids.insert(aid);
            }
            // If the alias points at a row that doesn't exist (FK-broken)
            // or fails the kind filter, we silently drop it — not our
            // concern here; v03-migration / merge_entities own integrity.
        }

        // Embedding-cohort fetch (only when caller supplied mention_embedding).
        if query.mention_embedding.is_some() {
            // We over-fetch (no LIMIT here): top_k is applied AFTER scoring,
            // so the SQL layer can't safely truncate without the cosine
            // computation. This is the brute-force step the future ANN
            // index will replace.
            let sql = match &kind_filter_text {
                Some(_) => {
                    "SELECT id, kind, canonical_name, last_seen,
                            identity_confidence, embedding
                     FROM graph_entities
                     WHERE namespace = ?1 AND kind = ?2
                       AND embedding IS NOT NULL"
                }
                None => {
                    "SELECT id, kind, canonical_name, last_seen,
                            identity_confidence, embedding
                     FROM graph_entities
                     WHERE namespace = ?1
                       AND embedding IS NOT NULL"
                }
            };
            let mut stmt = self.conn.prepare_cached(sql)?;
            let rows: Vec<CandidateScanRow> =
                if let Some(kt) = &kind_filter_text {
                    stmt.query_map(
                        rusqlite::params![&query.namespace, kt],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, f64>(4)?,
                                row.get::<_, Option<Vec<u8>>>(5)?,
                            ))
                        },
                    )?
                    .collect::<Result<_, _>>()?
                } else {
                    stmt.query_map(
                        rusqlite::params![&query.namespace],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, f64>(3)?,
                                row.get::<_, f64>(4)?,
                                row.get::<_, Option<Vec<u8>>>(5)?,
                            ))
                        },
                    )?
                    .collect::<Result<_, _>>()?
                };

            for (id_blob, kind_text, name, ls, ic, eb) in rows {
                let id = Uuid::from_slice(&id_blob).map_err(|_| {
                    GraphError::Invariant("uuid blob length != 16")
                })?;
                if seen_ids.contains(&id) {
                    continue;
                }
                let kind = text_to_kind(&kind_text)?;
                let embedding =
                    entity_embedding_from_blob(eb, self.embedding_dim)?;
                raw.push(RawCandidate {
                    id,
                    kind,
                    canonical_name: name,
                    last_seen: ls,
                    identity_confidence: ic,
                    embedding,
                });
                seen_ids.insert(id);
            }
        }

        if raw.is_empty() {
            return Ok(vec![]);
        }

        // ---- Stage 4: score each candidate.
        //
        // - alias_match: was this candidate fetched via the alias path?
        //   (`alias_hit_id == Some(c.id)` semantics, but we recorded it
        //   inline as the first row — index 0 of `raw`, *iff* alias_hit_id
        //   was set and the row was actually fetched.)
        // - embedding_score: cosine vs mention_embedding, or None.
        // - recency_score: linear decay over recency_window, clamped 0..=1.

        let mention_emb_norm = query
            .mention_embedding
            .as_ref()
            .map(|v| Self::l2_norm(v));

        // Recency window seconds; None = unbounded (we use the candidate
        // set's own (max_last_seen - min_last_seen) span as the scale).
        let recency_window_secs: Option<f64> =
            query.recency_window.map(|d| d.as_secs_f64());

        // For unbounded windows, derive the scale from the candidate set.
        let (min_ls, max_ls) = raw.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(lo, hi), c| (lo.min(c.last_seen), hi.max(c.last_seen)),
        );
        let unbounded_span = (max_ls - min_ls).max(1.0); // guard /0

        let mut scored: Vec<CandidateMatch> = raw
            .into_iter()
            .map(|c| {
                let alias_match = alias_hit_id == Some(c.id);
                let embedding_score = match (
                    query.mention_embedding.as_ref(),
                    c.embedding.as_ref(),
                ) {
                    (Some(mq), Some(ce)) => {
                        // Defensive dim check. Already guarded at decode,
                        // but if someone reuses these structs across stores
                        // configured for different dims, a runtime check
                        // here is cheap and prevents nonsense scores.
                        if mq.len() != ce.len() {
                            return Err(GraphError::Invariant(
                                "entity embedding dim mismatch",
                            ));
                        }
                        let mq_norm = mention_emb_norm.unwrap_or(1.0);
                        Some(Self::cosine(mq, ce, mq_norm))
                    }
                    _ => None,
                };
                let recency_score = match recency_window_secs {
                    Some(window) => {
                        let age = (query.now - c.last_seen).max(0.0);
                        if age >= window {
                            0.0_f32
                        } else {
                            (1.0 - age / window) as f32
                        }
                    }
                    None => {
                        // Unbounded: linear scale across the candidate-set
                        // span. Newest = 1.0, oldest = 0.0.
                        ((c.last_seen - min_ls) / unbounded_span) as f32
                    }
                };
                Ok(CandidateMatch {
                    entity_id: c.id,
                    kind: c.kind,
                    canonical_name: c.canonical_name,
                    alias_match,
                    embedding_score,
                    recency_score,
                    last_seen: c.last_seen,
                    identity_confidence: c.identity_confidence,
                })
            })
            .collect::<Result<Vec<_>, GraphError>>()?;

        // ---- Stage 5: bound output at `cap` and sort by entity_id for
        // determinism. Note: we do NOT sort by any signal — the contract
        // says callers must not rely on order being meaningful. Sorting
        // by id ascending is the cheapest stable deterministic order.
        scored.sort_by_key(|c| c.entity_id);
        if scored.len() > cap {
            scored.truncate(cap);
        }
        Ok(scored)
    }
    //
    // Phase 2 originally deferred these to a later slice; ISS-033 needs the
    // alias-exact lookup wired into `search_candidates`, so the stubs are
    // replaced with real impls here. Same write/read normalization
    // contract as v03-resolution: alias rows are keyed on
    // `entity::normalize_alias` output (lowercase + trim + NFKC), and
    // resolve_alias normalizes its argument identically. Asymmetric
    // normalization between writer and reader silently breaks the lookup,
    // so the entry points BOTH go through `normalize_alias`.
    //
    // The schema's PRIMARY KEY is (namespace, normalized, canonical_id):
    // - Same alias text (after normalization) pointing at the same canonical
    //   entity in the same namespace deduplicates on conflict (the alias
    //   surface form may have varied — we keep the most recent raw form via
    //   `ON CONFLICT DO UPDATE`).
    // - Same normalized form pointing at *different* canonical entities is
    //   permitted and represents an ambiguous alias — `resolve_alias` returns
    //   the first such row deterministically (lowest canonical_id), and
    //   v03-resolution treats this as a "needs disambiguation" signal.
    fn upsert_alias(
        &mut self,
        normalized: &str,
        alias_raw: &str,
        canonical_id: Uuid,
        source_episode: Option<Uuid>,
    ) -> Result<(), GraphError> {
        // Idempotent re-normalization: even if the caller already
        // normalized, doing it again is cheap and protects against caller
        // bugs (the index lookup is the single source of truth here).
        let norm = normalize_alias(normalized);
        let now = dt_to_unix(Utc::now());
        let canonical_blob = canonical_id.as_bytes().to_vec();
        let source_blob: Option<Vec<u8>> =
            source_episode.map(|u| u.as_bytes().to_vec());

        // ON CONFLICT: the row already exists for this (namespace,
        // normalized, canonical_id) triple. Refresh `alias` (raw form may
        // have varied across mentions) and leave `first_seen` /
        // `source_episode` alone (audit fields — first observation wins).
        self.conn.execute(
            "INSERT INTO graph_entity_aliases (
                normalized, canonical_id, alias,
                former_canonical_id, first_seen, source_episode, namespace
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)
            ON CONFLICT(namespace, normalized, canonical_id) DO UPDATE SET
                alias = excluded.alias",
            rusqlite::params![
                norm,
                canonical_blob,
                alias_raw,
                now,
                source_blob,
                self.namespace,
            ],
        )?;
        Ok(())
    }

    fn resolve_alias(
        &self,
        normalized: &str,
    ) -> Result<Option<Uuid>, GraphError> {
        let norm = normalize_alias(normalized);
        let mut stmt = self.conn.prepare_cached(
            "SELECT canonical_id FROM graph_entity_aliases
             WHERE namespace = ?1 AND normalized = ?2
             ORDER BY canonical_id ASC
             LIMIT 1",
        )?;
        let blob: Option<Vec<u8>> = stmt
            .query_row(
                rusqlite::params![self.namespace, norm],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        let Some(b) = blob else { return Ok(None) };
        let id = Uuid::from_slice(&b)
            .map_err(|_| GraphError::Invariant("uuid blob length != 16"))?;
        Ok(Some(id))
    }

    fn merge_entities(
        &mut self,
        winner: Uuid,
        loser: Uuid,
        batch_size: usize,
    ) -> Result<MergeReport, GraphError> {
        // Design §3.4 (Merge semantics) + §4.2 (resumable batched merges).
        //
        // Sequence:
        //   1. Validate inputs — both entities exist, same namespace,
        //      neither is its own self, loser is not already redirected
        //      somewhere else, winner is not redirected to loser, etc.
        //   2. **First batch tx** (only if not already begun): set
        //      `loser.merged_into = winner.id` + repoint loser's aliases'
        //      `canonical_id` to winner (preserve `former_canonical_id`).
        //      This is the redirect signal — once committed, readers
        //      doing entity-level lookups on the loser id transparently
        //      follow `merged_into` (see `get_entity`).
        //   3. **Edge-fanout batches**: iterate, each iteration picks up
        //      to `batch_size` live edges where loser is subject OR
        //      object, and for each: insert a successor edge with the
        //      loser slot replaced by winner; mark prior invalidated_by
        //      = successor.id. Each batch is its own transaction;
        //      between batches a reader may observe a partially-merged
        //      state (§8 "Reader semantics during merge").
        //   4. Stop when no live loser edges remain. Idempotent:
        //      re-calling is safe; if `loser.merged_into` is already set
        //      to winner, we skip step 2; if no live edges, loop exits
        //      immediately.
        //
        // Resumability: callers retry on transient failure (e.g.
        // SQLITE_BUSY); each batch either commits cleanly or rolls back
        // entirely, and the next call replays from current state.
        if winner == loser {
            return Err(GraphError::Invariant(
                "merge_entities: winner and loser must differ",
            ));
        }
        if batch_size == 0 {
            return Err(GraphError::Invariant(
                "merge_entities: batch_size must be > 0",
            ));
        }

        let winner_blob = winner.as_bytes().to_vec();
        let loser_blob = loser.as_bytes().to_vec();

        // ---- 1. Validation ----------------------------------------------
        // Both entities must exist in this namespace.
        let winner_present: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                "SELECT merged_into FROM graph_entities WHERE id = ?1 AND namespace = ?2",
                rusqlite::params![winner_blob, self.namespace],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?;
        let winner_merged_into = match winner_present {
            None => return Err(GraphError::EntityNotFound(winner)),
            Some(m) => opt_blob_to_uuid(m)?,
        };
        if winner_merged_into.is_some() {
            // Winner is itself a loser of a prior merge — refuse, the
            // caller should pick the ultimate winner explicitly.
            return Err(GraphError::Invariant(
                "merge_entities: winner is already merged into another entity",
            ));
        }

        let loser_present: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                "SELECT merged_into FROM graph_entities WHERE id = ?1 AND namespace = ?2",
                rusqlite::params![loser_blob, self.namespace],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?;
        let loser_merged_into = match loser_present {
            None => return Err(GraphError::EntityNotFound(loser)),
            Some(m) => opt_blob_to_uuid(m)?,
        };
        // Idempotence — `merged_into` already set:
        //  - If pointing at `winner`: continue (resume edge fanout).
        //  - If pointing elsewhere: refuse — different merge in flight.
        if let Some(prev_winner) = loser_merged_into {
            if prev_winner != winner {
                return Err(GraphError::Invariant(
                    "merge_entities: loser already merged into a different winner",
                ));
            }
        }

        let mut report = MergeReport {
            edges_superseded: 0,
            aliases_repointed: 0,
        };

        // ---- 2. First batch — set merged_into + repoint aliases --------
        // Skipped on resume (loser_merged_into already Some(winner)).
        if loser_merged_into.is_none() {
            let tx = self.conn.transaction()?;
            // 2a. Set loser.merged_into = winner.
            tx.execute(
                "UPDATE graph_entities
                 SET merged_into = ?1, updated_at = ?2
                 WHERE id = ?3 AND namespace = ?4",
                rusqlite::params![
                    winner_blob,
                    dt_to_unix(chrono::Utc::now()),
                    loser_blob,
                    self.namespace,
                ],
            )?;
            // 2b. Repoint loser's aliases. UPDATE rows where canonical_id =
            //     loser, set canonical_id = winner, former_canonical_id =
            //     loser (preserving the redirect chain). Skip rows that
            //     would collide with an existing (namespace, normalized,
            //     winner) PK — the alias is already known to winner; just
            //     drop the loser-pointing duplicate.
            //
            //     SQLite doesn't support UPDATE...ON CONFLICT in older
            //     versions, so we do it in two passes: DELETE colliders
            //     first, then UPDATE survivors.
            tx.execute(
                "DELETE FROM graph_entity_aliases
                 WHERE canonical_id = ?1
                   AND namespace = ?2
                   AND (namespace, normalized, ?3) IN (
                       SELECT namespace, normalized, canonical_id
                       FROM graph_entity_aliases
                       WHERE canonical_id = ?3 AND namespace = ?2
                   )",
                rusqlite::params![loser_blob, self.namespace, winner_blob],
            )?;
            let aliases_n = tx.execute(
                "UPDATE graph_entity_aliases
                 SET canonical_id = ?1,
                     former_canonical_id = COALESCE(former_canonical_id, ?2)
                 WHERE canonical_id = ?2 AND namespace = ?3",
                rusqlite::params![winner_blob, loser_blob, self.namespace],
            )?;
            report.aliases_repointed = aliases_n as u64;
            tx.commit()?;
        }

        // ---- 3. Edge fan-out batches ------------------------------------
        // Loop: each iteration pulls up to `batch_size` live edges where
        // loser appears as subject OR object_entity_id, and re-mints them
        // with winner in that slot. Continues until no live loser edges
        // remain.
        loop {
            // Read a batch of live edges referencing loser.
            // Use a UNION to combine the subject-side and object-side
            // candidates, capped at batch_size total. Order by
            // recorded_at ASC so older edges are processed first
            // (gives deterministic resumption).
            let mut stmt = self.conn.prepare_cached(
                "SELECT id, subject_id,
                        predicate_kind, predicate_label,
                        object_kind, object_entity_id, object_literal,
                        summary,
                        valid_from, valid_to, recorded_at, invalidated_at,
                        invalidated_by, supersedes,
                        episode_id, memory_id,
                        resolution_method,
                        activation, confidence,
                        agent_affect,
                        created_at
                 FROM graph_edges
                 WHERE namespace = ?1
                   AND invalidated_at IS NULL
                   AND (subject_id = ?2 OR (object_kind = 'entity' AND object_entity_id = ?2))
                 ORDER BY recorded_at ASC, id ASC
                 LIMIT ?3",
            )?;
            let cols: Vec<EdgeRowColumns> = stmt
                .query_map(
                    rusqlite::params![self.namespace, loser_blob, batch_size as i64],
                    row_to_edge_columns,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);

            if cols.is_empty() {
                break;
            }

            let prior_edges: Vec<Edge> =
                cols.into_iter().map(decode_edge_row).collect::<Result<Vec<_>, _>>()?;

            // Re-mint each prior with loser → winner remapping.
            let tx = self.conn.transaction()?;
            let now = chrono::Utc::now();
            let now_unix = dt_to_unix(now);
            for prior in &prior_edges {
                // Build successor with loser slot replaced.
                let new_subject = if prior.subject_id == loser {
                    winner
                } else {
                    prior.subject_id
                };
                let new_object = match &prior.object {
                    EdgeEnd::Entity { id } if *id == loser => EdgeEnd::Entity { id: winner },
                    other => other.clone(),
                };
                // Mint a new edge that mirrors prior except for the
                // loser slot, with `supersedes = Some(prior.id)` and a
                // fresh id + recorded_at = now. We don't change other
                // bi-temporal fields (`valid_from`/`valid_to`) — the
                // belief itself is unchanged, only the entity reference
                // is updated.
                let mut successor = Edge::new(
                    new_subject,
                    prior.predicate.clone(),
                    new_object,
                    prior.valid_from,
                    now,
                );
                successor.summary = prior.summary.clone();
                successor.valid_to = prior.valid_to;
                successor.activation = prior.activation;
                successor.confidence = prior.confidence;
                successor.agent_affect = prior.agent_affect.clone();
                successor.episode_id = prior.episode_id;
                successor.memory_id = prior.memory_id.clone();
                successor.resolution_method = prior.resolution_method.clone();
                successor.supersedes = Some(prior.id);
                successor.validate()?;

                // INSERT successor.
                let (predicate_kind, predicate_label) =
                    predicate_to_columns(&successor.predicate)?;
                let (object_kind, object_entity_blob, object_literal) =
                    edge_end_to_columns(&successor.object)?;
                let resolution_text = resolution_method_to_text(&successor.resolution_method)?;
                let agent_affect_json = match &successor.agent_affect {
                    Some(v) => Some(serde_json::to_string(v)?),
                    None => None,
                };
                tx.execute(
                    "INSERT INTO graph_edges (
                        id, subject_id,
                        predicate_kind, predicate_label,
                        object_kind, object_entity_id, object_literal,
                        summary,
                        valid_from, valid_to, recorded_at, invalidated_at,
                        invalidated_by, supersedes,
                        episode_id, memory_id,
                        resolution_method,
                        activation, confidence,
                        agent_affect,
                        created_at, namespace
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                        ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                        ?17, ?18, ?19, ?20, ?21, ?22
                    )",
                    rusqlite::params![
                        successor.id.as_bytes().to_vec(),
                        successor.subject_id.as_bytes().to_vec(),
                        predicate_kind,
                        predicate_label,
                        object_kind,
                        object_entity_blob,
                        object_literal,
                        successor.summary,
                        opt_dt_to_unix(successor.valid_from),
                        opt_dt_to_unix(successor.valid_to),
                        dt_to_unix(successor.recorded_at),
                        opt_dt_to_unix(successor.invalidated_at),
                        successor.invalidated_by.map(|u| u.as_bytes().to_vec()),
                        successor.supersedes.map(|u| u.as_bytes().to_vec()),
                        successor.episode_id.map(|u| u.as_bytes().to_vec()),
                        successor.memory_id,
                        resolution_text,
                        successor.activation,
                        successor.confidence,
                        agent_affect_json,
                        dt_to_unix(successor.created_at),
                        self.namespace,
                    ],
                )?;
                // Invalidate prior in this same tx.
                tx.execute(
                    "UPDATE graph_edges
                     SET invalidated_at = ?1, invalidated_by = ?2
                     WHERE id = ?3 AND namespace = ?4
                       AND invalidated_at IS NULL",
                    rusqlite::params![
                        now_unix,
                        successor.id.as_bytes().to_vec(),
                        prior.id.as_bytes().to_vec(),
                        self.namespace,
                    ],
                )?;
                report.edges_superseded += 1;
            }
            tx.commit()?;

            // Telemetry: a batch of N consolidation writes.
            self.sink
                .emit_operational_load("merge_entities_batch", prior_edges.len() as u32);

            if prior_edges.len() < batch_size {
                // We pulled less than a full batch — no more work.
                break;
            }
        }

        Ok(report)
    }

    // -------------------------------------------------- Edge (Phase 2/3)
    fn insert_edge(&mut self, edge: &Edge) -> Result<(), GraphError> {
        // Type-level invariants first — this is the only writer-side
        // validation we control. SQL CHECKs handle the structural XOR
        // (object_kind ↔ object_entity_id/object_literal) and the
        // valid_from ≤ valid_to bound, but bound-checking f64s in Rust
        // gives a typed `GraphError::Invariant` with a meaningful message
        // instead of a raw `SqliteFailure(CHECK constraint failed)`.
        edge.validate()?;

        let (predicate_kind, predicate_label) = predicate_to_columns(&edge.predicate)?;
        let (object_kind, object_entity_blob, object_literal) =
            edge_end_to_columns(&edge.object)?;
        let resolution_text = resolution_method_to_text(&edge.resolution_method)?;
        let agent_affect_json = match &edge.agent_affect {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };

        // Note: `confidence_source` is not persisted in this slice — §4.1
        // does not currently carry a column for it. On read the field is
        // re-defaulted to `Recovered` (the post-write canonical value for
        // non-`Migrated` edges). See DevNote #5 in the file header for the
        // rationale and the planned schema follow-up.
        self.conn.execute(
            "INSERT INTO graph_edges (
                id, subject_id,
                predicate_kind, predicate_label,
                object_kind, object_entity_id, object_literal,
                summary,
                valid_from, valid_to, recorded_at, invalidated_at,
                invalidated_by, supersedes,
                episode_id, memory_id,
                resolution_method,
                activation, confidence,
                agent_affect,
                created_at, namespace
            ) VALUES (
                ?1, ?2,
                ?3, ?4,
                ?5, ?6, ?7,
                ?8,
                ?9, ?10, ?11, ?12,
                ?13, ?14,
                ?15, ?16,
                ?17,
                ?18, ?19,
                ?20,
                ?21, ?22
            )",
            rusqlite::params![
                edge.id.as_bytes().to_vec(),
                edge.subject_id.as_bytes().to_vec(),
                predicate_kind,
                predicate_label,
                object_kind,
                object_entity_blob,
                object_literal,
                edge.summary,
                opt_dt_to_unix(edge.valid_from),
                opt_dt_to_unix(edge.valid_to),
                dt_to_unix(edge.recorded_at),
                opt_dt_to_unix(edge.invalidated_at),
                edge.invalidated_by.map(|u| u.as_bytes().to_vec()),
                edge.supersedes.map(|u| u.as_bytes().to_vec()),
                edge.episode_id.map(|u| u.as_bytes().to_vec()),
                edge.memory_id,
                resolution_text,
                edge.activation,
                edge.confidence,
                agent_affect_json,
                dt_to_unix(edge.created_at),
                self.namespace,
            ],
        )?;
        Ok(())
    }
    fn get_edge(&self, id: Uuid) -> Result<Option<Edge>, GraphError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, subject_id,
                    predicate_kind, predicate_label,
                    object_kind, object_entity_id, object_literal,
                    summary,
                    valid_from, valid_to, recorded_at, invalidated_at,
                    invalidated_by, supersedes,
                    episode_id, memory_id,
                    resolution_method,
                    activation, confidence,
                    agent_affect,
                    created_at
             FROM graph_edges
             WHERE id = ?1 AND namespace = ?2",
        )?;
        let row = stmt
            .query_row(
                rusqlite::params![id.as_bytes().to_vec(), self.namespace],
                row_to_edge_columns,
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some(cols) => decode_edge_row(cols).map(Some),
        }
    }
    fn find_edges(
        &self,
        subject_id: Uuid,
        predicate: &Predicate,
        object: Option<&EdgeEnd>,
        valid_only: bool,
    ) -> Result<Vec<Edge>, GraphError> {
        // ISS-034 + ISS-035: dual-mode lookup.
        //
        // - **Triple lookup** (`object = Some(_)`): per-triple match,
        //   `idx_graph_edges_spo`-backed, hard-capped at
        //   `MAX_FIND_EDGES_RESULTS` (logically 0-1 live edges).
        // - **Slot lookup** (`object = None`): all live/historical edges
        //   for `(subject, predicate)`. Required by the resolution
        //   pipeline (v03-resolution §3.4.4) to detect object change for
        //   functional predicates and to identify already-known objects
        //   for multi-valued predicates. **No row cap** — slot semantics
        //   demands the complete set; truncation would cause spurious
        //   `Add` decisions.
        let (predicate_kind, predicate_label) = predicate_to_columns(predicate)?;
        let subject_blob = subject_id.as_bytes().to_vec();

        let cols: Vec<EdgeRowColumns> = match object {
            None => {
                // ----- Slot lookup: (subject, predicate) -----
                //
                // Index: when `valid_only=true`, SQLite picks
                // `idx_graph_edges_live` (partial index over active edges
                // ordered by `(subject_id, predicate_kind, predicate_label,
                // recorded_at DESC)`). When `valid_only=false`, falls back
                // to `idx_graph_edges_spo`. No `LIMIT` — slot lookup must
                // return the complete set (ISS-035).
                let sql_live = "SELECT id, subject_id,
                                       predicate_kind, predicate_label,
                                       object_kind, object_entity_id, object_literal,
                                       summary,
                                       valid_from, valid_to, recorded_at, invalidated_at,
                                       invalidated_by, supersedes,
                                       episode_id, memory_id,
                                       resolution_method,
                                       activation, confidence,
                                       agent_affect,
                                       created_at
                                FROM graph_edges
                                WHERE subject_id = ?1 AND namespace = ?2
                                  AND predicate_kind = ?3 AND predicate_label = ?4
                                  AND invalidated_at IS NULL
                                ORDER BY recorded_at DESC, id ASC";
                let sql_all = "SELECT id, subject_id,
                                      predicate_kind, predicate_label,
                                      object_kind, object_entity_id, object_literal,
                                      summary,
                                      valid_from, valid_to, recorded_at, invalidated_at,
                                      invalidated_by, supersedes,
                                      episode_id, memory_id,
                                      resolution_method,
                                      activation, confidence,
                                      agent_affect,
                                      created_at
                               FROM graph_edges
                               WHERE subject_id = ?1 AND namespace = ?2
                                 AND predicate_kind = ?3 AND predicate_label = ?4
                               ORDER BY recorded_at DESC, id ASC";
                let mut stmt = self
                    .conn
                    .prepare_cached(if valid_only { sql_live } else { sql_all })?;
                let v: Vec<EdgeRowColumns> = stmt
                    .query_map(
                        rusqlite::params![
                            subject_blob,
                            self.namespace,
                            predicate_kind,
                            predicate_label,
                        ],
                        row_to_edge_columns,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                v
            }
            Some(obj) => {
                // ----- Triple lookup: (subject, predicate, object) -----
                let (object_kind, object_entity_blob, object_literal) =
                    edge_end_to_columns(obj)?;
                let cap = MAX_FIND_EDGES_RESULTS as i64;

                match (&object_entity_blob, &object_literal) {
            (Some(obj_blob), None) => {
                // Entity-object: full index hit.
                let sql_live = "SELECT id, subject_id,
                                       predicate_kind, predicate_label,
                                       object_kind, object_entity_id, object_literal,
                                       summary,
                                       valid_from, valid_to, recorded_at, invalidated_at,
                                       invalidated_by, supersedes,
                                       episode_id, memory_id,
                                       resolution_method,
                                       activation, confidence,
                                       agent_affect,
                                       created_at
                                FROM graph_edges
                                WHERE subject_id = ?1 AND namespace = ?2
                                  AND predicate_kind = ?3 AND predicate_label = ?4
                                  AND object_kind = ?5 AND object_entity_id = ?6
                                  AND invalidated_at IS NULL
                                ORDER BY recorded_at DESC, id ASC
                                LIMIT ?7";
                let sql_all = "SELECT id, subject_id,
                                      predicate_kind, predicate_label,
                                      object_kind, object_entity_id, object_literal,
                                      summary,
                                      valid_from, valid_to, recorded_at, invalidated_at,
                                      invalidated_by, supersedes,
                                      episode_id, memory_id,
                                      resolution_method,
                                      activation, confidence,
                                      agent_affect,
                                      created_at
                               FROM graph_edges
                               WHERE subject_id = ?1 AND namespace = ?2
                                 AND predicate_kind = ?3 AND predicate_label = ?4
                                 AND object_kind = ?5 AND object_entity_id = ?6
                               ORDER BY recorded_at DESC, id ASC
                               LIMIT ?7";
                let mut stmt = self
                    .conn
                    .prepare_cached(if valid_only { sql_live } else { sql_all })?;
                let v: Vec<EdgeRowColumns> = stmt
                    .query_map(
                        rusqlite::params![
                            subject_blob,
                            self.namespace,
                            predicate_kind,
                            predicate_label,
                            object_kind,
                            obj_blob,
                            cap,
                        ],
                        row_to_edge_columns,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                v
            }
            (None, Some(lit_text)) => {
                // Literal-object: index narrows on (subj, pred, 'literal', NULL);
                // literal text equality is an in-row filter.
                let sql_live = "SELECT id, subject_id,
                                       predicate_kind, predicate_label,
                                       object_kind, object_entity_id, object_literal,
                                       summary,
                                       valid_from, valid_to, recorded_at, invalidated_at,
                                       invalidated_by, supersedes,
                                       episode_id, memory_id,
                                       resolution_method,
                                       activation, confidence,
                                       agent_affect,
                                       created_at
                                FROM graph_edges
                                WHERE subject_id = ?1 AND namespace = ?2
                                  AND predicate_kind = ?3 AND predicate_label = ?4
                                  AND object_kind = 'literal'
                                  AND object_literal = ?5
                                  AND invalidated_at IS NULL
                                ORDER BY recorded_at DESC, id ASC
                                LIMIT ?6";
                let sql_all = "SELECT id, subject_id,
                                      predicate_kind, predicate_label,
                                      object_kind, object_entity_id, object_literal,
                                      summary,
                                      valid_from, valid_to, recorded_at, invalidated_at,
                                      invalidated_by, supersedes,
                                      episode_id, memory_id,
                                      resolution_method,
                                      activation, confidence,
                                      agent_affect,
                                      created_at
                               FROM graph_edges
                               WHERE subject_id = ?1 AND namespace = ?2
                                 AND predicate_kind = ?3 AND predicate_label = ?4
                                 AND object_kind = 'literal'
                                 AND object_literal = ?5
                               ORDER BY recorded_at DESC, id ASC
                               LIMIT ?6";
                let mut stmt = self
                    .conn
                    .prepare_cached(if valid_only { sql_live } else { sql_all })?;
                let v: Vec<EdgeRowColumns> = stmt
                    .query_map(
                        rusqlite::params![
                            subject_blob,
                            self.namespace,
                            predicate_kind,
                            predicate_label,
                            lit_text,
                            cap,
                        ],
                        row_to_edge_columns,
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                v
            }
            // edge_end_to_columns produces exactly one of (Some, None) or
            // (None, Some); this match arm is unreachable in practice.
            _ => return Err(GraphError::Invariant("EdgeEnd encoded with neither entity nor literal")),
                }
            }
        };

        cols.into_iter().map(decode_edge_row).collect()
    }
    fn invalidate_edge(
        &mut self,
        prior_id: Uuid,
        successor_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), GraphError> {
        // ISS-034: GUARD-3 primitive. Single UPDATE that closes out a prior
        // edge by setting `invalidated_at` and `invalidated_by`. Idempotent
        // when called twice with the same `successor_id`; errors when the
        // prior is already closed by a different successor.
        let prior_blob = prior_id.as_bytes().to_vec();
        let succ_blob = successor_id.as_bytes().to_vec();
        let now_unix = dt_to_unix(now);

        // Read current state (under the same connection — the caller is
        // expected to wrap this in a transaction via `with_graph_tx` when
        // ordering matters; the read+update here is itself atomic under
        // SQLite's serializable WAL semantics for a single connection).
        let mut stmt = self.conn.prepare_cached(
            "SELECT invalidated_at, invalidated_by
             FROM graph_edges
             WHERE id = ?1 AND namespace = ?2",
        )?;
        let row: Option<(Option<f64>, Option<Vec<u8>>)> = stmt
            .query_row(
                rusqlite::params![prior_blob, self.namespace],
                |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)),
            )
            .optional()?;

        let (existing_at, existing_by) = match row {
            None => return Err(GraphError::EdgeNotFound(prior_id)),
            Some(t) => t,
        };

        if let Some(_at) = existing_at {
            // Already closed. Idempotent only if `invalidated_by` matches.
            match existing_by {
                Some(by) if by == succ_blob => return Ok(()),
                _ => {
                    return Err(GraphError::Invariant(
                        "edge already invalidated by another successor",
                    ))
                }
            }
        }

        // Verify successor exists (FK on `invalidated_by`); the FK would
        // catch this at commit time, but checking up-front gives a typed
        // error instead of a SQLite constraint failure.
        let mut stmt_succ = self
            .conn
            .prepare_cached("SELECT 1 FROM graph_edges WHERE id = ?1 AND namespace = ?2")?;
        let succ_exists: Option<i64> = stmt_succ
            .query_row(rusqlite::params![succ_blob, self.namespace], |r| r.get::<_, i64>(0))
            .optional()?;
        if succ_exists.is_none() {
            return Err(GraphError::EdgeNotFound(successor_id));
        }

        let mut stmt_upd = self.conn.prepare_cached(
            "UPDATE graph_edges
             SET invalidated_at = ?1, invalidated_by = ?2
             WHERE id = ?3 AND namespace = ?4
               AND invalidated_at IS NULL",
        )?;
        let n = stmt_upd.execute(rusqlite::params![
            now_unix,
            succ_blob,
            prior_blob,
            self.namespace,
        ])?;
        if n == 0 {
            // Concurrent writer closed it between our SELECT and UPDATE.
            return Err(GraphError::Invariant(
                "edge already invalidated by another successor",
            ));
        }
        Ok(())
    }
    fn supersede_edge(
        &mut self,
        old: Uuid,
        successor: &Edge,
        at: DateTime<Utc>,
    ) -> Result<(), GraphError> {
        // §4.2 spec: convenience wrapper composing `insert_edge(successor)` +
        // `invalidate_edge(old, successor.id, at)` inside a single transaction.
        // Both writes commit together or neither does. Used by ad-hoc callers
        // that don't need the decomposed primitives the resolution pipeline
        // (§3.5) uses to batch successor inserts before invalidations.
        //
        // Invariant: `successor.supersedes` SHOULD point at `old`. We do not
        // enforce this — the caller controls the chain wiring per §3.4 — but
        // the FK on `invalidated_by` is enforced by `invalidate_edge` (looks
        // up `successor_id` after insert).
        //
        // Tx model: rusqlite's `Transaction` guard-rolls back on Drop unless
        // explicitly committed. We mirror the pattern used by
        // `record_extraction_failure` / `mark_failure_resolved` (a `tx =
        // self.conn.transaction()?` followed by manual commit on success).
        // Insert + invalidate both run on `self.conn` inside the same physical
        // transaction because rusqlite doesn't open a second connection — the
        // outer transaction's writes are visible to the inner SQL.

        // Verify successor wiring matches `old` if the caller set it. A
        // mismatched `supersedes` is a programming error; failing fast with
        // `Invariant` is preferable to silently writing a corrupt chain.
        if let Some(s) = successor.supersedes {
            if s != old {
                return Err(GraphError::Invariant(
                    "supersede_edge: successor.supersedes does not match `old`",
                ));
            }
        }

        let tx = self.conn.transaction()?;
        // Within the transaction, we drop our `&mut self.conn` borrow into a
        // raw `&Transaction` and call SQL directly. We can't call our own
        // trait methods through `&mut self` because the connection is now
        // owned by `tx`. Instead, we replay the relevant CRUD inline. This is
        // a small amount of duplication but avoids re-entrant borrows of
        // `self.conn`.

        // --- 1. INSERT successor edge (mirrors `insert_edge`) ---
        successor.validate()?;
        let (predicate_kind, predicate_label) = predicate_to_columns(&successor.predicate)?;
        let (object_kind, object_entity_blob, object_literal) =
            edge_end_to_columns(&successor.object)?;
        let resolution_text = resolution_method_to_text(&successor.resolution_method)?;
        let agent_affect_json = match &successor.agent_affect {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        tx.execute(
            "INSERT INTO graph_edges (
                id, subject_id,
                predicate_kind, predicate_label,
                object_kind, object_entity_id, object_literal,
                summary,
                valid_from, valid_to, recorded_at, invalidated_at,
                invalidated_by, supersedes,
                episode_id, memory_id,
                resolution_method,
                activation, confidence,
                agent_affect,
                created_at, namespace
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22
            )",
            rusqlite::params![
                successor.id.as_bytes().to_vec(),
                successor.subject_id.as_bytes().to_vec(),
                predicate_kind,
                predicate_label,
                object_kind,
                object_entity_blob,
                object_literal,
                successor.summary,
                opt_dt_to_unix(successor.valid_from),
                opt_dt_to_unix(successor.valid_to),
                dt_to_unix(successor.recorded_at),
                opt_dt_to_unix(successor.invalidated_at),
                successor.invalidated_by.map(|u| u.as_bytes().to_vec()),
                successor.supersedes.map(|u| u.as_bytes().to_vec()),
                successor.episode_id.map(|u| u.as_bytes().to_vec()),
                successor.memory_id,
                resolution_text,
                successor.activation,
                successor.confidence,
                agent_affect_json,
                dt_to_unix(successor.created_at),
                self.namespace,
            ],
        )?;

        // --- 2. INVALIDATE prior edge (mirrors `invalidate_edge`) ---
        let prior_blob = old.as_bytes().to_vec();
        let succ_blob = successor.id.as_bytes().to_vec();
        let now_unix = dt_to_unix(at);

        // Read current invalidation state.
        let row: Option<(Option<f64>, Option<Vec<u8>>)> = tx
            .query_row(
                "SELECT invalidated_at, invalidated_by
                 FROM graph_edges
                 WHERE id = ?1 AND namespace = ?2",
                rusqlite::params![prior_blob, self.namespace],
                |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)),
            )
            .optional()?;
        let (existing_at, existing_by) = match row {
            None => return Err(GraphError::EdgeNotFound(old)),
            Some(t) => t,
        };
        if existing_at.is_some() {
            // Already closed — idempotent only if same successor.
            match existing_by {
                Some(by) if by == succ_blob => {
                    tx.commit()?;
                    return Ok(());
                }
                _ => {
                    return Err(GraphError::Invariant(
                        "edge already invalidated by another successor",
                    ))
                }
            }
        }
        let n = tx.execute(
            "UPDATE graph_edges
             SET invalidated_at = ?1, invalidated_by = ?2
             WHERE id = ?3 AND namespace = ?4
               AND invalidated_at IS NULL",
            rusqlite::params![now_unix, succ_blob, prior_blob, self.namespace],
        )?;
        if n == 0 {
            return Err(GraphError::Invariant(
                "edge already invalidated by another successor",
            ));
        }

        tx.commit()?;
        Ok(())
    }
    fn edges_of(
        &self, subject: Uuid, predicate: Option<&Predicate>, include_invalidated: bool,
    ) -> Result<Vec<Edge>, GraphError> {
        // Four SQL shapes: predicate × include_invalidated. Each branch
        // builds its own prepared-cached statement and `collect`s into
        // `Vec<EdgeRowColumns>` *inside the branch* so the borrow on
        // `stmt` ends before the block does (otherwise the MappedRows
        // temporary outlives `stmt` and rustc rejects it).
        //
        // Indexes used:
        //   - predicate + live      → `idx_graph_edges_live`
        //                             (partial: WHERE invalidated_at IS NULL)
        //   - predicate + all       → `idx_graph_edges_subject_pred_recorded`
        //   - no predicate + any    → `idx_graph_edges_subject`
        let subject_blob = subject.as_bytes().to_vec();
        let cols: Vec<EdgeRowColumns> = match predicate {
            Some(p) => {
                let (kind, label) = predicate_to_columns(p)?;
                if include_invalidated {
                    let mut stmt = self.conn.prepare_cached(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                           AND predicate_kind = ?3 AND predicate_label = ?4
                         ORDER BY recorded_at DESC, id ASC",
                    )?;
                    let v: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params![subject_blob, self.namespace, kind, label],
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    v
                } else {
                    let mut stmt = self.conn.prepare_cached(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                           AND predicate_kind = ?3 AND predicate_label = ?4
                           AND invalidated_at IS NULL
                         ORDER BY recorded_at DESC, id ASC",
                    )?;
                    let v: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params![subject_blob, self.namespace, kind, label],
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    v
                }
            }
            None => {
                if include_invalidated {
                    let mut stmt = self.conn.prepare_cached(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                         ORDER BY recorded_at DESC, id ASC",
                    )?;
                    let v: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params![subject_blob, self.namespace],
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    v
                } else {
                    let mut stmt = self.conn.prepare_cached(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                           AND invalidated_at IS NULL
                         ORDER BY recorded_at DESC, id ASC",
                    )?;
                    let v: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params![subject_blob, self.namespace],
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    v
                }
            }
        };
        cols.into_iter().map(decode_edge_row).collect()
    }
    fn edges_as_of(&self, subject: Uuid, at: DateTime<Utc>) -> Result<Vec<Edge>, GraphError> {
        // GOAL-1.5: "what do I believe is true about `subject` as of real-world
        // time `at`?". Per design §4.2:
        //
        //   For each `(subject_id, predicate_label, object)` window, select the
        //   row with the largest `recorded_at <= at` such that
        //     - `valid_from IS NULL OR valid_from <= at`
        //     - `valid_to   IS NULL OR valid_to   >  at`
        //     - `invalidated_at IS NULL OR invalidated_at > at`
        //
        // Backed by `idx_graph_edges_subject_pred_recorded`
        // (subject_id, predicate_label, recorded_at DESC).
        //
        // Implementation: a SQL window function picks the freshest row per
        // group (`ROW_NUMBER() ... ORDER BY recorded_at DESC`) and the outer
        // SELECT keeps only `rn = 1`. SQLite ≥3.25 supports window funcs
        // (rusqlite ships modern SQLite; storage.rs requires it).
        //
        // The "object window" is keyed on the full `(object_kind,
        // object_entity_id, object_literal)` triple — for entity-objects only
        // `object_entity_id` participates; for literal-objects we partition on
        // the canonical-literal text. That matches the §4.2 invariant
        // "different object value ⇒ different window".
        let subject_blob = subject.as_bytes().to_vec();
        let at_unix = dt_to_unix(at);

        let mut stmt = self.conn.prepare_cached(
            "WITH ranked AS (
                SELECT
                    id, subject_id,
                    predicate_kind, predicate_label,
                    object_kind, object_entity_id, object_literal,
                    summary,
                    valid_from, valid_to, recorded_at, invalidated_at,
                    invalidated_by, supersedes,
                    episode_id, memory_id,
                    resolution_method,
                    activation, confidence,
                    agent_affect,
                    created_at,
                    ROW_NUMBER() OVER (
                        PARTITION BY predicate_kind, predicate_label,
                                     object_kind, object_entity_id, object_literal
                        ORDER BY recorded_at DESC, id ASC
                    ) AS rn
                FROM graph_edges
                WHERE subject_id = ?1
                  AND namespace = ?2
                  AND recorded_at <= ?3
                  AND (valid_from IS NULL OR valid_from <= ?3)
                  AND (valid_to   IS NULL OR valid_to   >  ?3)
                  AND (invalidated_at IS NULL OR invalidated_at > ?3)
            )
            SELECT
                id, subject_id,
                predicate_kind, predicate_label,
                object_kind, object_entity_id, object_literal,
                summary,
                valid_from, valid_to, recorded_at, invalidated_at,
                invalidated_by, supersedes,
                episode_id, memory_id,
                resolution_method,
                activation, confidence,
                agent_affect,
                created_at
            FROM ranked
            WHERE rn = 1
            ORDER BY recorded_at DESC, id ASC",
        )?;
        let cols: Vec<EdgeRowColumns> = stmt
            .query_map(
                rusqlite::params![subject_blob, self.namespace, at_unix],
                row_to_edge_columns,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        cols.into_iter().map(decode_edge_row).collect()
    }
    fn traverse(
        &self,
        start: Uuid,
        max_depth: usize,
        max_results: usize,
        predicate_filter: &[Predicate],
    ) -> Result<Vec<(Uuid, Edge)>, GraphError> {
        // GOAL-1.9 + design §4.2: BFS from `start` over **canonical** predicates
        // only. Proposed predicates short-circuit with `MalformedPredicate`.
        //
        // Contract recap:
        //   - Live edges only (`invalidated_at IS NULL`).
        //   - Visited set keyed on entity id; an entity is visited at most once
        //     even if reached via multiple edges.
        //   - BFS by depth; within a depth level edges are yielded in
        //     descending `activation`, ties by descending `recorded_at`.
        //   - Symmetric / inverse predicates traverse both directions:
        //       Symmetric        → match by `(predicate)`, walk subject ↔ object.
        //       Directed{inverse}→ outgoing on `predicate`; if inverse is set,
        //                          incoming on `inverse(predicate)` is also a
        //                          forward step.
        //   - Bounded by `max_depth` (hops, not edges) and `max_results`
        //     (total `(entity, edge)` pairs returned).
        //   - `predicate_filter == &[]` is interpreted as "all canonical
        //     predicates" (otherwise traversal would be empty — useless).

        // 1. Reject proposed predicates up-front (typed error rather than a
        //    silent no-result).
        for p in predicate_filter {
            if let Predicate::Proposed(label) = p {
                return Err(GraphError::MalformedPredicate(format!(
                    "traverse: proposed predicate '{}' is not allowed (canonical only)",
                    label
                )));
            }
        }

        if max_depth == 0 || max_results == 0 {
            return Ok(Vec::new());
        }

        // 2. Build the SQL "predicate IN (...)" filter once. When
        //    `predicate_filter` is empty we omit the filter (== all canonical).
        //    Each canonical predicate contributes a single (kind, label) row.
        use crate::graph::schema::{directionality, CanonicalPredicate, Directionality};

        let allowed_canonical: Vec<&CanonicalPredicate> = if predicate_filter.is_empty() {
            // No filter ⇒ "all canonical predicates currently in the catalog".
            // We materialize this list lazily via the `CanonicalPredicate`
            // exhaustive variant set used by the schema module. To avoid
            // duplicating the list, we use a tag check: the directionality
            // lookup is exhaustive, so any seen predicate-label whose
            // `classify` returns `Canonical(_)` is allowed.
            Vec::new()
        } else {
            predicate_filter
                .iter()
                .filter_map(|p| match p {
                    Predicate::Canonical(c) => Some(c),
                    Predicate::Proposed(_) => None, // already rejected above
                })
                .collect()
        };

        // 3. BFS state.
        //
        //    `visited`  - entities already enqueued or yielded as a frontier
        //                 source. Includes `start` so we don't loop back.
        //    `frontier` - current depth's entity ids, in order of insertion.
        //    `output`   - accumulated `(entity, edge)` pairs.
        let mut visited: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        visited.insert(start);
        let mut frontier: Vec<Uuid> = vec![start];
        let mut output: Vec<(Uuid, Edge)> = Vec::new();

        // Prepare the per-step query once; we re-bind `subject_id` per
        // frontier node. The query selects all live outgoing edges for the
        // node, optionally filtered by predicate. We sort by activation DESC,
        // recorded_at DESC inside SQL for the "most salient first" property.
        //
        // For incoming edges (symmetric / inverse case), we issue a second
        // query keyed on `object_entity_id`.
        for _depth in 0..max_depth {
            if output.len() >= max_results {
                break;
            }
            let mut next_frontier: Vec<Uuid> = Vec::new();

            for node in &frontier {
                if output.len() >= max_results {
                    break;
                }
                let node_blob = node.as_bytes().to_vec();

                // 3a. Outgoing edges (subject = node). For each canonical
                //     predicate variant in the filter — or all canonical if
                //     no filter — collect candidate edges, then process.
                let outgoing: Vec<EdgeRowColumns> = if allowed_canonical.is_empty() {
                    // No filter: select all live edges; we'll filter to
                    // canonical in Rust by re-classifying each row's
                    // (kind, label).
                    let mut stmt = self.conn.prepare_cached(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                           AND invalidated_at IS NULL
                           AND predicate_kind = 'canonical'
                         ORDER BY activation DESC, recorded_at DESC, id ASC",
                    )?;
                    let rows: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params![node_blob, self.namespace],
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                } else {
                    // Predicate filter present — bind labels via repeated
                    // OR-clauses. The catalog is small (≤20 canonical preds)
                    // and typical filters are ≤3 entries; hand-built IN
                    // clause from the typed enum, never from caller strings.
                    let labels: Vec<String> = allowed_canonical
                        .iter()
                        .map(|c| {
                            let pred = Predicate::Canonical((*c).clone());
                            predicate_to_columns(&pred).map(|(_kind, label)| label)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let placeholders = std::iter::repeat("?")
                        .take(labels.len())
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "SELECT id, subject_id,
                                predicate_kind, predicate_label,
                                object_kind, object_entity_id, object_literal,
                                summary,
                                valid_from, valid_to, recorded_at, invalidated_at,
                                invalidated_by, supersedes,
                                episode_id, memory_id,
                                resolution_method,
                                activation, confidence,
                                agent_affect,
                                created_at
                         FROM graph_edges
                         WHERE subject_id = ?1 AND namespace = ?2
                           AND invalidated_at IS NULL
                           AND predicate_kind = 'canonical'
                           AND predicate_label IN ({})
                         ORDER BY activation DESC, recorded_at DESC, id ASC",
                        placeholders
                    );
                    let mut stmt = self.conn.prepare(&sql)?;
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                        Vec::with_capacity(2 + labels.len());
                    params.push(Box::new(node_blob.clone()));
                    params.push(Box::new(self.namespace.clone()));
                    for l in &labels {
                        params.push(Box::new(l.clone()));
                    }
                    let param_refs: Vec<&dyn rusqlite::ToSql> =
                        params.iter().map(|b| b.as_ref()).collect();
                    let rows: Vec<EdgeRowColumns> = stmt
                        .query_map(
                            rusqlite::params_from_iter(param_refs),
                            row_to_edge_columns,
                        )?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };

                // 3b. Process outgoing edges: walker steps to the object
                //     entity; literal-objects are not graph nodes so they
                //     don't extend the frontier (but the edge IS still
                //     yielded — caller may want literal predicates for
                //     factual queries).
                for cols in outgoing {
                    if output.len() >= max_results {
                        break;
                    }
                    let edge = decode_edge_row(cols)?;
                    // Extract object id BEFORE moving `edge` into output.
                    let obj_entity = match &edge.object {
                        EdgeEnd::Entity { id } => Some(*id),
                        EdgeEnd::Literal { .. } => None,
                    };
                    match obj_entity {
                        Some(obj_id) => {
                            if !visited.contains(&obj_id) {
                                visited.insert(obj_id);
                                output.push((obj_id, edge));
                                next_frontier.push(obj_id);
                            }
                            // Already visited → skip (cycle / dup yield).
                        }
                        None => {
                            // Literal endpoint: yield (subject, edge); no
                            // frontier extension since a literal isn't an
                            // entity.
                            let subj = edge.subject_id;
                            output.push((subj, edge));
                        }
                    }
                }

                // 3c. Incoming edges for symmetric / inverse traversal.
                //     We collect the set of canonical predicates whose
                //     directionality requires walking incoming edges.
                let walk_incoming: Vec<CanonicalPredicate> = if allowed_canonical.is_empty() {
                    // "all canonical" — for incoming we still need to know
                    // which predicates carry symmetric / inverse semantics.
                    // We can't enumerate without the variant list, so we
                    // do an "all canonical incoming" pass; filtering by
                    // directionality happens at row-classify time.
                    Vec::new()
                } else {
                    allowed_canonical
                        .iter()
                        .filter_map(|c| {
                            let cp: CanonicalPredicate = (*c).clone();
                            match directionality(&cp) {
                                Directionality::Symmetric
                                | Directionality::Directed { inverse: Some(_) } => Some(cp),
                                Directionality::Directed { inverse: None } => None,
                            }
                        })
                        .collect()
                };

                let must_scan_incoming = allowed_canonical.is_empty() || !walk_incoming.is_empty();
                if must_scan_incoming && output.len() < max_results {
                    let incoming: Vec<EdgeRowColumns> = if walk_incoming.is_empty() && allowed_canonical.is_empty() {
                        // No filter → scan all live canonical incoming edges.
                        let mut stmt = self.conn.prepare_cached(
                            "SELECT id, subject_id,
                                    predicate_kind, predicate_label,
                                    object_kind, object_entity_id, object_literal,
                                    summary,
                                    valid_from, valid_to, recorded_at, invalidated_at,
                                    invalidated_by, supersedes,
                                    episode_id, memory_id,
                                    resolution_method,
                                    activation, confidence,
                                    agent_affect,
                                    created_at
                             FROM graph_edges
                             WHERE object_entity_id = ?1 AND namespace = ?2
                               AND object_kind = 'entity'
                               AND invalidated_at IS NULL
                               AND predicate_kind = 'canonical'
                             ORDER BY activation DESC, recorded_at DESC, id ASC",
                        )?;
                        let rows: Vec<EdgeRowColumns> = stmt
                            .query_map(
                                rusqlite::params![node_blob, self.namespace],
                                row_to_edge_columns,
                            )?
                            .collect::<Result<Vec<_>, _>>()?;
                        rows
                    } else if walk_incoming.is_empty() {
                        // Filter present but no symmetric/inverse → no incoming.
                        Vec::new()
                    } else {
                        // Build label list for symmetric + inverse(directed).
                        let mut labels: Vec<String> = Vec::new();
                        for c in &walk_incoming {
                            match directionality(c) {
                                Directionality::Symmetric => {
                                    let pred = Predicate::Canonical(c.clone());
                                    let (_k, l) = predicate_to_columns(&pred)?;
                                    labels.push(l);
                                }
                                Directionality::Directed { inverse: Some(inv) } => {
                                    let pred = Predicate::Canonical(inv);
                                    let (_k, l) = predicate_to_columns(&pred)?;
                                    labels.push(l);
                                }
                                Directionality::Directed { inverse: None } => {}
                            }
                        }
                        if labels.is_empty() {
                            Vec::new()
                        } else {
                            let placeholders = std::iter::repeat("?")
                                .take(labels.len())
                                .collect::<Vec<_>>()
                                .join(",");
                            let sql = format!(
                                "SELECT id, subject_id,
                                        predicate_kind, predicate_label,
                                        object_kind, object_entity_id, object_literal,
                                        summary,
                                        valid_from, valid_to, recorded_at, invalidated_at,
                                        invalidated_by, supersedes,
                                        episode_id, memory_id,
                                        resolution_method,
                                        activation, confidence,
                                        agent_affect,
                                        created_at
                                 FROM graph_edges
                                 WHERE object_entity_id = ?1 AND namespace = ?2
                                   AND object_kind = 'entity'
                                   AND invalidated_at IS NULL
                                   AND predicate_kind = 'canonical'
                                   AND predicate_label IN ({})
                                 ORDER BY activation DESC, recorded_at DESC, id ASC",
                                placeholders
                            );
                            let mut stmt = self.conn.prepare(&sql)?;
                            let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                                Vec::with_capacity(2 + labels.len());
                            params.push(Box::new(node_blob.clone()));
                            params.push(Box::new(self.namespace.clone()));
                            for l in &labels {
                                params.push(Box::new(l.clone()));
                            }
                            let param_refs: Vec<&dyn rusqlite::ToSql> =
                                params.iter().map(|b| b.as_ref()).collect();
                            let rows: Vec<EdgeRowColumns> = stmt
                                .query_map(
                                    rusqlite::params_from_iter(param_refs),
                                    row_to_edge_columns,
                                )?
                                .collect::<Result<Vec<_>, _>>()?;
                            rows
                        }
                    };

                    for cols in incoming {
                        if output.len() >= max_results {
                            break;
                        }
                        let edge = decode_edge_row(cols)?;
                        // For incoming, the "step target" is the SUBJECT
                        // (we're walking backwards along the edge).
                        // Visited check covers cycles.
                        let target = edge.subject_id;
                        if !visited.contains(&target) {
                            visited.insert(target);
                            output.push((target, edge));
                            next_frontier.push(target);
                        }
                    }
                }
            }

            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        // Telemetry: deep-traversal hint when `max_depth` exceeds a soft
        // threshold (§6 spec). The threshold is conservative — anything
        // beyond depth 3 already touches the consolidation layer's
        // throttle policy.
        const DEEP_TRAVERSAL_THRESHOLD: usize = 3;
        if max_depth > DEEP_TRAVERSAL_THRESHOLD {
            self.sink.emit_operational_load("traverse_deep", 1);
        }
        Ok(output)
    }

    // ---------------------------------------------- Provenance (Phase 2)
    fn entities_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError> {
        // §4.2 / GOAL-1.3 / GOAL-1.7 — episode → entity rollup.
        //
        // The link table (`graph_memory_entity_mentions`) holds memory↔entity
        // edges; the episode lives on `memories.episode_id` (additive column,
        // §4.1). Joining is the only correct way to roll up — duplicating
        // `episode_id` onto the mention rows would create a two-source-of-
        // truth invariant we'd then have to maintain on every update.
        //
        // DISTINCT: a single entity may be mentioned across many memories
        // within the episode; callers want the entity set, not the mention
        // multiset. Order: entity_id ASC for deterministic output (callers
        // that want recency rebuild from `mentions_of_entity`).
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT m.entity_id
             FROM graph_memory_entity_mentions AS m
             JOIN memories AS mem ON mem.id = m.memory_id
             WHERE mem.episode_id = ?1 AND m.namespace = ?2
             ORDER BY m.entity_id ASC",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![episode.as_bytes().to_vec(), self.namespace],
                |row| row.get::<_, Vec<u8>>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|b| {
                Uuid::from_slice(&b).map_err(|_| {
                    GraphError::Invariant(
                        "entities_in_episode: entity_id blob is not a valid UUID",
                    )
                })
            })
            .collect()
    }
    fn edges_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError> {
        // Returns just edge ids (not full Edge rows). Callers that want
        // hydrated edges chain `get_edge` per id; this keeps the join-table
        // lookup hot path cheap. ORDER BY recorded_at to mirror the
        // edges_of contract — most-recent first inside a single episode.
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM graph_edges
             WHERE episode_id = ?1 AND namespace = ?2
             ORDER BY recorded_at DESC, id ASC",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![episode.as_bytes().to_vec(), self.namespace],
                |row| row.get::<_, Vec<u8>>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|b| {
                Uuid::from_slice(&b)
                    .map_err(|_| GraphError::Invariant("edge id blob length != 16"))
            })
            .collect()
    }
    fn mentions_of_entity(&self, entity: Uuid) -> Result<EntityMentions, GraphError> {
        // §4.2 / GOAL-1.3 / GOAL-1.7 — entity → (episodes, memories) rollup.
        //
        // Doc contract on `EntityMentions`: episodes and memories are
        // de-duplicated and ordered by ascending recorded_at (oldest first).
        // We compute both in a single pass: project (memory_id, recorded_at,
        // episode_id) sorted by recorded_at ASC, then de-dupe each output
        // vector while preserving first-seen order. This costs O(n) extra
        // memory but avoids two passes over the join table for one query.
        //
        // Episode hydration: `memories.episode_id` is `BLOB` (nullable —
        // additive column, ALTER TABLE in §4.1). Memories with NULL
        // episode_id contribute to `memory_ids` but not `episode_ids`.
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.memory_id, m.recorded_at, mem.episode_id
             FROM graph_memory_entity_mentions AS m
             JOIN memories AS mem ON mem.id = m.memory_id
             WHERE m.entity_id = ?1 AND m.namespace = ?2
             ORDER BY m.recorded_at ASC, m.memory_id ASC",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![entity.as_bytes().to_vec(), self.namespace],
                |row| {
                    let mem_id: String = row.get(0)?;
                    let _rec_at: f64 = row.get(1)?;
                    let ep: Option<Vec<u8>> = row.get(2)?;
                    Ok((mem_id, ep))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;

        // Preserve first-seen order while de-duping. HashSet for O(1) seen
        // tracking; Vec for output.
        use std::collections::HashSet;
        let mut memory_ids: Vec<String> = Vec::with_capacity(rows.len());
        let mut episode_ids: Vec<Uuid> = Vec::new();
        let mut seen_mem: HashSet<String> = HashSet::with_capacity(rows.len());
        let mut seen_ep: HashSet<Uuid> = HashSet::new();
        for (mem_id, ep_blob) in rows {
            if seen_mem.insert(mem_id.clone()) {
                memory_ids.push(mem_id);
            }
            if let Some(blob) = ep_blob {
                let ep = Uuid::from_slice(&blob).map_err(|_| {
                    GraphError::Invariant(
                        "mentions_of_entity: episode_id blob is not a valid UUID",
                    )
                })?;
                if seen_ep.insert(ep) {
                    episode_ids.push(ep);
                }
            }
        }
        Ok(EntityMentions { episode_ids, memory_ids })
    }
    fn link_memory_to_entities(
        &mut self,
        memory_id: &str,
        entity_ids: &[(Uuid, f64, Option<String>)],
        at: DateTime<Utc>,
    ) -> Result<(), GraphError> {
        // §4.2 Memory↔Entity provenance — atomic batch with within-batch dedup
        // and `ON CONFLICT DO UPDATE` cross-batch dedup. See design §4.2 for
        // the upsert-rule contract.

        // (1) Validate inputs up front so we never open a transaction we'd
        //     then have to roll back for a trivially detectable bug.
        for (_eid, conf, _span) in entity_ids {
            if !conf.is_finite() || !(0.0..=1.0).contains(conf) {
                return Err(GraphError::Invariant(
                    "link_memory_to_entities: confidence out of [0,1]",
                ));
            }
        }

        // (2) Within-batch dedup. Fold duplicate `entity_id`s in input order
        //     using the same rules as cross-batch (max confidence, latest
        //     non-NULL span). This guarantees the SQL layer sees one row per
        //     entity_id and removes a class of "ON CONFLICT applied twice
        //     within the same statement" footguns.
        use std::collections::HashMap;
        let mut folded: HashMap<Uuid, (f64, Option<String>)> =
            HashMap::with_capacity(entity_ids.len());
        // Preserve first-seen order for deterministic iteration (HashMap
        // iteration order is randomized — that's fine for SQL, but tests want
        // determinism. Track order in a separate Vec).
        let mut order: Vec<Uuid> = Vec::with_capacity(entity_ids.len());
        for (eid, conf, span) in entity_ids {
            match folded.get_mut(eid) {
                Some((existing_conf, existing_span)) => {
                    if *conf > *existing_conf {
                        *existing_conf = *conf;
                    }
                    if span.is_some() {
                        *existing_span = span.clone();
                    }
                }
                None => {
                    folded.insert(*eid, (*conf, span.clone()));
                    order.push(*eid);
                }
            }
        }

        let recorded_at = dt_to_unix(at);
        let ns = self.namespace.clone();

        // (3) Atomic batch: cross-namespace check + upsert in one transaction.
        let tx = self.conn.transaction()?;
        {
            // Namespace pre-check. The PK `(memory_id, entity_id)` does NOT
            // include namespace; a pre-existing row with a different namespace
            // would silently be "stolen" by a naive upsert. Detect and fail
            // loud (design §4.2: cross-namespace memory↔entity link is an
            // invariant violation).
            let mut ns_check = tx.prepare_cached(
                "SELECT namespace FROM graph_memory_entity_mentions
                 WHERE memory_id = ?1 AND entity_id = ?2",
            )?;
            for eid in &order {
                let existing_ns: Option<String> = ns_check
                    .query_row(
                        rusqlite::params![memory_id, eid.as_bytes().to_vec()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(existing) = existing_ns {
                    if existing != ns {
                        // tx is dropped → automatic rollback. We surface a
                        // structured invariant for the caller.
                        return Err(GraphError::Invariant(
                            "cross-namespace memory↔entity link",
                        ));
                    }
                }
            }
            drop(ns_check);

            // Upsert. The DO UPDATE clause encodes the design §4.2 rules:
            //   confidence    ← max(existing, new)        (monotone)
            //   mention_span  ← COALESCE(new, existing)   (latest non-NULL)
            //   recorded_at   ← max(existing, new)        (latest wins)
            //   namespace     ← (preserved; not in DO UPDATE)
            let mut upsert = tx.prepare_cached(
                "INSERT INTO graph_memory_entity_mentions
                    (memory_id, entity_id, mention_span, confidence,
                     recorded_at, namespace)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(memory_id, entity_id) DO UPDATE SET
                    confidence   = MAX(confidence, excluded.confidence),
                    mention_span = COALESCE(excluded.mention_span, mention_span),
                    recorded_at  = MAX(recorded_at, excluded.recorded_at)",
            )?;
            for eid in &order {
                let (conf, span) = &folded[eid];
                upsert.execute(rusqlite::params![
                    memory_id,
                    eid.as_bytes().to_vec(),
                    span.as_deref(),
                    conf,
                    recorded_at,
                    ns,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn entities_linked_to_memory(&self, memory_id: &str) -> Result<Vec<Uuid>, GraphError> {
        // Namespace-scoped: rows from sibling namespaces never leak. Order:
        // recorded_at ASC, entity_id ASC (design §4.2 — stable, deterministic).
        let mut stmt = self.conn.prepare_cached(
            "SELECT entity_id FROM graph_memory_entity_mentions
             WHERE memory_id = ?1 AND namespace = ?2
             ORDER BY recorded_at ASC, entity_id ASC",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![memory_id, self.namespace],
                |row| {
                    let blob: Vec<u8> = row.get(0)?;
                    Ok(blob)
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|blob| {
                Uuid::from_slice(&blob).map_err(|_| {
                    GraphError::Invariant(
                        "entities_linked_to_memory: entity_id blob is not a valid UUID",
                    )
                })
            })
            .collect()
    }

    fn memories_mentioning_entity(
        &self,
        entity: Uuid,
        limit: usize,
    ) -> Result<Vec<String>, GraphError> {
        // limit == 0 is a "give me nothing" sentinel (caller-bug guard, not an
        // error — same convention as `search_candidates`, design §4.2).
        if limit == 0 {
            return Ok(Vec::new());
        }
        // SQLite LIMIT is i64. usize → i64 saturates at i64::MAX, which is
        // larger than any plausible row count; values above i64::MAX would be
        // a 128-PiB result set we'd never materialize anyway.
        let limit_i64: i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(
            "SELECT memory_id FROM graph_memory_entity_mentions
             WHERE entity_id = ?1 AND namespace = ?2
             ORDER BY recorded_at DESC, memory_id ASC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![entity.as_bytes().to_vec(), self.namespace, limit_i64],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    fn edges_sourced_from_memory(&self, memory_id: &str) -> Result<Vec<Edge>, GraphError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, subject_id,
                    predicate_kind, predicate_label,
                    object_kind, object_entity_id, object_literal,
                    summary,
                    valid_from, valid_to, recorded_at, invalidated_at,
                    invalidated_by, supersedes,
                    episode_id, memory_id,
                    resolution_method,
                    activation, confidence,
                    agent_affect,
                    created_at
             FROM graph_edges
             WHERE memory_id = ?1 AND namespace = ?2
             ORDER BY recorded_at DESC, id ASC",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![memory_id, self.namespace],
                row_to_edge_columns,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(decode_edge_row).collect()
    }

    // ------------------------------------------------- Topics (Phase 2)
    fn upsert_topic(&mut self, t: &KnowledgeTopic) -> Result<(), GraphError> {
        // §4.1 `knowledge_topics` is mirrored against `graph_entities` (the
        // PK is also a FK into entities). Caller MUST insert the mirror
        // entity before upserting the topic — surfacing the FK error here
        // is correct: it forces the integrity invariant at the seam.
        //
        // Embedding dim: §4.1 says topic embeddings share the system-wide
        // `embedding_dim` with entities. Reuse the same encoder so a single
        // change in dim policy propagates to both writers without per-call
        // duplication.
        t.validate_embedding_dim(self.embedding_dim)?;
        let embedding_blob =
            entity_embedding_to_blob(t.embedding.as_deref(), self.embedding_dim)?;

        // Vec<String>/Vec<Uuid> persist as JSON TEXT (column default '[]').
        // Serializing here keeps the SQL statement uniform; the schema stores
        // these as TEXT to avoid a third-table normalization that buys us
        // nothing for v0.3 (callers always read the full topic).
        let source_memories = serde_json::to_string(&t.source_memories)?;
        // Persist Uuids as their canonical hyphenated string form inside the
        // JSON array — matches how serde does Uuid by default and keeps the
        // text-blob roundtrip stable across `bytes`/`hyphenated` choices.
        let contributing_json = serde_json::to_string(
            &t.contributing_entities
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>(),
        )?;

        let cluster_weights_json = match &t.cluster_weights {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };

        // Upsert. PK is `topic_id`. On conflict update every mutable column
        // (everything except topic_id and namespace — namespace is part of
        // identity per §4.1 and changing it via an upsert would be a stealth
        // namespace move; reject by virtue of WHERE-pinning identity).
        let sql = "INSERT INTO knowledge_topics (
                topic_id, title, summary, embedding,
                source_memories, contributing_entities, cluster_weights,
                synthesis_run_id, synthesized_at,
                superseded_by, superseded_at,
                namespace
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(topic_id) DO UPDATE SET
                title = excluded.title,
                summary = excluded.summary,
                embedding = excluded.embedding,
                source_memories = excluded.source_memories,
                contributing_entities = excluded.contributing_entities,
                cluster_weights = excluded.cluster_weights,
                synthesis_run_id = excluded.synthesis_run_id,
                synthesized_at = excluded.synthesized_at,
                superseded_by = excluded.superseded_by,
                superseded_at = excluded.superseded_at
            WHERE knowledge_topics.namespace = excluded.namespace";
        let n = self.conn.execute(
            sql,
            rusqlite::params![
                t.topic_id.as_bytes().to_vec(),
                t.title,
                t.summary,
                embedding_blob,
                source_memories,
                contributing_json,
                cluster_weights_json,
                t.synthesis_run_id.map(|u| u.as_bytes().to_vec()),
                t.synthesized_at,
                t.superseded_by.map(|u| u.as_bytes().to_vec()),
                t.superseded_at,
                t.namespace,
            ],
        )?;
        // n == 0 means the WHERE clause filtered out the conflict row,
        // i.e. an attempt to upsert across namespaces. Surface as an
        // invariant — same shape as `link_memory_to_entities`.
        if n == 0 {
            return Err(GraphError::Invariant(
                "upsert_topic: cross-namespace topic upsert rejected",
            ));
        }
        Ok(())
    }
    fn get_topic(&self, id: Uuid) -> Result<Option<KnowledgeTopic>, GraphError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT topic_id, title, summary, embedding,
                    source_memories, contributing_entities, cluster_weights,
                    synthesis_run_id, synthesized_at,
                    superseded_by, superseded_at,
                    namespace
             FROM knowledge_topics WHERE topic_id = ?1 AND namespace = ?2",
        )?;
        let row = stmt
            .query_row(
                rusqlite::params![id.as_bytes().to_vec(), self.namespace],
                |row| {
                    let topic_blob: Vec<u8> = row.get(0)?;
                    let title: String = row.get(1)?;
                    let summary: String = row.get(2)?;
                    let emb_blob: Option<Vec<u8>> = row.get(3)?;
                    let source_mem_json: String = row.get(4)?;
                    let contrib_json: String = row.get(5)?;
                    let cluster_weights_json: Option<String> = row.get(6)?;
                    let run_blob: Option<Vec<u8>> = row.get(7)?;
                    let synthesized_at: f64 = row.get(8)?;
                    let superseded_by_blob: Option<Vec<u8>> = row.get(9)?;
                    let superseded_at: Option<f64> = row.get(10)?;
                    let namespace: String = row.get(11)?;
                    Ok((
                        topic_blob,
                        title,
                        summary,
                        emb_blob,
                        source_mem_json,
                        contrib_json,
                        cluster_weights_json,
                        run_blob,
                        synthesized_at,
                        superseded_by_blob,
                        superseded_at,
                        namespace,
                    ))
                },
            )
            .optional()?;
        let Some((
            topic_blob,
            title,
            summary,
            emb_blob,
            source_mem_json,
            contrib_json,
            cluster_weights_json,
            run_blob,
            synthesized_at,
            superseded_by_blob,
            superseded_at,
            namespace,
        )) = row
        else {
            return Ok(None);
        };
        let topic_id = Uuid::from_slice(&topic_blob)
            .map_err(|_| GraphError::Invariant("topic_id blob is not a valid UUID"))?;
        let embedding = entity_embedding_from_blob(emb_blob, self.embedding_dim)?;
        let source_memories: Vec<String> = serde_json::from_str(&source_mem_json)?;
        let contributing_strs: Vec<String> = serde_json::from_str(&contrib_json)?;
        let contributing_entities: Vec<Uuid> = contributing_strs
            .into_iter()
            .map(|s| {
                Uuid::parse_str(&s).map_err(|_| {
                    GraphError::Invariant(
                        "contributing_entities entry is not a valid UUID",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cluster_weights = match cluster_weights_json {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        };
        let synthesis_run_id = match run_blob {
            None => None,
            Some(b) => Some(Uuid::from_slice(&b).map_err(|_| {
                GraphError::Invariant("synthesis_run_id blob is not a valid UUID")
            })?),
        };
        let superseded_by = match superseded_by_blob {
            None => None,
            Some(b) => Some(Uuid::from_slice(&b).map_err(|_| {
                GraphError::Invariant("superseded_by blob is not a valid UUID")
            })?),
        };
        Ok(Some(KnowledgeTopic {
            topic_id,
            title,
            summary,
            embedding,
            source_memories,
            contributing_entities,
            cluster_weights,
            synthesis_run_id,
            synthesized_at,
            superseded_by,
            superseded_at,
            namespace,
        }))
    }
    fn list_topics(&self, namespace: &str, include_superseded: bool, limit: usize) -> Result<Vec<KnowledgeTopic>, GraphError> {
        // Caller-supplied namespace overrides the store's default. This is
        // intentional: `list_topics` is a cross-cutting query (knowledge
        // synthesis tooling) that may want to walk all namespaces a user
        // owns, not just the one bound at construction time. Single-row
        // CRUD (`get_topic`, `upsert_topic`) stays pinned to `self.namespace`
        // because identity-bearing operations should not silently slide.
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit_i64: i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        // Filter and ordering: most-recent synthesized_at first, ties broken
        // by topic_id ASC for determinism.
        let sql = if include_superseded {
            "SELECT topic_id FROM knowledge_topics
             WHERE namespace = ?1
             ORDER BY synthesized_at DESC, topic_id ASC
             LIMIT ?2"
        } else {
            "SELECT topic_id FROM knowledge_topics
             WHERE namespace = ?1 AND superseded_at IS NULL
             ORDER BY synthesized_at DESC, topic_id ASC
             LIMIT ?2"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let ids: Vec<Uuid> = stmt
            .query_map(rusqlite::params![namespace, limit_i64], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .map(|r| {
                r.map_err(GraphError::from).and_then(|b| {
                    Uuid::from_slice(&b).map_err(|_| {
                        GraphError::Invariant("list_topics: topic_id blob is not a valid UUID")
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Hydrate via `get_topic` (with caller's namespace, not self's).
        // We re-bind `self.namespace` only for this hydration loop to avoid
        // duplicating the column-decode logic. The bind/restore pattern is
        // simpler and more honest than threading a namespace argument
        // through the private decoder.
        let mut topics = Vec::with_capacity(ids.len());
        let mut hydrate_stmt = self.conn.prepare_cached(
            "SELECT topic_id, title, summary, embedding,
                    source_memories, contributing_entities, cluster_weights,
                    synthesis_run_id, synthesized_at,
                    superseded_by, superseded_at,
                    namespace
             FROM knowledge_topics WHERE topic_id = ?1 AND namespace = ?2",
        )?;
        for id in ids {
            // Same decode logic as `get_topic`. Inlined to avoid the extra
            // `prepare_cached` re-parse and to keep the FK-namespace pin.
            let row = hydrate_stmt
                .query_row(
                    rusqlite::params![id.as_bytes().to_vec(), namespace],
                    |row| {
                        let topic_blob: Vec<u8> = row.get(0)?;
                        let title: String = row.get(1)?;
                        let summary: String = row.get(2)?;
                        let emb_blob: Option<Vec<u8>> = row.get(3)?;
                        let source_mem_json: String = row.get(4)?;
                        let contrib_json: String = row.get(5)?;
                        let cluster_weights_json: Option<String> = row.get(6)?;
                        let run_blob: Option<Vec<u8>> = row.get(7)?;
                        let synthesized_at: f64 = row.get(8)?;
                        let superseded_by_blob: Option<Vec<u8>> = row.get(9)?;
                        let superseded_at: Option<f64> = row.get(10)?;
                        let namespace_col: String = row.get(11)?;
                        Ok((
                            topic_blob,
                            title,
                            summary,
                            emb_blob,
                            source_mem_json,
                            contrib_json,
                            cluster_weights_json,
                            run_blob,
                            synthesized_at,
                            superseded_by_blob,
                            superseded_at,
                            namespace_col,
                        ))
                    },
                )
                .optional()?;
            let Some((
                topic_blob,
                title,
                summary,
                emb_blob,
                source_mem_json,
                contrib_json,
                cluster_weights_json,
                run_blob,
                synthesized_at,
                superseded_by_blob,
                superseded_at,
                ns_col,
            )) = row
            else {
                continue;
            };
            let topic_id = Uuid::from_slice(&topic_blob)
                .map_err(|_| GraphError::Invariant("topic_id blob is not a valid UUID"))?;
            let embedding = entity_embedding_from_blob(emb_blob, self.embedding_dim)?;
            let source_memories: Vec<String> = serde_json::from_str(&source_mem_json)?;
            let contributing_strs: Vec<String> = serde_json::from_str(&contrib_json)?;
            let contributing_entities: Vec<Uuid> = contributing_strs
                .into_iter()
                .map(|s| {
                    Uuid::parse_str(&s).map_err(|_| {
                        GraphError::Invariant("contributing_entities entry is not a valid UUID")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let cluster_weights = match cluster_weights_json {
                Some(s) => Some(serde_json::from_str(&s)?),
                None => None,
            };
            let synthesis_run_id = match run_blob {
                None => None,
                Some(b) => Some(Uuid::from_slice(&b).map_err(|_| {
                    GraphError::Invariant("synthesis_run_id blob is not a valid UUID")
                })?),
            };
            let superseded_by = match superseded_by_blob {
                None => None,
                Some(b) => Some(Uuid::from_slice(&b).map_err(|_| {
                    GraphError::Invariant("superseded_by blob is not a valid UUID")
                })?),
            };
            topics.push(KnowledgeTopic {
                topic_id,
                title,
                summary,
                embedding,
                source_memories,
                contributing_entities,
                cluster_weights,
                synthesis_run_id,
                synthesized_at,
                superseded_by,
                superseded_at,
                namespace: ns_col,
            });
        }
        Ok(topics)
    }
    fn supersede_topic(&mut self, old: Uuid, successor: Uuid, at: DateTime<Utc>) -> Result<(), GraphError> {
        // §4.1 GUARD-3: supersede is monotonic, never erase. Mirror
        // `KnowledgeTopic::supersede` semantics — error if the row is already
        // superseded by a *different* successor; idempotent if same successor.
        //
        // Single transaction so the read/decide/write is atomic against a
        // concurrent supersede racing on the same `old` topic.
        let now_secs = dt_to_unix(at);
        let tx = self.conn.transaction()?;
        let existing: Option<(Option<Vec<u8>>, Option<f64>)> = tx
            .query_row(
                "SELECT superseded_by, superseded_at FROM knowledge_topics
                 WHERE topic_id = ?1 AND namespace = ?2",
                rusqlite::params![old.as_bytes().to_vec(), self.namespace],
                |row| Ok((row.get::<_, Option<Vec<u8>>>(0)?, row.get::<_, Option<f64>>(1)?)),
            )
            .optional()?;
        let Some((cur_by, _cur_at)) = existing else {
            // §4.1: caller must verify the topic exists. Surface a stable
            // invariant so the resolution driver can decide whether to log
            // a "stale supersede target" trace and continue, or escalate.
            return Err(GraphError::Invariant("supersede_topic: old topic not found"));
        };
        if let Some(blob) = cur_by {
            let existing_succ = Uuid::from_slice(&blob).map_err(|_| {
                GraphError::Invariant("supersede_topic: superseded_by blob is not a valid UUID")
            })?;
            if existing_succ == successor {
                // Idempotent: same successor, accept.
                return Ok(());
            } else {
                return Err(GraphError::Invariant("topic already superseded"));
            }
        }
        // Verify the successor exists in the same namespace before pointing
        // at it. Without this check a misuse would create a dangling FK
        // reference that the schema's `superseded_by REFERENCES
        // knowledge_topics(topic_id)` partially catches, but not on the
        // namespace dimension.
        let succ_present: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM knowledge_topics
                 WHERE topic_id = ?1 AND namespace = ?2",
                rusqlite::params![successor.as_bytes().to_vec(), self.namespace],
                |row| row.get(0),
            )
            .optional()?;
        if succ_present.is_none() {
            return Err(GraphError::Invariant(
                "supersede_topic: successor topic not found in same namespace",
            ));
        }
        tx.execute(
            "UPDATE knowledge_topics
             SET superseded_by = ?1, superseded_at = ?2
             WHERE topic_id = ?3 AND namespace = ?4",
            rusqlite::params![
                successor.as_bytes().to_vec(),
                now_secs,
                old.as_bytes().to_vec(),
                self.namespace,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    // -------------------------------------------- Pipeline runs (Phase 2)
    fn begin_pipeline_run(&mut self, kind: PipelineKind, input_summary: Json) -> Result<Uuid, GraphError> {
        self.begin_pipeline_run_inner(kind, None, None, input_summary)
    }
    fn begin_pipeline_run_for_memory(
        &mut self,
        kind: PipelineKind,
        memory_id: &str,
        episode_id: Uuid,
        input_summary: Json,
    ) -> Result<Uuid, GraphError> {
        // §3.1 / §6.3: only Resolution / Reextract are memory-scoped.
        // KnowledgeCompile callers must use `begin_pipeline_run` instead so
        // we don't index a "memory" that isn't one.
        if matches!(kind, PipelineKind::KnowledgeCompile) {
            return Err(GraphError::Invariant(
                "begin_pipeline_run_for_memory: KnowledgeCompile is not memory-scoped; use begin_pipeline_run",
            ));
        }
        self.begin_pipeline_run_inner(kind, Some(memory_id), Some(episode_id), input_summary)
    }
    fn latest_pipeline_run_for_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<PipelineRunRow>, GraphError> {
        // Latest-by-started_at over the partial index `idx_graph_pipeline_runs_memory`.
        // Bound the result to a single row — `LIMIT 1` keeps the planner on
        // the index seek even on legacy DBs that may have additional runs
        // for the same memory id.
        let mut stmt = self.conn.prepare(
            "SELECT run_id, kind, status, started_at, finished_at,
                    memory_id, episode_id, error_detail
             FROM graph_pipeline_runs
             WHERE memory_id = ?1 AND namespace = ?2
             ORDER BY started_at DESC
             LIMIT 1",
        )?;
        let row = stmt
            .query_row(
                rusqlite::params![memory_id, self.namespace],
                |row| {
                    let run_id_blob: Vec<u8> = row.get(0)?;
                    let kind_str: String = row.get(1)?;
                    let status_str: String = row.get(2)?;
                    let started_at: f64 = row.get(3)?;
                    let finished_at: Option<f64> = row.get(4)?;
                    let memory_id_opt: Option<String> = row.get(5)?;
                    let episode_id_blob: Option<Vec<u8>> = row.get(6)?;
                    let error_detail: Option<String> = row.get(7)?;
                    Ok((
                        run_id_blob,
                        kind_str,
                        status_str,
                        started_at,
                        finished_at,
                        memory_id_opt,
                        episode_id_blob,
                        error_detail,
                    ))
                },
            )
            .optional()?;
        let Some((run_id_blob, kind_str, status_str, started_at, finished_at, memory_id_opt, episode_id_blob, error_detail)) = row else {
            return Ok(None);
        };
        // Decode all SQLite-side encodings to canonical Rust types. Each
        // failure surfaces as `GraphError::Invariant` with a precise context
        // string — never silently default.
        if run_id_blob.len() != 16 {
            return Err(GraphError::Invariant(
                "graph_pipeline_runs.run_id length != 16",
            ));
        }
        let run_id = Uuid::from_slice(&run_id_blob).unwrap();
        // serde_json reads enum variants only when the input is a JSON
        // string (i.e. wrapped in quotes), matching the encoding done in
        // `begin_pipeline_run_inner` via `serde_json::to_string(&kind)`.
        let kind: PipelineKind = serde_json::from_str(&format!("\"{kind_str}\""))?;
        let status: RunStatus = serde_json::from_str(&format!("\"{status_str}\""))?;
        let started_at = unix_to_dt(started_at)?;
        let finished_at = match finished_at {
            Some(f) => Some(unix_to_dt(f)?),
            None => None,
        };
        let episode_id = opt_blob_to_uuid(episode_id_blob)?;
        Ok(Some(PipelineRunRow {
            run_id,
            kind,
            status,
            started_at,
            finished_at,
            memory_id: memory_id_opt,
            episode_id,
            error_detail,
        }))
    }
    fn finish_pipeline_run(
        &mut self, run_id: Uuid, status: RunStatus, output_summary: Option<Json>, error_detail: Option<&str>,
    ) -> Result<(), GraphError> {
        // §4.2 audit: terminal statuses only (Succeeded | Failed | Cancelled).
        // Reject `Running` — it would silently re-enter the unfinished state
        // and break the "append-only state machine" invariant
        // (`audit::PipelineRun::transition`).
        if matches!(status, RunStatus::Running) {
            return Err(GraphError::Invariant(
                "finish_pipeline_run: cannot transition to Running",
            ));
        }
        let status_str = serde_json::to_string(&status)?;
        let status_label = status_str.trim_matches('"').to_string();
        let now = dt_to_unix(Utc::now());
        let output_json = match output_summary {
            Some(v) => Some(serde_json::to_string(&v)?),
            None => None,
        };

        // Atomic guard: read the current status and only finish if it's
        // `running`. This makes finish idempotent under race (two workers
        // can never both succeed), and rejects double-finish at the SQL
        // layer — same shape as the in-memory `transition` check.
        let tx = self.conn.transaction()?;
        let cur: Option<String> = tx
            .query_row(
                "SELECT status FROM graph_pipeline_runs
                 WHERE run_id = ?1 AND namespace = ?2",
                rusqlite::params![run_id.as_bytes().to_vec(), self.namespace],
                |row| row.get(0),
            )
            .optional()?;
        let Some(cur_status) = cur else {
            return Err(GraphError::Invariant(
                "finish_pipeline_run: run not found",
            ));
        };
        if cur_status != "running" {
            return Err(GraphError::Invariant(
                "finish_pipeline_run: run is already terminal",
            ));
        }
        tx.execute(
            "UPDATE graph_pipeline_runs
             SET status = ?1, finished_at = ?2,
                 output_summary = ?3, error_detail = ?4
             WHERE run_id = ?5 AND namespace = ?6",
            rusqlite::params![
                status_label,
                now,
                output_json,
                error_detail,
                run_id.as_bytes().to_vec(),
                self.namespace,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn record_resolution_trace(&mut self, t: &ResolutionTrace) -> Result<(), GraphError> {
        // §4.1 CHECK constraint requires (edge_id IS NOT NULL OR
        // entity_id IS NOT NULL). Surface the violation client-side with a
        // typed invariant so we don't rely on the SQLite error text.
        if t.edge_id.is_none() && t.entity_id.is_none() {
            return Err(GraphError::Invariant(
                "record_resolution_trace: at least one of edge_id/entity_id must be set",
            ));
        }
        let candidates_json = match &t.candidates {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        // INSERT OR IGNORE: trace_id is the PK; replaying the same
        // resolution must be a no-op, not a duplicate-row error. Same
        // contract `apply_graph_delta` will rely on for idempotent retries.
        self.conn.execute(
            "INSERT OR IGNORE INTO graph_resolution_traces (
                trace_id, run_id, edge_id, entity_id,
                stage, decision, reason, candidates,
                recorded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                t.trace_id.as_bytes().to_vec(),
                t.run_id.as_bytes().to_vec(),
                t.edge_id.map(|u| u.as_bytes().to_vec()),
                t.entity_id.map(|u| u.as_bytes().to_vec()),
                t.stage,
                t.decision,
                t.reason,
                candidates_json,
                t.recorded_at,
            ],
        )?;
        Ok(())
    }

    // -------------------------------------------- Predicates (Phase 2)
    fn record_predicate_use(&mut self, p: &Predicate, raw: &str, at: DateTime<Utc>) -> Result<(), GraphError> {
        // Single-call variant per design §4.2 — for tests / callers that
        // bypass `apply_graph_delta`. Production hot path uses the in-tx
        // batched counter (`predicate_use_buffer`).
        //
        // Upsert keyed on (kind, label). On first sight: insert with
        // first_seen=last_seen=at, usage_count=1 and the caller's `raw`
        // string. On repeat: increment usage_count, push last_seen forward,
        // keep first_seen and raw_first_seen unchanged.
        let (kind_text, label) = predicate_to_columns(p)?;
        let now = dt_to_unix(at);
        self.conn.execute(
            "INSERT INTO graph_predicates (
                kind, label, raw_first_seen,
                usage_count, first_seen, last_seen
            ) VALUES (?1, ?2, ?3, 1, ?4, ?4)
            ON CONFLICT(kind, label) DO UPDATE SET
                usage_count = usage_count + 1,
                last_seen = max(graph_predicates.last_seen, excluded.last_seen)",
            rusqlite::params![kind_text, label, raw, now],
        )?;
        Ok(())
    }
    fn list_proposed_predicates(&self, min_usage: u64) -> Result<Vec<ProposedPredicateStats>, GraphError> {
        // GOAL-1.10: surface drift candidates only — canonical predicates
        // are operator-curated and not "proposed". Filter both by `kind`
        // and `min_usage`. SQLite stores `usage_count` as INTEGER (i64);
        // saturate u64 → i64 to avoid silently overflowing high counts.
        let threshold: i64 = i64::try_from(min_usage).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare_cached(
            "SELECT label, usage_count FROM graph_predicates
             WHERE kind = 'proposed' AND usage_count >= ?1
             ORDER BY usage_count DESC, label ASC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![threshold], |row| {
                let label: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((label, count))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(label, count)| ProposedPredicateStats {
                label,
                usage_count: count.max(0) as u64,
            })
            .collect())
    }

    // ------------------------------------------- Failures (Phase 2)
    fn record_extraction_failure(&mut self, f: &ExtractionFailure) -> Result<(), GraphError> {
        // §4.1 invariant: closed-set `stage` and `error_category` strings.
        // We enforce closure in code (not in the DB CHECK clause, which
        // would freeze the set forever — adding a stage in v0.4 would
        // require a migration) — same approach as the audit module's
        // `STAGE_*` / `CATEGORY_*` constants.
        validate_failure_closed_sets(&f.stage, &f.error_category)?;
        // INSERT OR IGNORE: id is PK; replays from the same pipeline run
        // must not double-count a failure. Same idempotence shape as
        // `record_resolution_trace`.
        self.conn.execute(
            "INSERT OR IGNORE INTO graph_extraction_failures (
                id, episode_id, stage, error_category,
                error_detail, occurred_at, retry_count, resolved_at,
                namespace
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
            rusqlite::params![
                f.id.as_bytes().to_vec(),
                f.episode_id.as_bytes().to_vec(),
                f.stage,
                f.error_category,
                f.error_detail,
                f.occurred_at,
                f.resolved_at,
                self.namespace,
            ],
        )?;
        Ok(())
    }
    fn list_failed_episodes(&self, unresolved_only: bool) -> Result<Vec<Uuid>, GraphError> {
        // Distinct episode ids — many failures may target the same episode
        // (e.g. resolution + persist both fail on the same input). Callers
        // care about the episode set, not the failure multiset.
        let sql = if unresolved_only {
            "SELECT DISTINCT episode_id FROM graph_extraction_failures
             WHERE namespace = ?1 AND resolved_at IS NULL
             ORDER BY episode_id ASC"
        } else {
            "SELECT DISTINCT episode_id FROM graph_extraction_failures
             WHERE namespace = ?1
             ORDER BY episode_id ASC"
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let blobs = stmt
            .query_map(rusqlite::params![self.namespace], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        blobs
            .into_iter()
            .map(|b| {
                Uuid::from_slice(&b).map_err(|_| {
                    GraphError::Invariant(
                        "list_failed_episodes: episode_id blob is not a valid UUID",
                    )
                })
            })
            .collect()
    }
    fn mark_failure_resolved(&mut self, failure_id: Uuid, at: DateTime<Utc>) -> Result<(), GraphError> {
        // GOAL-1.12: failures are append-only; `resolved_at` is the only
        // mutable field, and it is monotone — once set, never cleared.
        // Refuse to over-write a non-NULL `resolved_at` to make the
        // operator-visible status survive replays / merges.
        let now = dt_to_unix(at);
        let tx = self.conn.transaction()?;
        let cur: Option<Option<f64>> = tx
            .query_row(
                "SELECT resolved_at FROM graph_extraction_failures
                 WHERE id = ?1 AND namespace = ?2",
                rusqlite::params![failure_id.as_bytes().to_vec(), self.namespace],
                |row| row.get(0),
            )
            .optional()?;
        let Some(prev) = cur else {
            return Err(GraphError::Invariant(
                "mark_failure_resolved: failure not found",
            ));
        };
        if prev.is_some() {
            // Idempotent: already resolved. Ignore. The semantics are
            // "resolved at *some* time" — we don't claim the original
            // resolution moment can be updated.
            return Ok(());
        }
        tx.execute(
            "UPDATE graph_extraction_failures
             SET resolved_at = ?1
             WHERE id = ?2 AND namespace = ?3",
            rusqlite::params![now, failure_id.as_bytes().to_vec(), self.namespace],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------ Namespaces (Phase 2)
    fn list_namespaces(&self) -> Result<Vec<String>, GraphError> {
        // §4.1 lifecycle: namespace lives on graph_entities. Distinct
        // namespaces seen across the canonical entity table is the
        // authoritative answer; entity-less namespaces (e.g. orphan alias
        // rows) are not exposed here by design.
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT namespace FROM graph_entities ORDER BY namespace",
        )?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ----------------------------------------- Transaction (Phase 2)
    fn with_transaction(
        &mut self,
        f: &mut dyn FnMut(&Transaction<'_>) -> Result<(), GraphError>,
    ) -> Result<(), GraphError> {
        // Escape hatch: run an arbitrary closure inside a transaction.
        // GUARDs (CHECK / FK constraints) still apply at commit time.
        let tx = self.conn.transaction()?;
        match f(&tx) {
            Ok(()) => {
                tx.commit()?;
                Ok(())
            }
            Err(e) => {
                // `Drop` rolls back, but we call rollback explicitly so the
                // operator-facing error preserves the original context rather
                // than a generic "transaction dropped" message.
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    // ----------------------------------------- Atomic apply (Phase 4)
    fn apply_graph_delta(
        &mut self,
        delta: &GraphDelta,
    ) -> Result<ApplyReport, GraphError> {
        // Design §5bis: atomic, idempotent persistence of a `GraphDelta`.
        // Single SQLite transaction wraps:
        //   (1) idempotence short-circuit  — graph_applied_deltas PK match
        //   (2) merges                      — loser→winner remapping for edges
        //   (3) entity upserts              — INSERT OR REPLACE
        //   (4) edge inserts                — referential integrity verified
        //   (5) edge invalidations          — set valid_to + invalidated_at
        //   (6) mention upserts             — graph_memory_entity_mentions
        //   (7) predicate registry          — batched usage_count UPDATE
        //   (8) stage failures              — graph_extraction_failures
        //   (9) memory cache                — memories.entity_ids / edge_ids
        //  (10) idempotence row             — graph_applied_deltas write
        //
        // Crash-recovery: every write above is in the same tx, so a crash
        // before commit rolls back to a clean replay state, and a crash
        // after commit makes the next call observe `already_applied=true`.
        //
        // Float / reference validation is done up-front so an invalid
        // delta never opens a transaction (no work to roll back).

        let t_start = std::time::Instant::now();

        // ---- Pre-flight validation ----
        delta.validate_floats()?;
        delta.validate_references()?;

        let memory_id_text = delta.memory_id.to_string();
        let delta_hash = delta.delta_hash();
        let schema_version = GRAPH_DELTA_SCHEMA_VERSION as i64;

        // ---- Idempotence pre-check ----
        // Read outside the tx — if we already applied, no tx needed. The
        // idempotence-row is only inserted INSIDE the apply tx, so this read
        // is safe (any concurrent writer that's also applying the same
        // delta will block on the tx lock; whichever wins commits first,
        // the other observes the row on its retry).
        //
        // We need to distinguish three cases:
        //   (a) no row → fresh apply
        //   (b) row with same schema_version → return `already_applied`
        //   (c) row with same memory_id+delta_hash but DIFFERENT schema_version
        //       → error (§5bis "delta schema version mismatch")
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT schema_version FROM graph_applied_deltas
                 WHERE memory_id = ?1 AND delta_hash = ?2",
                rusqlite::params![memory_id_text, delta_hash.to_vec()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(existing_v) = existing {
            if existing_v == schema_version {
                return Ok(ApplyReport::already_applied_marker());
            } else {
                return Err(GraphError::Invariant("delta schema version mismatch"));
            }
        }

        // ---- Open transaction ----
        let tx = self.conn.transaction()?;
        let mut report = ApplyReport::new();
        let now_unix = dt_to_unix(chrono::Utc::now());

        // ---- (2) Merges ----
        // §5bis: applied before entity upserts so loser ids in edges remap
        // automatically. Each merge here is the SAME logic as
        // `merge_entities` step 2 (set merged_into + repoint aliases) BUT
        // we don't fan out edge supersession in this tx — that's a
        // potentially large operation that belongs in the dedicated
        // `merge_entities` API. Within `apply_graph_delta`, the delta has
        // already remapped loser ids to winner ids in `delta.edges`, so the
        // merge here is purely the redirect signal.
        for m in &delta.merges {
            let winner_blob = m.winner.as_bytes().to_vec();
            let loser_blob = m.loser.as_bytes().to_vec();
            // Verify both exist; this surfaces dangling refs as a typed
            // Invariant rather than a SQLite FK error.
            let lp: Option<()> = tx
                .query_row(
                    "SELECT 1 FROM graph_entities WHERE id = ?1 AND namespace = ?2",
                    rusqlite::params![loser_blob, self.namespace],
                    |_| Ok(()),
                )
                .optional()?;
            if lp.is_none() {
                return Err(GraphError::EntityNotFound(m.loser));
            }
            let wp: Option<()> = tx
                .query_row(
                    "SELECT 1 FROM graph_entities WHERE id = ?1 AND namespace = ?2",
                    rusqlite::params![winner_blob, self.namespace],
                    |_| Ok(()),
                )
                .optional()?;
            if wp.is_none() {
                return Err(GraphError::EntityNotFound(m.winner));
            }
            tx.execute(
                "UPDATE graph_entities
                 SET merged_into = ?1, updated_at = ?2
                 WHERE id = ?3 AND namespace = ?4",
                rusqlite::params![winner_blob, now_unix, loser_blob, self.namespace],
            )?;
            // Repoint aliases (drop colliding rows first, then UPDATE).
            tx.execute(
                "DELETE FROM graph_entity_aliases
                 WHERE canonical_id = ?1
                   AND namespace = ?2
                   AND (namespace, normalized, ?3) IN (
                       SELECT namespace, normalized, canonical_id
                       FROM graph_entity_aliases
                       WHERE canonical_id = ?3 AND namespace = ?2
                   )",
                rusqlite::params![loser_blob, self.namespace, winner_blob],
            )?;
            tx.execute(
                "UPDATE graph_entity_aliases
                 SET canonical_id = ?1,
                     former_canonical_id = COALESCE(former_canonical_id, ?2)
                 WHERE canonical_id = ?2 AND namespace = ?3",
                rusqlite::params![winner_blob, loser_blob, self.namespace],
            )?;
            report.entities_merged += 1;
        }

        // ---- (3) Entity upserts ----
        // Use INSERT OR REPLACE keyed on PK (id). REPLACE preserves FKs to
        // graph_entities only if SQLite is configured with cascade behavior;
        // here it's ON DELETE RESTRICT, which means REPLACE would conflict
        // if the entity has dependents. We use INSERT ... ON CONFLICT DO
        // UPDATE instead to avoid that footgun.
        for e in &delta.entities {
            // Validate embedding dim if present (mirrors `insert_entity`).
            if let Some(emb) = &e.embedding {
                if emb.len() != self.embedding_dim {
                    return Err(GraphError::Invariant("entity embedding dim mismatch"));
                }
            }
            let kind_text = kind_to_text(&e.kind)?;
            let attributes_json = serde_json::to_string(&e.attributes)?;
            let history_json = serde_json::to_string(&e.history)?;
            let agent_affect_json = match &e.agent_affect {
                Some(v) => Some(serde_json::to_string(v)?),
                None => None,
            };
            let fingerprint_blob: Option<Vec<u8>> = e.somatic_fingerprint.as_ref().map(|fp| {
                let arr = fp.as_array();
                let mut buf = Vec::with_capacity(32);
                for v in arr.iter() {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                buf
            });
            let embedding_blob: Option<Vec<u8>> = e.embedding.as_ref().map(|v| {
                let mut buf = Vec::with_capacity(v.len() * 4);
                for f in v.iter() {
                    buf.extend_from_slice(&f.to_le_bytes());
                }
                buf
            });
            let merged_into_blob: Option<Vec<u8>> =
                e.merged_into.map(|u| u.as_bytes().to_vec());
            tx.execute(
                "INSERT INTO graph_entities (
                    id, canonical_name, kind, summary, attributes,
                    first_seen, last_seen, created_at, updated_at,
                    activation, agent_affect, arousal, importance,
                    identity_confidence, somatic_fingerprint,
                    namespace, history, merged_into, embedding
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5,
                    ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13,
                    ?14, ?15,
                    ?16, ?17, ?18, ?19
                )
                ON CONFLICT(id) DO UPDATE SET
                    canonical_name = excluded.canonical_name,
                    summary = excluded.summary,
                    attributes = excluded.attributes,
                    last_seen = max(graph_entities.last_seen, excluded.last_seen),
                    updated_at = excluded.updated_at,
                    activation = excluded.activation,
                    agent_affect = excluded.agent_affect,
                    arousal = excluded.arousal,
                    importance = excluded.importance,
                    identity_confidence = excluded.identity_confidence,
                    somatic_fingerprint = excluded.somatic_fingerprint,
                    history = excluded.history,
                    embedding = excluded.embedding",
                rusqlite::params![
                    e.id.as_bytes().to_vec(),
                    e.canonical_name,
                    kind_text,
                    e.summary,
                    attributes_json,
                    dt_to_unix(e.first_seen),
                    dt_to_unix(e.last_seen),
                    dt_to_unix(e.created_at),
                    dt_to_unix(e.updated_at),
                    e.activation,
                    agent_affect_json,
                    e.arousal,
                    e.importance,
                    e.identity_confidence,
                    fingerprint_blob,
                    self.namespace,
                    history_json,
                    merged_into_blob,
                    embedding_blob,
                ],
            )?;
            report.entities_upserted += 1;
        }

        // ---- (4) Edge inserts ----
        // Track the inserted edge ids so we can write the cache update at
        // the end (memories.edge_ids JSON array).
        let mut inserted_edge_ids: Vec<Uuid> = Vec::with_capacity(delta.edges.len());
        for edge in &delta.edges {
            edge.validate()?;
            let (predicate_kind, predicate_label) = predicate_to_columns(&edge.predicate)?;
            let (object_kind, object_entity_blob, object_literal) =
                edge_end_to_columns(&edge.object)?;
            let resolution_text = resolution_method_to_text(&edge.resolution_method)?;
            let agent_affect_json = match &edge.agent_affect {
                Some(v) => Some(serde_json::to_string(v)?),
                None => None,
            };
            tx.execute(
                "INSERT INTO graph_edges (
                    id, subject_id,
                    predicate_kind, predicate_label,
                    object_kind, object_entity_id, object_literal,
                    summary,
                    valid_from, valid_to, recorded_at, invalidated_at,
                    invalidated_by, supersedes,
                    episode_id, memory_id,
                    resolution_method,
                    activation, confidence,
                    agent_affect,
                    created_at, namespace
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?22
                )",
                rusqlite::params![
                    edge.id.as_bytes().to_vec(),
                    edge.subject_id.as_bytes().to_vec(),
                    predicate_kind,
                    predicate_label,
                    object_kind,
                    object_entity_blob,
                    object_literal,
                    edge.summary,
                    opt_dt_to_unix(edge.valid_from),
                    opt_dt_to_unix(edge.valid_to),
                    dt_to_unix(edge.recorded_at),
                    opt_dt_to_unix(edge.invalidated_at),
                    edge.invalidated_by.map(|u| u.as_bytes().to_vec()),
                    edge.supersedes.map(|u| u.as_bytes().to_vec()),
                    edge.episode_id.map(|u| u.as_bytes().to_vec()),
                    edge.memory_id,
                    resolution_text,
                    edge.activation,
                    edge.confidence,
                    agent_affect_json,
                    dt_to_unix(edge.created_at),
                    self.namespace,
                ],
            )?;
            inserted_edge_ids.push(edge.id);
            report.edges_inserted += 1;

            // Buffer predicate use for batched flush at end (§4.2 batched
            // counter clause). This guarantees `usage_count` is exactly
            // accurate without a per-row UPDATE hotspot.
            *self
                .predicate_use_buffer
                .entry(predicate_label.clone())
                .or_insert(0) += 1;
        }

        // ---- (5) Edge invalidations ----
        for inv in &delta.edges_to_invalidate {
            let n = tx.execute(
                "UPDATE graph_edges
                 SET invalidated_at = ?1, invalidated_by = ?2
                 WHERE id = ?3 AND namespace = ?4 AND invalidated_at IS NULL",
                rusqlite::params![
                    inv.invalidated_at,
                    inv.superseded_by.map(|u| u.as_bytes().to_vec()),
                    inv.edge_id.as_bytes().to_vec(),
                    self.namespace,
                ],
            )?;
            // n == 0 means either the edge doesn't exist or is already
            // invalidated. Both are programming errors per §5bis
            // pre-validation; surface a typed Invariant rather than
            // continuing silently.
            if n == 0 {
                return Err(GraphError::Invariant(
                    "apply_graph_delta: invalidation target missing or already closed",
                ));
            }
            report.edges_invalidated += 1;
        }

        // ---- (6) Mentions ----
        // Track inserted entity ids for the memory-cache update.
        let mut mentioned_entity_ids: Vec<Uuid> = Vec::with_capacity(delta.mentions.len());
        for m in &delta.mentions {
            // Span is split into two columns; we serialize the JSON triple
            // (text, start, end) into a single mention_span text field per
            // §4.1 schema. Inspect existing schema to confirm the column
            // shape:
            //   graph_memory_entity_mentions
            //     (memory_id TEXT, entity_id BLOB, mention_text TEXT,
            //      mention_span TEXT, confidence REAL, recorded_at REAL,
            //      namespace TEXT, PK(memory_id, entity_id))
            let span_json = if m.span_start.is_some() || m.span_end.is_some() {
                Some(serde_json::to_string(&serde_json::json!({
                    "start": m.span_start,
                    "end": m.span_end,
                    "text": m.mention_text,
                }))?)
            } else if !m.mention_text.is_empty() {
                Some(serde_json::to_string(&serde_json::json!({
                    "text": m.mention_text,
                }))?)
            } else {
                None
            };
            tx.execute(
                "INSERT INTO graph_memory_entity_mentions (
                    memory_id, entity_id, mention_span, confidence,
                    recorded_at, namespace
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(memory_id, entity_id) DO UPDATE SET
                    confidence = max(graph_memory_entity_mentions.confidence, excluded.confidence),
                    mention_span = COALESCE(excluded.mention_span, graph_memory_entity_mentions.mention_span),
                    recorded_at = max(graph_memory_entity_mentions.recorded_at, excluded.recorded_at)",
                rusqlite::params![
                    m.memory_id.to_string(),
                    m.entity_id.as_bytes().to_vec(),
                    span_json,
                    m.confidence,
                    now_unix,
                    self.namespace,
                ],
            )?;
            mentioned_entity_ids.push(m.entity_id);
            report.mentions_inserted += 1;
        }

        // ---- (7) Proposed predicates ----
        for pp in &delta.proposed_predicates {
            tx.execute(
                "INSERT INTO graph_predicates (
                    kind, label, raw_first_seen,
                    usage_count, first_seen, last_seen
                 ) VALUES ('proposed', ?1, ?1, 0, ?2, ?2)
                 ON CONFLICT(kind, label) DO UPDATE SET
                    last_seen = max(graph_predicates.last_seen, excluded.last_seen)",
                rusqlite::params![pp.label, pp.first_seen_at],
            )?;
            report.predicates_registered += 1;
        }

        // ---- (7b) Flush batched predicate-use counter ----
        // Drain the buffer into one UPDATE per distinct predicate. Inserted
        // edges contributed (kind=canonical, label) entries; we INSERT-or-
        // increment so a never-before-seen canonical predicate is registered
        // on first use.
        let buffered_preds: Vec<(String, u64)> = self
            .predicate_use_buffer
            .drain()
            .collect();
        for (label, n) in buffered_preds {
            tx.execute(
                "INSERT INTO graph_predicates (
                    kind, label, raw_first_seen,
                    usage_count, first_seen, last_seen
                 ) VALUES ('canonical', ?1, ?1, ?2, ?3, ?3)
                 ON CONFLICT(kind, label) DO UPDATE SET
                    usage_count = graph_predicates.usage_count + excluded.usage_count,
                    last_seen   = max(graph_predicates.last_seen, excluded.last_seen)",
                rusqlite::params![label, n as i64, now_unix],
            )?;
        }

        // ---- (8) Stage failures ----
        for f in &delta.stage_failures {
            // The full ExtractionFailure id is generated here so the
            // delta can be replayed deterministically — we use a
            // content-derived id (BLAKE3 of episode + stage + occurred_at)
            // so a duplicate apply produces the same id and the PK
            // collision is harmless.
            //
            // Simpler approach: use the existing INSERT path but with
            // INSERT OR IGNORE so a replay (after the idempotence
            // short-circuit failed for some unusual reason) doesn't
            // duplicate. For v0 we rely on the idempotence row catching
            // replays before we get here; failures are inserted with a
            // fresh UUID.
            validate_failure_closed_sets(&f.stage, &f.error_category)?;
            let failure_id = Uuid::new_v4();
            tx.execute(
                "INSERT INTO graph_extraction_failures (
                    id, episode_id, stage, error_category, error_detail,
                    occurred_at, namespace
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    failure_id.as_bytes().to_vec(),
                    f.episode_id.as_bytes().to_vec(),
                    f.stage,
                    f.error_category,
                    f.error_detail,
                    f.occurred_at,
                    self.namespace,
                ],
            )?;
            report.failures_recorded += 1;
        }

        // ---- (9) Memory cache (memories.entity_ids / edge_ids) ----
        // §5bis: `apply_graph_delta` updates these atomically. Strategy:
        // read existing JSON, merge with delta's new ids, write back.
        // memories.entity_ids and edge_ids are nullable JSON arrays.
        if !mentioned_entity_ids.is_empty() || !inserted_edge_ids.is_empty() {
            let memory_id_str = delta.memory_id.to_string();
            // Read current JSON arrays.
            let cur: Option<(Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT entity_ids, edge_ids FROM memories WHERE id = ?1",
                    rusqlite::params![memory_id_str],
                    |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            // If the memories row doesn't exist, we don't fail — the cache
            // is advisory; the v0.2 stub schema in tests has no row, and
            // production memory inserts may happen out of order with graph
            // resolution.
            if let Some((cur_ents, cur_edges)) = cur {
                // Merge entity ids.
                let mut ent_set: std::collections::BTreeSet<String> = match cur_ents {
                    Some(s) => serde_json::from_str(&s).unwrap_or_default(),
                    None => Default::default(),
                };
                for u in &mentioned_entity_ids {
                    ent_set.insert(u.to_string());
                }
                let ent_json = serde_json::to_string(&ent_set)?;

                let mut edge_set: std::collections::BTreeSet<String> = match cur_edges {
                    Some(s) => serde_json::from_str(&s).unwrap_or_default(),
                    None => Default::default(),
                };
                for u in &inserted_edge_ids {
                    edge_set.insert(u.to_string());
                }
                let edge_json = serde_json::to_string(&edge_set)?;

                tx.execute(
                    "UPDATE memories SET entity_ids = ?1, edge_ids = ?2 WHERE id = ?3",
                    rusqlite::params![ent_json, edge_json, memory_id_str],
                )?;
            }
        }

        // ---- (10) Idempotence row ----
        report.tx_duration_us = t_start.elapsed().as_micros() as u64;
        let report_json = serde_json::to_string(&report)?;
        tx.execute(
            "INSERT INTO graph_applied_deltas (
                memory_id, delta_hash, schema_version, applied_at, report
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                memory_id_text,
                delta_hash.to_vec(),
                schema_version,
                now_unix,
                report_json,
            ],
        )?;

        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_types_default() {
        let m = MergeReport::default();
        assert_eq!(m.edges_superseded, 0);
        assert_eq!(m.aliases_repointed, 0);

        let em = EntityMentions::default();
        assert!(em.episode_ids.is_empty());
        assert!(em.memory_ids.is_empty());
    }

    #[test]
    fn proposed_predicate_stats_eq() {
        let a = ProposedPredicateStats { label: "x".into(), usage_count: 3 };
        let b = ProposedPredicateStats { label: "x".into(), usage_count: 3 };
        assert_eq!(a, b);
    }

    #[test]
    fn graph_tables_lists_ten_tables() {
        // Sanity: §4.1 specifies exactly 10 graph-layer tables.
        assert_eq!(GRAPH_TABLES.len(), 10);
        // No duplicates.
        let mut sorted: Vec<&str> = GRAPH_TABLES.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), GRAPH_TABLES.len());
    }

    // -------------------------------------------------- Phase 2 entity CRUD

    use crate::graph::affect::SomaticFingerprint;
    use crate::graph::entity::HistoryEntry;
    use crate::graph::storage_graph::init_graph_tables;
    use rusqlite::Connection;
    use serde_json::json;

    /// Open an in-memory connection with foreign-keys ON, the v0.2 `memories`
    /// stub table that some §4.1 tables reference, and the full graph schema.
    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT NOT NULL);",
        )
        .unwrap();
        init_graph_tables(&conn).expect("init graph tables");
        conn
    }

    /// Equivalence check that ignores the join-table-backed provenance vectors.
    /// `get_entity` returns empty `episode_mentions`/`memory_mentions` until
    /// Phase 3 wires the join tables; comparing those would force every test
    /// to clear them on its golden value, which is noise.
    fn assert_entity_core_eq(got: &Entity, want: &Entity) {
        assert_eq!(got.id, want.id, "id");
        assert_eq!(got.canonical_name, want.canonical_name, "canonical_name");
        assert_eq!(got.kind, want.kind, "kind");
        assert_eq!(got.summary, want.summary, "summary");
        assert_eq!(got.attributes, want.attributes, "attributes");
        assert_eq!(got.history.len(), want.history.len(), "history len");
        assert_eq!(got.merged_into, want.merged_into, "merged_into");
        assert_eq!(got.activation, want.activation, "activation");
        assert_eq!(got.arousal, want.arousal, "arousal");
        assert_eq!(got.importance, want.importance, "importance");
        assert_eq!(
            got.identity_confidence, want.identity_confidence,
            "identity_confidence"
        );
        assert_eq!(got.agent_affect, want.agent_affect, "agent_affect");
        assert_eq!(
            got.somatic_fingerprint, want.somatic_fingerprint,
            "somatic_fingerprint"
        );
        assert_eq!(got.embedding, want.embedding, "embedding"); // ISS-033
        // Timestamps round-trip through f64 so equality is approximate at the
        // nanosecond level; assert sub-millisecond agreement.
        let dt_close = |a: chrono::DateTime<chrono::Utc>, b: chrono::DateTime<chrono::Utc>| {
            (a.timestamp_nanos_opt().unwrap() - b.timestamp_nanos_opt().unwrap()).abs() < 1_000_000
        };
        assert!(dt_close(got.first_seen, want.first_seen), "first_seen");
        assert!(dt_close(got.last_seen, want.last_seen), "last_seen");
        assert!(dt_close(got.created_at, want.created_at), "created_at");
        assert!(dt_close(got.updated_at, want.updated_at), "updated_at");
    }

    #[test]
    fn insert_and_get_roundtrip_full_typed_fields() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let mut e = Entity::new("Alice".into(), EntityKind::Person, now);
        e.summary = "rolling".into();
        e.attributes = json!({"role": "ceo"});
        e.activation = 0.4;
        e.arousal = 0.5;
        e.importance = 0.6;
        e.identity_confidence = 0.7;
        e.agent_affect = Some(json!({"valence": 0.1}));
        e.somatic_fingerprint = Some(SomaticFingerprint::from_array([
            -0.1, 0.2, 0.3, 0.4, 0.5, -0.6, 0.7, 0.8,
        ]));
        e.history.push(HistoryEntry {
            at: now,
            field: "canonical_name".into(),
            old: json!("Al"),
            new: json!("Alice"),
            reason: "manual_update".into(),
        });

        store.insert_entity(&e).expect("insert ok");
        let got = store.get_entity(e.id).expect("get ok").expect("found");
        assert_entity_core_eq(&got, &e);
    }

    #[test]
    fn get_entity_missing_returns_none() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        assert!(store.get_entity(Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn insert_entity_rejects_reserved_attribute_keys() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let mut e = Entity::new("X".into(), EntityKind::Concept, now);
        // history is a reserved key promoted to a typed field; if we let it
        // through `attributes`, two writers could update history with no
        // audit trail and silently clobber merge invariants (§3.1).
        e.attributes = json!({"history": "should be rejected"});
        match store.insert_entity(&e) {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "reserved attribute key"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn insert_entity_persists_namespace_isolation() {
        let mut conn = fresh_conn();
        let now = Utc::now();
        let e = Entity::new("Alice".into(), EntityKind::Person, now);

        // Write through `ns_a`, read through `ns_b` — must not be visible.
        // Each store borrow is scoped so the &mut conn can be reused.
        let id = e.id;
        {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            store_a.insert_entity(&e).unwrap();
            assert!(store_a.get_entity(id).unwrap().is_some(), "visible in ns_a");
        }
        {
            let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
            assert!(store_b.get_entity(id).unwrap().is_none(), "hidden from ns_b");
        }
    }

    #[test]
    fn update_entity_cognitive_updates_and_validates_bounds() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let e = Entity::new("X".into(), EntityKind::Concept, now);
        let id = e.id;
        store.insert_entity(&e).unwrap();

        store
            .update_entity_cognitive(id, 0.8, 0.9, 0.95, Some(json!({"v": 0.2})))
            .expect("happy path");
        let got = store.get_entity(id).unwrap().unwrap();
        assert_eq!(got.activation, 0.8);
        assert_eq!(got.importance, 0.9);
        assert_eq!(got.identity_confidence, 0.95);
        assert_eq!(got.agent_affect, Some(json!({"v": 0.2})));

        // Out-of-range → Invariant, not silent clamp / not raw rusqlite error.
        match store.update_entity_cognitive(id, 1.5, 0.5, 0.5, None) {
            Err(GraphError::Invariant(_)) => {}
            other => panic!("expected Invariant for activation=1.5, got {:?}", other),
        }
        // NaN → Invariant (CHECK constraint at the SQLite layer is unreliable
        // for NaN on some builds; we enforce it in Rust).
        match store.update_entity_cognitive(id, f64::NAN, 0.5, 0.5, None) {
            Err(GraphError::Invariant(_)) => {}
            other => panic!("expected Invariant for NaN, got {:?}", other),
        }

        // Missing entity → EntityNotFound, not silent no-op.
        let missing = Uuid::new_v4();
        match store.update_entity_cognitive(missing, 0.1, 0.1, 0.1, None) {
            Err(GraphError::EntityNotFound(u)) => assert_eq!(u, missing),
            other => panic!("expected EntityNotFound, got {:?}", other),
        }
    }

    // ----- ISS-033: embedding column + blob roundtrip -----------------

    /// Helper: build a deterministic embedding vector of `dim` floats.
    fn make_embedding(dim: usize, seed: f32) -> Vec<f32> {
        (0..dim).map(|i| seed + (i as f32) * 0.001).collect()
    }

    #[test]
    fn entity_embedding_blob_roundtrip_helper() {
        // Direct test of the blob codec, independent of SQLite — pins the
        // little-endian convention before the storage layer round-trips it.
        let v = make_embedding(384, 0.1);
        let blob = entity_embedding_to_blob(Some(&v), 384).unwrap().unwrap();
        assert_eq!(blob.len(), 384 * 4, "4 bytes per f32, 384 floats");
        let back = entity_embedding_from_blob(Some(blob), 384)
            .unwrap()
            .unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn entity_embedding_blob_none_roundtrip() {
        // None ⇒ None on both sides — entity not yet embedded.
        assert!(entity_embedding_to_blob(None, 384).unwrap().is_none());
        assert!(entity_embedding_from_blob(None, 384).unwrap().is_none());
    }

    #[test]
    fn entity_embedding_blob_writer_rejects_dim_mismatch() {
        let v = make_embedding(100, 0.0);
        match entity_embedding_to_blob(Some(&v), 384) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch")
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn entity_embedding_blob_reader_rejects_corrupt_length() {
        // 1023 bytes is not a multiple of 4 AND not 384*4 — both ways wrong.
        let bad = vec![0u8; 1023];
        match entity_embedding_from_blob(Some(bad), 384) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch")
            }
            other => panic!("expected Invariant, got {:?}", other),
        }

        // Wrong-but-aligned length (e.g. shipped under dim=100, read under
        // dim=384) — must still be rejected, not silently truncated.
        let wrong_dim = vec![0u8; 100 * 4];
        match entity_embedding_from_blob(Some(wrong_dim), 384) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch")
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn insert_and_get_roundtrip_with_embedding() {
        // End-to-end: write entity with embedding, read back, embedding
        // matches bit-for-bit. This is the integration version of the
        // codec test above — proves the SqliteGraphStore wires the codec
        // into both INSERT and SELECT paths under the configured dim.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(384);
        let now = Utc::now();
        let mut e = Entity::new("Topic".into(), EntityKind::Topic, now);
        e.embedding = Some(make_embedding(384, 0.05));

        store.insert_entity(&e).expect("insert ok");
        let got = store.get_entity(e.id).expect("get ok").expect("found");
        assert_eq!(got.embedding, e.embedding);
    }

    #[test]
    fn insert_entity_rejects_dim_mismatch_at_write() {
        // Store configured for dim=384 but caller hands us a 100-dim vector.
        // Must fail before any SQL is executed.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(384);
        let now = Utc::now();
        let mut e = Entity::new("X".into(), EntityKind::Concept, now);
        e.embedding = Some(make_embedding(100, 0.0));
        match store.insert_entity(&e) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch")
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
        // And the row must NOT have been inserted. Even though the same
        // store didn't run an INSERT, a separate read must see no row —
        // proves the failure is pre-INSERT, not post-INSERT.
        assert!(store.get_entity(e.id).unwrap().is_none());
    }

    #[test]
    fn insert_entity_with_no_embedding_works() {
        // The fresh-entity case (Entity::new produces None) must keep
        // working without any embedding-related behavior change.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(384);
        let now = Utc::now();
        let e = Entity::new("X".into(), EntityKind::Concept, now);
        assert!(e.embedding.is_none());
        store.insert_entity(&e).expect("insert ok");
        let got = store.get_entity(e.id).expect("get ok").expect("found");
        assert!(got.embedding.is_none());
    }

    #[test]
    fn get_entity_rejects_stale_dim_blob() {
        // Simulate a dim-change without migration: a row written under dim=100
        // is read under dim=384. Reader must surface Invariant rather than
        // returning a corrupted vector or silently truncating.
        let mut conn = fresh_conn();
        let now = Utc::now();
        // Write under dim=100.
        let id;
        {
            let mut store_old = SqliteGraphStore::new(&mut conn).with_embedding_dim(100);
            let mut e = Entity::new("Stale".into(), EntityKind::Concept, now);
            e.embedding = Some(make_embedding(100, 0.0));
            id = e.id;
            store_old.insert_entity(&e).expect("insert ok at dim=100");
        }
        // Read under dim=384 — must reject.
        {
            let store_new = SqliteGraphStore::new(&mut conn).with_embedding_dim(384);
            match store_new.get_entity(id) {
                Err(GraphError::Invariant(msg)) => {
                    assert_eq!(msg, "entity embedding dim mismatch")
                }
                other => panic!("expected Invariant, got {:?}", other),
            }
        }
    }

    #[test]
    fn embed_scan_partial_index_exists() {
        // The §3.4.1 candidate-retrieval scan relies on a partial index
        // keyed by (namespace, last_seen DESC) WHERE embedding IS NOT NULL.
        // Pin its existence so no future schema cleanup silently drops it.
        let conn = fresh_conn();
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND name='idx_graph_entities_embed_scan'",
            )
            .unwrap();
        let exists: bool = stmt.exists([]).unwrap();
        assert!(exists, "ISS-033: idx_graph_entities_embed_scan must be present");
    }

    #[test]
    fn touch_entity_last_seen_is_monotonic() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let e = Entity::new("X".into(), EntityKind::Concept, t0);
        let id = e.id;
        store.insert_entity(&e).unwrap();

        let t1 = t0 + chrono::Duration::seconds(10);
        store.touch_entity_last_seen(id, t1).unwrap();
        let got = store.get_entity(id).unwrap().unwrap();
        // last_seen advanced.
        assert!(
            got.last_seen >= t1 - chrono::Duration::milliseconds(1),
            "last_seen should advance to t1, got {:?}",
            got.last_seen
        );

        // Stale write must not roll last_seen backwards (§3.1 monotonic invariant).
        let stale = t0 - chrono::Duration::seconds(5);
        store.touch_entity_last_seen(id, stale).unwrap();
        let got = store.get_entity(id).unwrap().unwrap();
        assert!(
            got.last_seen >= t1 - chrono::Duration::milliseconds(1),
            "last_seen must not regress on stale touch"
        );
    }

    #[test]
    fn list_entities_by_kind_filters_and_orders_by_recency() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();

        // Two People, one Concept. Different last_seen so ordering is testable.
        let mut alice = Entity::new("Alice".into(), EntityKind::Person, t0);
        let mut bob = Entity::new("Bob".into(), EntityKind::Person, t0);
        let concept = Entity::new("Justice".into(), EntityKind::Concept, t0);
        bob.last_seen = t0 + chrono::Duration::seconds(60); // most recent person
        alice.last_seen = t0 + chrono::Duration::seconds(30);

        store.insert_entity(&alice).unwrap();
        store.insert_entity(&bob).unwrap();
        store.insert_entity(&concept).unwrap();

        let people = store
            .list_entities_by_kind(&EntityKind::Person, 10)
            .unwrap();
        assert_eq!(people.len(), 2, "concept must be filtered out");
        assert_eq!(people[0].id, bob.id, "ORDER BY last_seen DESC: bob first");
        assert_eq!(people[1].id, alice.id);

        // Limit honored.
        let limited = store
            .list_entities_by_kind(&EntityKind::Person, 1)
            .unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, bob.id);

        // Custom `Other` kind round-trips through serde tag exactly.
        let robot = Entity::new(
            "R2".into(),
            EntityKind::other("robot"),
            t0,
        );
        store.insert_entity(&robot).unwrap();
        let robots = store
            .list_entities_by_kind(&EntityKind::other("robot"), 10)
            .unwrap();
        assert_eq!(robots.len(), 1);
        assert_eq!(robots[0].id, robot.id);
    }

    #[test]
    fn list_namespaces_returns_distinct_sorted() {
        let mut conn = fresh_conn();
        let now = Utc::now();
        let e1 = Entity::new("A".into(), EntityKind::Person, now);
        let e2 = Entity::new("B".into(), EntityKind::Person, now);
        let e3 = Entity::new("C".into(), EntityKind::Person, now);
        {
            let mut s = SqliteGraphStore::new(&mut conn).with_namespace("zeta");
            s.insert_entity(&e1).unwrap();
        }
        {
            let mut s = SqliteGraphStore::new(&mut conn).with_namespace("alpha");
            s.insert_entity(&e2).unwrap();
            s.insert_entity(&e3).unwrap();
        }
        let store = SqliteGraphStore::new(&mut conn);
        let ns = store.list_namespaces().unwrap();
        assert_eq!(ns, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn with_transaction_commits_on_ok_rolls_back_on_err() {
        let mut conn = fresh_conn();
        let now = Utc::now();
        let e = Entity::new("Committed".into(), EntityKind::Person, now);
        let id = e.id;

        // Commit path: insert via raw SQL inside the txn; visible after commit.
        {
            let mut store = SqliteGraphStore::new(&mut conn);
            store
                .with_transaction(&mut |tx: &Transaction<'_>| {
                    tx.execute(
                        "INSERT INTO graph_entities (
                            id, canonical_name, kind, first_seen, last_seen,
                            created_at, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?4)",
                        rusqlite::params![
                            id.as_bytes().to_vec(),
                            "Committed",
                            kind_to_text(&EntityKind::Person).unwrap(),
                            dt_to_unix(now),
                        ],
                    )?;
                    Ok(())
                })
                .expect("commit");
            assert!(store.get_entity(id).unwrap().is_some(), "committed");
        }

        // Rollback path: closure inserts another row, then errors. Row must
        // not survive — `with_transaction` calls `tx.rollback()`.
        let bad_id = Uuid::new_v4();
        {
            let mut store = SqliteGraphStore::new(&mut conn);
            let _ = e; // capture-clarity
            let err = store.with_transaction(&mut |tx: &Transaction<'_>| {
                tx.execute(
                    "INSERT INTO graph_entities (
                        id, canonical_name, kind, first_seen, last_seen,
                        created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?4)",
                    rusqlite::params![
                        bad_id.as_bytes().to_vec(),
                        "DoomedRow",
                        kind_to_text(&EntityKind::Person).unwrap(),
                        dt_to_unix(now),
                    ],
                )?;
                Err(GraphError::Invariant("simulated failure"))
            });
            assert!(matches!(err, Err(GraphError::Invariant("simulated failure"))));
            assert!(
                store.get_entity(bad_id).unwrap().is_none(),
                "rollback must drop the doomed insert"
            );
        }
    }

    #[test]
    fn dt_unix_helpers_roundtrip_subsecond() {
        let t = Utc::now();
        let secs = dt_to_unix(t);
        let back = unix_to_dt(secs).unwrap();
        let drift = (t.timestamp_nanos_opt().unwrap()
            - back.timestamp_nanos_opt().unwrap())
        .abs();
        assert!(drift < 1_000_000, "subsecond drift > 1ms: {drift}ns");
    }

    // -------------------------------------------------- Phase 2 edge CRUD

    use crate::graph::edge::{ConfidenceSource, Edge, EdgeEnd, ResolutionMethod};
    use crate::graph::schema::CanonicalPredicate;

    /// Insert a minimal subject entity for edges to attach to. Returns the id.
    /// Most edge tests don't care about entity attributes — this is the
    /// FK-satisfaction shim.
    fn insert_subject_entity(store: &mut SqliteGraphStore<'_>, name: &str) -> Uuid {
        let now = Utc::now();
        let e = Entity::new(name.into(), EntityKind::Person, now);
        let id = e.id;
        store.insert_entity(&e).expect("insert subject");
        id
    }

    /// Compare two edges for the round-trippable subset of fields (i.e.
    /// every field except `confidence_source`, which is not yet persisted
    /// in §4.1 — see DevNote #5). Timestamps are checked with
    /// sub-millisecond tolerance because of the f64 storage round-trip.
    fn assert_edge_core_eq(got: &Edge, want: &Edge) {
        assert_eq!(got.id, want.id, "id");
        assert_eq!(got.subject_id, want.subject_id, "subject_id");
        assert_eq!(got.predicate, want.predicate, "predicate");
        assert_eq!(got.object, want.object, "object");
        assert_eq!(got.summary, want.summary, "summary");
        assert_eq!(got.invalidated_by, want.invalidated_by, "invalidated_by");
        assert_eq!(got.supersedes, want.supersedes, "supersedes");
        assert_eq!(got.episode_id, want.episode_id, "episode_id");
        assert_eq!(got.memory_id, want.memory_id, "memory_id");
        assert_eq!(got.resolution_method, want.resolution_method, "resolution_method");
        assert_eq!(got.activation, want.activation, "activation");
        assert_eq!(got.confidence, want.confidence, "confidence");
        assert_eq!(got.agent_affect, want.agent_affect, "agent_affect");

        let dt_close = |a: chrono::DateTime<chrono::Utc>, b: chrono::DateTime<chrono::Utc>| {
            (a.timestamp_nanos_opt().unwrap() - b.timestamp_nanos_opt().unwrap()).abs() < 1_000_000
        };
        let opt_dt_close = |a: Option<chrono::DateTime<chrono::Utc>>,
                            b: Option<chrono::DateTime<chrono::Utc>>| match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => dt_close(x, y),
            _ => false,
        };
        assert!(opt_dt_close(got.valid_from, want.valid_from), "valid_from");
        assert!(opt_dt_close(got.valid_to, want.valid_to), "valid_to");
        assert!(dt_close(got.recorded_at, want.recorded_at), "recorded_at");
        assert!(opt_dt_close(got.invalidated_at, want.invalidated_at), "invalidated_at");
        assert!(dt_close(got.created_at, want.created_at), "created_at");
    }

    #[test]
    fn insert_and_get_edge_roundtrip_canonical_entity_object() {
        // Full happy path: canonical predicate + entity object + every
        // optional field populated. Round-trip every persisted column.
        // memory_id has FK → memories(id); seed the stub row.
        let mut conn = fresh_conn();
        conn.execute(
            "INSERT INTO memories (id, content) VALUES (?1, ?2)",
            rusqlite::params!["mem-1", "hello"],
        )
        .unwrap();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let mut e = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            Some(now),
            now,
        );
        e.summary = "Alice works at Acme".into();
        e.valid_to = Some(now + chrono::Duration::days(365));
        e.episode_id = Some(Uuid::new_v4());
        e.memory_id = Some("mem-1".into());
        e.resolution_method = ResolutionMethod::LlmTieBreaker;
        e.activation = 0.4;
        e.confidence = 0.9;
        e.agent_affect = Some(serde_json::json!({"valence": 0.2}));

        store.insert_edge(&e).expect("insert ok");
        let got = store.get_edge(e.id).expect("get ok").expect("found");
        assert_edge_core_eq(&got, &e);
        // confidence_source is re-defaulted on read (DevNote #5) — explicit assertion.
        assert_eq!(got.confidence_source, ConfidenceSource::Recovered);
    }

    #[test]
    fn insert_and_get_edge_roundtrip_proposed_literal() {
        // Cover the *other* axes: proposed predicate + literal object.
        // `Predicate::proposed` normalizes whitespace/case; verify the
        // round-trip preserves the normalized form.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Bob");

        let mut e = Edge::new(
            subj,
            Predicate::proposed("  Likes   Coffee  "),
            EdgeEnd::Literal {
                value: serde_json::json!({"strength": "strong"}),
            },
            None,
            now,
        );
        e.confidence = 0.55;

        store.insert_edge(&e).unwrap();
        let got = store.get_edge(e.id).unwrap().unwrap();
        assert_edge_core_eq(&got, &e);
        // Predicate normalization survived round-trip.
        match &got.predicate {
            Predicate::Proposed(s) => assert_eq!(s, "likes coffee"),
            other => panic!("expected Proposed, got {:?}", other),
        }
    }

    #[test]
    fn get_edge_missing_returns_none() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        assert!(store.get_edge(Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn insert_edge_rejects_invariant_violations_in_rust() {
        // Out-of-range cognitive scalars must be caught at the Rust layer
        // (CHECK constraints exist for confidence/activation but we want a
        // typed `GraphError::Invariant`, not a raw SqliteFailure).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let mut e = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            now,
        );
        e.confidence = 1.5;
        match store.insert_edge(&e) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "edge confidence out of range");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn insert_edge_namespace_isolation() {
        // Edge written through ns_a is invisible from ns_b — same as entity
        // namespace isolation. Subject + object FKs live in graph_entities,
        // which is also namespaced, so we insert subjects per-namespace.
        let mut conn = fresh_conn();
        let now = Utc::now();
        let edge_id = {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let s = insert_subject_entity(&mut store_a, "AliceA");
            let o = insert_subject_entity(&mut store_a, "AcmeA");
            let e = Edge::new(
                s,
                Predicate::Canonical(CanonicalPredicate::WorksAt),
                EdgeEnd::Entity { id: o },
                None,
                now,
            );
            let eid = e.id;
            store_a.insert_edge(&e).unwrap();
            assert!(store_a.get_edge(eid).unwrap().is_some(), "visible in ns_a");
            eid
        };
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        assert!(store_b.get_edge(edge_id).unwrap().is_none(), "hidden from ns_b");
    }

    #[test]
    fn edges_of_filters_by_predicate_and_orders_by_recency() {
        // Two predicates × two recorded_at times. Verify:
        //  - no-filter returns both, most-recent first
        //  - filtered returns only the matching predicate
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj1 = insert_subject_entity(&mut store, "Acme");
        let obj2 = insert_subject_entity(&mut store, "OtherCo");

        let mut e_old = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj1 },
            None,
            t0,
        );
        e_old.recorded_at = t0;
        let mut e_new = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj2 },
            None,
            t0,
        );
        e_new.recorded_at = t0 + chrono::Duration::seconds(60);
        let mut e_other = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::DependsOn),
            EdgeEnd::Entity { id: obj1 },
            None,
            t0,
        );
        e_other.recorded_at = t0 + chrono::Duration::seconds(30);
        store.insert_edge(&e_old).unwrap();
        store.insert_edge(&e_new).unwrap();
        store.insert_edge(&e_other).unwrap();

        let all = store.edges_of(subj, None, true).unwrap();
        assert_eq!(all.len(), 3);
        // Most-recent first.
        assert_eq!(all[0].id, e_new.id, "newest first");
        assert_eq!(all[2].id, e_old.id, "oldest last");

        let works = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let filtered = store.edges_of(subj, Some(&works), true).unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|e| matches!(
            &e.predicate,
            Predicate::Canonical(CanonicalPredicate::WorksAt)
        )));
    }

    // ----- ISS-034: find_edges + invalidate_edge -----

    fn make_edge(subj: Uuid, obj: Uuid, t: DateTime<Utc>) -> Edge {
        Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            t,
        )
    }

    #[test]
    fn find_edges_happy_path_entity_object() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let e1 = make_edge(subj, obj, t0);
        let e2 = make_edge(subj, obj, t0 + chrono::Duration::seconds(10));
        let e3 = make_edge(subj, obj, t0 + chrono::Duration::seconds(20));
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();
        store.insert_edge(&e3).unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj };
        let found = store.find_edges(subj, &pred, Some(&object), true).unwrap();
        assert_eq!(found.len(), 3, "all three live edges returned");
        assert_eq!(found[0].id, e3.id, "newest first");
        assert_eq!(found[2].id, e1.id, "oldest last");
    }

    #[test]
    fn find_edges_filters_by_object_identity() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let acme = insert_subject_entity(&mut store, "Acme");
        let other = insert_subject_entity(&mut store, "OtherCo");

        let e_acme = make_edge(subj, acme, now);
        let e_other = make_edge(subj, other, now + chrono::Duration::seconds(1));
        store.insert_edge(&e_acme).unwrap();
        store.insert_edge(&e_other).unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found_acme = store
            .find_edges(subj, &pred, Some(&EdgeEnd::Entity { id: acme }), true)
            .unwrap();
        assert_eq!(found_acme.len(), 1);
        assert_eq!(found_acme[0].id, e_acme.id);

        let found_other = store
            .find_edges(subj, &pred, Some(&EdgeEnd::Entity { id: other }), true)
            .unwrap();
        assert_eq!(found_other.len(), 1);
        assert_eq!(found_other[0].id, e_other.id);
    }

    #[test]
    fn find_edges_valid_only_filter() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let live = make_edge(subj, obj, now);
        let mut dead = make_edge(subj, obj, now + chrono::Duration::seconds(1));
        dead.invalidate(now + chrono::Duration::seconds(2));
        store.insert_edge(&live).unwrap();
        store.insert_edge(&dead).unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj };
        let live_only = store.find_edges(subj, &pred, Some(&object), true).unwrap();
        assert_eq!(live_only.len(), 1, "only live edge with valid_only=true");
        assert_eq!(live_only[0].id, live.id);

        let all = store.find_edges(subj, &pred, Some(&object), false).unwrap();
        assert_eq!(all.len(), 2, "both edges with valid_only=false");
    }

    #[test]
    fn find_edges_literal_object() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");

        let lit_a = EdgeEnd::Literal { value: serde_json::json!(42) };
        let lit_b = EdgeEnd::Literal { value: serde_json::json!("hello") };

        let e_a = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            lit_a.clone(),
            None,
            now,
        );
        let e_b = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            lit_b.clone(),
            None,
            now + chrono::Duration::seconds(1),
        );
        store.insert_edge(&e_a).unwrap();
        store.insert_edge(&e_b).unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found_a = store.find_edges(subj, &pred, Some(&lit_a), true).unwrap();
        assert_eq!(found_a.len(), 1);
        assert_eq!(found_a[0].id, e_a.id);

        let found_b = store.find_edges(subj, &pred, Some(&lit_b), true).unwrap();
        assert_eq!(found_b.len(), 1);
        assert_eq!(found_b[0].id, e_b.id);
    }

    #[test]
    fn find_edges_empty_result() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj };

        let found = store.find_edges(subj, &pred, Some(&object), true).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn find_edges_caps_at_max_results() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let n = MAX_FIND_EDGES_RESULTS + 5;
        for i in 0..n {
            let e = make_edge(subj, obj, t0 + chrono::Duration::seconds(i as i64));
            store.insert_edge(&e).unwrap();
        }
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj };
        let found = store.find_edges(subj, &pred, Some(&object), true).unwrap();
        assert_eq!(found.len(), MAX_FIND_EDGES_RESULTS, "result cap honored");
    }

    #[test]
    fn find_edges_uses_spo_index() {
        let mut conn = fresh_conn();
        {
            let mut store = SqliteGraphStore::new(&mut conn);
            let now = Utc::now();
            let s = insert_subject_entity(&mut store, "Alice");
            let o = insert_subject_entity(&mut store, "Acme");
            let e = make_edge(s, o, now);
            store.insert_edge(&e).unwrap();
        }
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT id FROM graph_edges
                 WHERE subject_id = ?1 AND namespace = ?2
                   AND predicate_kind = ?3 AND predicate_label = ?4
                   AND object_kind = ?5 AND object_entity_id = ?6
                   AND invalidated_at IS NULL
                 ORDER BY recorded_at DESC, id ASC
                 LIMIT 64",
            )
            .unwrap();
        let dummy_blob = Uuid::new_v4().as_bytes().to_vec();
        let plan: Vec<String> = stmt
            .query_map(
                rusqlite::params![dummy_blob, "default", "canonical", "WorksAt", "entity", dummy_blob],
                |r| r.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let plan_text = plan.join("\n");
        assert!(
            plan_text.contains("idx_graph_edges_spo"),
            "EXPLAIN QUERY PLAN should use idx_graph_edges_spo, got:\n{}",
            plan_text
        );
    }

    // ----- ISS-035: slot lookup tests (object = None) -----

    #[test]
    fn find_edges_slot_returns_all_objects_for_subject_predicate() {
        // Slot lookup `(subject, predicate)` should return every active
        // object the subject is attached to via that predicate. This is
        // the v03-resolution §3.4.4 use case: "what does Alice work at?"
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let alice = insert_subject_entity(&mut store, "Alice");
        let acme = insert_subject_entity(&mut store, "Acme");
        let beta = insert_subject_entity(&mut store, "Beta");
        let gamma = insert_subject_entity(&mut store, "Gamma");

        // Three live edges with different objects (multi-valued case for
        // testing — semantically WorksAt is functional, but we're testing
        // the SQL retrieval, not the resolution decision).
        store.insert_edge(&make_edge(alice, acme, t0)).unwrap();
        store
            .insert_edge(&make_edge(alice, beta, t0 + chrono::Duration::seconds(10)))
            .unwrap();
        store
            .insert_edge(&make_edge(alice, gamma, t0 + chrono::Duration::seconds(20)))
            .unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found = store.find_edges(alice, &pred, None, true).unwrap();
        assert_eq!(found.len(), 3, "slot lookup returns all live objects");

        // Returned edges, ordered by recorded_at DESC.
        let returned_objects: Vec<EdgeEnd> = found.iter().map(|e| e.object.clone()).collect();
        assert_eq!(returned_objects[0], EdgeEnd::Entity { id: gamma });
        assert_eq!(returned_objects[1], EdgeEnd::Entity { id: beta });
        assert_eq!(returned_objects[2], EdgeEnd::Entity { id: acme });
    }

    #[test]
    fn find_edges_slot_valid_only_filter() {
        // valid_only=true must skip invalidated edges in slot mode (same
        // contract as triple mode).
        let mut conn = fresh_conn();
        let t0 = Utc::now();
        let (alice, acme, beta, e1_id) = {
            let mut store = SqliteGraphStore::new(&mut conn);
            let alice = insert_subject_entity(&mut store, "Alice");
            let acme = insert_subject_entity(&mut store, "Acme");
            let beta = insert_subject_entity(&mut store, "Beta");
            let e1 = make_edge(alice, acme, t0);
            let e2 = make_edge(alice, beta, t0 + chrono::Duration::seconds(10));
            let e1_id = e1.id;
            store.insert_edge(&e1).unwrap();
            store.insert_edge(&e2).unwrap();
            // Invalidate e1 (Alice no longer at Acme).
            store
                .invalidate_edge(e1_id, e2.id, t0 + chrono::Duration::seconds(20))
                .unwrap();
            (alice, acme, beta, e1_id)
        };

        let store = SqliteGraphStore::new(&mut conn);
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);

        // valid_only=true: only the live edge.
        let live = store.find_edges(alice, &pred, None, true).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].object, EdgeEnd::Entity { id: beta });

        // valid_only=false: both edges.
        let all = store.find_edges(alice, &pred, None, false).unwrap();
        assert_eq!(all.len(), 2);
        // Sanity: the invalidated edge is in the result set.
        assert!(all.iter().any(|e| e.id == e1_id && e.object == EdgeEnd::Entity { id: acme }));
    }

    #[test]
    fn find_edges_slot_empty_result() {
        // No edges for the (subject, predicate) slot → empty Vec, no error.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let alice = insert_subject_entity(&mut store, "Alice");
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found = store.find_edges(alice, &pred, None, true).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn find_edges_slot_uncapped_above_max_results() {
        // ISS-035 critical guarantee: slot lookup must NOT cap results.
        // The whole point of slot mode is "give me everything for (S,P)";
        // truncation would cause spurious Add decisions in the resolution
        // pipeline (already-known objects missing from the result set get
        // re-Added). Verify by inserting MAX_FIND_EDGES_RESULTS + 10 edges
        // with distinct objects.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let alice = insert_subject_entity(&mut store, "Alice");

        let n = MAX_FIND_EDGES_RESULTS + 10;
        for i in 0..n {
            // Distinct object per edge — slot lookup should return all of them.
            let obj = insert_subject_entity(&mut store, &format!("Obj{}", i));
            let e = make_edge(alice, obj, t0 + chrono::Duration::seconds(i as i64));
            store.insert_edge(&e).unwrap();
        }
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found = store.find_edges(alice, &pred, None, true).unwrap();
        assert_eq!(
            found.len(),
            n,
            "slot lookup must be uncapped (got {}, expected {})",
            found.len(),
            n
        );
    }

    #[test]
    fn find_edges_slot_namespace_isolation() {
        // Slot lookup must respect namespace just like triple lookup.
        let mut conn = fresh_conn();
        let t0 = Utc::now();

        let (alice_a, _) = {
            let mut store_a = SqliteGraphStore::new(&mut conn);
            let alice = insert_subject_entity(&mut store_a, "Alice");
            let acme = insert_subject_entity(&mut store_a, "Acme");
            store_a.insert_edge(&make_edge(alice, acme, t0)).unwrap();
            (alice, acme)
        };

        // Same UUID, different namespace → no spillover.
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("other");
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let found = store_b.find_edges(alice_a, &pred, None, true).unwrap();
        assert!(found.is_empty(), "slot lookup must respect namespace");
    }

    #[test]
    fn invalidate_edge_happy_path() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let prior = make_edge(subj, obj, now);
        let successor = make_edge(subj, obj, now + chrono::Duration::seconds(1));
        store.insert_edge(&prior).unwrap();
        store.insert_edge(&successor).unwrap();

        let close_at = now + chrono::Duration::seconds(2);
        store.invalidate_edge(prior.id, successor.id, close_at).unwrap();

        let reloaded = store.get_edge(prior.id).unwrap().unwrap();
        assert!(reloaded.invalidated_at.is_some(), "invalidated_at set");
        assert_eq!(reloaded.invalidated_by, Some(successor.id), "invalidated_by set");
        let succ_reloaded = store.get_edge(successor.id).unwrap().unwrap();
        assert!(succ_reloaded.invalidated_at.is_none(), "successor unchanged");
    }

    #[test]
    fn invalidate_edge_idempotent_same_successor() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let prior = make_edge(subj, obj, now);
        let successor = make_edge(subj, obj, now + chrono::Duration::seconds(1));
        store.insert_edge(&prior).unwrap();
        store.insert_edge(&successor).unwrap();

        let close_at = now + chrono::Duration::seconds(2);
        store.invalidate_edge(prior.id, successor.id, close_at).unwrap();
        store
            .invalidate_edge(prior.id, successor.id, close_at + chrono::Duration::seconds(5))
            .expect("idempotent retry must be Ok");

        let reloaded = store.get_edge(prior.id).unwrap().unwrap();
        let first_close_unix = dt_to_unix(close_at);
        let actual_close_unix = dt_to_unix(reloaded.invalidated_at.unwrap());
        assert!(
            (first_close_unix - actual_close_unix).abs() < 1e-3,
            "no-op preserved original invalidated_at"
        );
    }

    #[test]
    fn invalidate_edge_already_closed_by_other_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let prior = make_edge(subj, obj, now);
        let succ_a = make_edge(subj, obj, now + chrono::Duration::seconds(1));
        let succ_b = make_edge(subj, obj, now + chrono::Duration::seconds(2));
        store.insert_edge(&prior).unwrap();
        store.insert_edge(&succ_a).unwrap();
        store.insert_edge(&succ_b).unwrap();

        store
            .invalidate_edge(prior.id, succ_a.id, now + chrono::Duration::seconds(3))
            .unwrap();
        match store.invalidate_edge(prior.id, succ_b.id, now + chrono::Duration::seconds(4)) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("already invalidated"), "msg: {}", msg);
            }
            other => panic!("expected Invariant error, got {:?}", other),
        }
    }

    #[test]
    fn invalidate_edge_missing_prior_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let successor = make_edge(subj, obj, now);
        store.insert_edge(&successor).unwrap();

        let missing = Uuid::new_v4();
        match store.invalidate_edge(missing, successor.id, now) {
            Err(GraphError::EdgeNotFound(id)) => assert_eq!(id, missing),
            other => panic!("expected EdgeNotFound, got {:?}", other),
        }
    }

    #[test]
    fn invalidate_edge_missing_successor_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let prior = make_edge(subj, obj, now);
        store.insert_edge(&prior).unwrap();

        let missing_succ = Uuid::new_v4();
        match store.invalidate_edge(prior.id, missing_succ, now) {
            Err(GraphError::EdgeNotFound(id)) => assert_eq!(id, missing_succ),
            other => panic!("expected EdgeNotFound, got {:?}", other),
        }
    }

    #[test]
    fn find_edges_skips_invalidated_when_valid_only() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let prior = make_edge(subj, obj, now);
        let successor = make_edge(subj, obj, now + chrono::Duration::seconds(1));
        store.insert_edge(&prior).unwrap();
        store.insert_edge(&successor).unwrap();
        store
            .invalidate_edge(prior.id, successor.id, now + chrono::Duration::seconds(2))
            .unwrap();

        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj };
        let live = store.find_edges(subj, &pred, Some(&object), true).unwrap();
        assert_eq!(live.len(), 1, "prior must be excluded");
        assert_eq!(live[0].id, successor.id);
    }

    #[test]
    fn find_edges_namespace_isolation() {
        let mut conn = fresh_conn();
        let now = Utc::now();
        let (subj_a, obj_a, edge_a) = {
            let mut store = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let s = insert_subject_entity(&mut store, "AliceA");
            let o = insert_subject_entity(&mut store, "AcmeA");
            let e = make_edge(s, o, now);
            store.insert_edge(&e).unwrap();
            (s, o, e.id)
        };
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let object = EdgeEnd::Entity { id: obj_a };
        let found = store_b.find_edges(subj_a, &pred, Some(&object), true).unwrap();
        assert!(found.is_empty(), "ns_b cannot see edge {} in ns_a", edge_a);
    }

    #[test]
    fn edges_of_include_invalidated_flag() {
        // Two edges, one invalidated (by direct write — supersede_edge is
        // Phase 3 and not yet implemented). Verify the include flag
        // gates visibility correctly.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let live = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            now,
        );
        let mut dead = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            now,
        );
        dead.invalidate(now + chrono::Duration::seconds(1));
        store.insert_edge(&live).unwrap();
        store.insert_edge(&dead).unwrap();

        // include_invalidated = false → only the live edge.
        let live_only = store.edges_of(subj, None, false).unwrap();
        assert_eq!(live_only.len(), 1);
        assert_eq!(live_only[0].id, live.id);
        assert!(live_only[0].is_live());

        // include_invalidated = true → both.
        let both = store.edges_of(subj, None, true).unwrap();
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn edges_in_episode_returns_ids_only() {
        // edges_in_episode returns Vec<Uuid>, *not* hydrated edges (callers
        // chain get_edge for hydration). Verify ids match and ordering is
        // recorded_at DESC inside an episode.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let ep = Uuid::new_v4();
        let other_ep = Uuid::new_v4();

        let mut e1 = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            t0,
        );
        e1.episode_id = Some(ep);
        let mut e2 = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::DependsOn),
            EdgeEnd::Entity { id: obj },
            None,
            t0 + chrono::Duration::seconds(10),
        );
        e2.episode_id = Some(ep);
        let mut e_other = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::Uses),
            EdgeEnd::Entity { id: obj },
            None,
            t0,
        );
        e_other.episode_id = Some(other_ep);
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();
        store.insert_edge(&e_other).unwrap();

        let ids = store.edges_in_episode(ep).unwrap();
        assert_eq!(ids.len(), 2);
        // Most-recent first.
        assert_eq!(ids[0], e2.id);
        assert_eq!(ids[1], e1.id);

        // Unknown episode → empty (not error).
        assert!(store.edges_in_episode(Uuid::new_v4()).unwrap().is_empty());
    }

    #[test]
    fn edges_sourced_from_memory_hydrates_full_edges() {
        // Symmetric to edges_in_episode, but returns hydrated edges (callers
        // need the predicate/object to render mention timelines).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        // graph_edges.memory_id has FK → memories(id); seed the stub row.
        conn.execute(
            "INSERT INTO memories (id, content) VALUES (?1, ?2)",
            rusqlite::params!["mem-1", "hello"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memories (id, content) VALUES (?1, ?2)",
            rusqlite::params!["mem-2", "world"],
        )
        .unwrap();

        let mut store = SqliteGraphStore::new(&mut conn);
        let mut e1 = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: obj },
            None,
            now,
        );
        e1.memory_id = Some("mem-1".into());
        let mut e2 = Edge::new(
            subj,
            Predicate::Canonical(CanonicalPredicate::DependsOn),
            EdgeEnd::Entity { id: obj },
            None,
            now,
        );
        e2.memory_id = Some("mem-2".into());
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();

        let from_mem1 = store.edges_sourced_from_memory("mem-1").unwrap();
        assert_eq!(from_mem1.len(), 1);
        assert_eq!(from_mem1[0].id, e1.id);
        assert_eq!(from_mem1[0].memory_id.as_deref(), Some("mem-1"));
        // Hydrated, not just an id.
        assert_eq!(
            from_mem1[0].predicate,
            Predicate::Canonical(CanonicalPredicate::WorksAt)
        );

        // Unknown memory → empty (not error).
        assert!(store
            .edges_sourced_from_memory("nonexistent")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn predicate_codec_roundtrips_canonical_and_proposed() {
        // Direct unit test of the helper pair — independent of SQL — so a
        // future regression on the codec is caught even if the per-method
        // tests fail for an unrelated reason.
        let canonical = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let (kind, label) = predicate_to_columns(&canonical).unwrap();
        assert_eq!(kind, "canonical");
        assert_eq!(label, "works_at");
        assert_eq!(columns_to_predicate(kind, &label).unwrap(), canonical);

        let proposed = Predicate::proposed("Mentioned With");
        let (kind, label) = predicate_to_columns(&proposed).unwrap();
        assert_eq!(kind, "proposed");
        assert_eq!(label, "mentioned with");
        assert_eq!(columns_to_predicate(kind, &label).unwrap(), proposed);

        // Unknown kind → Invariant (not panic).
        match columns_to_predicate("nonsense", "x") {
            Err(GraphError::Invariant(_)) => {}
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn edge_end_codec_xor_enforced_at_decode() {
        // Even if the SQL-level CHECK is bypassed (e.g. legacy data,
        // partial migration), the read boundary must reject malformed
        // (kind, entity_blob, literal_text) combinations.
        match columns_to_edge_end("entity", None, None) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "edge object kind/columns mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
        match columns_to_edge_end("literal", Some(vec![0u8; 16]), Some("\"x\"".into())) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "edge object kind/columns mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
        // Happy path stays happy.
        let id = Uuid::new_v4();
        let blob = id.as_bytes().to_vec();
        match columns_to_edge_end("entity", Some(blob), None).unwrap() {
            EdgeEnd::Entity { id: got } => assert_eq!(got, id),
            other => panic!("expected Entity, got {:?}", other),
        }
    }

    // ====================================================================
    // ISS-033 Layer 2 — alias / update_entity_embedding / search_candidates
    // ====================================================================

    /// Helper: insert an entity with a specified last_seen and optional
    /// embedding into the test store. Returns the persisted entity.
    fn insert_test_entity(
        store: &mut SqliteGraphStore<'_>,
        name: &str,
        kind: EntityKind,
        last_seen: DateTime<Utc>,
        embedding: Option<Vec<f32>>,
    ) -> Entity {
        let mut e = Entity::new(name.into(), kind, last_seen);
        e.last_seen = last_seen;
        e.embedding = embedding;
        store.insert_entity(&e).expect("insert ok");
        e
    }

    // -------------------------- Alias upsert/resolve --------------------

    #[test]
    fn upsert_and_resolve_alias_basic() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "Mel", EntityKind::Person, now, None);

        store.upsert_alias("mel", "Mel", e.id, None).unwrap();
        let got = store.resolve_alias("Mel").unwrap();
        assert_eq!(got, Some(e.id));
    }

    #[test]
    fn resolve_alias_normalizes_caller_input() {
        // Writer wrote "café" (NFC), reader queries "Café  " (mixed case +
        // trailing whitespace) — must hit because both go through
        // normalize_alias. This is the symmetry property search_candidates
        // depends on.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "Café", EntityKind::Place, now, None);

        store.upsert_alias("café", "Café", e.id, None).unwrap();
        let got = store.resolve_alias("Café  ").unwrap();
        assert_eq!(got, Some(e.id));
    }

    #[test]
    fn upsert_alias_is_idempotent_on_repeat() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "Mel", EntityKind::Person, now, None);

        store.upsert_alias("mel", "Mel", e.id, None).unwrap();
        // Second call with a different raw form: should update `alias`
        // (raw surface) but not duplicate the row.
        store.upsert_alias("mel", "MEL", e.id, None).unwrap();

        let cnt: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM graph_entity_aliases WHERE normalized = 'mel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "idempotent upsert should not duplicate");

        let raw: String = store
            .conn
            .query_row(
                "SELECT alias FROM graph_entity_aliases WHERE normalized = 'mel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, "MEL", "raw surface form refreshed by latest upsert");
    }

    #[test]
    fn resolve_alias_namespace_isolated() {
        let mut conn = fresh_conn();
        // Default namespace store inserts a row.
        {
            let mut store = SqliteGraphStore::new(&mut conn);
            let now = Utc::now();
            let e = insert_test_entity(&mut store, "Mel", EntityKind::Person, now, None);
            store.upsert_alias("mel", "Mel", e.id, None).unwrap();
        }
        // A different-namespace store on the SAME connection must not see it.
        let store2 = SqliteGraphStore::new(&mut conn).with_namespace("other");
        assert_eq!(store2.resolve_alias("mel").unwrap(), None);
    }

    #[test]
    fn resolve_alias_missing_returns_none() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        assert_eq!(store.resolve_alias("nope").unwrap(), None);
    }

    // -------------------------- update_entity_embedding -----------------

    #[test]
    fn update_entity_embedding_writes_blob() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "X", EntityKind::Concept, now, None);

        let v = vec![0.1, -0.2, 0.3, 0.4];
        store.update_entity_embedding(e.id, Some(&v)).unwrap();

        let got = store.get_entity(e.id).unwrap().unwrap();
        assert_eq!(got.embedding, Some(v));
    }

    #[test]
    fn update_entity_embedding_clears_when_none() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let now = Utc::now();
        let e = insert_test_entity(
            &mut store, "X", EntityKind::Concept, now,
            Some(vec![0.1, 0.2, 0.3, 0.4]),
        );

        store.update_entity_embedding(e.id, None).unwrap();
        let got = store.get_entity(e.id).unwrap().unwrap();
        assert!(got.embedding.is_none());
    }

    #[test]
    fn update_entity_embedding_rejects_dim_mismatch() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "X", EntityKind::Concept, now, None);

        let wrong = vec![0.1, 0.2, 0.3]; // 3 != 4
        match store.update_entity_embedding(e.id, Some(&wrong)) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn update_entity_embedding_missing_entity_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let v = vec![0.1, 0.2, 0.3, 0.4];
        match store.update_entity_embedding(Uuid::new_v4(), Some(&v)) {
            Err(GraphError::EntityNotFound(_)) => {}
            other => panic!("expected EntityNotFound, got {:?}", other),
        }
    }

    // -------------------------- search_candidates ------------------------

    fn make_query(
        text: &str,
        emb: Option<Vec<f32>>,
        top_k: usize,
        now_dt: DateTime<Utc>,
    ) -> CandidateQuery {
        CandidateQuery {
            mention_text: text.into(),
            mention_embedding: emb,
            kind_filter: None,
            namespace: "default".into(),
            top_k,
            recency_window: None,
            now: dt_to_unix(now_dt),
        }
    }

    #[test]
    fn search_candidates_alias_only_hit() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let now = Utc::now();
        let e = insert_test_entity(&mut store, "Mel", EntityKind::Person, now, None);
        store.upsert_alias("mel", "Mel", e.id, None).unwrap();
        // A second entity with no alias and no embedding — should not appear.
        insert_test_entity(&mut store, "Bob", EntityKind::Person, now, None);

        let q = make_query("Mel", None, 10, now);
        let got = store.search_candidates(&q).unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].entity_id, e.id);
        assert!(got[0].alias_match, "alias path");
        assert!(got[0].embedding_score.is_none(), "no mention emb → None");
    }

    #[test]
    fn search_candidates_embedding_only_hit() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let _e1 = insert_test_entity(
            &mut store, "A", EntityKind::Concept, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let _e2 = insert_test_entity(
            &mut store, "B", EntityKind::Concept, now,
            Some(vec![0.0, 1.0, 0.0]),
        );
        // No aliases anywhere; pure embedding cohort.
        let q = make_query("anything", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();

        assert_eq!(got.len(), 2);
        for c in &got {
            assert!(!c.alias_match);
            assert!(c.embedding_score.is_some());
        }
        // The vec aligned with [1,0,0] must score 1.0 cosine; the orthogonal
        // one must score 0.0.
        let scores: Vec<(String, f32)> = got
            .iter()
            .map(|c| (c.canonical_name.clone(), c.embedding_score.unwrap()))
            .collect();
        let s_a = scores.iter().find(|(n, _)| n == "A").unwrap().1;
        let s_b = scores.iter().find(|(n, _)| n == "B").unwrap().1;
        assert!((s_a - 1.0).abs() < 1e-6, "cosine of (1,0,0) with itself = 1");
        assert!(s_b.abs() < 1e-6, "cosine of orthogonal = 0");
    }

    #[test]
    fn search_candidates_alias_and_embedding_combined() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let e = insert_test_entity(
            &mut store, "Mel", EntityKind::Person, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        store.upsert_alias("mel", "Mel", e.id, None).unwrap();

        let q = make_query("Mel", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();

        assert_eq!(got.len(), 1);
        let c = &got[0];
        assert!(c.alias_match, "alias path");
        assert!(c.embedding_score.is_some(), "embedding signal present");
        assert!((c.embedding_score.unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn search_candidates_namespace_isolated() {
        let mut conn = fresh_conn();
        // Insert in namespace "alpha".
        {
            let mut store = SqliteGraphStore::new(&mut conn)
                .with_namespace("alpha")
                .with_embedding_dim(3);
            let now = Utc::now();
            insert_test_entity(
                &mut store, "Alpha", EntityKind::Concept, now,
                Some(vec![1.0, 0.0, 0.0]),
            );
        }
        // Query in namespace "beta" — must not see the alpha entity.
        let store = SqliteGraphStore::new(&mut conn)
            .with_namespace("beta")
            .with_embedding_dim(3);
        let q = CandidateQuery {
            mention_text: "Alpha".into(),
            mention_embedding: Some(vec![1.0, 0.0, 0.0]),
            kind_filter: None,
            namespace: "beta".into(),
            top_k: 10,
            recency_window: None,
            now: dt_to_unix(Utc::now()),
        };
        let got = store.search_candidates(&q).unwrap();
        assert!(got.is_empty(), "namespace hard filter");
    }

    #[test]
    fn search_candidates_kind_filter() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let _person = insert_test_entity(
            &mut store, "Mel", EntityKind::Person, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let _concept = insert_test_entity(
            &mut store, "Mel", EntityKind::Concept, now,
            Some(vec![1.0, 0.0, 0.0]),
        );

        let mut q = make_query("Mel", Some(vec![1.0, 0.0, 0.0]), 10, now);
        q.kind_filter = Some(EntityKind::Person);
        let got = store.search_candidates(&q).unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, EntityKind::Person);
    }

    #[test]
    fn search_candidates_empty_table_returns_empty() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 10, Utc::now());
        let got = store.search_candidates(&q).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn search_candidates_skips_null_embedding_in_embedding_scan() {
        // An entity with no embedding must NOT appear in the embedding
        // cohort. It can still appear via the alias path.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let _has = insert_test_entity(
            &mut store, "WithEmb", EntityKind::Concept, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let _none = insert_test_entity(
            &mut store, "NoEmb", EntityKind::Concept, now, None,
        );

        let q = make_query("noalias", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();
        assert_eq!(got.len(), 1, "only the embedded entity appears");
        assert_eq!(got[0].canonical_name, "WithEmb");
    }

    #[test]
    fn search_candidates_top_k_truncates() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        for i in 0..5 {
            insert_test_entity(
                &mut store,
                &format!("E{}", i),
                EntityKind::Concept,
                now,
                Some(vec![1.0, 0.0, 0.0]),
            );
        }
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 3, now);
        let got = store.search_candidates(&q).unwrap();
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn search_candidates_max_top_k_ceiling_enforced() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        // Insert > MAX_TOP_K to verify the ceiling actually clamps.
        for i in 0..(MAX_TOP_K + 10) {
            insert_test_entity(
                &mut store,
                &format!("E{}", i),
                EntityKind::Concept,
                now,
                Some(vec![1.0, 0.0, 0.0]),
            );
        }
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 1000, now);
        let got = store.search_candidates(&q).unwrap();
        assert_eq!(got.len(), MAX_TOP_K, "MAX_TOP_K hard cap");
    }

    #[test]
    fn search_candidates_top_k_zero_returns_empty() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        insert_test_entity(
            &mut store, "E", EntityKind::Concept, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 0, now);
        let got = store.search_candidates(&q).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn search_candidates_recency_window_decay() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        // Two entities at different last_seen offsets.
        let recent = now;
        let old = now - chrono::Duration::seconds(100);
        let very_old = now - chrono::Duration::seconds(10_000);

        let e_recent = insert_test_entity(
            &mut store, "Recent", EntityKind::Concept, recent,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let e_old = insert_test_entity(
            &mut store, "Old", EntityKind::Concept, old,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let e_outside = insert_test_entity(
            &mut store, "Outside", EntityKind::Concept, very_old,
            Some(vec![1.0, 0.0, 0.0]),
        );

        let mut q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 50, now);
        q.recency_window = Some(std::time::Duration::from_secs(1000));
        let got = store.search_candidates(&q).unwrap();

        // Map by id for assertions.
        let by_id: std::collections::HashMap<Uuid, &CandidateMatch> =
            got.iter().map(|c| (c.entity_id, c)).collect();

        let recent_score = by_id[&e_recent.id].recency_score;
        let old_score = by_id[&e_old.id].recency_score;
        let outside_score = by_id[&e_outside.id].recency_score;

        assert!((recent_score - 1.0).abs() < 1e-3, "recent ≈ 1.0");
        // age=100, window=1000 → 1 - 0.1 = 0.9
        assert!((old_score - 0.9).abs() < 1e-2, "old ≈ 0.9, got {}", old_score);
        assert_eq!(outside_score, 0.0, "outside window clamps to 0");
    }

    #[test]
    fn search_candidates_unbounded_recency_uses_set_span() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let recent = now;
        let oldest = now - chrono::Duration::seconds(100);

        let e_recent = insert_test_entity(
            &mut store, "Recent", EntityKind::Concept, recent,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let e_old = insert_test_entity(
            &mut store, "Old", EntityKind::Concept, oldest,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();

        let by_id: std::collections::HashMap<Uuid, &CandidateMatch> =
            got.iter().map(|c| (c.entity_id, c)).collect();
        // Newest in set → 1.0, oldest in set → 0.0.
        assert!((by_id[&e_recent.id].recency_score - 1.0).abs() < 1e-6);
        assert!(by_id[&e_old.id].recency_score.abs() < 1e-6);
    }

    #[test]
    fn search_candidates_rejects_caller_dim_mismatch() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        // Caller supplies a 3-dim embedding to a store configured for 4-dim.
        let q = make_query("x", Some(vec![0.1, 0.2, 0.3]), 10, Utc::now());
        match store.search_candidates(&q) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "entity embedding dim mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn search_candidates_signal_distinction_none_vs_zero() {
        // The fusion module relies on the distinction between
        // embedding_score = None (signal missing) and Some(0.0) (zero
        // similarity). Verify both branches occur for the right inputs.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let e = insert_test_entity(
            &mut store, "Mel", EntityKind::Person, now,
            Some(vec![0.0, 1.0, 0.0]),
        );
        store.upsert_alias("mel", "Mel", e.id, None).unwrap();

        // Branch 1: caller has NO mention embedding → score = None even
        // though candidate has one.
        let q1 = make_query("Mel", None, 10, now);
        let got1 = store.search_candidates(&q1).unwrap();
        assert_eq!(got1.len(), 1);
        assert!(got1[0].embedding_score.is_none(), "missing-signal = None");

        // Branch 2: caller has an orthogonal embedding → score = Some(0.0).
        let q2 = make_query("Mel", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got2 = store.search_candidates(&q2).unwrap();
        assert_eq!(got2.len(), 1);
        match got2[0].embedding_score {
            Some(s) => assert!(s.abs() < 1e-6, "zero similarity = Some(0.0)"),
            None => panic!("present-signal-zero must be Some(0.0), not None"),
        }
    }

    #[test]
    fn search_candidates_deterministic_order_by_id() {
        // Contract: order is by entity_id ASC, not by any signal.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        let mut ids = vec![];
        for i in 0..5 {
            let e = insert_test_entity(
                &mut store,
                &format!("E{}", i),
                EntityKind::Concept,
                now,
                Some(vec![1.0, 0.0, 0.0]),
            );
            ids.push(e.id);
        }
        let q = make_query("x", Some(vec![1.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();
        let got_ids: Vec<Uuid> = got.iter().map(|c| c.entity_id).collect();
        let mut sorted = got_ids.clone();
        sorted.sort();
        assert_eq!(got_ids, sorted, "ascending entity_id order");
    }

    #[test]
    fn search_candidates_mention_embedding_zero_vector_is_safe() {
        // A pathological caller feeding an all-zero mention embedding must
        // not panic or div-by-zero; cosine should be 0.0 (no signal value).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now = Utc::now();
        insert_test_entity(
            &mut store, "E", EntityKind::Concept, now,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let q = make_query("x", Some(vec![0.0, 0.0, 0.0]), 10, now);
        let got = store.search_candidates(&q).unwrap();
        assert_eq!(got.len(), 1);
        // dot product = 0, b_norm > 0, a_norm = guarded to 1.0 → cosine = 0.
        assert_eq!(got[0].embedding_score, Some(0.0));
    }

    // ===================================================================
    // Memory ↔ Entity provenance (link_memory_to_entities,
    // entities_linked_to_memory, memories_mentioning_entity)
    // ===================================================================
    //
    // These tests cover the §4.2 contract:
    //   - within-batch dedup (max conf, latest-non-NULL span)
    //   - cross-batch upsert (max conf, COALESCE span, max recorded_at)
    //   - namespace isolation (read + write)
    //   - cross-namespace conflict → Invariant
    //   - confidence range validation
    //   - limit semantics (0 → empty, ordering)

    /// Insert a stub `memories` row so FKs from `graph_memory_entity_mentions`
    /// resolve. `init_graph_tables` provides the join table; `fresh_conn`
    /// already created the stub `memories` table.
    fn insert_stub_memory(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO memories (id, content) VALUES (?1, ?2)",
            rusqlite::params![id, format!("content-{}", id)],
        )
        .expect("insert stub memory");
    }

    fn ts(secs: f64) -> DateTime<Utc> {
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
            .expect("valid timestamp")
    }

    #[test]
    fn link_memory_to_entities_happy_path_inserts_all() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);
        let b = insert_test_entity(&mut store, "B", EntityKind::Person, now, None);
        let c = insert_test_entity(&mut store, "C", EntityKind::Person, now, None);

        store
            .link_memory_to_entities(
                "mem-1",
                &[
                    (a.id, 0.9, Some("A-mention".into())),
                    (b.id, 0.7, None),
                    (c.id, 1.0, Some("C-mention".into())),
                ],
                ts(1000.0),
            )
            .expect("link ok");

        let got = store.entities_linked_to_memory("mem-1").unwrap();
        assert_eq!(got.len(), 3);
        // Order: recorded_at ASC, entity_id ASC. All share recorded_at=1000,
        // so order is by entity_id ascending — sort our expected for the check.
        let mut want = vec![a.id, b.id, c.id];
        want.sort();
        assert_eq!(got, want, "namespace-scoped read returns all three");
    }

    #[test]
    fn link_memory_to_entities_within_batch_dedup_max_confidence_and_latest_span() {
        // Same entity appears 3x in one call: confidence ought to fold to the
        // max (0.9), span ought to be the latest non-NULL ("third", set after
        // a NULL erase attempt that must not erase).
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        store
            .link_memory_to_entities(
                "mem-1",
                &[
                    (a.id, 0.4, Some("first".into())),
                    (a.id, 0.9, None),                  // null does NOT erase span
                    (a.id, 0.6, Some("third".into())), // latest non-NULL wins
                ],
                ts(1000.0),
            )
            .expect("link ok");

        // Inspect the row directly — confidence + span are not exposed by the
        // current trait reader, so we use the conn escape hatch (test-only).
        let (conf, span): (f64, Option<String>) = store
            .conn
            .query_row(
                "SELECT confidence, mention_span
                 FROM graph_memory_entity_mentions
                 WHERE memory_id = ?1 AND entity_id = ?2",
                rusqlite::params!["mem-1", a.id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((conf - 0.9).abs() < 1e-9, "max(conf) = 0.9, got {}", conf);
        assert_eq!(span.as_deref(), Some("third"), "latest non-NULL span wins");
    }

    #[test]
    fn link_memory_to_entities_cross_batch_upsert_is_monotone() {
        // First batch sets conf=0.5, span="initial", recorded_at=1000.
        // Second batch with conf=0.3 must NOT lower the stored confidence.
        // Second batch with NULL span must NOT erase "initial".
        // recorded_at must advance to the larger value.
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        store
            .link_memory_to_entities(
                "mem-1",
                &[(a.id, 0.5, Some("initial".into()))],
                ts(1000.0),
            )
            .unwrap();
        store
            .link_memory_to_entities(
                "mem-1",
                &[(a.id, 0.3, None)],
                ts(2000.0),
            )
            .unwrap();

        let (conf, span, rec_at): (f64, Option<String>, f64) = store
            .conn
            .query_row(
                "SELECT confidence, mention_span, recorded_at
                 FROM graph_memory_entity_mentions
                 WHERE memory_id = ?1 AND entity_id = ?2",
                rusqlite::params!["mem-1", a.id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!((conf - 0.5).abs() < 1e-9, "max(0.5, 0.3) = 0.5");
        assert_eq!(span.as_deref(), Some("initial"), "NULL did not erase span");
        assert!((rec_at - 2000.0).abs() < 1e-9, "recorded_at = max = 2000");
    }

    #[test]
    fn link_memory_to_entities_cross_batch_advances_confidence() {
        // Mirror image of the monotone test: when the new confidence IS
        // higher, the stored row must move up.
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        store
            .link_memory_to_entities("mem-1", &[(a.id, 0.4, None)], ts(1000.0))
            .unwrap();
        store
            .link_memory_to_entities(
                "mem-1",
                &[(a.id, 0.95, Some("better".into()))],
                ts(900.0), // earlier timestamp — recorded_at must NOT regress
            )
            .unwrap();

        let (conf, span, rec_at): (f64, Option<String>, f64) = store
            .conn
            .query_row(
                "SELECT confidence, mention_span, recorded_at
                 FROM graph_memory_entity_mentions
                 WHERE memory_id = ?1 AND entity_id = ?2",
                rusqlite::params!["mem-1", a.id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!((conf - 0.95).abs() < 1e-9, "max(0.4, 0.95) = 0.95");
        assert_eq!(span.as_deref(), Some("better"), "non-NULL replaces NULL");
        assert!((rec_at - 1000.0).abs() < 1e-9, "recorded_at never regresses");
    }

    #[test]
    fn link_memory_to_entities_rejects_confidence_out_of_range() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        for bad in &[-0.1, 1.1, f64::NAN, f64::INFINITY] {
            let err = store
                .link_memory_to_entities("mem-1", &[(a.id, *bad, None)], ts(1.0))
                .expect_err("must reject");
            match err {
                GraphError::Invariant(msg) => {
                    assert!(msg.contains("confidence"), "msg: {}", msg);
                }
                other => panic!("expected Invariant for {}, got {:?}", bad, other),
            }
        }
        // Sanity: nothing was written for any of those calls.
        assert!(store.entities_linked_to_memory("mem-1").unwrap().is_empty());
    }

    #[test]
    fn link_memory_to_entities_atomic_on_failure() {
        // FK violation on the second pair must roll back the whole batch — the
        // first pair, even though its INSERT would have succeeded standalone,
        // must not be visible. (Atomicity is a design §4.2 requirement.)
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let real = insert_test_entity(&mut store, "Real", EntityKind::Person, now, None);
        let phantom = Uuid::new_v4(); // not inserted into graph_entities → FK fail

        let err = store.link_memory_to_entities(
            "mem-1",
            &[(real.id, 0.9, None), (phantom, 0.5, None)],
            ts(1000.0),
        );
        assert!(err.is_err(), "FK violation must surface");

        let got = store.entities_linked_to_memory("mem-1").unwrap();
        assert!(got.is_empty(), "rolled back: real was not committed either");
    }

    #[test]
    fn entities_linked_to_memory_namespace_scoped() {
        // Write through ns_a; read from ns_b → empty.
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let now = Utc::now();
        let a_id = {
            let mut store_a =
                SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let a =
                insert_test_entity(&mut store_a, "A", EntityKind::Person, now, None);
            store_a
                .link_memory_to_entities("mem-1", &[(a.id, 0.5, None)], ts(1.0))
                .unwrap();
            a.id
        };

        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        assert!(
            store_b.entities_linked_to_memory("mem-1").unwrap().is_empty(),
            "ns_b must not see ns_a's link"
        );
        // Confirm ns_a still sees it (sanity).
        let store_a2 = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
        assert_eq!(
            store_a2.entities_linked_to_memory("mem-1").unwrap(),
            vec![a_id]
        );
    }

    #[test]
    fn link_memory_to_entities_cross_namespace_returns_invariant() {
        // Pre-existing row in ns_a; ns_b tries to link the same (memory,
        // entity) — must fail with Invariant, must NOT update the ns_a row.
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let now = Utc::now();
        let a_id = {
            let mut store_a =
                SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let a =
                insert_test_entity(&mut store_a, "A", EntityKind::Person, now, None);
            store_a
                .link_memory_to_entities(
                    "mem-1",
                    &[(a.id, 0.5, Some("ns_a span".into()))],
                    ts(1000.0),
                )
                .unwrap();
            a.id
        };

        let mut store_b =
            SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        let err = store_b.link_memory_to_entities(
            "mem-1",
            &[(a_id, 0.99, Some("ns_b span".into()))],
            ts(2000.0),
        );
        match err {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("cross-namespace"), "msg: {}", msg);
            }
            other => panic!("expected Invariant, got {:?}", other),
        }

        // ns_a row must be unchanged.
        let (conf, span): (f64, Option<String>) = conn
            .query_row(
                "SELECT confidence, mention_span
                 FROM graph_memory_entity_mentions
                 WHERE memory_id = ?1 AND entity_id = ?2",
                rusqlite::params!["mem-1", a_id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((conf - 0.5).abs() < 1e-9, "ns_a confidence preserved");
        assert_eq!(span.as_deref(), Some("ns_a span"), "ns_a span preserved");
    }

    #[test]
    fn entities_linked_to_memory_orders_by_recorded_at_then_entity_id() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-1");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);
        let b = insert_test_entity(&mut store, "B", EntityKind::Person, now, None);
        let c = insert_test_entity(&mut store, "C", EntityKind::Person, now, None);

        // c at t=1, a at t=2, b at t=2 — expected order: c, then min(a,b),
        // then max(a,b) by uuid.
        store
            .link_memory_to_entities("mem-1", &[(c.id, 0.5, None)], ts(1.0))
            .unwrap();
        store
            .link_memory_to_entities(
                "mem-1",
                &[(a.id, 0.5, None), (b.id, 0.5, None)],
                ts(2.0),
            )
            .unwrap();

        let got = store.entities_linked_to_memory("mem-1").unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], c.id, "earliest recorded_at first");
        let (lo, hi) = if a.id < b.id { (a.id, b.id) } else { (b.id, a.id) };
        assert_eq!(got[1], lo);
        assert_eq!(got[2], hi);
    }

    #[test]
    fn memories_mentioning_entity_orders_recent_first_with_limit() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-old");
        insert_stub_memory(&conn, "mem-mid");
        insert_stub_memory(&conn, "mem-new");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        store
            .link_memory_to_entities("mem-old", &[(a.id, 0.5, None)], ts(100.0))
            .unwrap();
        store
            .link_memory_to_entities("mem-mid", &[(a.id, 0.5, None)], ts(200.0))
            .unwrap();
        store
            .link_memory_to_entities("mem-new", &[(a.id, 0.5, None)], ts(300.0))
            .unwrap();

        // Full read: DESC by recorded_at.
        let got = store.memories_mentioning_entity(a.id, 10).unwrap();
        assert_eq!(got, vec!["mem-new", "mem-mid", "mem-old"]);

        // Limit truncates to most-recent N.
        let got_top2 = store.memories_mentioning_entity(a.id, 2).unwrap();
        assert_eq!(got_top2, vec!["mem-new", "mem-mid"]);

        // limit == 0 returns empty (caller-bug guard, NOT an error).
        let got_zero = store.memories_mentioning_entity(a.id, 0).unwrap();
        assert!(got_zero.is_empty());
    }

    #[test]
    fn memories_mentioning_entity_namespace_scoped() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-a");
        insert_stub_memory(&conn, "mem-b");
        let now = Utc::now();
        let entity_id = {
            // Insert the entity once in ns_a; link mem-a in ns_a.
            let mut store_a =
                SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let a =
                insert_test_entity(&mut store_a, "A", EntityKind::Person, now, None);
            store_a
                .link_memory_to_entities("mem-a", &[(a.id, 0.5, None)], ts(1.0))
                .unwrap();
            a.id
        };
        {
            // Insert the same entity again in ns_b (separate namespace =
            // separate row in graph_entities), then link mem-b. The trait's
            // current PK on graph_entities is `id`, so writing the same UUID
            // into ns_b would actually conflict — instead, use a fresh entity
            // for ns_b. The important thing for *this* test is that the
            // mention from ns_a does not surface when reading from ns_b.
            let mut store_b =
                SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
            let b =
                insert_test_entity(&mut store_b, "B", EntityKind::Person, now, None);
            store_b
                .link_memory_to_entities("mem-b", &[(b.id, 0.5, None)], ts(2.0))
                .unwrap();
            // From ns_b, querying the ns_a entity must return nothing.
            assert!(
                store_b
                    .memories_mentioning_entity(entity_id, 10)
                    .unwrap()
                    .is_empty(),
                "ns_b must not see ns_a's mentions"
            );
        }
    }

    // ============================================================
    // Phase 2 Batch B — episode rollups, topics, pipeline, predicates,
    // failures.
    //
    // Test conventions follow Batch A:
    //   * one `fresh_conn` per test (in-memory)
    //   * `insert_stub_memory` / `insert_test_entity` for FK satisfaction
    //   * timestamps via `ts(secs)` for deterministic ordering
    // ============================================================

    fn set_memory_episode(conn: &Connection, memory_id: &str, episode: Uuid) {
        // Helper: tag a stub `memories` row with an episode_id so the
        // `entities_in_episode` / `mentions_of_entity` joins resolve.
        conn.execute(
            "UPDATE memories SET episode_id = ?1 WHERE id = ?2",
            rusqlite::params![episode.as_bytes().to_vec(), memory_id],
        )
        .expect("set memory episode");
    }

    // ---- entities_in_episode --------------------------------------

    #[test]
    fn entities_in_episode_aggregates_distinct_across_memories() {
        let mut conn = fresh_conn();
        let ep = Uuid::new_v4();
        insert_stub_memory(&conn, "mem-1");
        insert_stub_memory(&conn, "mem-2");
        insert_stub_memory(&conn, "mem-3");
        set_memory_episode(&conn, "mem-1", ep);
        set_memory_episode(&conn, "mem-2", ep);
        // mem-3 belongs to a different episode and must NOT contribute.
        let other_ep = Uuid::new_v4();
        set_memory_episode(&conn, "mem-3", other_ep);

        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);
        let b = insert_test_entity(&mut store, "B", EntityKind::Person, now, None);
        let c = insert_test_entity(&mut store, "C", EntityKind::Person, now, None);

        // a + b mentioned in mem-1 (in-episode)
        store
            .link_memory_to_entities(
                "mem-1",
                &[(a.id, 1.0, None), (b.id, 1.0, None)],
                ts(1.0),
            )
            .unwrap();
        // b again in mem-2 (in-episode) — distinct rollup must dedup
        store
            .link_memory_to_entities("mem-2", &[(b.id, 1.0, None)], ts(2.0))
            .unwrap();
        // c only in mem-3 (other episode) — must NOT appear
        store
            .link_memory_to_entities("mem-3", &[(c.id, 1.0, None)], ts(3.0))
            .unwrap();

        let got = store.entities_in_episode(ep).unwrap();
        let mut want = vec![a.id, b.id];
        want.sort();
        assert_eq!(got, want, "rollup must dedup b across mem-1 and mem-2");
        assert!(!got.contains(&c.id));
    }

    #[test]
    fn entities_in_episode_namespace_scoped() {
        let mut conn = fresh_conn();
        let ep = Uuid::new_v4();
        insert_stub_memory(&conn, "mem-shared");
        set_memory_episode(&conn, "mem-shared", ep);
        let now = Utc::now();
        let a_id;
        {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let a =
                insert_test_entity(&mut store_a, "A", EntityKind::Person, now, None);
            a_id = a.id;
            store_a
                .link_memory_to_entities("mem-shared", &[(a.id, 1.0, None)], ts(1.0))
                .unwrap();
        }
        // ns_b doesn't see ns_a's mentions.
        {
            let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
            let got = store_b.entities_in_episode(ep).unwrap();
            assert!(got.is_empty());
        }
        // ns_a does.
        let store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
        assert_eq!(store_a.entities_in_episode(ep).unwrap(), vec![a_id]);
    }

    #[test]
    fn entities_in_episode_unknown_episode_returns_empty() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let got = store.entities_in_episode(Uuid::new_v4()).unwrap();
        assert!(got.is_empty());
    }

    // ---- mentions_of_entity --------------------------------------

    #[test]
    fn mentions_of_entity_aggregates_episodes_and_memories() {
        let mut conn = fresh_conn();
        let ep1 = Uuid::new_v4();
        let ep2 = Uuid::new_v4();
        insert_stub_memory(&conn, "mem-1");
        insert_stub_memory(&conn, "mem-2");
        insert_stub_memory(&conn, "mem-3");
        set_memory_episode(&conn, "mem-1", ep1);
        set_memory_episode(&conn, "mem-2", ep1); // same episode as mem-1
        set_memory_episode(&conn, "mem-3", ep2);

        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);

        store
            .link_memory_to_entities("mem-1", &[(a.id, 1.0, None)], ts(1.0))
            .unwrap();
        store
            .link_memory_to_entities("mem-2", &[(a.id, 1.0, None)], ts(2.0))
            .unwrap();
        store
            .link_memory_to_entities("mem-3", &[(a.id, 1.0, None)], ts(3.0))
            .unwrap();

        let m = store.mentions_of_entity(a.id).unwrap();
        assert_eq!(m.memory_ids, vec!["mem-1", "mem-2", "mem-3"]);
        // Episodes: ep1 first (mem-1 at ts=1), then ep2 (mem-3 at ts=3).
        // ep1 dedup'd (mem-2 also belongs to ep1).
        assert_eq!(m.episode_ids, vec![ep1, ep2]);
    }

    #[test]
    fn mentions_of_entity_skips_memories_with_null_episode() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-no-ep");
        // No `set_memory_episode` call → episode_id stays NULL.
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);
        store
            .link_memory_to_entities("mem-no-ep", &[(a.id, 1.0, None)], ts(1.0))
            .unwrap();
        let m = store.mentions_of_entity(a.id).unwrap();
        assert_eq!(m.memory_ids, vec!["mem-no-ep"]);
        assert!(
            m.episode_ids.is_empty(),
            "memory with NULL episode_id must not contribute"
        );
    }

    #[test]
    fn mentions_of_entity_orders_by_recorded_at_ascending() {
        let mut conn = fresh_conn();
        insert_stub_memory(&conn, "mem-late");
        insert_stub_memory(&conn, "mem-early");
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let a = insert_test_entity(&mut store, "A", EntityKind::Person, now, None);
        // Insert "late" first by SQL order, with a later recorded_at, then
        // "early" with an earlier recorded_at — output must be early-first.
        store
            .link_memory_to_entities("mem-late", &[(a.id, 1.0, None)], ts(10.0))
            .unwrap();
        store
            .link_memory_to_entities("mem-early", &[(a.id, 1.0, None)], ts(1.0))
            .unwrap();
        let m = store.mentions_of_entity(a.id).unwrap();
        assert_eq!(m.memory_ids, vec!["mem-early", "mem-late"]);
    }

    // ---- topic CRUD ----------------------------------------------

    fn fresh_topic(ns: &str) -> KnowledgeTopic {
        KnowledgeTopic::new(
            Uuid::new_v4(),
            "Title".into(),
            "Summary".into(),
            ns.into(),
            100.0,
        )
    }

    fn insert_mirror_entity(store: &mut SqliteGraphStore<'_>, id: Uuid) {
        // Topic rows reference graph_entities(id) via FK. Insert a Topic
        // entity with the same id to satisfy.
        let now = Utc::now();
        let mut e = Entity::new("mirror".into(), EntityKind::Topic, now);
        e.id = id;
        store.insert_entity(&e).expect("insert mirror");
    }

    #[test]
    fn upsert_topic_roundtrips_basic_fields() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let mut t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        t.source_memories = vec!["m1".into(), "m2".into()];
        t.contributing_entities = vec![Uuid::new_v4(), Uuid::new_v4()];
        t.cluster_weights = Some(json!({"a": 1, "b": 0.5}));
        store.upsert_topic(&t).unwrap();

        let got = store.get_topic(t.topic_id).unwrap().expect("present");
        assert_eq!(got.topic_id, t.topic_id);
        assert_eq!(got.title, t.title);
        assert_eq!(got.summary, t.summary);
        assert_eq!(got.source_memories, t.source_memories);
        assert_eq!(got.contributing_entities, t.contributing_entities);
        assert_eq!(got.cluster_weights, t.cluster_weights);
        assert_eq!(got.namespace, t.namespace);
        assert!(got.embedding.is_none());
        assert!(got.is_live());
    }

    #[test]
    fn upsert_topic_is_idempotent_and_updates_mutable_fields() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let mut t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        store.upsert_topic(&t).unwrap();

        // Repeat with a new title and summary; topic_id stable.
        t.title = "Updated".into();
        t.summary = "v2".into();
        t.synthesized_at = 200.0;
        store.upsert_topic(&t).unwrap();
        let got = store.get_topic(t.topic_id).unwrap().expect("present");
        assert_eq!(got.title, "Updated");
        assert_eq!(got.summary, "v2");
        assert_eq!(got.synthesized_at, 200.0);
    }

    #[test]
    fn upsert_topic_rejects_cross_namespace_clobber() {
        let mut conn = fresh_conn();
        // ns_a creates a topic
        let topic_id = Uuid::new_v4();
        {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            insert_mirror_entity(&mut store_a, topic_id);
            let mut t = fresh_topic("ns_a");
            t.topic_id = topic_id;
            store_a.upsert_topic(&t).unwrap();
        }
        // ns_b tries to upsert the same topic_id with a different namespace
        let mut store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        let mut t_b = fresh_topic("ns_b");
        t_b.topic_id = topic_id;
        match store_b.upsert_topic(&t_b) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("cross-namespace"));
            }
            other => panic!("expected cross-namespace invariant, got {:?}", other),
        }
    }

    #[test]
    fn upsert_topic_rejects_embedding_dim_mismatch() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(8);
        let mut t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        t.embedding = Some(vec![0.0; 16]); // wrong dim
        match store.upsert_topic(&t) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "knowledge topic embedding dim mismatch");
            }
            other => panic!("expected dim mismatch, got {:?}", other),
        }
    }

    #[test]
    fn upsert_topic_persists_embedding_blob() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let mut t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        t.embedding = Some(vec![1.0, -2.0, 3.0, -4.0]);
        store.upsert_topic(&t).unwrap();
        let got = store.get_topic(t.topic_id).unwrap().unwrap();
        assert_eq!(got.embedding, t.embedding);
    }

    #[test]
    fn get_topic_missing_returns_none() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        assert!(store.get_topic(Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn get_topic_namespace_scoped() {
        let mut conn = fresh_conn();
        let topic_id = Uuid::new_v4();
        {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            insert_mirror_entity(&mut store_a, topic_id);
            let mut t = fresh_topic("ns_a");
            t.topic_id = topic_id;
            store_a.upsert_topic(&t).unwrap();
        }
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
        assert!(store_b.get_topic(topic_id).unwrap().is_none());
        let store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
        assert!(store_a.get_topic(topic_id).unwrap().is_some());
    }

    #[test]
    fn list_topics_orders_by_synthesized_at_desc_and_filters_superseded() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        // Three topics with distinct synthesized_at; one will be superseded.
        let mut t1 = fresh_topic("default");
        t1.synthesized_at = 100.0;
        let mut t2 = fresh_topic("default");
        t2.synthesized_at = 200.0;
        let mut t3 = fresh_topic("default");
        t3.synthesized_at = 300.0;
        for t in [&t1, &t2, &t3] {
            insert_mirror_entity(&mut store, t.topic_id);
            store.upsert_topic(t).unwrap();
        }
        // Supersede t2 → t3.
        store.supersede_topic(t2.topic_id, t3.topic_id, ts(400.0)).unwrap();

        // include_superseded=false: only t1, t3 (t2 filtered out).
        let live = store.list_topics("default", false, 10).unwrap();
        let live_ids: Vec<Uuid> = live.iter().map(|t| t.topic_id).collect();
        assert_eq!(live_ids, vec![t3.topic_id, t1.topic_id]);

        // include_superseded=true: all three, t3 first.
        let all = store.list_topics("default", true, 10).unwrap();
        let all_ids: Vec<Uuid> = all.iter().map(|t| t.topic_id).collect();
        assert_eq!(all_ids, vec![t3.topic_id, t2.topic_id, t1.topic_id]);
    }

    #[test]
    fn list_topics_zero_limit_returns_empty() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        store.upsert_topic(&t).unwrap();
        let got = store.list_topics("default", true, 0).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn list_topics_namespace_filter() {
        let mut conn = fresh_conn();
        // Same physical conn, two namespaces. Topics from ns_a must not
        // appear in `list_topics("ns_b", ...)`.
        {
            let mut store_a = SqliteGraphStore::new(&mut conn).with_namespace("ns_a");
            let mut t = fresh_topic("ns_a");
            insert_mirror_entity(&mut store_a, t.topic_id);
            t.synthesized_at = 100.0;
            store_a.upsert_topic(&t).unwrap();
        }
        {
            let mut store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b");
            let mut t = fresh_topic("ns_b");
            insert_mirror_entity(&mut store_b, t.topic_id);
            t.synthesized_at = 200.0;
            store_b.upsert_topic(&t).unwrap();
        }
        let store = SqliteGraphStore::new(&mut conn);
        // Cross-cutting: caller chooses namespace.
        let a_topics = store.list_topics("ns_a", true, 10).unwrap();
        let b_topics = store.list_topics("ns_b", true, 10).unwrap();
        assert_eq!(a_topics.len(), 1);
        assert_eq!(b_topics.len(), 1);
        assert_eq!(a_topics[0].namespace, "ns_a");
        assert_eq!(b_topics[0].namespace, "ns_b");
    }

    #[test]
    fn supersede_topic_idempotent_for_same_successor() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t1 = fresh_topic("default");
        let t2 = fresh_topic("default");
        insert_mirror_entity(&mut store, t1.topic_id);
        insert_mirror_entity(&mut store, t2.topic_id);
        store.upsert_topic(&t1).unwrap();
        store.upsert_topic(&t2).unwrap();
        store.supersede_topic(t1.topic_id, t2.topic_id, ts(10.0)).unwrap();
        // Second call w/ same successor: no-op, no error.
        store.supersede_topic(t1.topic_id, t2.topic_id, ts(20.0)).unwrap();
        let got = store.get_topic(t1.topic_id).unwrap().unwrap();
        assert_eq!(got.superseded_by, Some(t2.topic_id));
    }

    #[test]
    fn supersede_topic_rejects_double_supersede_with_different_successor() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t1 = fresh_topic("default");
        let t2 = fresh_topic("default");
        let t3 = fresh_topic("default");
        for t in [&t1, &t2, &t3] {
            insert_mirror_entity(&mut store, t.topic_id);
            store.upsert_topic(t).unwrap();
        }
        store.supersede_topic(t1.topic_id, t2.topic_id, ts(10.0)).unwrap();
        match store.supersede_topic(t1.topic_id, t3.topic_id, ts(20.0)) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "topic already superseded");
            }
            other => panic!("expected double-supersede invariant, got {:?}", other),
        }
    }

    #[test]
    fn supersede_topic_missing_old_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        store.upsert_topic(&t).unwrap();
        match store.supersede_topic(Uuid::new_v4(), t.topic_id, ts(1.0)) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("old topic not found"));
            }
            other => panic!("expected not-found invariant, got {:?}", other),
        }
    }

    #[test]
    fn supersede_topic_missing_successor_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t = fresh_topic("default");
        insert_mirror_entity(&mut store, t.topic_id);
        store.upsert_topic(&t).unwrap();
        match store.supersede_topic(t.topic_id, Uuid::new_v4(), ts(1.0)) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("successor topic not found"));
            }
            other => panic!("expected successor-not-found invariant, got {:?}", other),
        }
    }

    // ---- pipeline runs --------------------------------------------

    #[test]
    fn begin_pipeline_run_inserts_running_row() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = store
            .begin_pipeline_run(
                PipelineKind::Resolution,
                json!({"episodes": 1}),
            )
            .unwrap();
        let (kind, status, finished, ns): (String, String, Option<f64>, String) = store
            .conn()
            .query_row(
                "SELECT kind, status, finished_at, namespace
                 FROM graph_pipeline_runs WHERE run_id = ?1",
                rusqlite::params![id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(kind, "resolution");
        assert_eq!(status, "running");
        assert!(finished.is_none());
        assert_eq!(ns, "default");
    }

    #[test]
    fn finish_pipeline_run_succeeds_then_rejects_double_finish() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = store
            .begin_pipeline_run(PipelineKind::Resolution, Json::Null)
            .unwrap();
        store
            .finish_pipeline_run(id, RunStatus::Succeeded, Some(json!({"n": 5})), None)
            .unwrap();
        // Re-finish must fail.
        match store.finish_pipeline_run(id, RunStatus::Failed, None, Some("retry")) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("terminal"));
            }
            other => panic!("expected double-finish invariant, got {:?}", other),
        }
    }

    #[test]
    fn finish_pipeline_run_rejects_running_status() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = store
            .begin_pipeline_run(PipelineKind::Resolution, Json::Null)
            .unwrap();
        match store.finish_pipeline_run(id, RunStatus::Running, None, None) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("Running"));
            }
            other => panic!("expected reject-Running invariant, got {:?}", other),
        }
    }

    #[test]
    fn finish_pipeline_run_unknown_run_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        match store.finish_pipeline_run(
            Uuid::new_v4(),
            RunStatus::Succeeded,
            None,
            None,
        ) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("not found"));
            }
            other => panic!("expected not-found invariant, got {:?}", other),
        }
    }

    // ---- §3.1 / §6.3 — memory-scoped runs + extraction_status -----

    /// Insert a row into the test `memories` table so FK references on
    /// `graph_pipeline_runs.memory_id` resolve. Returns the id used.
    fn insert_test_memory(store: &mut SqliteGraphStore<'_>, id: &str) {
        store
            .conn()
            .execute(
                "INSERT INTO memories (id, content) VALUES (?1, ?2)",
                rusqlite::params![id, "test content"],
            )
            .unwrap();
    }

    #[test]
    fn begin_pipeline_run_for_memory_writes_indexed_columns() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        insert_test_memory(&mut store, "mem-1");
        let episode_id = Uuid::new_v4();
        let run_id = store
            .begin_pipeline_run_for_memory(
                PipelineKind::Resolution,
                "mem-1",
                episode_id,
                json!({"episodes": 1}),
            )
            .unwrap();
        // Verify memory_id and episode_id were persisted into their
        // dedicated columns (not just buried in input_summary JSON).
        let (memory_id, episode_blob): (Option<String>, Option<Vec<u8>>) = store
            .conn()
            .query_row(
                "SELECT memory_id, episode_id FROM graph_pipeline_runs WHERE run_id = ?1",
                rusqlite::params![run_id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(memory_id.as_deref(), Some("mem-1"));
        assert_eq!(
            episode_blob.unwrap(),
            episode_id.as_bytes().to_vec(),
        );
    }

    #[test]
    fn begin_pipeline_run_for_memory_rejects_knowledge_compile() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        insert_test_memory(&mut store, "mem-1");
        // KnowledgeCompile is not memory-scoped — this method must reject it
        // so we don't index a non-existent semantic relationship.
        match store.begin_pipeline_run_for_memory(
            PipelineKind::KnowledgeCompile,
            "mem-1",
            Uuid::new_v4(),
            Json::Null,
        ) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("not memory-scoped"));
            }
            other => panic!("expected invariant rejection, got {:?}", other),
        }
    }

    #[test]
    fn begin_pipeline_run_does_not_set_memory_columns() {
        // The non-memory variant must leave memory_id / episode_id NULL —
        // otherwise `latest_pipeline_run_for_memory` would surface
        // KnowledgeCompile rows when callers query for resolution status.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let run_id = store
            .begin_pipeline_run(PipelineKind::KnowledgeCompile, Json::Null)
            .unwrap();
        let (memory_id, episode_id): (Option<String>, Option<Vec<u8>>) = store
            .conn()
            .query_row(
                "SELECT memory_id, episode_id FROM graph_pipeline_runs WHERE run_id = ?1",
                rusqlite::params![run_id.as_bytes().to_vec()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(memory_id.is_none(), "memory_id must be NULL");
        assert!(episode_id.is_none(), "episode_id must be NULL");
    }

    #[test]
    fn latest_pipeline_run_for_memory_returns_none_for_unknown() {
        let conn = fresh_conn();
        let mut owned = conn;
        let store = SqliteGraphStore::new(&mut owned);
        let got = store.latest_pipeline_run_for_memory("never-seen").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn latest_pipeline_run_for_memory_returns_running_row() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        insert_test_memory(&mut store, "mem-A");
        let episode_id = Uuid::new_v4();
        let run_id = store
            .begin_pipeline_run_for_memory(
                PipelineKind::Resolution,
                "mem-A",
                episode_id,
                Json::Null,
            )
            .unwrap();
        let row = store
            .latest_pipeline_run_for_memory("mem-A")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.run_id, run_id);
        assert_eq!(row.kind, PipelineKind::Resolution);
        assert_eq!(row.status, RunStatus::Running);
        assert!(row.finished_at.is_none());
        assert_eq!(row.memory_id.as_deref(), Some("mem-A"));
        assert_eq!(row.episode_id, Some(episode_id));
        assert!(row.error_detail.is_none());
    }

    #[test]
    fn latest_pipeline_run_for_memory_picks_latest_by_started_at() {
        // Two runs for the same memory: the index ORDER BY started_at DESC
        // must surface the most-recent run (post-reextract scenario).
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        insert_test_memory(&mut store, "mem-B");
        let first = store
            .begin_pipeline_run_for_memory(
                PipelineKind::Resolution,
                "mem-B",
                Uuid::new_v4(),
                Json::Null,
            )
            .unwrap();
        store
            .finish_pipeline_run(first, RunStatus::Succeeded, None, None)
            .unwrap();
        // Sleep an instant so the second `started_at` is strictly greater.
        // SQLite REAL has microsecond resolution from `Utc::now()`; the
        // monotonic gap on macOS is well under 1ms but the kernel-clock
        // call below makes the ordering deterministic.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = store
            .begin_pipeline_run_for_memory(
                PipelineKind::Reextract,
                "mem-B",
                Uuid::new_v4(),
                Json::Null,
            )
            .unwrap();
        let row = store
            .latest_pipeline_run_for_memory("mem-B")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.run_id, second);
        assert_eq!(row.kind, PipelineKind::Reextract);
    }

    #[test]
    fn latest_pipeline_run_for_memory_decodes_failed_status() {
        // Failed runs carry an `error_detail` string. Verify the decoder
        // round-trips it intact — `extraction_status` callers parse it
        // back into a `StageFailure` JSON.
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        insert_test_memory(&mut store, "mem-C");
        let run_id = store
            .begin_pipeline_run_for_memory(
                PipelineKind::Resolution,
                "mem-C",
                Uuid::new_v4(),
                Json::Null,
            )
            .unwrap();
        let detail = r#"{"stage":"edge_extract","kind":"llm_timeout"}"#;
        store
            .finish_pipeline_run(run_id, RunStatus::Failed, None, Some(detail))
            .unwrap();
        let row = store
            .latest_pipeline_run_for_memory("mem-C")
            .unwrap()
            .expect("row exists");
        assert_eq!(row.status, RunStatus::Failed);
        assert_eq!(row.error_detail.as_deref(), Some(detail));
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn latest_pipeline_run_for_memory_ignores_other_namespaces() {
        // The store's namespace acts as a hard filter: a run written under
        // namespace "default" is invisible to a store scoped to "alt".
        // Without this guarantee, `extraction_status` could leak across
        // tenant boundaries (master §1 / NG1).
        let mut conn = fresh_conn();
        // Both stores share the connection but disagree on namespace.
        {
            let mut store_a = SqliteGraphStore::new(&mut conn);
            insert_test_memory(&mut store_a, "mem-D");
            store_a
                .begin_pipeline_run_for_memory(
                    PipelineKind::Resolution,
                    "mem-D",
                    Uuid::new_v4(),
                    Json::Null,
                )
                .unwrap();
        }
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("alt");
        let got = store_b.latest_pipeline_run_for_memory("mem-D").unwrap();
        assert!(got.is_none(), "alt namespace must not see default's runs");
    }

    // ---- resolution traces ----------------------------------------

    #[test]
    fn record_resolution_trace_persists_and_is_idempotent() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        // FK target: entity_id references graph_entities(id). Insert a real
        // entity so the trace's entity_id resolves.
        let ent = insert_test_entity(&mut store, "X", EntityKind::Person, now, None);
        let run_id = store
            .begin_pipeline_run(PipelineKind::Resolution, Json::Null)
            .unwrap();
        let trace_id = Uuid::new_v4();
        let t = ResolutionTrace {
            trace_id,
            run_id,
            edge_id: None,
            entity_id: Some(ent.id),
            stage: crate::graph::audit::STAGE_DEDUP.to_string(),
            decision: crate::graph::audit::DECISION_MATCHED_EXISTING.to_string(),
            reason: Some("alias hit".into()),
            candidates: Some(json!([{"id": "x", "score": 0.9}])),
            recorded_at: 1.0,
        };
        store.record_resolution_trace(&t).unwrap();
        // Replay must not error nor duplicate.
        store.record_resolution_trace(&t).unwrap();
        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM graph_resolution_traces WHERE trace_id = ?1",
                rusqlite::params![trace_id.as_bytes().to_vec()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn record_resolution_trace_rejects_both_targets_null() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let run_id = store
            .begin_pipeline_run(PipelineKind::Resolution, Json::Null)
            .unwrap();
        let t = ResolutionTrace {
            trace_id: Uuid::new_v4(),
            run_id,
            edge_id: None,
            entity_id: None,
            stage: crate::graph::audit::STAGE_DEDUP.to_string(),
            decision: crate::graph::audit::DECISION_NEW.to_string(),
            reason: None,
            candidates: None,
            recorded_at: 1.0,
        };
        match store.record_resolution_trace(&t) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("at least one"));
            }
            other => panic!("expected null-target invariant, got {:?}", other),
        }
    }

    // ---- predicates -----------------------------------------------

    #[test]
    fn record_predicate_use_inserts_then_increments() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let p = Predicate::Canonical(CanonicalPredicate::WorksAt);
        store.record_predicate_use(&p, "works_at", ts(1.0)).unwrap();
        store.record_predicate_use(&p, "works_at", ts(2.0)).unwrap();
        store.record_predicate_use(&p, "works_at", ts(3.0)).unwrap();
        let (count, first, last): (i64, f64, f64) = store
            .conn()
            .query_row(
                "SELECT usage_count, first_seen, last_seen FROM graph_predicates
                 WHERE kind = 'canonical' AND label = 'works_at'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(first, 1.0);
        assert_eq!(last, 3.0);
    }

    #[test]
    fn list_proposed_predicates_filters_by_min_usage_and_kind() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        // canonical (must NOT show up regardless of usage)
        let canon = Predicate::Canonical(CanonicalPredicate::DependsOn);
        for _ in 0..10 {
            store.record_predicate_use(&canon, "depends_on", ts(1.0)).unwrap();
        }
        // proposed: "advises" ×3, "knows" ×1
        let advises = Predicate::proposed("advises");
        let knows = Predicate::proposed("knows");
        for _ in 0..3 {
            store.record_predicate_use(&advises, "advises", ts(2.0)).unwrap();
        }
        store.record_predicate_use(&knows, "knows", ts(3.0)).unwrap();

        let got = store.list_proposed_predicates(2).unwrap();
        // Only "advises" passes min_usage=2; canonical never appears.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "advises");
        assert_eq!(got[0].usage_count, 3);
    }

    // ---- extraction failures --------------------------------------

    fn fresh_failure(stage: &str, category: &str, episode: Uuid, secs: f64) -> ExtractionFailure {
        ExtractionFailure {
            id: Uuid::new_v4(),
            episode_id: episode,
            stage: stage.to_string(),
            error_category: category.to_string(),
            error_detail: Some("boom".into()),
            occurred_at: secs,
            resolved_at: None,
        }
    }

    #[test]
    fn record_extraction_failure_persists_and_is_idempotent() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let ep = Uuid::new_v4();
        let f = fresh_failure(
            crate::graph::audit::STAGE_ENTITY_EXTRACT,
            crate::graph::audit::CATEGORY_LLM_TIMEOUT,
            ep,
            1.0,
        );
        store.record_extraction_failure(&f).unwrap();
        // Idempotent replay.
        store.record_extraction_failure(&f).unwrap();
        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM graph_extraction_failures WHERE id = ?1",
                rusqlite::params![f.id.as_bytes().to_vec()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn record_extraction_failure_rejects_unknown_stage() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let f = fresh_failure(
            "not_a_real_stage",
            crate::graph::audit::CATEGORY_LLM_TIMEOUT,
            Uuid::new_v4(),
            1.0,
        );
        match store.record_extraction_failure(&f) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("stage"));
            }
            other => panic!("expected unknown-stage invariant, got {:?}", other),
        }
    }

    #[test]
    fn record_extraction_failure_rejects_unknown_category() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let f = fresh_failure(
            crate::graph::audit::STAGE_ENTITY_EXTRACT,
            "not_a_real_category",
            Uuid::new_v4(),
            1.0,
        );
        match store.record_extraction_failure(&f) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("error_category"));
            }
            other => panic!("expected unknown-category invariant, got {:?}", other),
        }
    }

    #[test]
    fn list_failed_episodes_dedups_and_filters_unresolved() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let ep1 = Uuid::new_v4();
        let ep2 = Uuid::new_v4();
        // Two failures on ep1 (one resolved), one on ep2 (unresolved).
        let mut f1a = fresh_failure(
            crate::graph::audit::STAGE_ENTITY_EXTRACT,
            crate::graph::audit::CATEGORY_LLM_TIMEOUT,
            ep1,
            1.0,
        );
        f1a.resolved_at = Some(50.0);
        let f1b = fresh_failure(
            crate::graph::audit::STAGE_PERSIST,
            crate::graph::audit::CATEGORY_DB_ERROR,
            ep1,
            2.0,
        );
        let f2 = fresh_failure(
            crate::graph::audit::STAGE_DEDUP,
            crate::graph::audit::CATEGORY_INTERNAL,
            ep2,
            3.0,
        );
        store.record_extraction_failure(&f1a).unwrap();
        store.record_extraction_failure(&f1b).unwrap();
        store.record_extraction_failure(&f2).unwrap();

        // unresolved_only=true: ep1 (still has unresolved f1b) + ep2.
        let mut got = store.list_failed_episodes(true).unwrap();
        got.sort();
        let mut want = vec![ep1, ep2];
        want.sort();
        assert_eq!(got, want);

        // Resolve f1b too; now only ep2 remains unresolved.
        store.mark_failure_resolved(f1b.id, ts(100.0)).unwrap();
        let unresolved = store.list_failed_episodes(true).unwrap();
        assert_eq!(unresolved, vec![ep2]);

        // unresolved_only=false: both episodes always.
        let mut all = store.list_failed_episodes(false).unwrap();
        all.sort();
        assert_eq!(all, want);
    }

    #[test]
    fn mark_failure_resolved_is_monotone_and_idempotent() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let ep = Uuid::new_v4();
        let f = fresh_failure(
            crate::graph::audit::STAGE_PERSIST,
            crate::graph::audit::CATEGORY_DB_ERROR,
            ep,
            1.0,
        );
        store.record_extraction_failure(&f).unwrap();
        store.mark_failure_resolved(f.id, ts(50.0)).unwrap();
        // Second call: idempotent, no error, no overwrite of the resolved_at.
        store.mark_failure_resolved(f.id, ts(99.0)).unwrap();
        let resolved_at: Option<f64> = store
            .conn()
            .query_row(
                "SELECT resolved_at FROM graph_extraction_failures WHERE id = ?1",
                rusqlite::params![f.id.as_bytes().to_vec()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(resolved_at, Some(50.0), "resolved_at must be monotone");
    }

    #[test]
    fn mark_failure_resolved_unknown_failure_errors() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        match store.mark_failure_resolved(Uuid::new_v4(), ts(1.0)) {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("not found"));
            }
            other => panic!("expected not-found invariant, got {:?}", other),
        }
    }

    // ============================================================
    // Phase 3 implementations: supersede_edge / edges_as_of / traverse
    // ============================================================

    #[test]
    fn supersede_edge_inserts_successor_and_invalidates_prior() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj1 = insert_subject_entity(&mut store, "Acme");
        let obj2 = insert_subject_entity(&mut store, "Initech");

        let prior = make_edge(subj, obj1, t0);
        store.insert_edge(&prior).unwrap();

        // Successor points at obj2 (different employer); supersedes = prior.id.
        let mut succ = make_edge(subj, obj2, t0 + chrono::Duration::seconds(10));
        succ.supersedes = Some(prior.id);

        let invalidate_at = t0 + chrono::Duration::seconds(20);
        store.supersede_edge(prior.id, &succ, invalidate_at).unwrap();

        // Successor row exists.
        let got_succ = store.get_edge(succ.id).unwrap().expect("successor present");
        assert_eq!(got_succ.id, succ.id);
        assert_eq!(got_succ.supersedes, Some(prior.id));

        // Prior is invalidated and points at successor.
        let got_prior = store.get_edge(prior.id).unwrap().expect("prior present");
        assert!(got_prior.invalidated_at.is_some());
        assert_eq!(got_prior.invalidated_by, Some(succ.id));
    }

    #[test]
    fn supersede_edge_rolls_back_on_invariant_violation() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let other_succ_id = Uuid::new_v4();

        let prior = make_edge(subj, obj, t0);
        store.insert_edge(&prior).unwrap();
        // Manually pre-invalidate prior with a different successor (simulate
        // race / earlier closure). Do this by inserting a real edge as the
        // "other successor" first so the FK passes.
        let other_succ = make_edge(subj, obj, t0 + chrono::Duration::seconds(5));
        store.insert_edge(&other_succ).unwrap();
        store.invalidate_edge(prior.id, other_succ.id, t0 + chrono::Duration::seconds(6)).unwrap();
        let _ = other_succ_id; // unused; keep helper for symmetry

        // Now try to supersede prior again with a NEW successor — must error,
        // and the new successor must NOT be persisted (rollback).
        let mut new_succ = make_edge(subj, obj, t0 + chrono::Duration::seconds(20));
        new_succ.supersedes = Some(prior.id);
        let new_succ_id = new_succ.id;
        let r = store.supersede_edge(prior.id, &new_succ, t0 + chrono::Duration::seconds(30));
        assert!(matches!(r, Err(GraphError::Invariant(_))), "got {:?}", r);

        // Rollback: new_succ row must not exist.
        assert!(store.get_edge(new_succ_id).unwrap().is_none(),
            "successor must not be persisted on rollback");
    }

    #[test]
    fn supersede_edge_rejects_mismatched_supersedes_pointer() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");
        let prior = make_edge(subj, obj, t0);
        let other = make_edge(subj, obj, t0);
        store.insert_edge(&prior).unwrap();

        let mut succ = make_edge(subj, obj, t0 + chrono::Duration::seconds(10));
        // Wire successor.supersedes to a DIFFERENT edge id.
        succ.supersedes = Some(other.id);
        let r = store.supersede_edge(prior.id, &succ, t0 + chrono::Duration::seconds(20));
        match r {
            Err(GraphError::Invariant(msg)) => {
                assert!(msg.contains("supersedes"));
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn edges_as_of_returns_only_freshest_per_window() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        // Three edges in the SAME (subject, predicate, object) window —
        // edges_as_of should pick exactly one.
        let e1 = make_edge(subj, obj, t0);
        let e2 = make_edge(subj, obj, t0 + chrono::Duration::seconds(10));
        let e3 = make_edge(subj, obj, t0 + chrono::Duration::seconds(20));
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();
        store.insert_edge(&e3).unwrap();

        // Querying as-of t0+15 → e2 is the freshest with recorded_at <= 15.
        let got = store.edges_as_of(subj, t0 + chrono::Duration::seconds(15)).unwrap();
        assert_eq!(got.len(), 1, "exactly one row per window");
        assert_eq!(got[0].id, e2.id, "freshest before t=15");

        // As-of t0+25 → e3 (the latest) is freshest.
        let got = store.edges_as_of(subj, t0 + chrono::Duration::seconds(25)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, e3.id);

        // As-of t0-1 → nothing recorded yet.
        let got = store.edges_as_of(subj, t0 - chrono::Duration::seconds(1)).unwrap();
        assert!(got.is_empty(), "no rows before any recorded_at");
    }

    #[test]
    fn edges_as_of_excludes_invalidated_in_the_past() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let subj = insert_subject_entity(&mut store, "Alice");
        let obj = insert_subject_entity(&mut store, "Acme");

        let e1 = make_edge(subj, obj, t0);
        store.insert_edge(&e1).unwrap();
        // Invalidate at t0+5 (no successor needed for as-of test — make a
        // stub successor for the FK).
        let mut succ = make_edge(subj, obj, t0 + chrono::Duration::seconds(4));
        succ.supersedes = Some(e1.id);
        store.insert_edge(&succ).unwrap();
        store.invalidate_edge(e1.id, succ.id, t0 + chrono::Duration::seconds(5)).unwrap();

        // As-of t0+3 → e1 is still believed (invalidation hasn't happened yet
        // from `at`'s perspective).
        let got = store.edges_as_of(subj, t0 + chrono::Duration::seconds(3)).unwrap();
        assert!(got.iter().any(|e| e.id == e1.id), "e1 still live as-of t=3");

        // As-of t0+10 → e1 is invalidated and successor is now the freshest.
        let got = store.edges_as_of(subj, t0 + chrono::Duration::seconds(10)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, succ.id);
    }

    #[test]
    fn traverse_bfs_canonical_only_with_visited_dedup() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        // Build A → B → C plus a back-edge B → A (cycle) to verify the
        // visited set caps at one yield per entity. Use WorksAt (Directed,
        // no inverse) so only outgoing edges are walked — keeps the test
        // focused on cycle handling, not symmetric semantics.
        let a = insert_subject_entity(&mut store, "A");
        let b = insert_subject_entity(&mut store, "B");
        let c = insert_subject_entity(&mut store, "C");
        let e_ab = Edge::new(
            a,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: b },
            None,
            t0,
        );
        let e_bc = Edge::new(
            b,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: c },
            None,
            t0 + chrono::Duration::seconds(1),
        );
        let e_ba = Edge::new(
            b,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: a },
            None,
            t0 + chrono::Duration::seconds(2),
        );
        store.insert_edge(&e_ab).unwrap();
        store.insert_edge(&e_bc).unwrap();
        store.insert_edge(&e_ba).unwrap();

        // Walk from A, depth=2, only WorksAt. Expect to see B then C
        // exactly once each. A is start (visited from depth 0) — the
        // B → A back-edge must NOT yield A again.
        let pf = vec![Predicate::Canonical(CanonicalPredicate::WorksAt)];
        let got = store.traverse(a, 2, 100, &pf).unwrap();
        let visited_ids: Vec<Uuid> = got.iter().map(|(id, _)| *id).collect();
        assert!(visited_ids.contains(&b), "B reached at depth 1");
        assert!(visited_ids.contains(&c), "C reached at depth 2");
        assert_eq!(
            visited_ids.iter().filter(|&&i| i == a).count(),
            0,
            "A not re-yielded via back-edge"
        );
    }

    #[test]
    fn traverse_rejects_proposed_predicate_filter() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        // Proposed-predicate rejection happens up-front before any DB read,
        // so an empty store is fine.
        let pf = vec![Predicate::Proposed("custom_relation".into())];
        let r = store.traverse(Uuid::new_v4(), 2, 10, &pf);
        match r {
            Err(GraphError::MalformedPredicate(msg)) => {
                assert!(msg.contains("custom_relation"));
            }
            other => panic!("expected MalformedPredicate, got {:?}", other),
        }
    }

    #[test]
    fn traverse_max_depth_zero_returns_empty() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let got = store.traverse(Uuid::new_v4(), 0, 100, &[]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn traverse_max_results_caps_output() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        // A → {B0…B4} via WorksAt (note: WorksAt is functional in cardinality
        // terms but the trait doesn't enforce that here — traverse just walks
        // outgoing live edges).
        let a = insert_subject_entity(&mut store, "A");
        for i in 0..5 {
            let bi = insert_subject_entity(&mut store, &format!("B{}", i));
            let e = Edge::new(
                a,
                Predicate::Canonical(CanonicalPredicate::WorksAt),
                EdgeEnd::Entity { id: bi },
                None,
                t0 + chrono::Duration::seconds(i as i64),
            );
            store.insert_edge(&e).unwrap();
        }
        let pf = vec![Predicate::Canonical(CanonicalPredicate::WorksAt)];
        let got = store.traverse(a, 5, 3, &pf).unwrap();
        assert_eq!(got.len(), 3, "max_results caps at 3");
    }

    #[test]
    fn merge_entities_basic_redirects_loser_and_supersedes_edges() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let winner = insert_subject_entity(&mut store, "Alice");
        let loser = insert_subject_entity(&mut store, "Alise");
        let target = insert_subject_entity(&mut store, "Acme");
        let other = insert_subject_entity(&mut store, "Bob");

        // loser → target (subject side); other → loser (object side).
        let e1 = make_edge(loser, target, t0);
        let e2 = make_edge(other, loser, t0 + chrono::Duration::seconds(5));
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();

        let report = store.merge_entities(winner, loser, 100).unwrap();
        assert!(report.edges_superseded >= 2, "both edges re-minted");

        // Loser is redirected via the typed `merged_into` field.
        let got_loser = store.get_entity(loser).unwrap().expect("loser still exists");
        assert_eq!(got_loser.merged_into, Some(winner));

        // Originals are invalidated (preserved per GUARD-3).
        let got_e1 = store.get_edge(e1.id).unwrap().expect("e1 row still exists");
        assert!(got_e1.invalidated_at.is_some());
        let got_e2 = store.get_edge(e2.id).unwrap().expect("e2 row still exists");
        assert!(got_e2.invalidated_at.is_some());

        // Successor for e1 has subject = winner.
        let live_winner_edges = store.edges_of(winner, None, false).unwrap();
        assert!(
            live_winner_edges
                .iter()
                .any(|e| matches!(&e.object, EdgeEnd::Entity { id } if *id == target)),
            "winner now points at target"
        );
    }

    #[test]
    fn merge_entities_idempotent_resume() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let winner = insert_subject_entity(&mut store, "W");
        let loser = insert_subject_entity(&mut store, "L");
        let target = insert_subject_entity(&mut store, "T");
        let e1 = make_edge(loser, target, t0);
        store.insert_edge(&e1).unwrap();

        let r1 = store.merge_entities(winner, loser, 100).unwrap();
        assert!(r1.edges_superseded >= 1);

        // Second call: merged_into already set + no live loser edges → no-op.
        let r2 = store.merge_entities(winner, loser, 100).unwrap();
        assert_eq!(r2.edges_superseded, 0);
        assert_eq!(r2.aliases_repointed, 0);
    }

    #[test]
    fn merge_entities_rejects_self_merge() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let a = insert_subject_entity(&mut store, "A");
        match store.merge_entities(a, a, 10) {
            Err(GraphError::Invariant(msg)) => assert!(msg.contains("differ")),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn merge_entities_rejects_already_merged_to_different_winner() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let w1 = insert_subject_entity(&mut store, "W1");
        let w2 = insert_subject_entity(&mut store, "W2");
        let l = insert_subject_entity(&mut store, "L");
        store.merge_entities(w1, l, 10).unwrap();
        match store.merge_entities(w2, l, 10) {
            Err(GraphError::Invariant(msg)) => assert!(msg.contains("different winner")),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    // ============================================================
    // Phase 4: apply_graph_delta
    // ============================================================

    #[test]
    fn apply_graph_delta_writes_entities_edges_and_idempotence_row() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let mem_id = Uuid::new_v4();
        let mut delta = GraphDelta::new(mem_id);

        // Build delta: 2 entities + 1 edge between them.
        let alice = Entity::new("Alice".into(), EntityKind::Person, now);
        let acme = Entity::new("Acme".into(), EntityKind::Organization, now);
        let edge = Edge::new(
            alice.id,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: acme.id },
            None,
            now,
        );
        delta.entities.push(alice.clone());
        delta.entities.push(acme.clone());
        delta.edges.push(edge.clone());

        let report = store.apply_graph_delta(&delta).expect("apply ok");
        assert!(!report.already_applied);
        assert_eq!(report.entities_upserted, 2);
        assert_eq!(report.edges_inserted, 1);

        // Round-trip readback.
        assert!(store.get_entity(alice.id).unwrap().is_some());
        assert!(store.get_entity(acme.id).unwrap().is_some());
        assert!(store.get_edge(edge.id).unwrap().is_some());
    }

    #[test]
    fn apply_graph_delta_is_idempotent_on_replay() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let mem_id = Uuid::new_v4();
        let mut delta = GraphDelta::new(mem_id);
        delta.entities.push(Entity::new("X".into(), EntityKind::Concept, now));

        let r1 = store.apply_graph_delta(&delta).unwrap();
        assert!(!r1.already_applied);
        assert_eq!(r1.entities_upserted, 1);

        // Replay: must short-circuit, no side effects.
        let r2 = store.apply_graph_delta(&delta).unwrap();
        assert!(r2.already_applied);
        assert_eq!(r2.entities_upserted, 0);
        assert_eq!(r2.edges_inserted, 0);

        // Sanity: only ONE row in graph_applied_deltas (no duplicate).
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM graph_applied_deltas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "exactly one idempotence row after replay");
    }

    #[test]
    fn apply_graph_delta_rolls_back_on_validation_failure() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let mem_id = Uuid::new_v4();
        let mut delta = GraphDelta::new(mem_id);

        // Edge references an entity NOT in the delta and NOT in the store —
        // validate_references should reject. Pre-write count: 0 entities,
        // 0 edges.
        let phantom = Uuid::new_v4();
        let real = Entity::new("Real".into(), EntityKind::Person, now);
        let edge = Edge::new(
            real.id,
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: phantom },
            None,
            now,
        );
        delta.entities.push(real.clone());
        delta.edges.push(edge);

        // validate_references is part of the pre-flight; should error before
        // opening a tx.
        let result = store.apply_graph_delta(&delta);
        assert!(result.is_err(), "must reject dangling reference");

        // No rows persisted: rollback is whole-delta atomic.
        assert!(store.get_entity(real.id).unwrap().is_none(),
            "no entity persisted on rollback");
        let n: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM graph_applied_deltas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "no idempotence row on failure");
    }

    #[test]
    fn apply_graph_delta_invalidates_existing_edge() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let now = Utc::now();
        let subj = insert_subject_entity(&mut store, "S");
        let obj = insert_subject_entity(&mut store, "O");
        let prior = make_edge(subj, obj, now);
        store.insert_edge(&prior).unwrap();

        // Build delta: invalidate prior + insert successor.
        let mem_id = Uuid::new_v4();
        let mut delta = GraphDelta::new(mem_id);
        let mut succ = make_edge(subj, obj, now + chrono::Duration::seconds(10));
        succ.supersedes = Some(prior.id);
        delta.edges.push(succ.clone());
        delta.edges_to_invalidate.push(crate::graph::delta::EdgeInvalidation {
            edge_id: prior.id,
            invalidated_at: dt_to_unix(now + chrono::Duration::seconds(10)),
            superseded_by: Some(succ.id),
        });

        let report = store.apply_graph_delta(&delta).unwrap();
        assert_eq!(report.edges_inserted, 1);
        assert_eq!(report.edges_invalidated, 1);

        let got_prior = store.get_edge(prior.id).unwrap().unwrap();
        assert!(got_prior.invalidated_at.is_some());
        assert_eq!(got_prior.invalidated_by, Some(succ.id));
    }

    #[test]
    fn traverse_symmetric_predicate_walks_both_directions() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let t0 = Utc::now();
        let alice = insert_subject_entity(&mut store, "Alice");
        let bob = insert_subject_entity(&mut store, "Bob");
        // Alice MarriedTo Bob — Symmetric directionality, so traversal from
        // Bob should reach Alice via the incoming edge.
        let e = Edge::new(
            alice,
            Predicate::Canonical(CanonicalPredicate::MarriedTo),
            EdgeEnd::Entity { id: bob },
            None,
            t0,
        );
        store.insert_edge(&e).unwrap();

        let pf = vec![Predicate::Canonical(CanonicalPredicate::MarriedTo)];
        let got = store.traverse(bob, 1, 10, &pf).unwrap();
        let ids: Vec<Uuid> = got.iter().map(|(i, _)| *i).collect();
        assert!(ids.contains(&alice), "symmetric: Bob reaches Alice");
    }
}
