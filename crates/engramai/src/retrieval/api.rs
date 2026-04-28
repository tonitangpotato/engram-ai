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
        }
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

        // Stage A: dispatch.
        let classifier =
            crate::retrieval::classifier::HeuristicClassifier::with_null_lookup();
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

        let (candidates, outcome) = self.with_graph_read(|graph| {
            crate::retrieval::orchestrator::execute_plan(
                dispatched, graph, &loader, self_state,
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

    #[test]
    fn scored_result_score_dispatches_by_variant() {
        let mem = ScoredResult::Memory {
            record: MemoryRecord {
                id: "m1".into(),
                content: "x".into(),
                memory_type: crate::types::MemoryType::Factual,
                layer: crate::types::MemoryLayer::Working,
                created_at: chrono::Utc::now(),
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
        assert!(
            matches!(resp.outcome, RetrievalOutcome::NoEntityFound { .. }),
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
    fn graph_query_affective_without_state_yields_no_cognitive_state() {
        // Cold-start Memory + no override + Affective intent → the plan
        // sees self_state == None and returns NoCognitiveState per §6.2.
        // This is the contract the cognitive_regression driver checks
        // against to detect regressions to a pure stub.
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
            matches!(resp.outcome, RetrievalOutcome::NoCognitiveState { .. }),
            "cold-start affective query → NoCognitiveState; got {:?}",
            resp.outcome
        );
    }
}
