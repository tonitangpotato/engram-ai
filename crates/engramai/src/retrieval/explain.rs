//! # Explain trace — design §6.3 / GOAL-3.11
//!
//! The opt-in observability bundle returned in
//! [`crate::retrieval::api::GraphQueryResponse::trace`] when
//! `GraphQuery::explain == true`. Every intermediate score, every
//! downgrade, every stage timing flows through here and serializes
//! deterministically to JSON for debugging, benchmark diffing, and
//! caller-side logging.
//!
//! ## Variant catalogue (design §6.3)
//!
//! [`PlanTrace`] aggregates seven sub-traces:
//!
//! - [`ClassifierTrace`] — signal scores, chosen intent, method,
//!   optional `LlmCost` (Stage-2 fallback).
//! - [`PlanDetail`] — per-plan steps, per-step timings, cost-cap hits
//!   recorded by the [`BudgetController`](crate::retrieval::budget).
//! - `Vec<Downgrade>` — every §3.4 / §4.*.2 / §4.4 downgrade fired
//!   along the path, with `from`, `to`, and a stable snake_case
//!   `reason` slug.
//! - [`FusionTrace`] — per-plan fusion weights + per-candidate
//!   sub-scores + any renormalization triggered by the
//!   "Missing signal normalization" rule (§5.2).
//! - [`BiTemporalTrace`] (optional) — as-of-T projection details
//!   (§4.6) when the query carries `as_of` or `include_superseded`.
//! - [`AffectTrace`] (optional) — GOAL-3.8 rank-divergence metric
//!   when the Affective plan ran with sampling enabled (§4.5 step 5).
//! - `total_latency` and `per_stage_latency` — wall-clock captured
//!   by the orchestrator using [`Stage`](crate::retrieval::budget::Stage).
//!
//! ## Cost guarantee (GOAL-3.11)
//!
//! Trace assembly happens **only** when `query.explain == true`. The
//! production hot path (`explain == false`) MUST NOT pay any of the
//! cost recorded here — no allocation, no clone, no
//! `Instant::now()` for fields that would never be read. The
//! orchestrator pattern is:
//!
//! ```ignore
//! let mut trace = if query.explain {
//!     Some(PlanTraceBuilder::new())
//! } else {
//!     None
//! };
//!
//! // ... per stage:
//! if let Some(b) = trace.as_mut() {
//!     b.record_stage(Stage::EntityResolution, t_elapsed);
//! }
//!
//! let resp = GraphQueryResponse {
//!     trace: trace.map(PlanTraceBuilder::finish),
//!     // ...
//! };
//! ```
//!
//! [`PlanTraceBuilder::record_stage`] / `record_downgrade` /
//! `set_classifier` / `set_fusion` / `set_bi_temporal` /
//! `set_affect` / `record_hybrid_truncated` / `set_total_latency`
//! are the only mutators; each is a thin push / setter so the
//! `if let Some(b) = ...` overhead is the only thing the hot path
//! pays when explain is off.
//!
//! ## Spec references
//!
//! - Design §6.3 — `PlanTrace` + sub-traces shape.
//! - Design §8.1 — Prometheus metrics (always-on, separate from
//!   trace; wired by `task:retr-impl-metrics` / T15).
//! - Design §8.2 — traces (opt-in, owned here).
//! - GOAL-3.2 — `classifier_method` MUST be observable.
//! - GOAL-3.8 — Kendall-tau divergence sampled on Affective.
//! - GOAL-3.11 — opt-in trace, zero hot-path cost when off.
//! - GOAL-3.13 — classifier LLM cost counted independently of
//!   resolution / compiler counters; recorded in
//!   [`ClassifierTrace::llm_cost`].
//!
//! ## Versioning posture
//!
//! Sub-trace structs are intentionally **not** `#[non_exhaustive]` —
//! they are JSON-serialized observability payloads, and adding a new
//! field is the *normal* additive change. Callers that consume
//! `PlanTrace` JSON should tolerate unknown fields (serde does so by
//! default with `#[serde(deny_unknown_fields)]` *not* set, which is
//! the case here).

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::retrieval::api::{EntityId, SubScores};
use crate::retrieval::plans::hybrid::DroppedSignal;

// ---------------------------------------------------------------------------
// LlmCost — design §6.2a / GOAL-3.13
// ---------------------------------------------------------------------------

/// Token / latency / call-count tally for a Stage-2 classifier LLM
/// fallback (design §6.2a, GOAL-3.13).
///
/// **Boundary contract:** this struct counts **only** classifier
/// Stage-2 LLM calls. Resolution-pipeline LLM calls (write path,
/// stages 3/4/5) and Knowledge Compiler L5 synthesis calls live in
/// the resolution stats surface and never appear here. Cost
/// independence is the GOAL-3.13 invariant.
///
/// Fields use `usize` for counts/tokens (saturating arithmetic on
/// overflow is safe — token totals far exceed 2^32 only for batch
/// jobs, which the retrieval read path is not). Duration is captured
/// at the moment the orchestrator finishes consuming the LLM
/// response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmCost {
    /// Number of classifier LLM invocations issued for this query.
    /// Almost always 0 (rule-only) or 1 (single Stage-2 call).
    /// Higher values can occur if the orchestrator re-tries on a
    /// transient provider error before falling back to
    /// `HeuristicTimeout`.
    pub calls: usize,

    /// Sum of prompt tokens billed across `calls`.
    pub prompt_tokens: usize,

    /// Sum of completion tokens billed across `calls`.
    pub completion_tokens: usize,

    /// Cumulative wall-clock time spent waiting on the provider.
    /// Excludes our local serialization / deserialization overhead;
    /// pairs with the `retrieval_classifier_llm_duration_seconds`
    /// histogram (§8.1).
    #[serde(with = "duration_secs_f64")]
    pub duration: Duration,
}

impl LlmCost {
    /// Construct a zero-cost record (rule-only path; the field is
    /// `Some(LlmCost::default())` only when an attempt was *made*
    /// — use `None` if Stage 2 was never invoked).
    pub fn zero() -> Self {
        Self::default()
    }

    /// Total tokens billed = `prompt + completion`. Convenience for
    /// metrics emission and per-query cost summaries.
    pub fn total_tokens(&self) -> usize {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

// ---------------------------------------------------------------------------
// SignalScoreSnapshot — observability projection of classifier scores
// ---------------------------------------------------------------------------

/// Lightweight `Serialize`-friendly snapshot of the classifier
/// signal scores at decision time.
///
/// The classifier's internal `SignalScores` type is currently `bool`-
/// or `f64`-valued (heuristic emits binary signals today, but
/// stage-2 may emit continuous scores). We record everything as
/// `f64` so the trace JSON has a single shape regardless of
/// signal-evolution. `0.0` / `1.0` reproduce binary outcomes
/// faithfully.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SignalScoreSnapshot {
    pub entity: f64,
    pub temporal: f64,
    #[serde(rename = "abstract")]
    pub abstract_signal: f64,
    pub affective: f64,
}

impl SignalScoreSnapshot {
    /// Construct a snapshot from individual signal magnitudes.
    /// Callers are responsible for clamping to `[0, 1]`; we don't
    /// re-clamp here so traces preserve any out-of-range value the
    /// classifier emitted (which itself is a debuggability signal).
    pub fn new(entity: f64, temporal: f64, abstract_signal: f64, affective: f64) -> Self {
        Self {
            entity,
            temporal,
            abstract_signal,
            affective,
        }
    }
}

// ---------------------------------------------------------------------------
// ClassifierTrace
// ---------------------------------------------------------------------------

/// Provenance of the chosen plan — design §6.3 (`PlanTrace.classifier`).
///
/// Carries the per-signal score snapshot, the chosen `intent` (as a
/// stable lowercase string for JSON), the `method` slug
/// (`heuristic` | `llm_fallback` | `heuristic_timeout` |
/// `caller_override`), and an optional [`LlmCost`] populated only
/// when Stage-2 ran.
///
/// Strings are used for `intent` / `method` (instead of the upstream
/// `Intent` / `ClassifierMethod` enums) so the trace can serialize
/// without forcing `Serialize` derives on the classifier surface
/// (those enums are part of the runtime API, not an observability
/// contract). The classifier's own `Intent::as_str` /
/// `ClassifierMethod`-equivalent helpers feed these fields.
///
/// The `downgrade_hint` slug records whether the classifier nudged
/// the orchestrator toward Associative even when `intent` was a
/// concrete plan (§3.4 lattice).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ClassifierTrace {
    /// Per-signal magnitudes at decision time.
    pub signal_scores: SignalScoreSnapshot,

    /// Stable slug for the routed intent
    /// (`factual|episodic|abstract|affective|hybrid`).
    pub intent: String,

    /// Stable slug for the routing method
    /// (`heuristic|llm_fallback|heuristic_timeout|caller_override`).
    pub method: String,

    /// Stable slug for the downgrade nudge
    /// (`none|associative`). Kept verbose (rather than `Option<_>`)
    /// because the trace consumer wants a single-shape field.
    pub downgrade_hint: String,

    /// `Some(_)` iff Stage-2 was attempted (success **or** timeout).
    /// `None` on the rule-only path. GOAL-3.13: this is the only
    /// place classifier LLM cost is recorded in retrieval traces.
    pub llm_cost: Option<LlmCost>,
}

impl ClassifierTrace {
    /// Construct from already-stable string slugs. Callers are
    /// expected to feed the upstream `Intent::as_str()` and a
    /// matching `ClassifierMethod` slug helper.
    pub fn new(
        signal_scores: SignalScoreSnapshot,
        intent: impl Into<String>,
        method: impl Into<String>,
        downgrade_hint: impl Into<String>,
    ) -> Self {
        Self {
            signal_scores,
            intent: intent.into(),
            method: method.into(),
            downgrade_hint: downgrade_hint.into(),
            llm_cost: None,
        }
    }

    /// Builder: attach LLM cost (Stage-2 path).
    pub fn with_llm_cost(mut self, cost: LlmCost) -> Self {
        self.llm_cost = Some(cost);
        self
    }
}

// ---------------------------------------------------------------------------
// PerStageTiming — `(stage_slug, duration)` pair for JSON friendliness
// ---------------------------------------------------------------------------

/// Single per-stage latency entry.
///
/// We serialize `Duration` as `f64` seconds (via
/// [`duration_secs_f64`]) so the JSON shape is stable across
/// languages and prometheus scrape compatible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerStageTiming {
    /// Stable lowercase stage tag (mirrors
    /// [`crate::retrieval::budget::Stage::as_str`]).
    pub stage: String,
    #[serde(with = "duration_secs_f64")]
    pub duration: Duration,
}

impl PerStageTiming {
    pub fn new(stage: impl Into<String>, duration: Duration) -> Self {
        Self {
            stage: stage.into(),
            duration,
        }
    }
}

// ---------------------------------------------------------------------------
// PlanDetail
// ---------------------------------------------------------------------------

/// Per-plan execution detail — design §6.3 (`PlanTrace.plan`).
///
/// Captures *what the executing plan did*: which step ran, how long
/// it took, and which cost caps (if any) tripped during the run.
/// The actual plan kind is the `plan_used` field on
/// `GraphQueryResponse` — `PlanDetail` records the *internals* of
/// that plan's execution.
///
/// `cost_caps_hit` carries the slug list returned by
/// [`BudgetController::cost_caps_hit`](crate::retrieval::budget::BudgetController::cost_caps_hit)
/// (each [`CostCap`](crate::retrieval::budget::CostCap) variant has a
/// stable `as_str()` slug).
///
/// `cutoff_reason` is `Some(slug)` if the plan returned partial
/// results because budget tripped; `None` on a clean run. Slugs
/// are snake_case constants (`outer_deadline`, `stage_deadline`,
/// `cost_cap_hit`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlanDetail {
    /// Per-step latency, ordered by execution order. Each entry's
    /// `stage` slug is meaningful within the executing plan only;
    /// distinct plans may emit overlapping slugs (`fusion`, `rerank`)
    /// — that's fine, they are scoped to this `PlanDetail`.
    pub steps: Vec<PerStageTiming>,

    /// Stable slugs for any cost caps reached during the run.
    pub cost_caps_hit: Vec<String>,

    /// `Some(snake_case_reason)` if the plan returned partial
    /// results because budget tripped. `None` on a clean run.
    pub cutoff_reason: Option<String>,

    /// Free-form notes the plan wants surfaced in the trace
    /// (e.g. "factual: 3 anchors resolved, 0 edges survived
    /// projection — falling through to associative fallback").
    /// Plans should keep these short and human-friendly.
    pub notes: Vec<String>,
}

impl PlanDetail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_step(mut self, stage: impl Into<String>, duration: Duration) -> Self {
        self.steps.push(PerStageTiming::new(stage, duration));
        self
    }

    pub fn with_cost_cap_hit(mut self, cap: impl Into<String>) -> Self {
        self.cost_caps_hit.push(cap.into());
        self
    }

    pub fn with_cutoff_reason(mut self, reason: impl Into<String>) -> Self {
        self.cutoff_reason = Some(reason.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Downgrade
// ---------------------------------------------------------------------------

/// Single downgrade entry — design §6.3 (`PlanTrace.downgrades`).
///
/// Each entry records *one* downgrade hop on the path from the
/// classified intent to the actually-executed plan. A query may
/// downgrade more than once (e.g., `Hybrid → Associative` if both
/// sub-plans degrade) — the trace records every hop in order.
///
/// `from` and `to` are stable lowercase plan slugs
/// (`factual|episodic|abstract|affective|hybrid|associative`). The
/// `reason` slug uses snake_case, drawn from a documented vocabulary
/// per plan: see `RetrievalOutcome::DowngradedFromAbstract` /
/// `DowngradedFromEpisodic` doc comments for examples
/// (`l5_unavailable`, `no_time_expression`, etc.). Free-form so
/// plans can evolve their downgrade vocabulary without breaking
/// trace consumers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Downgrade {
    pub from: String,
    pub to: String,
    pub reason: String,
}

impl Downgrade {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            reason: reason.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// FusionTrace
// ---------------------------------------------------------------------------

/// Per-candidate fusion record — design §6.3 (`FusionTrace.candidates`).
///
/// Surfaces the per-candidate sub-scores and final fused score so
/// callers (and benchmark diffs) can pinpoint why a candidate ranked
/// where it did. `id` is the stable `MemoryId` / `EntityId` /
/// topic-id rendered as a string; `kind` distinguishes "memory" from
/// "topic" so consumers can join back to `ScoredResult`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FusionCandidate {
    /// Stable string id of the fused result (`MemoryId` for memory
    /// candidates, `KnowledgeTopic.id` for topics).
    pub id: String,

    /// `"memory"` | `"topic"` — kind tag matching `ScoredResult`.
    pub kind: String,

    /// Per-signal sub-scores feeding the fused score.
    pub sub_scores: SubScores,

    /// Final fused score after weighting + (optional) renormalization.
    pub fused_score: f64,
}

/// Fusion observability — design §6.3 (`PlanTrace.fusion`).
///
/// Records the per-plan weights actually applied (which may differ
/// from the configured weights after "Missing signal normalization"
/// — §5.2), the renormalization scale factor (`1.0` if none), the
/// ranked candidate list, and any `DroppedSignal` entries from the
/// Hybrid plan's truncation step (§4.7).
///
/// `weights` is rendered as a `Vec<(name, weight)>` instead of a
/// `HashMap` so ordering is stable in JSON output (deterministic
/// trace diffs are a §5.4 reproducibility requirement).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FusionTrace {
    /// Stable plan slug whose weight matrix row was used
    /// (`factual|episodic|abstract|affective|hybrid|associative`).
    pub plan: String,

    /// Per-signal weights `(signal_slug, weight)` ordered alphabetically
    /// by signal name. Includes only signals with non-zero weight
    /// after renormalization.
    pub weights: Vec<(String, f64)>,

    /// Scale factor applied to renormalize when one or more signals
    /// were missing for a candidate (§5.2). `1.0` means "no
    /// renormalization triggered for this plan call". Per-candidate
    /// renorms are not individually surfaced — the value here is the
    /// *plan-level* renorm decision (binary today: applied or not).
    pub renorm_scale: f64,

    /// Whether at least one candidate triggered renormalization. Pairs
    /// with `renorm_scale` for diagnosability when renorm fires
    /// inconsistently across candidates.
    pub renorm_applied: bool,

    /// Ranked candidate list with sub-scores. Order matches the
    /// final response ordering (descending fused score, with
    /// deterministic tie-break per §5.4).
    pub candidates: Vec<FusionCandidate>,

    /// Hybrid-plan truncation telemetry (§4.7). Empty for non-Hybrid
    /// plans. Each entry's `kind` is a stable lowercase signal slug
    /// (`entity|temporal|abstract|affective`).
    pub hybrid_truncated: Vec<HybridTruncatedEntry>,
}

/// Trace-friendly projection of [`DroppedSignal`].
///
/// We snapshot to a stable string + score pair so the trace JSON is
/// independent of `SignalKind`'s `Serialize` derivation status. The
/// orchestrator converts `DroppedSignal` → `HybridTruncatedEntry` at
/// trace assembly time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HybridTruncatedEntry {
    /// Lowercase signal kind (`entity|temporal|abstract|affective`).
    pub kind: String,
    /// Score that the dropped signal carried at decision time.
    pub score: f64,
}

impl HybridTruncatedEntry {
    /// Convert a [`DroppedSignal`] into a serializable trace entry.
    /// `SignalKind` enum has a stable lowercase rendering via its
    /// `as_str()` method (in `classifier::heuristic`).
    pub fn from_dropped(d: DroppedSignal) -> Self {
        // SignalKind is Copy + has a known small variant set; we
        // map the four declared variants to their lowercase slugs.
        let kind = match d.kind {
            crate::retrieval::classifier::heuristic::SignalKind::Entity => "entity",
            crate::retrieval::classifier::heuristic::SignalKind::Temporal => "temporal",
            crate::retrieval::classifier::heuristic::SignalKind::Abstract => "abstract",
            crate::retrieval::classifier::heuristic::SignalKind::Affective => "affective",
        };
        Self {
            kind: kind.to_string(),
            score: d.score,
        }
    }
}

// ---------------------------------------------------------------------------
// BiTemporalTrace
// ---------------------------------------------------------------------------

/// Bi-temporal projection record — design §6.3 (`PlanTrace.bi_temporal`).
///
/// Populated only when the query carries `as_of: Some(_)`,
/// `include_superseded: true`, or both. Records the projection
/// instant actually used, whether superseded edges were included,
/// and how many edges were filtered out by each clause (so callers
/// can spot a projection that aggressively pruned the graph).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BiTemporalTrace {
    /// Projection instant; `None` means "now" was used.
    pub as_of: Option<DateTime<Utc>>,

    /// Whether `include_superseded` was opted in (GOAL-3.5).
    pub include_superseded: bool,

    /// Edges filtered out because their `valid_from > as_of` or
    /// `valid_to <= as_of` (with the half-open convention from
    /// `v03-graph-layer/design.md` §4.6).
    pub edges_filtered_validity: usize,

    /// Edges filtered because they were superseded **and**
    /// `include_superseded == false`.
    pub edges_filtered_superseded: usize,

    /// Edges that survived projection and entered scoring.
    pub edges_survived: usize,
}

// ---------------------------------------------------------------------------
// AffectTrace
// ---------------------------------------------------------------------------

/// Affect-divergence record — design §6.3 (`PlanTrace.affect`),
/// GOAL-3.8.
///
/// Populated when the Affective plan's sample-rate gate
/// (`affect_divergence_sample_rate`, default `0.01`) fires for this
/// query — or unconditionally when `query.explain == true` per
/// §4.5 step 5. Captures the Kendall-tau correlation between the
/// active-state ranking and the neutral-state ranking computed
/// alongside it; the GOAL-3.8 acceptance criterion asserts τ < 0.9
/// on the benchmark set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AffectTrace {
    /// Kendall-tau correlation in `[-1.0, 1.0]`. The acceptance
    /// gate (GOAL-3.8) wants this `< 0.9` on the benchmark.
    pub kendall_tau: f64,

    /// Number of candidates that participated in the comparison
    /// (top-K from the Affective plan's local pool; not the full
    /// associative seed set). Useful for understanding tau's
    /// statistical weight per query.
    pub k_compared: usize,

    /// Whether this trace entry came from the sample-rate gate
    /// (`true` = sampled production query) or from
    /// `query.explain == true` (which always computes divergence).
    /// Lets benchmark drivers distinguish "real" samples from
    /// explain-driven measurements.
    pub from_sample_gate: bool,
}

// ---------------------------------------------------------------------------
// PlanTrace
// ---------------------------------------------------------------------------

/// Opt-in observability bundle — design §6.3 / GOAL-3.11.
///
/// Returned in [`crate::retrieval::api::GraphQueryResponse::trace`]
/// when `GraphQuery::explain == true`. Default `false` keeps the
/// production hot path overhead-free. See module docs for the cost
/// guarantee and the recommended `if let Some(b) = trace.as_mut()`
/// pattern.
///
/// All fields use owned types (`String`, `Vec<_>`, `Option<_>`) so
/// the trace can outlive the orchestrator's borrow stack and be
/// serialized freely — there is no shared state between traces.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlanTrace {
    /// Classifier provenance — signal scores, intent, method,
    /// optional LLM cost.
    pub classifier: ClassifierTrace,

    /// Per-plan execution detail — steps, cost-cap hits, cutoff,
    /// notes.
    pub plan: PlanDetail,

    /// Every downgrade hop along the path from classified intent
    /// to executing plan. Empty on the no-downgrade path.
    pub downgrades: Vec<Downgrade>,

    /// Fusion record — weights, renormalization, ranked candidates,
    /// hybrid truncation.
    pub fusion: FusionTrace,

    /// Bi-temporal projection details (§4.6). `None` if the query
    /// did not exercise as-of-T or include-superseded.
    pub bi_temporal: Option<BiTemporalTrace>,

    /// Affect-divergence record (GOAL-3.8). `None` outside Affective
    /// plan runs and outside sampled-or-explain mode.
    pub affect: Option<AffectTrace>,

    /// Total wall-clock observed by the orchestrator from
    /// `graph_query` entry to response assembly.
    #[serde(with = "duration_secs_f64")]
    pub total_latency: Duration,

    /// Coarse-grained per-stage latencies recorded by the
    /// orchestrator (separate from `plan.steps`, which is
    /// plan-internal). Each entry's `stage` slug is a
    /// `Stage::as_str()` value.
    pub per_stage_latency: Vec<PerStageTiming>,

    /// Anchor entities resolved by the Factual / Hybrid plans.
    /// Empty for plans that don't resolve anchors. Surfaced
    /// separately from `plan.notes` because callers query against
    /// it programmatically (e.g., "did anchor X get resolved?").
    pub anchors: Vec<EntityId>,
}

// ---------------------------------------------------------------------------
// PlanTraceBuilder
// ---------------------------------------------------------------------------

/// Mutator surface for opt-in trace assembly.
///
/// The orchestrator wraps a `PlanTraceBuilder` in
/// `Option<PlanTraceBuilder>` and gates every mutation with
/// `if let Some(b) = trace.as_mut()` so the `explain == false` path
/// pays nothing beyond the `Option::is_some` check (LLVM elides this
/// in release builds when the surrounding closure is monomorphized).
///
/// Each mutator is a thin push / setter — *no* hidden allocation
/// behind opt-in flags, *no* `Instant::now()` unless the caller
/// supplies the duration. The hot-path cost guarantee depends on
/// the builder NOT calling out to the system clock or doing extra
/// work; that is the orchestrator's responsibility.
#[derive(Debug, Default)]
pub struct PlanTraceBuilder {
    inner: PlanTrace,
}

impl PlanTraceBuilder {
    /// Construct an empty builder. The orchestrator instantiates
    /// this only when `query.explain == true`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the classifier sub-trace.
    pub fn set_classifier(&mut self, classifier: ClassifierTrace) -> &mut Self {
        self.inner.classifier = classifier;
        self
    }

    /// Set the plan-detail sub-trace.
    pub fn set_plan_detail(&mut self, plan: PlanDetail) -> &mut Self {
        self.inner.plan = plan;
        self
    }

    /// Push one downgrade hop.
    pub fn record_downgrade(&mut self, downgrade: Downgrade) -> &mut Self {
        self.inner.downgrades.push(downgrade);
        self
    }

    /// Set the fusion sub-trace.
    pub fn set_fusion(&mut self, fusion: FusionTrace) -> &mut Self {
        self.inner.fusion = fusion;
        self
    }

    /// Set the bi-temporal sub-trace (`Some(_)`).
    pub fn set_bi_temporal(&mut self, bt: BiTemporalTrace) -> &mut Self {
        self.inner.bi_temporal = Some(bt);
        self
    }

    /// Set the affect divergence sub-trace (`Some(_)`).
    pub fn set_affect(&mut self, affect: AffectTrace) -> &mut Self {
        self.inner.affect = Some(affect);
        self
    }

    /// Push one orchestrator-level stage timing entry.
    pub fn record_stage(&mut self, stage: impl Into<String>, duration: Duration) -> &mut Self {
        self.inner
            .per_stage_latency
            .push(PerStageTiming::new(stage, duration));
        self
    }

    /// Set the total wall-clock latency.
    pub fn set_total_latency(&mut self, total: Duration) -> &mut Self {
        self.inner.total_latency = total;
        self
    }

    /// Push one resolved anchor entity.
    pub fn record_anchor(&mut self, entity: EntityId) -> &mut Self {
        self.inner.anchors.push(entity);
        self
    }

    /// Push one Hybrid-plan truncation entry into the fusion
    /// sub-trace. (`PlanTraceBuilder::set_fusion` must have been
    /// called first if the caller wants `weights` / `candidates`
    /// preserved; calling this on a default fusion is fine — just
    /// produces a fusion struct with only `hybrid_truncated`
    /// populated.)
    pub fn record_hybrid_truncated(&mut self, entry: HybridTruncatedEntry) -> &mut Self {
        self.inner.fusion.hybrid_truncated.push(entry);
        self
    }

    /// Consume the builder and return the assembled trace.
    pub fn finish(self) -> PlanTrace {
        self.inner
    }
}

// ---------------------------------------------------------------------------
// duration_secs_f64 — serde helper for stable JSON shape
// ---------------------------------------------------------------------------

/// Serialize/deserialize a `Duration` as `f64` seconds.
///
/// Trace JSON consumers (benchmark-diff scripts, prometheus
/// translators, debugging dashboards) expect a numeric duration —
/// not the `{ secs: u64, nanos: u32 }` shape `serde`'s default
/// `Duration` impl emits. We funnel every `Duration` field through
/// this helper so the JSON shape is uniform.
pub mod duration_secs_f64 {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(d: &Duration, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_f64(d.as_secs_f64())
    }

    pub fn deserialize<'de, D>(d: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = f64::deserialize(d)?;
        if !secs.is_finite() || secs < 0.0 {
            return Err(serde::de::Error::custom(
                "duration must be a finite non-negative f64 seconds value",
            ));
        }
        Ok(Duration::from_secs_f64(secs))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::api::SubScores;
    use crate::retrieval::classifier::heuristic::SignalKind;

    // -----------------------------------------------------------------
    // PlanTraceBuilder — cost guarantee shape (GOAL-3.11)
    // -----------------------------------------------------------------

    #[test]
    fn default_plantrace_serializes_to_empty_shape() {
        let trace = PlanTrace::default();
        let json = serde_json::to_string(&trace).expect("default serializes");
        // Sanity: present fields all default; total_latency renders as 0.0.
        assert!(json.contains("\"total_latency\":0.0"));
        assert!(json.contains("\"downgrades\":[]"));
        assert!(json.contains("\"per_stage_latency\":[]"));
        assert!(json.contains("\"anchors\":[]"));
    }

    #[test]
    fn builder_assembles_full_trace() {
        let mut b = PlanTraceBuilder::new();
        b.set_classifier(
            ClassifierTrace::new(
                SignalScoreSnapshot::new(1.0, 0.0, 0.0, 0.0),
                "factual",
                "heuristic",
                "none",
            )
            .with_llm_cost(LlmCost {
                calls: 1,
                prompt_tokens: 100,
                completion_tokens: 20,
                duration: Duration::from_millis(180),
            }),
        )
        .set_plan_detail(
            PlanDetail::new()
                .with_step("entity_resolution", Duration::from_micros(120))
                .with_step("edge_traversal", Duration::from_micros(450))
                .with_cost_cap_hit("max_anchors")
                .with_note("3 anchors resolved"),
        )
        .record_downgrade(Downgrade::new("episodic", "associative", "no_time_expression"))
        .set_fusion(FusionTrace {
            plan: "factual".into(),
            weights: vec![("graph".into(), 0.6), ("vector".into(), 0.4)],
            renorm_scale: 1.0,
            renorm_applied: false,
            candidates: vec![FusionCandidate {
                id: "abc".into(),
                kind: "memory".into(),
                sub_scores: SubScores {
                    vector_score: Some(0.8),
                    graph_score: Some(0.9),
                    ..Default::default()
                },
                fused_score: 0.84,
            }],
            hybrid_truncated: vec![],
        })
        .set_bi_temporal(BiTemporalTrace {
            as_of: None,
            include_superseded: false,
            edges_filtered_validity: 2,
            edges_filtered_superseded: 0,
            edges_survived: 5,
        })
        .set_affect(AffectTrace {
            kendall_tau: 0.42,
            k_compared: 10,
            from_sample_gate: false,
        })
        .record_stage("entity_resolution", Duration::from_micros(120))
        .record_stage("fusion", Duration::from_micros(50))
        .set_total_latency(Duration::from_millis(2));

        let trace = b.finish();
        assert_eq!(trace.classifier.intent, "factual");
        assert_eq!(trace.classifier.method, "heuristic");
        assert_eq!(trace.classifier.llm_cost.as_ref().unwrap().calls, 1);
        assert_eq!(trace.plan.steps.len(), 2);
        assert_eq!(trace.plan.cost_caps_hit, vec!["max_anchors"]);
        assert_eq!(trace.downgrades.len(), 1);
        assert_eq!(trace.downgrades[0].reason, "no_time_expression");
        assert_eq!(trace.fusion.candidates.len(), 1);
        assert!(trace.bi_temporal.is_some());
        assert!(trace.affect.is_some());
        assert_eq!(trace.per_stage_latency.len(), 2);
        assert_eq!(trace.total_latency, Duration::from_millis(2));
    }

    #[test]
    fn builder_default_when_explain_off_path_pays_nothing() {
        // We can't test runtime overhead, but we can assert the
        // `Option::is_some` gate logic compiles + the no-op case
        // produces a default trace if the caller forgets to feed it.
        let mut maybe: Option<PlanTraceBuilder> = None;
        if let Some(b) = maybe.as_mut() {
            b.set_total_latency(Duration::from_secs(99));
        }
        assert!(maybe.is_none());
    }

    // -----------------------------------------------------------------
    // JSON serialization shape — design §6.3 stability
    // -----------------------------------------------------------------

    #[test]
    fn duration_serializes_as_f64_seconds() {
        let pst = PerStageTiming::new("fusion", Duration::from_millis(1500));
        let json = serde_json::to_string(&pst).unwrap();
        assert!(json.contains("\"duration\":1.5"), "got: {json}");
    }

    #[test]
    fn duration_round_trips_through_json() {
        let original = PerStageTiming::new("rerank", Duration::from_micros(123_456));
        let s = serde_json::to_string(&original).unwrap();
        let back: PerStageTiming = serde_json::from_str(&s).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn duration_rejects_negative_or_nan_on_deserialize() {
        let bad_neg = "{\"stage\":\"x\",\"duration\":-1.0}";
        assert!(serde_json::from_str::<PerStageTiming>(bad_neg).is_err());
        let bad_nan = "{\"stage\":\"x\",\"duration\":\"NaN\"}";
        // serde_json doesn't natively parse "NaN" string into f64;
        // negative is the canonical bad case we surface in errors.
        assert!(serde_json::from_str::<PerStageTiming>(bad_nan).is_err());
    }

    #[test]
    fn signal_score_snapshot_renames_abstract() {
        let s = SignalScoreSnapshot::new(0.1, 0.2, 0.3, 0.4);
        let json = serde_json::to_string(&s).unwrap();
        // `abstract` is a Rust keyword; JSON field uses unaliased form.
        assert!(json.contains("\"abstract\":0.3"), "got: {json}");
        assert!(!json.contains("abstract_signal"));
    }

    #[test]
    fn full_plantrace_round_trips_through_json() {
        let mut b = PlanTraceBuilder::new();
        b.set_classifier(ClassifierTrace::new(
            SignalScoreSnapshot::new(1.0, 0.0, 0.0, 0.0),
            "factual",
            "heuristic",
            "none",
        ))
        .set_total_latency(Duration::from_millis(7));
        let trace = b.finish();
        let s = serde_json::to_string(&trace).unwrap();
        let back: PlanTrace = serde_json::from_str(&s).unwrap();
        assert_eq!(back, trace);
    }

    // -----------------------------------------------------------------
    // LlmCost
    // -----------------------------------------------------------------

    #[test]
    fn llm_cost_total_tokens_saturates() {
        let c = LlmCost {
            calls: 1,
            prompt_tokens: usize::MAX - 5,
            completion_tokens: 100,
            duration: Duration::ZERO,
        };
        // saturating add — does not panic.
        assert_eq!(c.total_tokens(), usize::MAX);
    }

    #[test]
    fn llm_cost_zero_is_default() {
        assert_eq!(LlmCost::zero(), LlmCost::default());
    }

    // -----------------------------------------------------------------
    // HybridTruncatedEntry — bridge from DroppedSignal
    // -----------------------------------------------------------------

    #[test]
    fn hybrid_truncated_entry_from_dropped_maps_kind_slug() {
        let cases = [
            (SignalKind::Entity, "entity"),
            (SignalKind::Temporal, "temporal"),
            (SignalKind::Abstract, "abstract"),
            (SignalKind::Affective, "affective"),
        ];
        for (kind, expected) in cases {
            let d = DroppedSignal { kind, score: 0.7 };
            let e = HybridTruncatedEntry::from_dropped(d);
            assert_eq!(e.kind, expected);
            assert_eq!(e.score, 0.7);
        }
    }

    // -----------------------------------------------------------------
    // Downgrade vocabulary
    // -----------------------------------------------------------------

    #[test]
    fn downgrade_carries_arbitrary_reason_slug() {
        let d = Downgrade::new("abstract", "associative", "l5_unavailable");
        assert_eq!(d.from, "abstract");
        assert_eq!(d.to, "associative");
        assert_eq!(d.reason, "l5_unavailable");
    }

    // -----------------------------------------------------------------
    // PlanDetail builder ergonomics
    // -----------------------------------------------------------------

    #[test]
    fn plan_detail_builder_chains_pushes_in_order() {
        let pd = PlanDetail::new()
            .with_step("a", Duration::from_micros(1))
            .with_step("b", Duration::from_micros(2))
            .with_cost_cap_hit("max_edges_visited")
            .with_note("first")
            .with_note("second")
            .with_cutoff_reason("stage_deadline");
        assert_eq!(pd.steps.len(), 2);
        assert_eq!(pd.steps[0].stage, "a");
        assert_eq!(pd.steps[1].stage, "b");
        assert_eq!(pd.cost_caps_hit, vec!["max_edges_visited"]);
        assert_eq!(pd.notes, vec!["first", "second"]);
        assert_eq!(pd.cutoff_reason.as_deref(), Some("stage_deadline"));
    }
}
