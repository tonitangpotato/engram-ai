//! # Knowledge Compiler (v0.3 — L5 Synthesis)
//!
//! Background job that distills clusters of memories into `KnowledgeTopic`
//! rows. See `.gid/features/v03-resolution/design.md` §5bis (Knowledge
//! Compiler).
//!
//! **Scope boundary.** This is the **v0.3** Knowledge Compiler. The existing
//! [`crate::compiler`] module is the **v0.2** KnowledgeCompiler (`api.rs`,
//! `compilation.rs`, `intake.rs`, etc.) and is preserved verbatim during the
//! migration window. The two modules share *nothing* but the conceptual goal;
//! the v0.2 module continues to serve v0.2 callers, and is replaced by this
//! module at end-of-migration (out of scope tonight, per build plan).
//!
//! ## Why a separate module?
//!
//! - v0.2 `compiler` predates the v0.3 `graph_*` schema; rewriting it in
//!   place would entangle a behavior change with a storage migration. Side-
//!   by-side keeps the migration auditable (GUARD-3 — never destructive).
//! - v0.2 writes its own `topics` rows; v0.3 writes `knowledge_topics` rows
//!   tied to the v0.3 `Entity::Topic` UUID (v03-graph-layer §4.1). The
//!   storage *contract* differs.
//!
//! ## Stages (design §5bis.2 / §5bis.3 / §5bis.4)
//!
//! ```text
//! K1: candidate selection   — memories above importance threshold,
//!                              created since the last run watermark
//! K2: clustering            — pluggable [`Clusterer`] trait;
//!                              default = embedding-space Infomap
//! K3: synthesis & persist   — per-cluster: aggregate entities → LLM
//!                              summary → topic embedding → atomic
//!                              upsert with supersession check
//! ```
//!
//! ## Public surface
//!
//! - [`compile`] — entry point; the function `Memory::compile_knowledge`
//!   delegates to.
//! - [`list_topics`] — thin re-export over the storage `list_topics` for
//!   `Memory::list_knowledge_topics`'s body. (Memory's wrapper already
//!   exists in `memory.rs`; this is mirrored here for callers that want
//!   to bypass the `Memory` facade — e.g. tests.)
//! - [`Clusterer`] / [`ProtoCluster`] — clustering trait + result type;
//!   any deterministic implementation is acceptable (§5bis.3 contract).
//! - [`Summarizer`] / [`Embedder`] — abstractions over the LLM and
//!   embedder so the compiler is unit-testable without the network. The
//!   production wiring uses the engramai `EmbeddingClient` + an
//!   anthropic-backed summarizer; tests use fixed-output stubs.
//! - [`CompileMetrics`] — atomic in-memory counters for the
//!   `knowledge_compile_*` metric family (§5bis.7 cost isolation).
//! - [`KnowledgeCompileConfig`] — knob bag: `compile_min_importance`,
//!   `compile_max_candidates_per_run`, `compile_max_duration`,
//!   `topic_supersede_threshold`, `compiler_interval_hours`.
//! - The aggregate report is the existing [`crate::memory::CompileReport`]
//!   (re-exported) — its location predates this module per the §6.2
//!   "stable public surface" note in `memory.rs`.
//!
//! ## What this module does NOT do
//!
//! - **Scheduling.** The "every `compiler_interval_hours`" cadence is the
//!   caller's responsibility (cron, systemd timer, in-process scheduler).
//!   The module is an on-demand entry point only.
//! - **v0.2 compatibility.** This compiler does not read or write v0.2
//!   `topics`. v0.2 callers continue to use [`crate::compiler`].
//! - **Retrieval-time L5 plan.** That is `v03-retrieval` §4.4
//!   (`task:retr-impl-abstract-l5`), which *consumes* the
//!   `knowledge_topics` rows this module produces.

pub mod candidates;
pub mod clusterer;
pub mod config;
pub mod metrics;
pub mod summarizer;
pub mod synthesis;

pub use candidates::{select_candidates, CandidateMemory};
pub use clusterer::{Clusterer, ClusterError, EmbeddingInfomapClusterer, ProtoCluster};
pub use config::KnowledgeCompileConfig;
pub use metrics::CompileMetrics;
pub use summarizer::{Embedder, EmbedError, FirstSentenceSummarizer, IdentityEmbedder, Summarizer, SummarizeError};
pub use synthesis::{persist_cluster, PersistOutcome};

// Re-export the aggregate report from its current location in `memory.rs`.
// Per the `CompileReport` doc-comment there: A.2 may re-export it from the
// `knowledge_compile` module to preserve `Memory::compile_knowledge`'s
// signature without breaking any caller. The struct definition stays in
// `memory.rs` for that reason — moving it would force every downstream
// caller to change imports.
pub use crate::memory::CompileReport;

use std::time::Instant;
use uuid::Uuid;

use crate::graph::audit::{PipelineKind, RunStatus};
use crate::graph::store::{GraphRead, GraphWrite, SqliteGraphStore};
use crate::memory::Memory;

/// Entry point for one knowledge-compile run over `namespace`.
///
/// This is the body that `Memory::compile_knowledge` should call once the
/// stub is replaced (TODO in `memory.rs`). Splitting it into a free
/// function (rather than an inherent method on `Memory`) keeps the
/// compile pipeline injectable — tests can drive it with stub
/// `Summarizer` / `Embedder` / `Clusterer` impls without constructing a
/// full `Memory`.
///
/// # Stages
///
/// 1. K1 — [`select_candidates`] picks memories above
///    `config.compile_min_importance` since the last run, capped at
///    `config.compile_max_candidates_per_run`.
/// 2. K2 — `clusterer.cluster(...)` groups them into [`ProtoCluster`]s.
/// 3. K3 — for each cluster, [`persist_cluster`] runs the LLM summary,
///    embeds, and atomically upserts the `KnowledgeTopic` (with
///    supersession check on `topic_supersede_threshold`).
///
/// # Run ledger (§5bis.1)
///
/// One `graph_pipeline_runs` row of kind [`PipelineKind::KnowledgeCompile`]
/// is opened at the start and closed with the aggregate counters. Per-cluster
/// failures are recorded as `graph_extraction_failures` (`stage = "knowledge_compile"`)
/// **without** failing the whole run — partial progress is preserved per
/// design §5bis.5.
///
/// # Errors
///
/// * Returns `Err` only on fatal failures (e.g. cannot open the run ledger,
///   cannot read candidates). Per-cluster LLM / embedding failures are
///   recorded and skipped, so the run can still produce some topics.
pub fn compile<C, S, E>(
    memory: &mut Memory,
    namespace: &str,
    config: &KnowledgeCompileConfig,
    clusterer: &C,
    summarizer: &S,
    embedder: &E,
    metrics: &CompileMetrics,
) -> Result<CompileReport, Box<dyn std::error::Error>>
where
    C: Clusterer,
    S: Summarizer,
    E: Embedder,
{
    let started = Instant::now();
    let run_id_local = Uuid::new_v4();

    // Open run ledger row (§5bis.1).
    let input_summary = serde_json::json!({
        "namespace": namespace,
        "compile_min_importance": config.compile_min_importance,
        "compile_max_candidates_per_run": config.compile_max_candidates_per_run,
        "topic_supersede_threshold": config.topic_supersede_threshold,
        "local_run_id": run_id_local,
    });
    let pipeline_run_id = {
        let conn = memory.storage_mut().connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace(namespace);
        store.begin_pipeline_run(PipelineKind::KnowledgeCompile, input_summary)?
    };

    // K1 — candidate selection.
    let candidates = select_candidates(memory, namespace, config)?;
    let candidates_considered = candidates.len();
    metrics.record_candidates(candidates_considered);

    if candidates.is_empty() {
        // Close run with zero-counter summary; not a failure.
        finish_run(
            memory,
            namespace,
            pipeline_run_id,
            RunStatus::Succeeded,
            CompileSummary::default(),
            None,
        )?;
        return Ok(CompileReport {
            run_id: pipeline_run_id,
            candidates_considered: 0,
            clusters_formed: 0,
            topics_written: 0,
            topics_superseded: 0,
            llm_calls: 0,
            duration: started.elapsed(),
        });
    }

    // K2 — clustering. Pure function: deterministic on the same input set.
    let clusters = clusterer
        .cluster(&candidates, /* affect_bias = */ None)
        .map_err(|e| -> Box<dyn std::error::Error> {
            // Cluster failure is fatal for the run — the next attempt is the
            // next scheduler tick. Record and bail.
            let detail = format!("clusterer error: {e}");
            // Best-effort close; ignore inner error to surface the original.
            let _ = finish_run(
                memory,
                namespace,
                pipeline_run_id,
                RunStatus::Failed,
                CompileSummary::default(),
                Some(detail.clone()),
            );
            detail.into()
        })?;
    metrics.record_clusters(clusters.len());

    // K3 — synthesize each cluster. Per-cluster failure is non-fatal.
    let mut topics_written = 0;
    let mut topics_superseded = 0;
    let mut llm_calls = 0;
    let deadline = started + config.compile_max_duration;

    for cluster in &clusters {
        if Instant::now() >= deadline {
            // Timeout: stop the loop, mark partial. §5bis.5: completed
            // clusters are kept (each was committed in its own inner tx).
            let summary = CompileSummary {
                topics_written,
                topics_superseded,
                llm_calls,
            };
            finish_run(
                memory,
                namespace,
                pipeline_run_id,
                RunStatus::Failed,
                summary,
                Some(format!(
                    "timeout after {:?}, {} clusters completed of {}",
                    config.compile_max_duration,
                    topics_written,
                    clusters.len()
                )),
            )?;
            return Ok(CompileReport {
                run_id: pipeline_run_id,
                candidates_considered,
                clusters_formed: clusters.len(),
                topics_written,
                topics_superseded,
                llm_calls,
                duration: started.elapsed(),
            });
        }

        match persist_cluster(
            memory,
            namespace,
            pipeline_run_id,
            cluster,
            config,
            summarizer,
            embedder,
            metrics,
        ) {
            Ok(PersistOutcome { superseded_old, llm_calls_made }) => {
                topics_written += 1;
                if superseded_old.is_some() {
                    topics_superseded += 1;
                }
                llm_calls += llm_calls_made;
            }
            Err(e) => {
                // Non-fatal: log/record and continue with next cluster.
                // (The synthesis module already wrote a stage_failures row;
                // here we only ensure the run keeps going.)
                metrics.record_cluster_failure();
                log::warn!(
                    "knowledge_compile: cluster failed in run {pipeline_run_id} ({namespace}): {e}"
                );
            }
        }
    }

    // Close run successfully (partial topics is still success, per
    // §5bis.5 — failed clusters are recorded as stage failures, not
    // a run-level failure).
    let summary = CompileSummary {
        topics_written,
        topics_superseded,
        llm_calls,
    };
    finish_run(
        memory,
        namespace,
        pipeline_run_id,
        RunStatus::Succeeded,
        summary,
        None,
    )?;

    Ok(CompileReport {
        run_id: pipeline_run_id,
        candidates_considered,
        clusters_formed: clusters.len(),
        topics_written,
        topics_superseded,
        llm_calls,
        duration: started.elapsed(),
    })
}

/// Aggregate counters folded into `output_summary` of the run row.
///
/// Sub-schema of `graph_pipeline_runs.output_summary` per §5bis.1; readers
/// that want resumable state (the K1 watermark) parse this back.
#[derive(Debug, Clone, Default)]
struct CompileSummary {
    topics_written: usize,
    topics_superseded: usize,
    llm_calls: usize,
}

fn finish_run(
    memory: &mut Memory,
    namespace: &str,
    run_id: Uuid,
    status: RunStatus,
    summary: CompileSummary,
    error_detail: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = serde_json::json!({
        "topics_written": summary.topics_written,
        "topics_superseded": summary.topics_superseded,
        "llm_calls": summary.llm_calls,
    });
    let conn = memory.storage_mut().connection_mut();
    let mut store = SqliteGraphStore::new(conn).with_namespace(namespace);
    store.finish_pipeline_run(run_id, status, Some(output), error_detail.as_deref())?;
    Ok(())
}

/// Convenience read for callers that want to enumerate live (or all) topics
/// in `namespace`. `Memory::list_knowledge_topics` already exists in
/// `memory.rs`; this is the equivalent for callers that don't have a
/// `Memory` (e.g. integration tests that drove `compile()` directly).
pub fn list_topics(
    memory: &mut Memory,
    namespace: &str,
    include_superseded: bool,
    limit: usize,
) -> Result<Vec<crate::graph::topic::KnowledgeTopic>, Box<dyn std::error::Error>> {
    let conn = memory.storage_mut().connection_mut();
    let store = SqliteGraphStore::new(conn).with_namespace(namespace);
    Ok(store.list_topics(namespace, include_superseded, limit)?)
}

#[cfg(test)]
mod tests;
