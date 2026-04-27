//! # v0.3 Retrieval — Metrics (§8.1)
//!
//! Prometheus-style metrics surface for the retrieval pipeline. Implements
//! the eleven counters / histograms / gauges enumerated in
//! `.gid/features/v03-retrieval/design.md` §8.1, plus the supporting
//! aggregation primitives (atomic counters, lock-free histograms, gauges).
//!
//! ## Design constraints
//!
//! 1. **Always-on, zero-cost when unused.** No `Mutex` on the hot path.
//!    All updates are `AtomicU64` adds. A `MetricsRegistry` that has not
//!    been observed is just a constant-size struct of zeroed atomics.
//! 2. **No external Prometheus dep.** v0.3 keeps the metrics surface
//!    in-tree and exposes a Prometheus-text exporter (`render_prometheus`)
//!    so callers can plug into any scrape stack without dragging in the
//!    `prometheus` crate (which would force a global registry on consumers).
//! 3. **Stable label cardinality.** All labels are bounded by the type
//!    system — `Intent`, `Stage`, `ClassifierMethod`, `RetrievalOutcome`,
//!    `CostCap`, `SignalKind`, `BiTemporalMode`. No free-form strings.
//! 4. **Deterministic export.** Iterators sort by label value before
//!    rendering so test snapshots are stable.
//!
//! ## Surfaces (§8.1)
//!
//! Counters:
//!   - `retrieval_queries_total{plan}`
//!   - `retrieval_downgrades_total{from,to,reason}`
//!   - `retrieval_cost_cap_hit_total{cap}`
//!   - `retrieval_classifier_method_total{method}`
//!   - `retrieval_bi_temporal_queries_total{mode}`
//!   - `retrieval_classifier_llm_calls_total`
//!   - `retrieval_classifier_llm_tokens_total{direction}`
//!   - `retrieval_hybrid_truncation_total{dropped_kind}`
//!
//! Histograms:
//!   - `retrieval_latency_seconds{plan,stage}`
//!   - `retrieval_classifier_llm_duration_seconds`
//!
//! Gauges / ratios:
//!   - `retrieval_empty_result_rate{plan,outcome}` (counter pair → ratio)
//!   - `retrieval_affect_rank_divergence` (last sampled Kendall-tau)

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::retrieval::budget::{CostCap, Stage};
use crate::retrieval::classifier::{ClassifierMethod, Intent};
use crate::retrieval::outcomes::RetrievalOutcome;

// ---------------------------------------------------------------------------
// Label trait
// ---------------------------------------------------------------------------

/// Trait for enums that can produce a stable, low-cardinality string label
/// for Prometheus export. Anything used as a metric label must implement
/// this so the registry can iterate it without allocation.
pub trait MetricLabel: Copy + 'static {
    /// Stable string identifier, e.g. `"factual"`, `"l1"`, `"timeout"`.
    fn label(&self) -> &'static str;

    /// All possible values, in stable order. Used to size dense arrays
    /// and render exports deterministically.
    fn all() -> &'static [Self];
}

// ---------------------------------------------------------------------------
// Aggregation primitives
// ---------------------------------------------------------------------------

/// Lock-free monotonic counter. Updates are `Relaxed` adds — Prometheus
/// scrapes are eventually-consistent so we don't need any ordering.
#[derive(Debug, Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    pub const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

/// Signed gauge. Stored as `AtomicI64` so we can `set`, increment and
/// decrement without losing sign (e.g. negative Kendall-tau divergence).
///
/// Floats are encoded by multiplying by [`Gauge::SCALE`] and storing the
/// rounded integer. `set_f64` / `get_f64` handle the conversion. Resolution
/// is `1e-6` over the range `±9.2e12`, which is overkill for the rank
/// divergence metric (range `[-1.0, 1.0]`) but cheap.
#[derive(Debug, Default)]
pub struct Gauge {
    value: AtomicI64,
}

impl Gauge {
    pub const SCALE: i64 = 1_000_000;

    pub const fn new() -> Self {
        Self {
            value: AtomicI64::new(0),
        }
    }

    #[inline]
    pub fn set(&self, v: i64) {
        self.value.store(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn set_f64(&self, v: f64) {
        let scaled = (v * Self::SCALE as f64).round() as i64;
        self.value.store(scaled, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn get_f64(&self) -> f64 {
        self.value.load(Ordering::Relaxed) as f64 / Self::SCALE as f64
    }
}

/// Fixed-bucket histogram with atomic-counter buckets. Bucket boundaries
/// are seconds (or whatever unit the caller chose), provided at
/// construction. The last implicit bucket is `+Inf`.
///
/// p50 / p95 / p99 are computed via linear interpolation across cumulative
/// bucket counts at scrape time — exact percentiles are not the goal,
/// scrape-time cheap percentiles are.
#[derive(Debug)]
pub struct Histogram {
    /// Upper bounds of finite buckets (sorted ascending).
    bounds: &'static [f64],
    /// `bounds.len() + 1` counts. Index `bounds.len()` is the `+Inf` bucket.
    counts: Box<[AtomicU64]>,
    /// Total observations (for fast count() without summing).
    total: AtomicU64,
    /// Sum of observed values (scaled by [`Histogram::SUM_SCALE`]).
    sum_scaled: AtomicU64,
}

impl Histogram {
    /// Sum is stored in micro-units so `0.000123 s` adds `123` to the
    /// integer accumulator — keeps everything lock-free without an
    /// `AtomicF64`.
    pub const SUM_SCALE: f64 = 1_000_000.0;

    /// Create a histogram with the given upper-bound buckets (in seconds).
    /// Bounds must be sorted ascending and non-empty; otherwise this
    /// panics in debug and returns a single-bucket histogram in release.
    pub fn new(bounds: &'static [f64]) -> Self {
        debug_assert!(!bounds.is_empty(), "histogram bounds cannot be empty");
        debug_assert!(
            bounds.windows(2).all(|w| w[0] < w[1]),
            "histogram bounds must be strictly ascending"
        );
        let n = bounds.len() + 1;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(AtomicU64::new(0));
        }
        Self {
            bounds,
            counts: v.into_boxed_slice(),
            total: AtomicU64::new(0),
            sum_scaled: AtomicU64::new(0),
        }
    }

    /// Observe a value (typically a duration in seconds).
    pub fn observe(&self, v: f64) {
        // Find the first bucket whose upper bound is >= v.
        let mut idx = self.bounds.len(); // +Inf
        for (i, &b) in self.bounds.iter().enumerate() {
            if v <= b {
                idx = i;
                break;
            }
        }
        self.counts[idx].fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        let scaled = (v.max(0.0) * Self::SUM_SCALE).round() as u64;
        self.sum_scaled.fetch_add(scaled, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn sum(&self) -> f64 {
        self.sum_scaled.load(Ordering::Relaxed) as f64 / Self::SUM_SCALE
    }

    /// Snapshot of cumulative bucket counts in Prometheus order (each
    /// entry is `count(observations <= upper_bound)`). The final entry
    /// corresponds to `+Inf` and equals [`Histogram::count`].
    pub fn cumulative_counts(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.counts.len());
        let mut acc = 0u64;
        for c in self.counts.iter() {
            acc += c.load(Ordering::Relaxed);
            out.push(acc);
        }
        out
    }

    /// Estimated quantile via linear interpolation across buckets.
    /// `q` must be in `[0.0, 1.0]`. Returns `0.0` for an empty histogram.
    pub fn quantile(&self, q: f64) -> f64 {
        let q = q.clamp(0.0, 1.0);
        let cum = self.cumulative_counts();
        let total = *cum.last().unwrap_or(&0);
        if total == 0 {
            return 0.0;
        }
        let target = (q * total as f64).ceil() as u64;
        let target = target.max(1);
        // Find first cumulative count >= target.
        let mut prev_cum = 0u64;
        let mut prev_bound = 0.0f64;
        for (i, &c) in cum.iter().enumerate() {
            if c >= target {
                let bucket_count = c - prev_cum;
                let upper = if i < self.bounds.len() {
                    self.bounds[i]
                } else {
                    // +Inf bucket: treat as the last finite bound (best
                    // we can do without the actual max observation).
                    *self.bounds.last().unwrap_or(&0.0)
                };
                if bucket_count == 0 {
                    return upper;
                }
                let frac = (target - prev_cum) as f64 / bucket_count as f64;
                return prev_bound + (upper - prev_bound) * frac;
            }
            prev_cum = c;
            prev_bound = if i < self.bounds.len() {
                self.bounds[i]
            } else {
                prev_bound
            };
        }
        *self.bounds.last().unwrap_or(&0.0)
    }

    pub fn p50(&self) -> f64 {
        self.quantile(0.50)
    }
    pub fn p95(&self) -> f64 {
        self.quantile(0.95)
    }
    pub fn p99(&self) -> f64 {
        self.quantile(0.99)
    }

    pub fn bounds(&self) -> &'static [f64] {
        self.bounds
    }
}

/// Default latency buckets, in seconds — geometric, covering ~50µs to ~5s.
/// Matches typical retrieval p50 (sub-ms) → p99 (multi-second under cost
/// cap) range observed in §11 benchmarks.
pub const DEFAULT_LATENCY_BUCKETS: &[f64] = &[
    0.000_05, 0.000_1, 0.000_25, 0.000_5, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
    1.0, 2.5, 5.0,
];

/// LLM call duration buckets, seconds — covers 10ms (cache hit) to 30s
/// (cold provider with high latency).
pub const DEFAULT_LLM_DURATION_BUCKETS: &[f64] = &[
    0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0,
];

// ---------------------------------------------------------------------------
// Label implementations
// ---------------------------------------------------------------------------

impl MetricLabel for Intent {
    fn label(&self) -> &'static str {
        self.as_str()
    }
    fn all() -> &'static [Self] {
        const ALL: [Intent; 5] = [
            Intent::Factual,
            Intent::Episodic,
            Intent::Abstract,
            Intent::Affective,
            Intent::Hybrid,
        ];
        &ALL
    }
}

impl MetricLabel for Stage {
    fn label(&self) -> &'static str {
        self.as_str()
    }
    fn all() -> &'static [Self] {
        const ALL: [Stage; 13] = [
            Stage::EntityResolution,
            Stage::EdgeTraversal,
            Stage::MemoryLookup,
            Stage::Rerank,
            Stage::Fusion,
            Stage::TimeParse,
            Stage::TimeBoundedRecall,
            Stage::OptionalGraphFilter,
            Stage::SeedRecall,
            Stage::EntityExtract,
            Stage::EdgeHop,
            Stage::Scoring,
            Stage::RrfFusion,
        ];
        &ALL
    }
}

impl MetricLabel for ClassifierMethod {
    fn label(&self) -> &'static str {
        match self {
            ClassifierMethod::Heuristic => "heuristic",
            ClassifierMethod::LlmFallback => "llm",
            ClassifierMethod::HeuristicTimeout => "timeout",
            ClassifierMethod::CallerOverride => "override",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [ClassifierMethod; 4] = [
            ClassifierMethod::Heuristic,
            ClassifierMethod::LlmFallback,
            ClassifierMethod::HeuristicTimeout,
            ClassifierMethod::CallerOverride,
        ];
        &ALL
    }
}

impl MetricLabel for CostCap {
    fn label(&self) -> &'static str {
        self.as_str()
    }
    fn all() -> &'static [Self] {
        const ALL: [CostCap; 7] = [
            CostCap::MaxAnchors,
            CostCap::MaxHops,
            CostCap::MaxEdgesVisited,
            CostCap::MaxCandidates,
            CostCap::KSeed,
            CostCap::KPool,
            CostCap::KSeedAffective,
        ];
        &ALL
    }
}

/// Bi-temporal query mode (§3.4 / §3.5). Lives here as a metrics-only
/// label until/unless the retrieval API needs to surface it directly —
/// the existing API already encodes the same distinction in `TimeWindow`
/// (`Current`, `AsOf`, `IncludeSuperseded`), but those variants carry
/// payloads we don't want in label cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiTemporalMode {
    Current,
    AsOf,
    IncludeSuperseded,
}

impl MetricLabel for BiTemporalMode {
    fn label(&self) -> &'static str {
        match self {
            BiTemporalMode::Current => "current",
            BiTemporalMode::AsOf => "as_of",
            BiTemporalMode::IncludeSuperseded => "include_superseded",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [BiTemporalMode; 3] = [
            BiTemporalMode::Current,
            BiTemporalMode::AsOf,
            BiTemporalMode::IncludeSuperseded,
        ];
        &ALL
    }
}

/// LLM token direction label (`prompt` vs `completion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenDirection {
    Prompt,
    Completion,
}

impl MetricLabel for TokenDirection {
    fn label(&self) -> &'static str {
        match self {
            TokenDirection::Prompt => "prompt",
            TokenDirection::Completion => "completion",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [TokenDirection; 2] = [TokenDirection::Prompt, TokenDirection::Completion];
        &ALL
    }
}

/// Stable label for the `dropped_kind` axis of
/// `retrieval_hybrid_truncation_total`. Mirrors `SignalKind` but lives
/// here so we don't pin metrics to that internal enum's exact shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DroppedKind {
    Entity,
    Temporal,
    Abstract,
    Affective,
}

impl MetricLabel for DroppedKind {
    fn label(&self) -> &'static str {
        match self {
            DroppedKind::Entity => "entity",
            DroppedKind::Temporal => "temporal",
            DroppedKind::Abstract => "abstract",
            DroppedKind::Affective => "affective",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [DroppedKind; 4] = [
            DroppedKind::Entity,
            DroppedKind::Temporal,
            DroppedKind::Abstract,
            DroppedKind::Affective,
        ];
        &ALL
    }
}

/// Outcome label for the empty-result-rate metric. We keep the full set
/// here statically so [`MetricLabel::all`] has a stable shape; this
/// mirrors `RetrievalOutcome::slug` for the variants we count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutcomeLabel {
    Ok,
    NoEntityFound,
    EntityFoundNoEdges,
    NoMemoriesInWindow,
    AmbiguousQuery,
    L5NotReady,
    DowngradedFromAbstract,
    DowngradedFromEpisodic,
    NoCognitiveState,
}

impl OutcomeLabel {
    pub fn from_outcome(o: &RetrievalOutcome) -> Self {
        match o {
            RetrievalOutcome::Ok => Self::Ok,
            RetrievalOutcome::NoEntityFound { .. } => Self::NoEntityFound,
            RetrievalOutcome::EntityFoundNoEdges { .. } => Self::EntityFoundNoEdges,
            RetrievalOutcome::NoMemoriesInWindow { .. } => Self::NoMemoriesInWindow,
            RetrievalOutcome::AmbiguousQuery { .. } => Self::AmbiguousQuery,
            RetrievalOutcome::L5NotReady { .. } => Self::L5NotReady,
            RetrievalOutcome::DowngradedFromAbstract { .. } => Self::DowngradedFromAbstract,
            RetrievalOutcome::DowngradedFromEpisodic { .. } => Self::DowngradedFromEpisodic,
            RetrievalOutcome::NoCognitiveState => Self::NoCognitiveState,
        }
    }
}

impl MetricLabel for OutcomeLabel {
    fn label(&self) -> &'static str {
        match self {
            OutcomeLabel::Ok => "ok",
            OutcomeLabel::NoEntityFound => "no_entity_found",
            OutcomeLabel::EntityFoundNoEdges => "entity_found_no_edges",
            OutcomeLabel::NoMemoriesInWindow => "no_memories_in_window",
            OutcomeLabel::AmbiguousQuery => "ambiguous_query",
            OutcomeLabel::L5NotReady => "l5_not_ready",
            OutcomeLabel::DowngradedFromAbstract => "downgraded_from_abstract",
            OutcomeLabel::DowngradedFromEpisodic => "downgraded_from_episodic",
            OutcomeLabel::NoCognitiveState => "no_cognitive_state",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [OutcomeLabel; 9] = [
            OutcomeLabel::Ok,
            OutcomeLabel::NoEntityFound,
            OutcomeLabel::EntityFoundNoEdges,
            OutcomeLabel::NoMemoriesInWindow,
            OutcomeLabel::AmbiguousQuery,
            OutcomeLabel::L5NotReady,
            OutcomeLabel::DowngradedFromAbstract,
            OutcomeLabel::DowngradedFromEpisodic,
            OutcomeLabel::NoCognitiveState,
        ];
        &ALL
    }
}

/// Stable downgrade reason label. Free-form `String` reasons in
/// `RetrievalOutcome::DowngradedFromAbstract` / `DowngradedFromEpisodic`
/// are kept out of metric label cardinality on purpose; we coerce the
/// known values here and bucket everything else as `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DowngradeReason {
    L5Unavailable,
    AllBelowMinTopicScore,
    NoTimeExpression,
    WindowOutsideCutoff,
    NoCognitiveState,
    HybridCap,
    Other,
}

impl DowngradeReason {
    /// Best-effort mapping from a free-form reason string. Unknown
    /// values fall through to `Other` so metrics never panic on a new
    /// plan-defined reason.
    pub fn from_str(s: &str) -> Self {
        match s {
            "L5_unavailable" | "l5_unavailable" => Self::L5Unavailable,
            "all_below_min_topic_score" => Self::AllBelowMinTopicScore,
            "no_time_expression" => Self::NoTimeExpression,
            "window_outside_cutoff" => Self::WindowOutsideCutoff,
            "no_cognitive_state" => Self::NoCognitiveState,
            "hybrid_cap" => Self::HybridCap,
            _ => Self::Other,
        }
    }
}

impl MetricLabel for DowngradeReason {
    fn label(&self) -> &'static str {
        match self {
            DowngradeReason::L5Unavailable => "l5_unavailable",
            DowngradeReason::AllBelowMinTopicScore => "all_below_min_topic_score",
            DowngradeReason::NoTimeExpression => "no_time_expression",
            DowngradeReason::WindowOutsideCutoff => "window_outside_cutoff",
            DowngradeReason::NoCognitiveState => "no_cognitive_state",
            DowngradeReason::HybridCap => "hybrid_cap",
            DowngradeReason::Other => "other",
        }
    }
    fn all() -> &'static [Self] {
        const ALL: [DowngradeReason; 7] = [
            DowngradeReason::L5Unavailable,
            DowngradeReason::AllBelowMinTopicScore,
            DowngradeReason::NoTimeExpression,
            DowngradeReason::WindowOutsideCutoff,
            DowngradeReason::NoCognitiveState,
            DowngradeReason::HybridCap,
            DowngradeReason::Other,
        ];
        &ALL
    }
}

// ---------------------------------------------------------------------------
// Dense label tables
// ---------------------------------------------------------------------------

/// 1D table indexed by a single `MetricLabel`. Stores `Counter` per label
/// in a fixed-size `Box<[Counter]>` — O(1) lookup, no hashing.
pub struct LabelVec1<L: MetricLabel> {
    counters: Box<[Counter]>,
    _marker: std::marker::PhantomData<L>,
}

impl<L: MetricLabel> std::fmt::Debug for LabelVec1<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelVec1")
            .field("len", &self.counters.len())
            .finish()
    }
}

impl<L: MetricLabel + PartialEq> LabelVec1<L> {
    pub fn new() -> Self {
        let n = L::all().len();
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(Counter::new());
        }
        Self {
            counters: v.into_boxed_slice(),
            _marker: std::marker::PhantomData,
        }
    }

    fn idx(&self, label: L) -> usize {
        L::all()
            .iter()
            .position(|x| *x == label)
            .expect("MetricLabel::all() must contain every value")
    }

    pub fn inc(&self, label: L) {
        self.counters[self.idx(label)].inc();
    }

    pub fn add(&self, label: L, n: u64) {
        self.counters[self.idx(label)].add(n);
    }

    pub fn get(&self, label: L) -> u64 {
        self.counters[self.idx(label)].get()
    }

    /// Iterate `(label, value)` in `L::all()` order.
    pub fn iter(&self) -> impl Iterator<Item = (L, u64)> + '_ {
        L::all()
            .iter()
            .copied()
            .zip(self.counters.iter().map(|c| c.get()))
    }
}

impl<L: MetricLabel + PartialEq> Default for LabelVec1<L> {
    fn default() -> Self {
        Self::new()
    }
}

/// 2D table — counter per `(L1, L2)` pair.
pub struct LabelVec2<L1: MetricLabel, L2: MetricLabel> {
    counters: Box<[Counter]>,
    _m1: std::marker::PhantomData<L1>,
    _m2: std::marker::PhantomData<L2>,
}

impl<L1: MetricLabel, L2: MetricLabel> std::fmt::Debug for LabelVec2<L1, L2> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelVec2")
            .field("len", &self.counters.len())
            .finish()
    }
}

impl<L1: MetricLabel + PartialEq, L2: MetricLabel + PartialEq> LabelVec2<L1, L2> {
    pub fn new() -> Self {
        let n = L1::all().len() * L2::all().len();
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(Counter::new());
        }
        Self {
            counters: v.into_boxed_slice(),
            _m1: std::marker::PhantomData,
            _m2: std::marker::PhantomData,
        }
    }

    fn idx(&self, a: L1, b: L2) -> usize {
        let i = L1::all().iter().position(|x| *x == a).expect("L1 in all()");
        let j = L2::all().iter().position(|x| *x == b).expect("L2 in all()");
        i * L2::all().len() + j
    }

    pub fn inc(&self, a: L1, b: L2) {
        self.counters[self.idx(a, b)].inc();
    }

    pub fn add(&self, a: L1, b: L2, n: u64) {
        self.counters[self.idx(a, b)].add(n);
    }

    pub fn get(&self, a: L1, b: L2) -> u64 {
        self.counters[self.idx(a, b)].get()
    }

    pub fn iter(&self) -> impl Iterator<Item = (L1, L2, u64)> + '_ {
        let l2_len = L2::all().len();
        self.counters.iter().enumerate().map(move |(idx, c)| {
            let i = idx / l2_len;
            let j = idx % l2_len;
            (L1::all()[i], L2::all()[j], c.get())
        })
    }
}

impl<L1: MetricLabel + PartialEq, L2: MetricLabel + PartialEq> Default for LabelVec2<L1, L2> {
    fn default() -> Self {
        Self::new()
    }
}

/// 3D table — counter per `(L1, L2, L3)` triple.
pub struct LabelVec3<L1: MetricLabel, L2: MetricLabel, L3: MetricLabel> {
    counters: Box<[Counter]>,
    _m: std::marker::PhantomData<(L1, L2, L3)>,
}

impl<L1: MetricLabel, L2: MetricLabel, L3: MetricLabel> std::fmt::Debug for LabelVec3<L1, L2, L3> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelVec3")
            .field("len", &self.counters.len())
            .finish()
    }
}

impl<L1: MetricLabel + PartialEq, L2: MetricLabel + PartialEq, L3: MetricLabel + PartialEq>
    LabelVec3<L1, L2, L3>
{
    pub fn new() -> Self {
        let n = L1::all().len() * L2::all().len() * L3::all().len();
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(Counter::new());
        }
        Self {
            counters: v.into_boxed_slice(),
            _m: std::marker::PhantomData,
        }
    }

    fn idx(&self, a: L1, b: L2, c: L3) -> usize {
        let i = L1::all().iter().position(|x| *x == a).expect("L1 in all()");
        let j = L2::all().iter().position(|x| *x == b).expect("L2 in all()");
        let k = L3::all().iter().position(|x| *x == c).expect("L3 in all()");
        ((i * L2::all().len()) + j) * L3::all().len() + k
    }

    pub fn inc(&self, a: L1, b: L2, c: L3) {
        self.counters[self.idx(a, b, c)].inc();
    }

    pub fn get(&self, a: L1, b: L2, c: L3) -> u64 {
        self.counters[self.idx(a, b, c)].get()
    }

    pub fn iter(&self) -> impl Iterator<Item = (L1, L2, L3, u64)> + '_ {
        let l2_len = L2::all().len();
        let l3_len = L3::all().len();
        self.counters.iter().enumerate().map(move |(idx, c)| {
            let i = idx / (l2_len * l3_len);
            let rem = idx % (l2_len * l3_len);
            let j = rem / l3_len;
            let k = rem % l3_len;
            (L1::all()[i], L2::all()[j], L3::all()[k], c.get())
        })
    }
}

impl<L1: MetricLabel + PartialEq, L2: MetricLabel + PartialEq, L3: MetricLabel + PartialEq> Default
    for LabelVec3<L1, L2, L3>
{
    fn default() -> Self {
        Self::new()
    }
}

/// 2D histogram table — `Histogram` per `(L1, L2)` pair.
pub struct HistogramVec2<L1: MetricLabel, L2: MetricLabel> {
    histograms: Box<[Histogram]>,
    _m: std::marker::PhantomData<(L1, L2)>,
}

impl<L1: MetricLabel, L2: MetricLabel> std::fmt::Debug for HistogramVec2<L1, L2> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistogramVec2")
            .field("len", &self.histograms.len())
            .finish()
    }
}

impl<L1: MetricLabel + PartialEq, L2: MetricLabel + PartialEq> HistogramVec2<L1, L2> {
    pub fn new(bounds: &'static [f64]) -> Self {
        let n = L1::all().len() * L2::all().len();
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(Histogram::new(bounds));
        }
        Self {
            histograms: v.into_boxed_slice(),
            _m: std::marker::PhantomData,
        }
    }

    fn idx(&self, a: L1, b: L2) -> usize {
        let i = L1::all().iter().position(|x| *x == a).expect("L1 in all()");
        let j = L2::all().iter().position(|x| *x == b).expect("L2 in all()");
        i * L2::all().len() + j
    }

    pub fn observe(&self, a: L1, b: L2, v: f64) {
        self.histograms[self.idx(a, b)].observe(v);
    }

    pub fn get(&self, a: L1, b: L2) -> &Histogram {
        &self.histograms[self.idx(a, b)]
    }

    pub fn iter(&self) -> impl Iterator<Item = (L1, L2, &Histogram)> + '_ {
        let l2_len = L2::all().len();
        self.histograms.iter().enumerate().map(move |(idx, h)| {
            let i = idx / l2_len;
            let j = idx % l2_len;
            (L1::all()[i], L2::all()[j], h)
        })
    }
}

// ---------------------------------------------------------------------------
// MetricsRegistry — the canonical §8.1 surface
// ---------------------------------------------------------------------------

/// Container for every retrieval metric in §8.1. Construct once per
/// process (typically held inside the orchestrator / engine) and share
/// via `Arc`. All updates are lock-free.
///
/// **Zero-cost when unused:** every counter is an `AtomicU64` initialized
/// to 0; histograms allocate one `Box<[AtomicU64]>` of `bounds.len() + 1`
/// entries at construction. No work happens until something is observed.
#[derive(Debug)]
pub struct MetricsRegistry {
    /// `retrieval_queries_total{plan}` — incremented once per executed
    /// retrieval, regardless of outcome.
    pub queries_total: LabelVec1<Intent>,

    /// `retrieval_latency_seconds{plan,stage}` — per-stage latency.
    pub latency_seconds: HistogramVec2<Intent, Stage>,

    /// `retrieval_downgrades_total{from,to,reason}`.
    pub downgrades_total: LabelVec3<Intent, Intent, DowngradeReason>,

    /// `retrieval_cost_cap_hit_total{cap}`.
    pub cost_cap_hit_total: LabelVec1<CostCap>,

    /// `retrieval_empty_result_rate{plan,outcome}` — exposed as a counter
    /// pair (`outcome=ok` vs others) so the consumer can compute the
    /// ratio at scrape time. Storing the raw counts avoids an
    /// observation-window argument in the metric API.
    pub outcomes_total: LabelVec2<Intent, OutcomeLabel>,

    /// `retrieval_classifier_method_total{method}`.
    pub classifier_method_total: LabelVec1<ClassifierMethod>,

    /// `retrieval_bi_temporal_queries_total{mode}`.
    pub bi_temporal_queries_total: LabelVec1<BiTemporalMode>,

    /// `retrieval_classifier_llm_calls_total`.
    pub classifier_llm_calls_total: Counter,

    /// `retrieval_classifier_llm_tokens_total{direction}`.
    pub classifier_llm_tokens_total: LabelVec1<TokenDirection>,

    /// `retrieval_classifier_llm_duration_seconds`.
    pub classifier_llm_duration_seconds: Histogram,

    /// `retrieval_hybrid_truncation_total{dropped_kind}`.
    pub hybrid_truncation_total: LabelVec1<DroppedKind>,

    /// `retrieval_affect_rank_divergence` — last sampled Kendall-tau.
    /// Range `[-1.0, 1.0]`.
    pub affect_rank_divergence: Gauge,
}

impl MetricsRegistry {
    /// Construct a new registry with default histogram buckets.
    pub fn new() -> Self {
        Self::with_buckets(DEFAULT_LATENCY_BUCKETS, DEFAULT_LLM_DURATION_BUCKETS)
    }

    /// Construct a registry with custom histogram bucket boundaries.
    /// Useful for tests that want predictable bucket layouts.
    pub fn with_buckets(
        latency_bounds: &'static [f64],
        llm_duration_bounds: &'static [f64],
    ) -> Self {
        Self {
            queries_total: LabelVec1::new(),
            latency_seconds: HistogramVec2::new(latency_bounds),
            downgrades_total: LabelVec3::new(),
            cost_cap_hit_total: LabelVec1::new(),
            outcomes_total: LabelVec2::new(),
            classifier_method_total: LabelVec1::new(),
            bi_temporal_queries_total: LabelVec1::new(),
            classifier_llm_calls_total: Counter::new(),
            classifier_llm_tokens_total: LabelVec1::new(),
            classifier_llm_duration_seconds: Histogram::new(llm_duration_bounds),
            hybrid_truncation_total: LabelVec1::new(),
            affect_rank_divergence: Gauge::new(),
        }
    }

    // ---- High-level convenience recorders ---------------------------------

    /// Record one completed retrieval: bumps query count, classifier
    /// method, plan→outcome, and (if present) bi-temporal mode.
    pub fn record_query(
        &self,
        plan: Intent,
        method: ClassifierMethod,
        outcome: &RetrievalOutcome,
        bi_temporal: Option<BiTemporalMode>,
    ) {
        self.queries_total.inc(plan);
        self.classifier_method_total.inc(method);
        self.outcomes_total
            .inc(plan, OutcomeLabel::from_outcome(outcome));
        if let Some(mode) = bi_temporal {
            self.bi_temporal_queries_total.inc(mode);
        }
    }

    /// Record a single stage timing.
    pub fn record_stage_latency(&self, plan: Intent, stage: Stage, seconds: f64) {
        self.latency_seconds.observe(plan, stage, seconds);
    }

    /// Record a downgrade event; `reason_str` is a free-form reason from
    /// `RetrievalOutcome` and is mapped via [`DowngradeReason::from_str`].
    pub fn record_downgrade(&self, from: Intent, to: Intent, reason_str: &str) {
        self.downgrades_total
            .inc(from, to, DowngradeReason::from_str(reason_str));
    }

    pub fn record_cost_cap_hit(&self, cap: CostCap) {
        self.cost_cap_hit_total.inc(cap);
    }

    pub fn record_classifier_llm_call(&self, prompt_tokens: u64, completion_tokens: u64, seconds: f64) {
        self.classifier_llm_calls_total.inc();
        self.classifier_llm_tokens_total
            .add(TokenDirection::Prompt, prompt_tokens);
        self.classifier_llm_tokens_total
            .add(TokenDirection::Completion, completion_tokens);
        self.classifier_llm_duration_seconds.observe(seconds);
    }

    pub fn record_hybrid_truncation(&self, kind: DroppedKind) {
        self.hybrid_truncation_total.inc(kind);
    }

    /// Update the affect rank-divergence gauge (Kendall-tau).
    pub fn set_affect_rank_divergence(&self, tau: f64) {
        self.affect_rank_divergence.set_f64(tau);
    }

    /// Compute empty-result rate for a plan: `non_ok / total`. Returns
    /// `None` when there are zero observations to avoid `0/0 = NaN`
    /// surfacing as garbage in dashboards.
    pub fn empty_result_rate(&self, plan: Intent) -> Option<f64> {
        let mut total = 0u64;
        let mut empty = 0u64;
        for (_p, o, c) in self.outcomes_total.iter() {
            // restrict to the requested plan
            if !matches!(self.outcomes_total.get(plan, o), c0 if c0 == c) {
                // (no-op — we only enter the matching plan rows below)
            }
        }
        // simpler: iterate OutcomeLabel::all() against the chosen plan
        for &o in OutcomeLabel::all() {
            let c = self.outcomes_total.get(plan, o);
            total += c;
            if !matches!(o, OutcomeLabel::Ok) {
                empty += c;
            }
        }
        if total == 0 {
            None
        } else {
            Some(empty as f64 / total as f64)
        }
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Prometheus text exporter
// ---------------------------------------------------------------------------

/// Render the registry to Prometheus text format (v0.0.4). Output is
/// deterministic for fixed inputs so it can be snapshot-tested.
pub fn render_prometheus(reg: &MetricsRegistry) -> String {
    let mut out = String::with_capacity(4096);

    // queries_total
    out.push_str("# HELP retrieval_queries_total Total retrieval queries by plan.\n");
    out.push_str("# TYPE retrieval_queries_total counter\n");
    for (plan, v) in reg.queries_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_queries_total{{plan=\"{}\"}} {}\n",
                plan.label(),
                v
            ),
        );
    }

    // latency_seconds histogram
    out.push_str("# HELP retrieval_latency_seconds Per-stage retrieval latency.\n");
    out.push_str("# TYPE retrieval_latency_seconds histogram\n");
    for (plan, stage, h) in reg.latency_seconds.iter() {
        let cum = h.cumulative_counts();
        for (i, &b) in h.bounds().iter().enumerate() {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "retrieval_latency_seconds_bucket{{plan=\"{}\",stage=\"{}\",le=\"{}\"}} {}\n",
                    plan.label(),
                    stage.label(),
                    b,
                    cum[i]
                ),
            );
        }
        // +Inf bucket
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_latency_seconds_bucket{{plan=\"{}\",stage=\"{}\",le=\"+Inf\"}} {}\n",
                plan.label(),
                stage.label(),
                *cum.last().unwrap_or(&0)
            ),
        );
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_latency_seconds_sum{{plan=\"{}\",stage=\"{}\"}} {}\n",
                plan.label(),
                stage.label(),
                h.sum()
            ),
        );
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_latency_seconds_count{{plan=\"{}\",stage=\"{}\"}} {}\n",
                plan.label(),
                stage.label(),
                h.count()
            ),
        );
    }

    // downgrades_total
    out.push_str("# HELP retrieval_downgrades_total Plan downgrades by from/to/reason.\n");
    out.push_str("# TYPE retrieval_downgrades_total counter\n");
    for (from, to, reason, v) in reg.downgrades_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_downgrades_total{{from=\"{}\",to=\"{}\",reason=\"{}\"}} {}\n",
                from.label(),
                to.label(),
                reason.label(),
                v
            ),
        );
    }

    // cost_cap_hit_total
    out.push_str("# HELP retrieval_cost_cap_hit_total Cost cap saturations by cap.\n");
    out.push_str("# TYPE retrieval_cost_cap_hit_total counter\n");
    for (cap, v) in reg.cost_cap_hit_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_cost_cap_hit_total{{cap=\"{}\"}} {}\n",
                cap.label(),
                v
            ),
        );
    }

    // empty_result_rate (counter pair → caller computes ratio)
    out.push_str("# HELP retrieval_outcomes_total Retrieval outcomes by plan and outcome (used for empty_result_rate).\n");
    out.push_str("# TYPE retrieval_outcomes_total counter\n");
    for (plan, outcome, v) in reg.outcomes_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_outcomes_total{{plan=\"{}\",outcome=\"{}\"}} {}\n",
                plan.label(),
                outcome.label(),
                v
            ),
        );
    }

    // classifier_method_total
    out.push_str("# HELP retrieval_classifier_method_total Classifier resolution method.\n");
    out.push_str("# TYPE retrieval_classifier_method_total counter\n");
    for (m, v) in reg.classifier_method_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_classifier_method_total{{method=\"{}\"}} {}\n",
                m.label(),
                v
            ),
        );
    }

    // bi_temporal_queries_total
    out.push_str("# HELP retrieval_bi_temporal_queries_total Bi-temporal queries by mode.\n");
    out.push_str("# TYPE retrieval_bi_temporal_queries_total counter\n");
    for (mode, v) in reg.bi_temporal_queries_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_bi_temporal_queries_total{{mode=\"{}\"}} {}\n",
                mode.label(),
                v
            ),
        );
    }

    // classifier llm calls / tokens / duration
    out.push_str("# HELP retrieval_classifier_llm_calls_total Classifier Stage 2 LLM calls.\n");
    out.push_str("# TYPE retrieval_classifier_llm_calls_total counter\n");
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!(
            "retrieval_classifier_llm_calls_total {}\n",
            reg.classifier_llm_calls_total.get()
        ),
    );

    out.push_str("# HELP retrieval_classifier_llm_tokens_total Classifier LLM tokens.\n");
    out.push_str("# TYPE retrieval_classifier_llm_tokens_total counter\n");
    for (dir, v) in reg.classifier_llm_tokens_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_classifier_llm_tokens_total{{direction=\"{}\"}} {}\n",
                dir.label(),
                v
            ),
        );
    }

    out.push_str("# HELP retrieval_classifier_llm_duration_seconds Classifier LLM latency.\n");
    out.push_str("# TYPE retrieval_classifier_llm_duration_seconds histogram\n");
    {
        let h = &reg.classifier_llm_duration_seconds;
        let cum = h.cumulative_counts();
        for (i, &b) in h.bounds().iter().enumerate() {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "retrieval_classifier_llm_duration_seconds_bucket{{le=\"{}\"}} {}\n",
                    b, cum[i]
                ),
            );
        }
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_classifier_llm_duration_seconds_bucket{{le=\"+Inf\"}} {}\n",
                *cum.last().unwrap_or(&0)
            ),
        );
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_classifier_llm_duration_seconds_sum {}\n",
                h.sum()
            ),
        );
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_classifier_llm_duration_seconds_count {}\n",
                h.count()
            ),
        );
    }

    // hybrid_truncation_total
    out.push_str("# HELP retrieval_hybrid_truncation_total Strong signals dropped at Hybrid 2-plan cap.\n");
    out.push_str("# TYPE retrieval_hybrid_truncation_total counter\n");
    for (k, v) in reg.hybrid_truncation_total.iter() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "retrieval_hybrid_truncation_total{{dropped_kind=\"{}\"}} {}\n",
                k.label(),
                v
            ),
        );
    }

    // affect rank divergence
    out.push_str("# HELP retrieval_affect_rank_divergence Last sampled Kendall-tau between affect-weighted and base ranking.\n");
    out.push_str("# TYPE retrieval_affect_rank_divergence gauge\n");
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!(
            "retrieval_affect_rank_divergence {}\n",
            reg.affect_rank_divergence.get_f64()
        ),
    );

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_basic() {
        let c = Counter::new();
        assert_eq!(c.get(), 0);
        c.inc();
        c.inc();
        c.add(8);
        assert_eq!(c.get(), 10);
    }

    #[test]
    fn gauge_signed_and_float() {
        let g = Gauge::new();
        g.set_f64(-0.42);
        let v = g.get_f64();
        assert!((v + 0.42).abs() < 1e-5, "got {}", v);
        g.set(0);
        assert_eq!(g.get_f64(), 0.0);
        g.set_f64(0.999_999);
        assert!((g.get_f64() - 0.999_999).abs() < 1e-5);
    }

    #[test]
    fn histogram_buckets_and_quantiles() {
        let h = Histogram::new(&[0.01, 0.05, 0.1, 0.5, 1.0]);
        // 100 obs, evenly spaced
        for i in 0..100 {
            h.observe(i as f64 / 100.0); // 0.00, 0.01, ..., 0.99
        }
        assert_eq!(h.count(), 100);
        let sum = h.sum();
        // expected sum ~= 49.5
        assert!((sum - 49.5).abs() < 0.1, "sum was {}", sum);
        let cum = h.cumulative_counts();
        // last cumulative count must equal total
        assert_eq!(*cum.last().unwrap(), 100);
        // monotonic non-decreasing
        for w in cum.windows(2) {
            assert!(w[0] <= w[1]);
        }
        // quantiles in expected ranges
        assert!(h.p50() > 0.0 && h.p50() < 1.0);
        assert!(h.p99() >= h.p95());
        assert!(h.p95() >= h.p50());
    }

    #[test]
    fn histogram_quantile_empty() {
        let h = Histogram::new(&[0.1, 1.0]);
        assert_eq!(h.p50(), 0.0);
        assert_eq!(h.p99(), 0.0);
        assert_eq!(h.count(), 0);
    }

    #[test]
    fn label_vec1_intent() {
        let v: LabelVec1<Intent> = LabelVec1::new();
        v.inc(Intent::Factual);
        v.inc(Intent::Factual);
        v.inc(Intent::Hybrid);
        assert_eq!(v.get(Intent::Factual), 2);
        assert_eq!(v.get(Intent::Hybrid), 1);
        assert_eq!(v.get(Intent::Episodic), 0);
        // iteration order is L::all() order
        let kinds: Vec<_> = v.iter().map(|(l, _)| l).collect();
        assert_eq!(kinds, Intent::all().to_vec());
    }

    #[test]
    fn label_vec2_pair() {
        let v: LabelVec2<Intent, OutcomeLabel> = LabelVec2::new();
        v.inc(Intent::Factual, OutcomeLabel::Ok);
        v.inc(Intent::Factual, OutcomeLabel::Ok);
        v.inc(Intent::Episodic, OutcomeLabel::AmbiguousQuery);
        assert_eq!(v.get(Intent::Factual, OutcomeLabel::Ok), 2);
        assert_eq!(v.get(Intent::Factual, OutcomeLabel::AmbiguousQuery), 0);
        assert_eq!(v.get(Intent::Episodic, OutcomeLabel::AmbiguousQuery), 1);
    }

    #[test]
    fn label_vec3_triple() {
        let v: LabelVec3<Intent, Intent, DowngradeReason> = LabelVec3::new();
        v.inc(Intent::Episodic, Intent::Factual, DowngradeReason::NoTimeExpression);
        v.inc(Intent::Episodic, Intent::Factual, DowngradeReason::NoTimeExpression);
        v.inc(Intent::Affective, Intent::Factual, DowngradeReason::NoCognitiveState);
        assert_eq!(
            v.get(Intent::Episodic, Intent::Factual, DowngradeReason::NoTimeExpression),
            2
        );
        assert_eq!(
            v.get(Intent::Affective, Intent::Factual, DowngradeReason::NoCognitiveState),
            1
        );
        assert_eq!(
            v.get(Intent::Factual, Intent::Hybrid, DowngradeReason::Other),
            0
        );
    }

    #[test]
    fn registry_record_query_counts_correctly() {
        let r = MetricsRegistry::new();
        r.record_query(
            Intent::Factual,
            ClassifierMethod::Heuristic,
            &RetrievalOutcome::Ok,
            None,
        );
        r.record_query(
            Intent::Factual,
            ClassifierMethod::Heuristic,
            &RetrievalOutcome::Ok,
            Some(BiTemporalMode::Current),
        );
        r.record_query(
            Intent::Factual,
            ClassifierMethod::LlmFallback,
            &RetrievalOutcome::AmbiguousQuery {
                candidate_intents: vec![Intent::Factual, Intent::Episodic],
                reason: "tie".into(),
            },
            None,
        );

        assert_eq!(r.queries_total.get(Intent::Factual), 3);
        assert_eq!(r.queries_total.get(Intent::Episodic), 0);
        assert_eq!(
            r.classifier_method_total
                .get(ClassifierMethod::Heuristic),
            2
        );
        assert_eq!(
            r.classifier_method_total
                .get(ClassifierMethod::LlmFallback),
            1
        );
        assert_eq!(
            r.outcomes_total.get(Intent::Factual, OutcomeLabel::Ok),
            2
        );
        assert_eq!(
            r.outcomes_total
                .get(Intent::Factual, OutcomeLabel::AmbiguousQuery),
            1
        );
        assert_eq!(
            r.bi_temporal_queries_total.get(BiTemporalMode::Current),
            1
        );
    }

    #[test]
    fn registry_empty_result_rate() {
        let r = MetricsRegistry::new();
        // unobserved plan → None
        assert!(r.empty_result_rate(Intent::Factual).is_none());

        r.outcomes_total.add(Intent::Factual, OutcomeLabel::Ok, 7);
        r.outcomes_total
            .add(Intent::Factual, OutcomeLabel::NoEntityFound, 3);
        let rate = r.empty_result_rate(Intent::Factual).unwrap();
        assert!((rate - 0.3).abs() < 1e-9, "got {}", rate);
    }

    #[test]
    fn registry_downgrade_reason_mapping() {
        let r = MetricsRegistry::new();
        r.record_downgrade(Intent::Episodic, Intent::Factual, "no_time_expression");
        r.record_downgrade(Intent::Episodic, Intent::Factual, "no_time_expression");
        r.record_downgrade(Intent::Abstract, Intent::Factual, "L5_unavailable");
        r.record_downgrade(Intent::Abstract, Intent::Factual, "weird_new_reason");

        assert_eq!(
            r.downgrades_total.get(
                Intent::Episodic,
                Intent::Factual,
                DowngradeReason::NoTimeExpression
            ),
            2
        );
        assert_eq!(
            r.downgrades_total.get(
                Intent::Abstract,
                Intent::Factual,
                DowngradeReason::L5Unavailable
            ),
            1
        );
        assert_eq!(
            r.downgrades_total
                .get(Intent::Abstract, Intent::Factual, DowngradeReason::Other),
            1
        );
    }

    #[test]
    fn registry_classifier_llm_call() {
        let r = MetricsRegistry::new();
        r.record_classifier_llm_call(120, 30, 0.250);
        r.record_classifier_llm_call(80, 20, 0.500);

        assert_eq!(r.classifier_llm_calls_total.get(), 2);
        assert_eq!(
            r.classifier_llm_tokens_total.get(TokenDirection::Prompt),
            200
        );
        assert_eq!(
            r.classifier_llm_tokens_total
                .get(TokenDirection::Completion),
            50
        );
        assert_eq!(r.classifier_llm_duration_seconds.count(), 2);
        let sum = r.classifier_llm_duration_seconds.sum();
        assert!((sum - 0.75).abs() < 1e-3, "sum was {}", sum);
    }

    #[test]
    fn registry_stage_latency_histogram() {
        let r = MetricsRegistry::new();
        r.record_stage_latency(Intent::Factual, Stage::EntityResolution, 0.001);
        r.record_stage_latency(Intent::Factual, Stage::EntityResolution, 0.002);
        r.record_stage_latency(Intent::Factual, Stage::EntityResolution, 0.05);

        let h = r
            .latency_seconds
            .get(Intent::Factual, Stage::EntityResolution);
        assert_eq!(h.count(), 3);
        // unrelated bucket untouched
        let h2 = r.latency_seconds.get(Intent::Episodic, Stage::Fusion);
        assert_eq!(h2.count(), 0);
    }

    #[test]
    fn registry_affect_rank_gauge() {
        let r = MetricsRegistry::new();
        // initial value is 0
        assert_eq!(r.affect_rank_divergence.get_f64(), 0.0);
        r.set_affect_rank_divergence(0.832);
        assert!((r.affect_rank_divergence.get_f64() - 0.832).abs() < 1e-5);
        r.set_affect_rank_divergence(-0.1);
        assert!((r.affect_rank_divergence.get_f64() + 0.1).abs() < 1e-5);
    }

    #[test]
    fn registry_hybrid_truncation_and_cost_cap() {
        let r = MetricsRegistry::new();
        r.record_hybrid_truncation(DroppedKind::Affective);
        r.record_hybrid_truncation(DroppedKind::Affective);
        r.record_hybrid_truncation(DroppedKind::Temporal);
        r.record_cost_cap_hit(CostCap::MaxAnchors);
        r.record_cost_cap_hit(CostCap::MaxAnchors);
        r.record_cost_cap_hit(CostCap::KSeed);

        assert_eq!(r.hybrid_truncation_total.get(DroppedKind::Affective), 2);
        assert_eq!(r.hybrid_truncation_total.get(DroppedKind::Temporal), 1);
        assert_eq!(r.hybrid_truncation_total.get(DroppedKind::Entity), 0);
        assert_eq!(r.cost_cap_hit_total.get(CostCap::MaxAnchors), 2);
        assert_eq!(r.cost_cap_hit_total.get(CostCap::KSeed), 1);
    }

    #[test]
    fn render_prometheus_contains_all_metric_names() {
        let r = MetricsRegistry::new();
        r.record_query(
            Intent::Factual,
            ClassifierMethod::Heuristic,
            &RetrievalOutcome::Ok,
            Some(BiTemporalMode::Current),
        );
        r.record_stage_latency(Intent::Factual, Stage::EntityResolution, 0.002);
        r.record_downgrade(Intent::Episodic, Intent::Factual, "no_time_expression");
        r.record_cost_cap_hit(CostCap::MaxAnchors);
        r.record_classifier_llm_call(10, 5, 0.1);
        r.record_hybrid_truncation(DroppedKind::Affective);
        r.set_affect_rank_divergence(0.5);

        let out = render_prometheus(&r);

        // Every § 8.1 metric family is named in the output.
        for name in [
            "retrieval_queries_total",
            "retrieval_latency_seconds",
            "retrieval_downgrades_total",
            "retrieval_cost_cap_hit_total",
            "retrieval_outcomes_total",
            "retrieval_classifier_method_total",
            "retrieval_bi_temporal_queries_total",
            "retrieval_classifier_llm_calls_total",
            "retrieval_classifier_llm_tokens_total",
            "retrieval_classifier_llm_duration_seconds",
            "retrieval_hybrid_truncation_total",
            "retrieval_affect_rank_divergence",
        ] {
            assert!(
                out.contains(name),
                "rendered output missing metric {name}\n---\n{out}"
            );
        }

        // HELP / TYPE comment lines
        assert!(out.contains("# HELP retrieval_queries_total"));
        assert!(out.contains("# TYPE retrieval_latency_seconds histogram"));

        // At least one observed sample is present with its label.
        assert!(out.contains("retrieval_queries_total{plan=\"factual\"} 1"));
        assert!(out.contains("retrieval_cost_cap_hit_total{cap=\"max_anchors\"} 1"));
        assert!(out.contains("retrieval_classifier_llm_calls_total 1"));
        assert!(out.contains("retrieval_affect_rank_divergence 0.5"));

        // Histogram +Inf bucket appears for stage histograms.
        assert!(out.contains("le=\"+Inf\""));
    }

    #[test]
    fn render_prometheus_is_deterministic() {
        let r = MetricsRegistry::new();
        r.record_query(
            Intent::Hybrid,
            ClassifierMethod::Heuristic,
            &RetrievalOutcome::Ok,
            None,
        );
        let a = render_prometheus(&r);
        let b = render_prometheus(&r);
        assert_eq!(a, b);
    }

    #[test]
    fn registry_is_thread_safe() {
        // Spawn multiple writers; ensure final counts match the sum of
        // their individual writes. AtomicU64 backs this; this test just
        // guards against accidental introduction of an interior mutex
        // or non-atomic path.
        use std::sync::Arc;
        use std::thread;

        let r = Arc::new(MetricsRegistry::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let r = Arc::clone(&r);
            handles.push(thread::spawn(move || {
                for _ in 0..1_000 {
                    r.queries_total.inc(Intent::Factual);
                    r.record_stage_latency(Intent::Factual, Stage::Fusion, 0.001);
                    r.classifier_llm_calls_total.inc();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(r.queries_total.get(Intent::Factual), 8_000);
        assert_eq!(r.classifier_llm_calls_total.get(), 8_000);
        assert_eq!(
            r.latency_seconds
                .get(Intent::Factual, Stage::Fusion)
                .count(),
            8_000
        );
    }
}
