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
    delta::{ApplyReport, GraphDelta},
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
    /// Invalidate `old` pointing at `successor`; mark `successor.supersedes
    /// = old`. Atomic; rolls back on any invariant break.
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
    fn finish_pipeline_run(
        &mut self,
        run_id: Uuid,
        status: RunStatus,
        output_summary: Option<Json>,
        error_detail: Option<&str>,
    ) -> Result<(), GraphError>;
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

    fn merge_entities(&mut self, _winner: Uuid, _loser: Uuid, _batch_size: usize) -> Result<MergeReport, GraphError> {
        unimplemented!("Phase 3: merge_entities")
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
    fn supersede_edge(&mut self, _old: Uuid, _successor: &Edge, _at: DateTime<Utc>) -> Result<(), GraphError> {
        unimplemented!("Phase 3: supersede_edge")
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
    fn edges_as_of(&self, _subject: Uuid, _at: DateTime<Utc>) -> Result<Vec<Edge>, GraphError> {
        unimplemented!("Phase 3: edges_as_of")
    }
    fn traverse(
        &self, _start: Uuid, _max_depth: usize, _max_results: usize, _predicate_filter: &[Predicate],
    ) -> Result<Vec<(Uuid, Edge)>, GraphError> {
        unimplemented!("Phase 3: traverse")
    }

    // ---------------------------------------------- Provenance (Phase 2)
    fn entities_in_episode(&self, _episode: Uuid) -> Result<Vec<Uuid>, GraphError> {
        unimplemented!("Phase 2: entities_in_episode")
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
    fn mentions_of_entity(&self, _entity: Uuid) -> Result<EntityMentions, GraphError> {
        unimplemented!("Phase 2: mentions_of_entity")
    }
    fn link_memory_to_entities(
        &mut self, _memory_id: &str, _entity_ids: &[(Uuid, f64, Option<String>)], _at: DateTime<Utc>,
    ) -> Result<(), GraphError> {
        unimplemented!("Phase 2: link_memory_to_entities")
    }
    fn entities_linked_to_memory(&self, _memory_id: &str) -> Result<Vec<Uuid>, GraphError> {
        unimplemented!("Phase 2: entities_linked_to_memory")
    }
    fn memories_mentioning_entity(&self, _entity: Uuid, _limit: usize) -> Result<Vec<String>, GraphError> {
        unimplemented!("Phase 2: memories_mentioning_entity")
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
    fn upsert_topic(&mut self, _t: &KnowledgeTopic) -> Result<(), GraphError> {
        unimplemented!("Phase 2: upsert_topic")
    }
    fn get_topic(&self, _id: Uuid) -> Result<Option<KnowledgeTopic>, GraphError> {
        unimplemented!("Phase 2: get_topic")
    }
    fn list_topics(&self, _namespace: &str, _include_superseded: bool, _limit: usize) -> Result<Vec<KnowledgeTopic>, GraphError> {
        unimplemented!("Phase 2: list_topics")
    }
    fn supersede_topic(&mut self, _old: Uuid, _successor: Uuid, _at: DateTime<Utc>) -> Result<(), GraphError> {
        unimplemented!("Phase 2: supersede_topic")
    }

    // -------------------------------------------- Pipeline runs (Phase 2)
    fn begin_pipeline_run(&mut self, _kind: PipelineKind, _input_summary: Json) -> Result<Uuid, GraphError> {
        unimplemented!("Phase 2: begin_pipeline_run")
    }
    fn finish_pipeline_run(
        &mut self, _run_id: Uuid, _status: RunStatus, _output_summary: Option<Json>, _error_detail: Option<&str>,
    ) -> Result<(), GraphError> {
        unimplemented!("Phase 2: finish_pipeline_run")
    }
    fn record_resolution_trace(&mut self, _t: &ResolutionTrace) -> Result<(), GraphError> {
        unimplemented!("Phase 2: record_resolution_trace")
    }

    // -------------------------------------------- Predicates (Phase 2)
    fn record_predicate_use(&mut self, _p: &Predicate, _raw: &str, _at: DateTime<Utc>) -> Result<(), GraphError> {
        unimplemented!("Phase 2: record_predicate_use")
    }
    fn list_proposed_predicates(&self, _min_usage: u64) -> Result<Vec<ProposedPredicateStats>, GraphError> {
        unimplemented!("Phase 2: list_proposed_predicates")
    }

    // ------------------------------------------- Failures (Phase 2)
    fn record_extraction_failure(&mut self, _f: &ExtractionFailure) -> Result<(), GraphError> {
        unimplemented!("Phase 2: record_extraction_failure")
    }
    fn list_failed_episodes(&self, _unresolved_only: bool) -> Result<Vec<Uuid>, GraphError> {
        unimplemented!("Phase 2: list_failed_episodes")
    }
    fn mark_failure_resolved(&mut self, _failure_id: Uuid, _at: DateTime<Utc>) -> Result<(), GraphError> {
        unimplemented!("Phase 2: mark_failure_resolved")
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
        _delta: &GraphDelta,
    ) -> Result<ApplyReport, GraphError> {
        unimplemented!("Phase 4: apply_graph_delta — composes Phase 3 transactional methods")
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
}
