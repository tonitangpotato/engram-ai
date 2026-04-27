//! Cost-isolated counters for the Knowledge Compiler (§5bis.7 / GOAL-3.13).
//!
//! Per design §5bis.7, LLM calls and timing made by the compiler are tagged
//! with `purpose = "knowledge_compile"` and counted separately from
//! retrieval-time LLM calls. This module exposes the metric *namespace*
//! contract — wiring these counters into a global metrics registry
//! (Prometheus, OpenTelemetry, etc.) is out of scope for this task and
//! belongs to whichever observability adapter the deployment uses.
//!
//! ## Metric names (§5bis.7)
//!
//! - `knowledge_compile_llm_calls_total{model=...}` — counter
//! - `knowledge_compile_duration_seconds` — histogram
//! - `knowledge_compile_topics_written_total` — counter per run
//!
//! Additional counters added here for operational visibility (not on the
//! design's contract list, but useful and cheap):
//!
//! - `knowledge_compile_candidates_total` — K1 candidates considered
//! - `knowledge_compile_clusters_total` — K2 clusters formed
//! - `knowledge_compile_topics_superseded_total` — K3 supersession count
//! - `knowledge_compile_cluster_failures_total` — K3 per-cluster failures
//! - `knowledge_compile_embedding_calls_total` — K3 embedder calls
//!
//! All counters are `AtomicU64` so the type is `Sync` and shareable across
//! threads (the compiler is single-threaded today, but the metrics struct
//! lives in the operator's hot dashboard path which may sample concurrently).

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters for one process-lifetime of compile activity.
///
/// `Default` zero-initializes everything. Reset between runs is **not**
/// provided intentionally — these are *cumulative* operational counters,
/// matching how Prometheus counters work. Per-run aggregates are returned
/// in [`crate::memory::CompileReport`].
#[derive(Debug, Default)]
pub struct CompileMetrics {
    candidates_total: AtomicU64,
    clusters_total: AtomicU64,
    topics_written_total: AtomicU64,
    topics_superseded_total: AtomicU64,
    cluster_failures_total: AtomicU64,
    llm_calls_total: AtomicU64,
    embedding_calls_total: AtomicU64,
}

impl CompileMetrics {
    /// Construct a fresh zero-initialized counter set.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record_candidates(&self, n: usize) {
        self.candidates_total.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub(crate) fn record_clusters(&self, n: usize) {
        self.clusters_total.fetch_add(n as u64, Ordering::Relaxed);
    }

    pub(crate) fn record_topic_written(&self) {
        self.topics_written_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_topic_superseded(&self) {
        self.topics_superseded_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_cluster_failure(&self) {
        self.cluster_failures_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_llm_call(&self) {
        self.llm_calls_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_embedding_call(&self) {
        self.embedding_calls_total.fetch_add(1, Ordering::Relaxed);
    }

    // ─── Read-side accessors (operator-facing) ───────────────────────

    pub fn candidates_total(&self) -> u64 {
        self.candidates_total.load(Ordering::Relaxed)
    }
    pub fn clusters_total(&self) -> u64 {
        self.clusters_total.load(Ordering::Relaxed)
    }
    pub fn topics_written_total(&self) -> u64 {
        self.topics_written_total.load(Ordering::Relaxed)
    }
    pub fn topics_superseded_total(&self) -> u64 {
        self.topics_superseded_total.load(Ordering::Relaxed)
    }
    pub fn cluster_failures_total(&self) -> u64 {
        self.cluster_failures_total.load(Ordering::Relaxed)
    }
    pub fn llm_calls_total(&self) -> u64 {
        self.llm_calls_total.load(Ordering::Relaxed)
    }
    pub fn embedding_calls_total(&self) -> u64 {
        self.embedding_calls_total.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_zero() {
        let m = CompileMetrics::new();
        assert_eq!(m.candidates_total(), 0);
        assert_eq!(m.clusters_total(), 0);
        assert_eq!(m.topics_written_total(), 0);
        assert_eq!(m.topics_superseded_total(), 0);
        assert_eq!(m.cluster_failures_total(), 0);
        assert_eq!(m.llm_calls_total(), 0);
        assert_eq!(m.embedding_calls_total(), 0);
    }

    #[test]
    fn counters_accumulate() {
        let m = CompileMetrics::new();
        m.record_candidates(5);
        m.record_candidates(3);
        m.record_clusters(2);
        m.record_topic_written();
        m.record_topic_written();
        m.record_topic_superseded();
        m.record_cluster_failure();
        m.record_llm_call();
        m.record_llm_call();
        m.record_llm_call();
        m.record_embedding_call();

        assert_eq!(m.candidates_total(), 8);
        assert_eq!(m.clusters_total(), 2);
        assert_eq!(m.topics_written_total(), 2);
        assert_eq!(m.topics_superseded_total(), 1);
        assert_eq!(m.cluster_failures_total(), 1);
        assert_eq!(m.llm_calls_total(), 3);
        assert_eq!(m.embedding_calls_total(), 1);
    }
}
