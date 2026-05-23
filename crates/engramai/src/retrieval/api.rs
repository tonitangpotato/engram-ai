//! # Retrieval Public API surface (`task:retr-impl-graph-query-api`)
//!
//! Defines the **structured query API** for v0.3 retrieval. This module owns
//! only the *contracts* — type definitions and stub method signatures on
//! [`Memory`]. The execution backends (plans, fusion, classifier wiring,
//! trace assembly, typed-outcome detail) are implemented by sibling tasks:
//!
//! - `task:retr-impl-classifier-heuristic` / `task:retr-impl-classifier-llm`
//!   — populate `ClassifierTrace` and decide `Intent`.
//! - `task:retr-impl-{factual,episodic,associative,abstract-l5,affective,hybrid}`
//!   — concrete plan executions consumed by [`Memory::graph_query`].
//! - `task:retr-impl-fusion` — assembles [`SubScores`] and ranks
//!   [`ScoredResult`] via per-plan fusion weights / RRF.
//! - `task:retr-impl-typed-outcomes` (T12) — fleshes out
//!   [`RetrievalOutcome`] beyond the placeholder variants below; the stub
//!   here is the surface area required for the API to compile, no more.
//! - `task:retr-impl-explain-trace` (T14) — replaces the [`PlanTrace`]
//!   placeholder with the full struct from design §6.3.
//! - `task:retr-impl-budget-cutoff` — wires per-stage [`Duration`] caps
//!   into the body of `graph_query` / `graph_query_locked`.
//!
//! ## Spec references
//!
//! - Design §6.2 — `GraphQuery`, `ScoredResult`, `GraphQueryResponse`
//!   (`.gid/features/v03-retrieval/design.md`).
//! - Design §6.2a — types referenced by the public API (`RetrievalError`,
//!   `SubScores`, `TimeWindow`).
//! - Design §6.4 — `RetrievalOutcome` (placeholder here, full T12).
//! - Design §6.5 — Tier API (`MemoryTier`, `recall_tier`, `list_tier`).
//! - GOAL-3.9 (formal tier API), GOAL-3.10 (typed outcomes), GOAL-3.11
//!   (opt-in trace).
//!
//! ## Async signatures
//!
//! Per design §6.2/§6.5 the public methods are `async fn`. The rest of
//! engramai is currently synchronous (`&mut self` / blocking SQLite). Rust
//! permits async function *definitions* without an async runtime — and the
//! retrieval surface is intentionally async-shaped so the future plan
//! executors (which will likely block on tokio I/O for the LLM fallback
//! and prometheus emission) need not break the public contract when they
//! land. Today the bodies are pure stubs: they just return
//! [`RetrievalError::Internal`] with a `not yet implemented` message.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::Memory;
use crate::retrieval::classifier::Intent;
use crate::types::{MemoryRecord, RecallResult};
use crate::graph::KnowledgeTopic;
use crate::store_api::MemoryId;

/// Stable identifier for a graph entity (L3'). Mirrors the v0.3 graph
/// layer's UUID-keyed entity rows; we re-export as a type alias so the
/// retrieval API doesn't bake `uuid::Uuid` into its signature today —
/// promotion to a newtype later is a non-breaking change for downstream
/// callers that only use the alias.
///
/// See `.gid/features/v03-graph-layer/design.md` §3 (Entity).
pub type EntityId = uuid::Uuid;

// ---------------------------------------------------------------------------
// 6.2 — GraphQuery / ScoredResult / GraphQueryResponse
// ---------------------------------------------------------------------------

/// Time-window selector for [`GraphQuery::time_window`]. (Design §6.2a.)
///
/// Variants mirror what the heuristic temporal scorer can extract from the
/// query string (`At` / `Range` / `Relative`) plus the explicit "no temporal
/// component" sentinel (`None`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TimeWindow {
    /// No temporal scoping — Episodic plan downgrades per design §4.2.
    None,
    /// Single-instant projection (point-in-time recall).
    At(DateTime<Utc>),
    /// Closed/half-open interval; either endpoint may be unbounded.
    Range {
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    },
    /// Relative window expressed as a [`Duration`] looking *backward* from
    /// `query_time` (or "now" if not pinned). The exact semantics — whether
    /// `Relative(d)` means `[now - d, now]` or `[now - d, ∞)` — are pinned
    /// by `task:retr-impl-episodic`.
    Relative(Duration),
}

/// Structured retrieval query — the v0.3 public entry point.
///
/// Only [`GraphQuery::text`] is required; every other field has a sensible
/// default that matches v0.2 `recall()` behavior so existing callers can
/// migrate one field at a time.
///
/// Construct with [`GraphQuery::new`] and the builder-style `with_*` setters
/// for ergonomics, or use `GraphQuery { text: "...".into(), ..Default::default() }`.
#[derive(Debug, Clone)]
pub struct GraphQuery {
    /// Free-text user query. Required.
    pub text: String,

    /// Caller-specified intent (§3.3 override). `None` → classifier decides.
    pub intent: Option<Intent>,

    /// Top-K cutoff. Defaults to `10`.
    pub limit: usize,

    /// Override the heuristic temporal parse (§4.2).
    pub time_window: Option<TimeWindow>,

    /// Bi-temporal projection: see only edges valid at this instant
    /// (design §4.6). Defaults to `None` = "now".
    pub as_of: Option<DateTime<Utc>>,

    /// GOAL-3.5 — opt in to including superseded edges in the response
    /// (history view). Default `false`.
    pub include_superseded: bool,

    /// Restrict factual / hybrid plans to a fixed entity set.
    pub entity_filter: Option<Vec<EntityId>>,

    /// Drop low-confidence edges from the candidate pool (design §5.1).
    pub min_confidence: Option<f64>,

    /// GOAL-3.9 — explicit tier scoping for [`MemoryTier`].
    pub tier: Option<MemoryTier>,

    /// Reproducibility pin (§5.4): freeze `query_time` so repeat queries
    /// against the same DB return byte-identical responses. `None` → "now".
    pub query_time: Option<DateTime<Utc>>,

    /// GOAL-3.11 — opt-in `PlanTrace` assembly. Default `false` keeps the
    /// production hot path overhead-free.
    pub explain: bool,

    /// Tenant / logical-space isolation boundary (ISS-056).
    ///
    /// When `Some(ns)`, retrieval adapters scope their SQL against that
    /// namespace's `memories` / `graph_entities` / `graph_topics` rows.
    /// When `None`, falls back to the literal `"default"` namespace —
    /// matching pre-ISS-056 behavior so single-tenant callers are
    /// unchanged.
    ///
    /// Multi-tenant callers (LoCoMo benchmark, multi-conversation
    /// ingest, etc.) MUST set this via
    /// [`GraphQuery::with_namespace`] — otherwise queries hit the
    /// `default` namespace and return zero results against data
    /// ingested under any other `--ns`.
    pub namespace: Option<String>,

    /// Per-query override of the cognitive self-state passed into the
    /// affective retrieval plan (`task:retr-impl-cognitive-state-readback`
    /// / GOAL-5.6).
    ///
    /// When `Some(fp)`, this fingerprint is used verbatim as `s_now` for
    /// affective ranking, **bypassing** the live
    /// [`Memory::current_self_state`](crate::Memory::current_self_state)
    /// readback. This exists for two reasons:
    ///
    /// 1. **Deterministic benchmarks (GOAL-5.6).** The
    ///    `cognitive_regression` driver needs to compare ranking under
    ///    state S1 vs ranking under state S2 against the *same* `Memory`
    ///    without mutating its hub between runs. Passing fingerprints in
    ///    via `with_self_state_override` keeps each run reproducible.
    /// 2. **Reproducibility records (§5.4).** Saved query traces include
    ///    the exact `s_now` used, so replays can pin it explicitly.
    ///
    /// When `None` (default), `Memory::graph_query` reads the live
    /// fingerprint from the interoceptive hub. If the hub is empty the
    /// affective plan downgrades to associative per §6.2.
    pub self_state_override: Option<crate::graph::affect::SomaticFingerprint>,

    /// ISS-139 — per-query override for the MMR diversity λ.
    ///
    /// When `Some(λ)`, the post-fusion reranker uses this λ instead of
    /// `FusionConfig::locked().mmr_lambda`. When `None` (default), the
    /// locked config's value applies (currently `1.0` = MMR off,
    /// byte-identical to pre-ISS-139 behavior).
    ///
    /// **Range**: `λ ∈ [0.0, 1.0]`. `1.0` = pure relevance (no-op);
    /// `0.0` = pure diversity (don't use); literature recommends
    /// `0.5..0.8` for list-style queries. Out-of-range values cause
    /// `MmrReranker::new` to panic — this is intentional fail-fast,
    /// not a silent clamp.
    ///
    /// Intended consumers: benchmark drivers (LoCoMo, cogmembench)
    /// that want to compare with/without MMR on the same query set,
    /// and reproducibility records that pin the exact λ used. Normal
    /// callers should leave this `None`.
    pub mmr_lambda_override: Option<f32>,
}

impl GraphQuery {
    /// Construct a query with only `text` set; all other fields default.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            intent: None,
            limit: 10,
            time_window: None,
            as_of: None,
            include_superseded: false,
            entity_filter: None,
            min_confidence: None,
            tier: None,
            query_time: None,
            explain: false,
            self_state_override: None,
            namespace: None,
            mmr_lambda_override: None,
        }
    }

    /// Builder: tenant / logical-space namespace (ISS-056).
    ///
    /// Sets the namespace that retrieval adapters scope their SQL
    /// against. Without this, the query hits the `"default"` namespace
    /// regardless of where the underlying data was ingested.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Builder: top-K cutoff.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Builder: caller-specified intent (skips the classifier per §3.3).
    pub fn with_intent(mut self, intent: Intent) -> Self {
        self.intent = Some(intent);
        self
    }

    /// Builder: as-of-T projection (§4.6).
    pub fn with_as_of(mut self, as_of: DateTime<Utc>) -> Self {
        self.as_of = Some(as_of);
        self
    }

    /// Builder: opt in to explain trace (GOAL-3.11).
    pub fn with_explain(mut self, on: bool) -> Self {
        self.explain = on;
        self
    }

    /// Builder: explicit tier scoping (GOAL-3.9).
    pub fn with_tier(mut self, tier: MemoryTier) -> Self {
        self.tier = Some(tier);
        self
    }

    /// Builder: pin the cognitive self-state for this query
    /// (GOAL-5.6 / `task:retr-impl-cognitive-state-readback`).
    ///
    /// When set, this fingerprint replaces the live readback from
    /// [`Memory::current_self_state`](crate::Memory::current_self_state)
    /// for affective ranking. See
    /// [`GraphQuery::self_state_override`] for the full rationale.
    pub fn with_self_state_override(
        mut self,
        fp: crate::graph::affect::SomaticFingerprint,
    ) -> Self {
        self.self_state_override = Some(fp);
        self
    }

    /// Builder: per-query MMR diversity λ override (ISS-139).
    ///
    /// See [`GraphQuery::mmr_lambda_override`] for semantics. Pass
    /// `None` to fall back to `FusionConfig::locked().mmr_lambda`.
    pub fn with_mmr_lambda(mut self, lambda: Option<f32>) -> Self {
        self.mmr_lambda_override = lambda;
        self
    }
}

impl Default for GraphQuery {
    fn default() -> Self {
        Self::new(String::new())
    }
}

/// Per-signal sub-scores recorded for a fused candidate (§5.1, §6.2a).
///
/// Each field is `Option<f64>` in `[0, 1]` — `None` means "this plan did not
/// emit that signal" (e.g., the Affective plan emits no `bm25_score`).
/// Population is owned by `task:retr-impl-fusion`; the type is co-located
/// with the API so `ScoredResult::Memory` can reference it without a
/// dependency on the (not-yet-existent) fusion module.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SubScores {
    pub vector_score: Option<f64>,
    pub bm25_score: Option<f64>,
    pub graph_score: Option<f64>,
    pub recency_score: Option<f64>,
    pub actr_score: Option<f64>,
    pub affect_similarity: Option<f64>,
}

/// Heterogeneous result row — design §6.2.
///
/// `Memory` carries a per-record candidate with its fused score and
/// per-signal breakdown (for explain). `Topic` carries an L5
/// [`KnowledgeTopic`] returned by the Abstract plan, with its source-memory
/// and contributing-entity provenance (design §4.4).
#[derive(Debug, Clone)]
pub enum ScoredResult {
    /// Per-record candidate (Factual / Episodic / Associative / Affective /
    /// Hybrid plans).
    Memory {
        record: MemoryRecord,
        score: f64,
        sub_scores: SubScores,
        /// Optional candidate embedding (ISS-139 MMR support).
        ///
        /// Populated by adapters that already have the embedding in
        /// hand from the vector-search step (e.g. `HybridSeedRecaller`).
        /// `None` when the candidate came from a path that doesn't
        /// touch embeddings (e.g. graph-only walks, FTS-only seeds);
        /// rerankers that need vector similarity must then fall back
        /// to `Storage::get_embedding(record.id, model)` per
        /// candidate.
        ///
        /// Default `None` keeps construction sites that don't have an
        /// embedding cheap. ~1.5KB per populated candidate × ~200
        /// pool candidates ≈ 300KB transient memory at the rerank
        /// boundary — acceptable per ISS-139 design note.
        embedding: Option<Vec<f32>>,
    },
    /// L5 topic candidate (Abstract plan, optionally Hybrid).
    Topic {
        topic: KnowledgeTopic,
        score: f64,
        source_memories: Vec<MemoryId>,
        contributing_entities: Vec<EntityId>,
    },
}

impl ScoredResult {
    /// Convenience: extract the fused score regardless of variant. Useful
    /// for sort/cmp in benchmarks and tests.
    pub fn score(&self) -> f64 {
        match self {
            ScoredResult::Memory { score, .. } | ScoredResult::Topic { score, .. } => *score,
        }
    }
}

/// Response envelope for [`Memory::graph_query`]. Design §6.2.
///
/// Returning a struct (not a bare `Vec<ScoredResult>`) is deliberate so
/// future-proofing — adding `outcome`, `trace`, or further metadata fields
/// — does not require a breaking API change.
#[derive(Debug, Clone)]
pub struct GraphQueryResponse {
    /// Ordered top-K candidates (descending score).
    pub results: Vec<ScoredResult>,
    /// The plan that actually executed (may differ from `query.intent`
    /// after `RetrievalOutcome::Downgraded*`).
    pub plan_used: Intent,
    /// Typed success/failure surface (§6.4). See [`RetrievalOutcome`].
    pub outcome: RetrievalOutcome,
    /// Filled iff `query.explain == true`. Owned by
    /// `task:retr-impl-explain-trace`.
    pub trace: Option<PlanTrace>,
}

// ---------------------------------------------------------------------------
// 6.4 — RetrievalOutcome / RetrievalError
// ---------------------------------------------------------------------------
//
// The full surface lives in [`crate::retrieval::outcomes`] (owned by
// `task:retr-impl-typed-outcomes`, T12). Re-exported here so existing
// `use crate::retrieval::api::{RetrievalOutcome, RetrievalError}` paths
// keep compiling without churn — the module split is internal.

pub use crate::retrieval::outcomes::{RetrievalError, RetrievalOutcome};

// ---------------------------------------------------------------------------
// 6.3 — PlanTrace (full surface in `crate::retrieval::explain`)
// ---------------------------------------------------------------------------
//
// Owned by `task:retr-impl-explain-trace` (T14). Re-exported here so the
// `GraphQueryResponse` field type stays addressable from the API surface
// without a separate `use crate::retrieval::explain::PlanTrace;` import in
// every caller.

pub use crate::retrieval::explain::PlanTrace;

// ---------------------------------------------------------------------------
// 6.5 — Tier API
// ---------------------------------------------------------------------------

/// Memory tier — design §6.5, GOAL-3.9.
///
/// Tiers are an externally-visible projection of engramai's internal
/// trace-strength model. The exact thresholds (`τ_hot` / `τ_warm`) live
/// in `RetrievalConfig` and are wired by `task:retr-impl-budget-cutoff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryTier {
    /// Hot: high short-term trace strength (`short_term_strength ≥ τ_hot`).
    Working,
    /// Warm: high long-term trace + recent activation
    /// (`long_term_strength ≥ τ_warm AND recent_activation`).
    Core,
    /// Cold: below activation threshold.
    Archived,
}

impl MemoryTier {
    /// Stable string form for logging / metrics (paired with
    /// `retrieval_tier_*` Prometheus labels in `task:retr-impl-metrics`).
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryTier::Working => "working",
            MemoryTier::Core => "core",
            MemoryTier::Archived => "archived",
        }
    }
}

// ---------------------------------------------------------------------------
// Memory impl — stub method bodies (executors land in sibling tasks)
// ---------------------------------------------------------------------------

impl Memory {
    /// Structured graph-aware retrieval (design §6.2 / GOAL-3.1).
    ///
    /// **Partial implementation:** the classifier-dispatch stage is wired
    /// (`task:retr-impl-orchestrator-classifier-dispatch`) — incoming
    /// queries are classified into an [`Intent`] + executable
    /// [`PlanKind`](crate::retrieval::dispatch::PlanKind) and a
    /// [`PlanContext`](crate::retrieval::dispatch::PlanContext) is built.
    /// Plan **execution** is the next task
    /// (`task:retr-impl-orchestrator-plan-execution`); until it lands the
    /// method returns [`RetrievalError::Internal`] so callers see a clear
    /// "dispatched but not executed" message rather than silently
    /// succeeding with empty results.
    pub async fn graph_query(
        &self,
        query: GraphQuery,
    ) -> Result<GraphQueryResponse, RetrievalError> {
        // Extract per-query self-state override before `dispatch` consumes
        // the query. `None` here means "fall back to live hub readback".
        let self_state_override = query.self_state_override;
        // ISS-056: extract namespace before `query` is moved into
        // `dispatch`. Falls back to `\"default\"` when unset, matching
        // pre-ISS-056 single-tenant behavior.
        let namespace: String = query
            .namespace
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // ISS-064: fast-fail when caller explicitly targets a namespace
        // that holds no memories AND no graph entities. Without this,
        // queries silently return empty for typos like "defualt" with
        // no signal in logs (§6 of ISS-064 trace). We only fast-fail
        // when the caller *explicitly* passed a namespace — `None` is
        // the implicit-default path which must keep working even when
        // the DB is freshly initialized.
        if query.namespace.is_some() {
            let mem_has = self
                .storage()
                .list_namespaces()
                .map(|ns| ns.iter().any(|n| n == &namespace))
                .unwrap_or(false);
            let graph_has = self.with_graph_read(|g| {
                g.list_namespaces()
                    .map(|ns| ns.iter().any(|n| n == &namespace))
                    .unwrap_or(false)
            })?;
            if !mem_has && !graph_has {
                log::warn!(
                    target: "engramai::retrieval",
                    "graph_query: namespace {:?} not found (no memories, no entities) — returning empty result set",
                    namespace
                );
                return Ok(GraphQueryResponse {
                    results: Vec::new(),
                    plan_used: crate::retrieval::classifier::Intent::Episodic,
                    outcome: crate::retrieval::outcomes::RetrievalOutcome::EmptyResultSet {
                        reason: "namespace_not_found".to_string(),
                    },
                    trace: None,
                });
            }
        }

        // Stage A: dispatch.
        let classifier =
            crate::retrieval::classifier::HeuristicClassifier::with_null_lookup();
        // Capture the user text + MMR override before `dispatch()` takes
        // ownership of `query`. The text is needed by the MMR reranker
        // hook (Stage C.5) for trace honesty; the override picks the λ
        // (None → use `FusionConfig::locked().mmr_lambda`).
        let query_text = query.text.clone();
        let mmr_lambda_override = query.mmr_lambda_override;
        let dispatched = crate::retrieval::dispatch::dispatch(query, &classifier);
        let plan_kind = dispatched.plan_kind;
        let intent = dispatched.intent;
        let limit = dispatched.context.limit;
        let explain = dispatched.context.explain;

        // Stage B: plan execution. The orchestrator extracts the graph
        // store from `with_graph_read` and runs the dispatched plan
        // against `Null*` collaborators (deferred until per-recaller
        // tasks land — see `crate::retrieval::orchestrator` module note).
        // The `StorageLoader` borrows `&Storage`, hydrating
        // `MemoryRecord`s lazily.
        let loader =
            crate::retrieval::orchestrator::StorageLoader::new(self.storage());

        // Self-state resolution (`task:retr-impl-cognitive-state-readback`
        // / GOAL-5.6):
        //   1. If the caller pinned a fingerprint via
        //      `GraphQuery::with_self_state_override`, use it verbatim.
        //      This path drives the cognitive_regression benchmark and
        //      reproducibility replays.
        //   2. Otherwise read the live snapshot off the interoceptive
        //      hub via `Memory::current_self_state`. Returns `None` when
        //      the hub is empty (cold start), so the affective plan
        //      downgrades to associative routing per §6.2 instead of
        //      ranking against a synthetic neutral state.
        let self_state =
            self_state_override.or_else(|| self.current_self_state());

        // Phase-3 (ISS-049): construct the real graph/storage-backed
        // adapters. The five `Null*` stubs from Phase 2 are gone; each
        // collaborator now wraps live state (`Storage`, `EmbeddingProvider`,
        // and the `&dyn GraphRead` borrowed inside `with_graph_read`).
        //
        // Namespace: bound to `"default"` as a Phase-3 interim. The
        // resolution layer + fingerprinting work that wires real
        // multi-namespace dispatch is Phase 4 (see ISS-049 plan).
        //
        // Embedding model: read off the live provider's config so the
        // hybrid recallers query the same embedding row that ingestion
        // wrote. If embeddings are disabled, we fall back to an empty
        // model id — the hybrid path still serves keyword-only signal.
        let storage = self.storage();
        let embedding = self.embedding_provider();
        let embedding_model_owned: String = embedding
            .map(|p| p.config().model_id())
            .unwrap_or_default();
        // ISS-056: namespace was extracted from `query` before dispatch
        // (see top of this fn). Re-borrow as `&str` for adapter ctors.
        let namespace: &str = namespace.as_str();

        let (candidates, outcome) = self.with_graph_read(|graph| {
            let entity_resolver =
                crate::retrieval::adapters::GraphEntityResolver::new(graph);
            let episodic_store =
                crate::retrieval::adapters::StorageEpisodicStore::new(storage, graph);
            let seed_recaller =
                crate::retrieval::adapters::HybridSeedRecaller::new(
                    storage,
                    embedding,
                    namespace,
                    embedding_model_owned.as_str(),
                );
            let topic_searcher =
                crate::retrieval::adapters::GraphTopicSearcher::new(graph, embedding);
            let affective_recaller =
                crate::retrieval::adapters::HybridAffectiveSeedRecaller::new(
                    storage,
                    embedding,
                    namespace,
                    embedding_model_owned.as_str(),
                );
            let collaborators =
                crate::retrieval::orchestrator::PlanCollaborators {
                    entity_resolver: &entity_resolver,
                    episodic_store: &episodic_store,
                    seed_recaller: &seed_recaller,
                    topic_searcher: &topic_searcher,
                    affective_recaller: &affective_recaller,
                };
            crate::retrieval::orchestrator::execute_plan(
                dispatched,
                graph,
                &loader,
                &collaborators,
                self_state,
            )
        })?;

        // Stage C: fusion + ranking. Hybrid bypasses `fuse_and_rank`
        // because RRF already produced a fused score (§5.2). Other
        // plans flow through the per-intent weighted combine.
        let cfg = crate::retrieval::fusion::FusionConfig::locked();
        let mut ranked = match plan_kind {
            crate::retrieval::dispatch::PlanKind::Hybrid => candidates,
            _ => crate::retrieval::fusion::fuse_and_rank(intent, &cfg, candidates),
        };

        // Stage C.5 (ISS-139): optional post-fusion MMR reranker.
        //
        // Runs **before** `top_k` truncation so the diversity pick can
        // displace lower-ranked relevant-but-redundant items from the
        // final result set. At effective `λ == 1.0` (the locked
        // default unless the caller passes `with_mmr_lambda(Some(<1.0))`)
        // MMR degenerates to pure relevance and returns the input
        // unchanged (byte-identical, preserves the §5.4 reproducibility
        // envelope). Lower λ shifts toward diversity.
        //
        // Source of λ: per-query override wins over the locked config
        // default. See `GraphQuery::mmr_lambda_override` for the
        // rationale of putting the knob on the query rather than
        // mutating `FusionConfig::locked()`.
        //
        // Hook location chosen per ISS-139 §"Hook location": single
        // chokepoint covers all 7 plans, runs once per query, and
        // doesn't need plumbing into each plan's adapter.
        let effective_lambda = mmr_lambda_override.unwrap_or(cfg.mmr_lambda);
        if effective_lambda < 1.0 {
            use crate::retrieval::fusion::Reranker;
            let mmr = crate::retrieval::fusion::MmrReranker::new(effective_lambda);
            // `query` arg is ignored by MmrReranker (see its docstring);
            // passing `query_text` for trace/log honesty if a future
            // Reranker decides to use it.
            ranked = mmr.rerank(&query_text, &ranked)?;
        }

        // Top-K cutoff.
        if ranked.len() > limit {
            ranked.truncate(limit);
        }

        // Stage D: trace assembly is owned by `task:retr-impl-explain-trace`.
        // Until that lands, `explain == true` queries get `trace = None`.
        let _ = explain; // explicit intent; trace is None until T14.

        Ok(GraphQueryResponse {
            results: ranked,
            plan_used: intent,
            outcome,
            trace: None,
        })
    }

    /// Deterministic-mode variant (design §6.2 / §5.4).
    ///
    /// Equivalent to [`Memory::graph_query`] but pins the fusion config to
    /// `FusionConfig::locked()` — no env, no files, no flags. Intended for
    /// benchmarks, reproducibility records, and byte-identical-output tests.
    pub async fn graph_query_locked(
        &self,
        query: GraphQuery,
    ) -> Result<GraphQueryResponse, RetrievalError> {
        // §5.4 — locked path pins `FusionConfig::locked()` and disables
        // any env / file / flag overrides. Today `graph_query` already
        // uses `FusionConfig::locked()` unconditionally (the env-aware
        // alternative isn't wired yet — `task:retr-impl-fusion-config-loader`),
        // so the two methods are behaviorally equivalent. They remain
        // separate API entries so future work can diverge them without
        // a breaking change to benchmark callers.
        self.graph_query(query).await
    }

    /// Tier-scoped recall — design §6.5 / GOAL-3.9.
    ///
    /// **Stub:** returns `RetrievalError::Internal` until the tier
    /// classifier wires up.
    pub async fn recall_tier(
        &self,
        _tier: MemoryTier,
        _query: &str,
        _k: usize,
    ) -> Result<Vec<RecallResult>, RetrievalError> {
        Err(RetrievalError::Internal(
            "Memory::recall_tier — tier classifier not yet wired \
             (task:retr-impl-budget-cutoff supplies τ_hot / τ_warm thresholds)"
                .into(),
        ))
    }

    /// Tier-scoped enumeration — design §6.5 / GOAL-3.9.
    ///
    /// **Stub:** see [`Memory::recall_tier`].
    pub async fn list_tier(
        &self,
        _tier: MemoryTier,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<MemoryRecord>, RetrievalError> {
        Err(RetrievalError::Internal(
            "Memory::list_tier — tier classifier not yet wired \
             (task:retr-impl-budget-cutoff supplies τ_hot / τ_warm thresholds)"
                .into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests — surface-level invariants only (executors are tested elsewhere)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_query_new_sets_defaults() {
        let q = GraphQuery::new("hello");
        assert_eq!(q.text, "hello");
        assert!(q.intent.is_none());
        assert_eq!(q.limit, 10);
        assert!(q.time_window.is_none());
        assert!(q.as_of.is_none());
        assert!(!q.include_superseded);
        assert!(q.entity_filter.is_none());
        assert!(q.min_confidence.is_none());
        assert!(q.tier.is_none());
        assert!(q.query_time.is_none());
        assert!(!q.explain);
        // ISS-056: namespace defaults to None (→ "default" at runtime).
        assert!(q.namespace.is_none());
    }

    #[test]
    fn graph_query_builders_compose() {
        let q = GraphQuery::new("entities at issue")
            .with_limit(25)
            .with_intent(Intent::Factual)
            .with_explain(true)
            .with_tier(MemoryTier::Core);
        assert_eq!(q.limit, 25);
        assert_eq!(q.intent, Some(Intent::Factual));
        assert!(q.explain);
        assert_eq!(q.tier, Some(MemoryTier::Core));
    }

    /// ISS-056: `with_namespace` sets the namespace field.
    #[test]
    fn graph_query_with_namespace_sets_field() {
        let q = GraphQuery::new("conv-26 query").with_namespace("conv26");
        assert_eq!(q.namespace.as_deref(), Some("conv26"));
    }

    /// ISS-056: `with_namespace` accepts both `&str` and `String`.
    #[test]
    fn graph_query_with_namespace_accepts_into_string() {
        let q1 = GraphQuery::new("a").with_namespace("ns_a");
        let q2 = GraphQuery::new("b").with_namespace(String::from("ns_b"));
        assert_eq!(q1.namespace.as_deref(), Some("ns_a"));
        assert_eq!(q2.namespace.as_deref(), Some("ns_b"));
    }

    /// ISS-056: `with_namespace` is composable with other builders.
    #[test]
    fn graph_query_with_namespace_composes() {
        let q = GraphQuery::new("locomo")
            .with_namespace("conv26")
            .with_limit(25)
            .with_intent(Intent::Factual);
        assert_eq!(q.namespace.as_deref(), Some("conv26"));
        assert_eq!(q.limit, 25);
        assert_eq!(q.intent, Some(Intent::Factual));
    }

    /// ISS-056: `Default` and the struct-literal pattern leave namespace
    /// as `None`, which the retrieval entry point resolves to `"default"`.
    /// This preserves pre-ISS-056 behavior for single-tenant callers.
    #[test]
    fn graph_query_default_namespace_is_none() {
        let q1 = GraphQuery::default();
        let q2 = GraphQuery {
            text: "x".into(),
            ..Default::default()
        };
        assert!(q1.namespace.is_none());
        assert!(q2.namespace.is_none());
    }

    #[test]
    fn scored_result_score_dispatches_by_variant() {
        let mem = ScoredResult::Memory {
            record: MemoryRecord {
                id: "m1".into(),
                content: "x".into(),
                memory_type: crate::types::MemoryType::Factual,
                layer: crate::types::MemoryLayer::Working,
                created_at: chrono::Utc::now(),
                occurred_at: None,
                access_times: vec![],
                working_strength: 0.0,
                core_strength: 0.0,
                importance: 0.0,
                pinned: false,
                consolidation_count: 0,
                last_consolidated: None,
                source: String::new(),
                contradicts: None,
                contradicted_by: None,
                superseded_by: None,
                metadata: None,
            },
            score: 0.42,
            sub_scores: SubScores::default(),
            embedding: None,
        };
        assert!((mem.score() - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn memory_tier_as_str_is_stable() {
        assert_eq!(MemoryTier::Working.as_str(), "working");
        assert_eq!(MemoryTier::Core.as_str(), "core");
        assert_eq!(MemoryTier::Archived.as_str(), "archived");
    }

    /// Minimal blocking executor — drives the async stubs to completion in
    /// tests without pulling in a full runtime (engramai's dev-dependencies
    /// don't include tokio). Sufficient because the stubs never actually
    /// `.await` anything (they return `Err(_)` synchronously after the
    /// first poll).
    fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
        use std::pin::Pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct NoopWaker;
        impl Wake for NoopWaker {
            fn wake(self: Arc<Self>) {}
        }

        // Safety: shadow `fut` as a pinned reference we own on the stack.
        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        let waker = Waker::from(Arc::new(NoopWaker));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    /// Without a graph store installed, `graph_query` surfaces a typed
    /// `Internal` error from `with_graph_read` rather than crashing.
    /// This is the v0.2-compat path: callers without v0.3 ingestion
    /// fall back to the legacy `recall()` API.
    #[test]
    fn graph_query_without_graph_store_returns_internal_error() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-no-graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None).expect("memory init");
        let err = block_on(mem.graph_query(GraphQuery::new("x")))
            .expect_err("no graph store → Internal error");
        match err {
            RetrievalError::Internal(msg) => {
                assert!(
                    msg.contains("no graph store installed"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn graph_query_locked_delegates_to_graph_query() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-locked.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None).expect("memory init");
        // Same Internal error surface as graph_query — confirms the
        // delegation rather than the old stub message.
        let err = block_on(mem.graph_query_locked(GraphQuery::new("x")))
            .expect_err("no graph store → Internal error");
        assert!(matches!(err, RetrievalError::Internal(_)));
    }

    /// End-to-end: graph store installed but empty → Factual override
    /// downgrades to `NoEntityFound` (the orchestrator does not error;
    /// it surfaces a typed outcome with empty results).
    #[test]
    fn graph_query_with_empty_graph_returns_typed_outcome() {
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-empty-graph.db");
        let graph_db = tmp.path().join("retrieval-api-empty-graph.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        let q = GraphQuery::new("alice").with_intent(Intent::Factual);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");
        assert!(resp.results.is_empty(), "empty graph → no candidates");
        assert_eq!(resp.plan_used, Intent::Factual);
        // ISS-063 (2026-04-28): Factual on an empty graph used to return
        // `NoEntityFound`. New contract: Factual is empty → orchestrator
        // runs Associative fallback (§3.4) → Associative is also empty
        // on an empty graph → terminal `EmptyResultSet`. The reason
        // string distinguishes "Factual emitted NoEntityFound, fallback
        // also empty" from "Associative was the primary plan".
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "factual_then_associative_empty"
            ),
            "got outcome {:?}",
            resp.outcome
        );
        assert!(resp.trace.is_none(), "explain off → trace None");
    }

    #[test]
    fn tier_methods_stub_return_internal_error() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-stub-tier.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None).expect("memory init");
        assert!(matches!(
            block_on(mem.recall_tier(MemoryTier::Working, "q", 5)).expect_err("stub"),
            RetrievalError::Internal(_)
        ));
        assert!(matches!(
            block_on(mem.list_tier(MemoryTier::Core, 10, 0)).expect_err("stub"),
            RetrievalError::Internal(_)
        ));
    }

    // ── Self-state readback (task:retr-impl-cognitive-state-readback) ─

    #[test]
    fn graph_query_default_has_no_self_state_override() {
        // GraphQuery::new must default the override to None so existing
        // callers preserve behavior.
        let q = GraphQuery::new("hello");
        assert!(q.self_state_override.is_none());
    }

    #[test]
    fn graph_query_with_self_state_override_sets_field() {
        use crate::graph::affect::SomaticFingerprint;
        let fp = SomaticFingerprint::from_array([0.5, 0.5, 0.5, 0.5, 0.0, 0.1, 0.0, 0.5]);
        let q = GraphQuery::new("hello").with_self_state_override(fp);
        assert_eq!(q.self_state_override, Some(fp));
    }

    #[test]
    fn memory_current_self_state_none_on_cold_start() {
        // Fresh Memory has no interoceptive signals → readback is None
        // so the orchestrator downgrades the affective plan rather than
        // ranking against a synthetic neutral state.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-cold-state.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None).expect("memory init");
        assert!(
            mem.current_self_state().is_none(),
            "cold-start Memory has no interoceptive signals"
        );
    }

    #[test]
    fn memory_current_self_state_some_after_signal_processed() {
        // After ingesting a signal the hub has a populated domain →
        // readback returns Some(fingerprint).
        use crate::interoceptive::types::{InteroceptiveSignal, SignalSource};
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-warm-state.db");
        let mut mem = Memory::new(db_path.to_str().unwrap(), None).expect("memory init");
        mem.interoceptive_hub_mut().process_signal(InteroceptiveSignal::new(
            SignalSource::Feedback,
            Some("coding".to_string()),
            0.6,
            0.4,
        ));
        let fp = mem
            .current_self_state()
            .expect("hub has signals, fingerprint should be Some");
        // Coding domain valence_trend is updated via EWMA from one sample,
        // and is the only domain → average follows that domain.
        assert!(
            fp.valence() > 0.0,
            "positive feedback should yield positive valence, got {}",
            fp.valence()
        );
    }

    #[test]
    fn graph_query_affective_with_override_against_empty_graph_does_not_panic() {
        // Smoke test: an Affective query with self_state_override set
        // routes through the orchestrator without panicking on the
        // self_state plumbing. Empty graph still returns a typed outcome.
        use crate::graph::affect::SomaticFingerprint;
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-affective-override.db");
        let graph_db = tmp.path().join("retrieval-api-affective-override.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        let fp = SomaticFingerprint::from_array([0.7, 0.3, 0.6, 0.6, 0.1, 0.2, 0.1, 0.5]);
        let q = GraphQuery::new("how do I feel about engram")
            .with_intent(Intent::Affective)
            .with_self_state_override(fp);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");
        assert_eq!(resp.plan_used, Intent::Affective);
        assert!(resp.results.is_empty(), "empty graph → no results");
        // Outcome should NOT be NoCognitiveState — the override carries a
        // valid fingerprint through to the plan. The exact downgrade
        // outcome (e.g. NoSeeds) depends on the affective plan's empty-
        // graph behavior; we only assert the override was honored.
        assert!(
            !matches!(resp.outcome, RetrievalOutcome::NoCognitiveState { .. }),
            "self_state_override should suppress NoCognitiveState; got {:?}",
            resp.outcome
        );
    }

    #[test]
    fn graph_query_affective_without_state_falls_back_to_associative() {
        // Cold-start Memory + no override + Affective intent → the plan
        // sees self_state == None and emits its DowngradedNoSelfState
        // marker. ISS-063 (2026-04-28): the orchestrator now runs
        // Associative as the §3.4 fallback. On an empty graph the
        // fallback also returns nothing → terminal `EmptyResultSet`
        // with reason `"affective_then_associative_empty"`. Pre-ISS-063
        // this returned `NoCognitiveState` directly; the cognitive_regression
        // driver should now check the reason string instead of the
        // legacy variant.
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("retrieval-api-affective-cold.db");
        let graph_db = tmp.path().join("retrieval-api-affective-cold.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        let q = GraphQuery::new("how do I feel about engram").with_intent(Intent::Affective);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");
        assert_eq!(resp.plan_used, Intent::Affective);
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "affective_then_associative_empty"
            ),
            "cold-start affective query on empty graph → EmptyResultSet \
             (affective_then_associative_empty); got {:?}",
            resp.outcome
        );
    }

    /// **Diagnostic test for ISS-063 (downgrade-to-fallback contract).**
    ///
    /// This test documents the *currently broken* behavior so it's
    /// visible from the test suite. Per design §3.4 / §6.4, an
    /// `Intent::Episodic` query with no time window should:
    ///   1. Have `EpisodicPlan` emit `DowngradedFromEpisodic`, AND
    ///   2. Have the orchestrator dispatch `Intent::Factual` as
    ///      fallback, returning the Factual plan's results.
    ///
    /// Today only step 1 happens. The orchestrator translates the
    /// downgrade into `RetrievalOutcome::DowngradedFromEpisodic` and
    /// returns *empty results*. This is the actual root cause of
    /// ISS-060 / ISS-061 in the LoCoMo conv-26 run.
    ///
    /// `#[ignore]` so CI stays green; ISS-063's fix flips the
    /// assertions and removes the attribute.
    ///
    /// **ISS-063 fixed:** Episodic with no time window → fallback to
    /// Associative (design §3.4). On an empty graph the fallback also
    /// produces nothing → `EmptyResultSet { reason:
    /// "episodic_then_associative_empty" }`. The `plan_used` is still
    /// the originally classified intent (Episodic) — what changed is
    /// the orchestrator no longer surfaces a bare `DowngradedFromEpisodic`
    /// with empty results; it ran the fallback and reports the path.
    #[test]
    fn iss063_episodic_no_window_falls_back_to_associative() {
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("iss063-episodic.db");
        let graph_db = tmp.path().join("iss063-episodic.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        let q = GraphQuery::new("what did I work on")
            .with_intent(Intent::Episodic);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");

        assert_eq!(
            resp.plan_used,
            Intent::Episodic,
            "plan_used reflects the dispatched primary plan, not the \
             fallback target (the fallback path is encoded in the \
             outcome reason)"
        );
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "episodic_then_associative_empty"
            ),
            "Episodic-empty + empty graph → EmptyResultSet \
             (episodic_then_associative_empty); got {:?}",
            resp.outcome
        );
        assert!(resp.results.is_empty(), "empty graph → no candidates");
    }

    /// **ISS-063:** Abstract with no L5 topics installed →
    /// `DowngradedL5Unavailable` from the plan → orchestrator runs
    /// Associative → empty graph → `EmptyResultSet { reason:
    /// "abstract_then_associative_empty" }`.
    #[test]
    fn iss063_abstract_no_l5_falls_back_to_associative() {
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("iss063-abstract.db");
        let graph_db = tmp.path().join("iss063-abstract.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        let q = GraphQuery::new("explain the architecture")
            .with_intent(Intent::Abstract);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");

        assert_eq!(resp.plan_used, Intent::Abstract);
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "abstract_then_associative_empty"
            ),
            "Abstract-empty + empty graph → EmptyResultSet \
             (abstract_then_associative_empty); got {:?}",
            resp.outcome
        );
        assert!(resp.results.is_empty(), "empty graph → no candidates");
    }

    /// **ISS-063:** Associative as the *primary* plan (not fallback) →
    /// empty graph → terminal `EmptyResultSet { reason:
    /// "associative_empty" }`. Replaces the dead-code path
    /// `Ok if !empty => Ok, _ => Ok` that hid empty results behind
    /// `Ok` and made `Ok` ambiguous.
    ///
    /// **Note on dispatch:** `Associative` is a `PlanKind` leaf, *not* an
    /// `Intent`. The classifier reaches `PlanKind::Associative` from
    /// `(Intent::Factual, DowngradeHint::Associative)` — i.e. queries
    /// with no strong signals (no entity, no time window, no topic, no
    /// mood). A bare `GraphQuery::new("anything")` with no
    /// `.with_intent()` and no entities/times/topics is exactly that
    /// path: classifier sees zero strong signals → `Intent::Factual` +
    /// `Associative` hint → `PlanKind::Associative` executes.
    /// `plan_used` still reports the *intent* (`Factual`), not the
    /// `PlanKind`; the distinguishing signal is the reason string
    /// `"associative_empty"` (only the Associative leaf emits this).
    #[test]
    fn iss063_associative_direct_empty_returns_empty_result_set() {
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("iss063-associative.db");
        let graph_db = tmp.path().join("iss063-associative.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        // No `.with_intent()` and no entities/times/topics → classifier
        // produces zero strong signals → (Intent::Factual,
        // DowngradeHint::Associative) → PlanKind::Associative.
        let q = GraphQuery::new("anything");
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");

        // plan_used reports the dispatched *intent*, which is Factual
        // (the intent that carries the Associative downgrade hint).
        // The Associative leaf is identified by the reason string below.
        assert_eq!(resp.plan_used, Intent::Factual);
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "associative_empty"
            ),
            "Associative direct + empty graph → EmptyResultSet \
             (associative_empty); got {:?}",
            resp.outcome
        );
        assert!(resp.results.is_empty(), "empty graph → no candidates");
    }

    /// **ISS-063 → ISS-083:** Hybrid with no signals → no sub-plans
    /// selected → empty `scored` → orchestrator runs Factual fallback
    /// (ISS-083). Substrate is empty, so Factual *also* returns empty,
    /// and the terminal outcome is
    /// `EmptyResultSet { reason: "hybrid_subplans_empty_factual_also_empty" }`.
    /// Replaces the dead-code path `if empty { Ok } else { Ok }`.
    #[test]
    fn iss063_hybrid_all_empty_returns_empty_result_set() {
        use crate::retrieval::classifier::Intent;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("iss063-hybrid.db");
        let graph_db = tmp.path().join("iss063-hybrid.graph.db");
        let mem = Memory::new(db_path.to_str().unwrap(), None)
            .expect("memory init")
            .with_graph_store(&graph_db)
            .expect("install graph store");

        // Caller-override Hybrid skips classifier (signal_scores=None
        // in execute_plan) → all-zero signals → 0 sub-plans selected
        // → empty scored. ISS-083: orchestrator now runs Factual
        // fallback; with empty substrate Factual is also empty, so we
        // terminate with `hybrid_subplans_empty_factual_also_empty`.
        let q = GraphQuery::new("anything").with_intent(Intent::Hybrid);
        let resp = block_on(mem.graph_query(q)).expect("orchestrator runs");

        assert_eq!(resp.plan_used, Intent::Hybrid);
        assert!(
            matches!(
                resp.outcome,
                RetrievalOutcome::EmptyResultSet { ref reason }
                    if reason == "hybrid_subplans_empty_factual_also_empty"
            ),
            "Hybrid all-empty + Factual fallback empty → EmptyResultSet \
             (hybrid_subplans_empty_factual_also_empty); got {:?}",
            resp.outcome
        );
        assert!(resp.results.is_empty(), "no sub-plans → no candidates");
    }
}
