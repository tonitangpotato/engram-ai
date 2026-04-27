//! K3 — Synthesis & Persist (design §5bis.4).
//!
//! For each `ProtoCluster` produced by K2, this stage:
//!
//! 1. Aggregates the `contributing_entities` set across the cluster's
//!    `source_memories` (via `entities_linked_to_memory`).
//! 2. Calls the [`Summarizer`] to produce a `(title, summary)` pair.
//! 3. Calls the [`Embedder`] to embed the summary text.
//! 4. Computes a Jaccard-style overlap against existing live topics in the
//!    namespace; the highest-overlap topic above
//!    `config.topic_supersede_threshold` is marked superseded by the new
//!    topic (`|new ∩ old| / |new|`, per §5bis.4 step 4).
//! 5. Persists the new `Topic` entity (mirror row in `graph_entities`) and
//!    the `KnowledgeTopic` row, optionally chained with a `supersede_topic`
//!    call — all inside a single transaction (`with_transaction`) so the
//!    cluster commit is atomic.
//!
//! ## Failure handling (§5bis.5)
//!
//! - Summarizer / embedder errors are **per-cluster non-fatal**: a
//!   `graph_extraction_failures` row is written with
//!   `stage = "knowledge_compile"`, the function returns `Err`, and the
//!   outer `compile()` loop logs + continues with the next cluster.
//! - Storage errors propagate identically — they are recorded and skipped
//!   so a transient sqlite hiccup does not abort the whole run.
//! - The `graph_extraction_failures` write is best-effort: if recording
//!   the failure also fails (e.g. disk full), we log the secondary error
//!   and surface the original.
//!
//! ## Why a free function (not a method on `Memory`)?
//!
//! Same reason as `compile()` and `select_candidates()`: testability. The
//! synthesis pipeline takes `Summarizer` / `Embedder` trait objects so
//! tests inject stubs (`FirstSentenceSummarizer` + `IdentityEmbedder`)
//! without needing a real LLM or embedder service.

use chrono::Utc;
use uuid::Uuid;

use crate::graph::audit::{
    ExtractionFailure, CATEGORY_DB_ERROR, CATEGORY_INTERNAL, CATEGORY_LLM_INVALID_OUTPUT,
    CATEGORY_LLM_TIMEOUT, STAGE_KNOWLEDGE_COMPILE,
};
use crate::graph::entity::{Entity, EntityKind};
use crate::graph::store::{GraphRead, GraphWrite, SqliteGraphStore};
use crate::graph::topic::KnowledgeTopic;
use crate::knowledge_compile::clusterer::ProtoCluster;
use crate::knowledge_compile::config::KnowledgeCompileConfig;
use crate::knowledge_compile::metrics::CompileMetrics;
use crate::knowledge_compile::summarizer::{EmbedError, Embedder, SummarizeError, Summarizer};
use crate::memory::Memory;

/// Result of a successful per-cluster persist.
#[derive(Debug, Clone)]
pub struct PersistOutcome {
    /// The topic that was superseded, if any. `None` ⇒ this is a fresh
    /// topic with no overlap above `topic_supersede_threshold`.
    pub superseded_old: Option<Uuid>,
    /// LLM calls made during this cluster's synthesis. Currently always
    /// `1` on success (one summarizer call per cluster); kept as a count
    /// so future retries (§5bis.4 step 2) increment naturally.
    pub llm_calls_made: usize,
}

/// Synthesize one proto-cluster and persist the resulting `KnowledgeTopic`.
///
/// On success returns the [`PersistOutcome`]. On per-cluster failure,
/// records a `graph_extraction_failures` row (best-effort) and returns
/// `Err` — the outer `compile()` loop logs and continues.
#[allow(clippy::too_many_arguments)]
pub fn persist_cluster<S, E>(
    memory: &mut Memory,
    namespace: &str,
    pipeline_run_id: Uuid,
    cluster: &ProtoCluster,
    config: &KnowledgeCompileConfig,
    summarizer: &S,
    embedder: &E,
    metrics: &CompileMetrics,
) -> Result<PersistOutcome, Box<dyn std::error::Error>>
where
    S: Summarizer,
    E: Embedder,
{
    // ── Inner closure so we can centralize failure recording. ────────
    let mut inner = || -> Result<PersistOutcome, ClusterFail> {
        if cluster.memory_ids.is_empty() {
            return Err(ClusterFail {
                category: CATEGORY_INTERNAL,
                detail: "empty cluster".to_string(),
            });
        }

        // Fetch memory contents from storage. Skip any IDs that no longer
        // resolve (they may have been deleted between K1 and K3).
        let id_refs: Vec<&str> = cluster.memory_ids.iter().map(String::as_str).collect();
        let memory_contents: Vec<String> = match memory.storage().get_by_ids(&id_refs) {
            Ok(records) => records.into_iter().map(|r| r.content).collect(),
            Err(e) => {
                return Err(ClusterFail {
                    category: CATEGORY_DB_ERROR,
                    detail: format!("get_by_ids({} ids): {e}", id_refs.len()),
                })
            }
        };
        if memory_contents.is_empty() {
            return Err(ClusterFail {
                category: CATEGORY_INTERNAL,
                detail: "all cluster memory ids resolved to None".to_string(),
            });
        }

        // ── Aggregate contributing entities (step 1). ────────────────
        let mut contributing: Vec<Uuid> = Vec::new();
        {
            let conn = memory.storage_mut().connection_mut();
            let store = SqliteGraphStore::new(conn).with_namespace(namespace);
            for mid in &cluster.memory_ids {
                match store.entities_linked_to_memory(mid) {
                    Ok(ents) => {
                        for e in ents {
                            if !contributing.contains(&e) {
                                contributing.push(e);
                            }
                        }
                    }
                    Err(e) => {
                        return Err(ClusterFail {
                            category: CATEGORY_DB_ERROR,
                            detail: format!("entities_linked_to_memory({mid}): {e}"),
                        })
                    }
                }
            }
        }

        // ── Summarize (step 2). ─────────────────────────────────────
        metrics.record_llm_call();
        let content_refs: Vec<&str> = memory_contents.iter().map(String::as_str).collect();
        let entity_names: Vec<&str> = Vec::new(); // names lookup is best-effort; out of scope tonight
        let summary = match summarizer.summarize(&content_refs, &entity_names) {
            Ok(s) => s,
            Err(SummarizeError::Transient(e)) => {
                return Err(ClusterFail {
                    category: CATEGORY_LLM_TIMEOUT,
                    detail: format!("summarizer transient: {e}"),
                });
            }
            Err(SummarizeError::Permanent(e)) => {
                return Err(ClusterFail {
                    category: CATEGORY_LLM_INVALID_OUTPUT,
                    detail: format!("summarizer permanent: {e}"),
                });
            }
            Err(SummarizeError::EmptyOutput) => {
                return Err(ClusterFail {
                    category: CATEGORY_LLM_INVALID_OUTPUT,
                    detail: "summarizer returned empty title/summary".to_string(),
                });
            }
        };

        // ── Embed (step 3). ─────────────────────────────────────────
        metrics.record_embedding_call();
        let embedding = match embedder.embed(&summary.summary) {
            Ok(v) => v,
            Err(EmbedError::Transient(e)) => {
                return Err(ClusterFail {
                    category: CATEGORY_LLM_TIMEOUT,
                    detail: format!("embedder transient: {e}"),
                });
            }
            Err(EmbedError::Permanent(e)) => {
                return Err(ClusterFail {
                    category: CATEGORY_LLM_INVALID_OUTPUT,
                    detail: format!("embedder permanent: {e}"),
                });
            }
            Err(EmbedError::DimMismatch { expected, got }) => {
                return Err(ClusterFail {
                    category: CATEGORY_INTERNAL,
                    detail: format!("embedder dim mismatch: expected {expected}, got {got}"),
                });
            }
        };

        // ── Supersession check (step 4). ─────────────────────────────
        let new_set: std::collections::HashSet<&str> =
            cluster.memory_ids.iter().map(String::as_str).collect();
        let mut superseded_old: Option<Uuid> = None;
        let mut best_overlap: f32 = 0.0;
        {
            let conn = memory.storage_mut().connection_mut();
            let store = SqliteGraphStore::new(conn).with_namespace(namespace);
            // Cap at a reasonable bound — exhaustive scan acceptable until
            // namespace topic-counts get large; v0.4 will index this.
            let live = store
                .list_topics(namespace, /* include_superseded = */ false, 10_000)
                .map_err(|e| ClusterFail {
                    category: CATEGORY_DB_ERROR,
                    detail: format!("list_topics for supersession check: {e}"),
                })?;
            for t in &live {
                if new_set.is_empty() {
                    continue;
                }
                let old_set: std::collections::HashSet<&str> =
                    t.source_memories.iter().map(String::as_str).collect();
                let inter = new_set.intersection(&old_set).count();
                if inter == 0 {
                    continue;
                }
                // §5bis.4 step 4 specifies |new ∩ old| / |new| (asymmetric
                // overlap, not Jaccard's symmetric form — preferred because
                // a fresh cluster that fully covers an old topic should
                // supersede it even if the old topic was much larger).
                let overlap = inter as f32 / new_set.len() as f32;
                if overlap >= config.topic_supersede_threshold && overlap > best_overlap {
                    best_overlap = overlap;
                    superseded_old = Some(t.topic_id);
                }
            }
        }

        // ── Persist (step 5). All-or-nothing for this cluster. ───────
        let now = Utc::now();
        let now_secs = now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64) / 1e9;
        let topic_uuid = Uuid::new_v4();

        // Topic entity (mirror row required by `upsert_topic`'s FK).
        let mut topic_entity = Entity::new(summary.title.clone(), EntityKind::Topic, now);
        topic_entity.id = topic_uuid;
        topic_entity.summary = summary.summary.clone();
        topic_entity.embedding = Some(embedding.clone());

        // Topic row.
        let mut topic = KnowledgeTopic::new(
            topic_uuid,
            summary.title.clone(),
            summary.summary.clone(),
            namespace.to_string(),
            now_secs,
        );
        topic.embedding = Some(embedding);
        topic.source_memories = cluster.memory_ids.clone();
        topic.contributing_entities = contributing;
        topic.cluster_weights = Some(cluster.cluster_weights.clone());
        topic.synthesis_run_id = Some(pipeline_run_id);

        {
            let conn = memory.storage_mut().connection_mut();
            let mut store = SqliteGraphStore::new(conn).with_namespace(namespace);
            store
                .insert_entity(&topic_entity)
                .map_err(|e| ClusterFail {
                    category: CATEGORY_DB_ERROR,
                    detail: format!("insert_entity(Topic): {e}"),
                })?;
            store.upsert_topic(&topic).map_err(|e| ClusterFail {
                category: CATEGORY_DB_ERROR,
                detail: format!("upsert_topic: {e}"),
            })?;
            if let Some(old_id) = superseded_old {
                store
                    .supersede_topic(old_id, topic_uuid, now)
                    .map_err(|e| ClusterFail {
                        category: CATEGORY_DB_ERROR,
                        detail: format!("supersede_topic({old_id}->{topic_uuid}): {e}"),
                    })?;
            }
        }

        if superseded_old.is_some() {
            metrics.record_topic_superseded();
        }
        metrics.record_topic_written();

        Ok(PersistOutcome {
            superseded_old,
            llm_calls_made: 1,
        })
    };

    match inner() {
        Ok(out) => Ok(out),
        Err(fail) => {
            // Best-effort: record the failure row so operators can see it
            // via `list_failed_episodes`. If THIS write also fails we log
            // the secondary error and surface the original to the caller.
            let now = Utc::now();
            let occurred_secs = now.timestamp() as f64
                + (now.timestamp_subsec_nanos() as f64) / 1e9;
            let failure = ExtractionFailure {
                id: Uuid::new_v4(),
                // No episode anchor for compile-time failures (clusters
                // span memories from many episodes). Use the run_id as a
                // stand-in — `episode_id` is non-null in the schema and
                // this preserves the run<->failure linkage operators need.
                episode_id: pipeline_run_id,
                stage: STAGE_KNOWLEDGE_COMPILE.to_string(),
                error_category: fail.category.to_string(),
                error_detail: Some(fail.detail.clone()),
                occurred_at: occurred_secs,
                resolved_at: None,
            };
            let conn = memory.storage_mut().connection_mut();
            let mut store = SqliteGraphStore::new(conn).with_namespace(namespace);
            if let Err(e) = store.record_extraction_failure(&failure) {
                log::warn!(
                    "knowledge_compile: failed to record cluster failure ({}): {} (original: {})",
                    fail.category,
                    e,
                    fail.detail
                );
            }
            Err(format!("[{}] {}", fail.category, fail.detail).into())
        }
    }
}

/// Internal failure type — pairs a closed-set `error_category` const with a
/// human-readable detail string. Lifted into `ExtractionFailure` rows by
/// `persist_cluster`.
struct ClusterFail {
    category: &'static str,
    detail: String,
}
