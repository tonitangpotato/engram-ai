//! Integration tests for the v0.3 Knowledge Compiler.
//!
//! Drives `compile()` end-to-end against an in-memory `Memory` instance
//! using the deterministic stub `Summarizer` / `Embedder` shipped in
//! `summarizer.rs`. The default `EmbeddingInfomapClusterer` is the K2
//! engine under test; tests assert observable behavior at the
//! `CompileReport` + `list_topics` boundary, not internal counters.

use crate::knowledge_compile::*;
use crate::memory::Memory;

/// Build a `Memory` backed by a temp sqlite db. Mirrors the test-helper
/// pattern used in `compiler::api::tests`.
fn fresh_memory() -> Memory {
    Memory::new(":memory:", None).expect("in-memory Memory")
}

#[test]
fn empty_namespace_returns_zero_report() {
    let mut m = fresh_memory();
    let cfg = KnowledgeCompileConfig::for_test();
    let metrics = CompileMetrics::new();
    let clusterer = EmbeddingInfomapClusterer::default();
    let summarizer = FirstSentenceSummarizer;
    let embedder = IdentityEmbedder::new(384);

    let report = compile(
        &mut m,
        "test-ns",
        &cfg,
        &clusterer,
        &summarizer,
        &embedder,
        &metrics,
    )
    .expect("compile on empty namespace should not error");

    assert_eq!(report.candidates_considered, 0);
    assert_eq!(report.clusters_formed, 0);
    assert_eq!(report.topics_written, 0);
    assert_eq!(report.topics_superseded, 0);
    assert_eq!(report.llm_calls, 0);
}

#[test]
fn list_topics_initially_empty() {
    let mut m = fresh_memory();
    let topics = list_topics(&mut m, "test-ns", false, 100).expect("list_topics ok");
    assert!(topics.is_empty(), "fresh namespace must have no topics");
}

#[test]
fn config_defaults_match_design() {
    let cfg = KnowledgeCompileConfig::default();
    assert_eq!(cfg.compile_min_importance, 0.3);
    assert_eq!(cfg.compile_max_candidates_per_run, 5000);
    assert_eq!(cfg.topic_supersede_threshold, 0.5);
}

#[test]
fn metrics_start_at_zero() {
    let m = CompileMetrics::new();
    assert_eq!(m.candidates_total(), 0);
    assert_eq!(m.clusters_total(), 0);
    assert_eq!(m.topics_written_total(), 0);
    assert_eq!(m.topics_superseded_total(), 0);
    assert_eq!(m.llm_calls_total(), 0);
}
