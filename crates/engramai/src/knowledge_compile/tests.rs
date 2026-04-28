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

// ════════════════════════════════════════════════════════════════════════
//  Memory::compile_knowledge wiring tests (ISS-051 §A.2)
//
//  These tests assert that the public `Memory::compile_knowledge*` entry
//  points actually drive the K1→K3 pipeline (write a row into
//  `graph_pipeline_runs`) — not the old zero-counter stub.
//
//  We test against `compile_knowledge_with` (deterministic injection)
//  rather than `compile_knowledge` (env-based) to avoid touching
//  process-global env state, which is racy under parallel `cargo test`
//  and would interact with `Memory::auto_configure_extractor` reading
//  `ANTHROPIC_*` in other tests in this crate.
// ════════════════════════════════════════════════════════════════════════

/// Count rows in `graph_pipeline_runs` for a given namespace.
fn count_pipeline_runs(m: &mut Memory, namespace: &str) -> i64 {
    let conn = m.storage_mut().connection_mut();
    conn.query_row(
        "SELECT COUNT(*) FROM graph_pipeline_runs
         WHERE namespace = ?1 AND kind = 'knowledge_compile'",
        [namespace],
        |row| row.get(0),
    )
    .expect("count query")
}

/// Build a Memory that has a non-functional but `is_some()` embedding
/// provider — sufficient for the empty-namespace path that never
/// actually calls `embed()`. We point it at a non-existent Ollama host
/// so accidentally activating the embed path errors loudly instead of
/// silently hitting a real model.
fn memory_with_dummy_embedder() -> Memory {
    let mut m = fresh_memory();
    let cfg = crate::embeddings::EmbeddingConfig::ollama("nonexistent-model", 768);
    let provider = crate::embeddings::EmbeddingProvider::new(cfg);
    m.set_embedding_provider_for_test(provider);
    m
}

#[test]
fn compile_knowledge_with_drives_real_pipeline_not_stub() {
    // Stub-buster: the pre-ISS-051 implementation returned a zero-counter
    // `CompileReport` without touching the database. A real run *must*
    // write a `graph_pipeline_runs` row even on an empty namespace.
    let mut m = memory_with_dummy_embedder();
    let summarizer = FirstSentenceSummarizer; // Never actually called here.

    let pre = count_pipeline_runs(&mut m, "test-ns");
    let report = m
        .compile_knowledge_with("test-ns", &summarizer)
        .expect("empty-namespace compile must succeed");
    let post = count_pipeline_runs(&mut m, "test-ns");

    assert_eq!(
        post - pre,
        1,
        "compile_knowledge must write exactly one graph_pipeline_runs row \
         (proves the stub was replaced); got delta={}",
        post - pre
    );
    assert_eq!(report.candidates_considered, 0);
    assert_eq!(report.clusters_formed, 0);
    assert_eq!(report.topics_written, 0);
    assert_ne!(
        report.run_id,
        uuid::Uuid::nil(),
        "run_id must be a real ledger uuid, not nil"
    );
}

#[test]
fn compile_knowledge_with_errors_when_no_embedder() {
    let mut m = fresh_memory();
    // Force embedding to None even if Ollama happens to be running on
    // this host (otherwise Memory::new auto-detects and configures one).
    m.clear_embedding_provider_for_test();
    let summarizer = FirstSentenceSummarizer;

    let err = m
        .compile_knowledge_with("test-ns", &summarizer)
        .expect_err("must error when embedding=None");
    let msg = err.to_string();
    assert!(
        msg.contains("embedding") || msg.contains("embedder"),
        "error must mention the embedder requirement; got: {msg}"
    );
}

#[test]
fn compile_knowledge_restores_embedder_on_success() {
    // Regression guard for the `take()` / restore pattern: after a
    // successful run, `Memory::embedding` must still be `Some` so
    // subsequent calls (e.g. memory store + embed) keep working.
    let mut m = memory_with_dummy_embedder();
    let summarizer = FirstSentenceSummarizer;

    assert!(m.has_embedding_support(), "precondition: embedder installed");
    let _ = m
        .compile_knowledge_with("test-ns", &summarizer)
        .expect("compile ok");
    assert!(
        m.has_embedding_support(),
        "embedder must be restored after compile_knowledge"
    );
}
