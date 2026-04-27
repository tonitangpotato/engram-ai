//! K1 — Candidate Selection (design §5bis.2).
//!
//! Selects the slice of memories eligible for compilation in this run:
//!
//! 1. Owned by `namespace`.
//! 2. Importance ≥ `config.compile_min_importance`.
//! 3. Created since the last successful run's high-water mark (computed
//!    from `graph_pipeline_runs.output_summary.high_water_created_at` —
//!    when no prior successful run exists, all eligible memories qualify).
//! 4. Capped at `config.compile_max_candidates_per_run` (newest first).
//!
//! ## Why a free function?
//!
//! Same reasoning as the entry-point `compile()`: testability without
//! constructing a full `Memory`. The function does need a `&mut Memory`
//! to read both `Storage` (for memory rows) and `SqliteGraphStore` (for
//! the run watermark) — the borrow shape comes from the shared sqlite
//! connection.

use chrono::{DateTime, Utc};

use crate::graph::audit::{PipelineKind, RunStatus};
use crate::knowledge_compile::config::KnowledgeCompileConfig;
use crate::memory::Memory;
use crate::types::MemoryRecord;

/// One memory eligible for compilation. Carries the bits the K2 clusterer
/// needs (id, embedding, importance, content, created_at) without forcing
/// every consumer to read the whole `MemoryRecord` blob.
#[derive(Debug, Clone)]
pub struct CandidateMemory {
    pub id: String,
    pub content: String,
    pub importance: f64,
    pub created_at_secs: f64,
    /// Embedding fetched from `embeddings` (PK: `(memory_id, model)`).
    /// `None` if no embedding exists yet — such memories are excluded
    /// from clustering by default (no signal to cluster on), but the
    /// candidate is still surfaced to the caller for visibility into
    /// "candidates considered" counters.
    pub embedding: Option<Vec<f32>>,
}

/// Run K1 — return all eligible candidates for this run.
///
/// Per §5bis.2 ordering ("newest first") + cap, the returned slice is
/// `created_at DESC` and length ≤ `config.compile_max_candidates_per_run`.
///
/// # Errors
///
/// * Storage I/O: bubbles up. Cannot continue without the candidate set.
pub fn select_candidates(
    memory: &mut Memory,
    namespace: &str,
    config: &KnowledgeCompileConfig,
) -> Result<Vec<CandidateMemory>, Box<dyn std::error::Error>> {
    let watermark_secs = read_watermark(memory, namespace)?;

    // Pull all live memories in the namespace, then filter.
    // (`Storage::all_in_namespace` returns *live* rows — superseded /
    // soft-deleted are already excluded by its WHERE clause.)
    let all = memory.storage().all_in_namespace(Some(namespace))?;

    // Apply K1 filters.
    let mut filtered: Vec<MemoryRecord> = all
        .into_iter()
        .filter(|m| m.importance >= config.compile_min_importance as f64)
        .filter(|m| match watermark_secs {
            Some(wm) => datetime_to_secs(&m.created_at) >= wm,
            None => true,
        })
        .collect();

    // Newest first, then cap.
    filtered.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    filtered.truncate(config.compile_max_candidates_per_run);

    // Materialize candidates (with embeddings).
    let mut out = Vec::with_capacity(filtered.len());
    for rec in filtered {
        let embedding = memory
            .storage()
            .get_embedding_for_memory(&rec.id)
            .unwrap_or(None);
        out.push(CandidateMemory {
            id: rec.id,
            content: rec.content,
            importance: rec.importance,
            created_at_secs: datetime_to_secs(&rec.created_at),
            embedding,
        });
    }

    Ok(out)
}

/// Read the high-water `created_at` from the most recent *successful*
/// `KnowledgeCompile` run. `None` if no successful run exists yet.
///
/// Stored under `output_summary.high_water_created_at` (a sub-schema
/// of `graph_pipeline_runs.output_summary`, per §5bis.2 — keeps watermark
/// state out of a separate compiler-state table). Writers (the K3
/// finisher, in `synthesis.rs`) update this on each successful run so
/// the next run picks up where this one left off.
///
/// # Why direct SQL
///
/// `GraphRead` does not expose a "latest run by kind" method — only
/// "latest run *for memory*" (§6.3 of the design). Adding one for
/// `KnowledgeCompile` is the right long-term fix (see ISS pending — the
/// tracker for design §6.3 widening), but tonight we use the same
/// `with_transaction` escape hatch the rest of the codebase uses for
/// these kinds of cross-cutting reads.
fn read_watermark(
    memory: &mut Memory,
    namespace: &str,
) -> Result<Option<f64>, Box<dyn std::error::Error>> {
    let kind_str = serde_json::to_string(&PipelineKind::KnowledgeCompile)?;
    let kind_value = kind_str.trim_matches('"').to_string();

    let status_str = serde_json::to_string(&RunStatus::Succeeded)?;
    let status_value = status_str.trim_matches('"').to_string();

    let conn = memory.storage_mut().connection_mut();
    let mut stmt = conn.prepare(
        "SELECT output_summary FROM graph_pipeline_runs \
         WHERE kind = ?1 AND status = ?2 AND namespace = ?3 \
         ORDER BY started_at DESC LIMIT 1",
    )?;
    let row: Option<Option<String>> = stmt
        .query_row(
            rusqlite::params![&kind_value, &status_value, namespace],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok();

    let mut watermark: Option<f64> = None;
    if let Some(Some(json_str)) = row {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(secs) = v.get("high_water_created_at").and_then(|x| x.as_f64()) {
                watermark = Some(secs);
            }
        }
    }

    Ok(watermark)
}

fn datetime_to_secs(dt: &DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + (dt.timestamp_subsec_nanos() as f64) / 1e9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_compile::config::KnowledgeCompileConfig;
    use crate::memory::Memory;
    use crate::types::MemoryType;

    fn fresh_memory() -> Memory {
        Memory::new(":memory:", None).expect("in-memory Memory")
    }

    fn add(m: &mut Memory, content: &str, importance: f64, ns: Option<&str>) -> String {
        m.add_to_namespace(content, MemoryType::Factual, Some(importance), None, None, ns)
            .expect("add_to_namespace")
    }

    #[test]
    fn select_returns_empty_for_fresh_namespace() {
        let mut m = fresh_memory();
        let cfg = KnowledgeCompileConfig::for_test();
        let cands = select_candidates(&mut m, "default", &cfg).unwrap();
        assert!(cands.is_empty());
    }

    #[test]
    fn select_filters_below_importance_floor() {
        let mut m = fresh_memory();

        // Two memories: one above floor, one below.
        let id1 = add(&mut m, "high importance fact", 0.8, None);
        let id2 = add(&mut m, "low importance fact", 0.1, None);

        let mut cfg = KnowledgeCompileConfig::for_test();
        cfg.compile_min_importance = 0.5;

        let cands = select_candidates(&mut m, "default", &cfg).unwrap();
        let ids: Vec<&str> = cands.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains(&id1.as_str()),
            "high-importance memory should pass filter"
        );
        assert!(
            !ids.contains(&id2.as_str()),
            "low-importance memory should be filtered out"
        );
    }

    #[test]
    fn select_caps_at_max_candidates_per_run() {
        let mut m = fresh_memory();
        for i in 0..5 {
            add(&mut m, &format!("memory number {i}"), 0.5, None);
        }
        let mut cfg = KnowledgeCompileConfig::for_test();
        cfg.compile_max_candidates_per_run = 3;
        let cands = select_candidates(&mut m, "default", &cfg).unwrap();
        assert_eq!(cands.len(), 3, "cap of 3 must be honored");
    }

    #[test]
    fn select_ignores_other_namespaces() {
        let mut m = fresh_memory();
        let id_alpha = add(&mut m, "alpha memory", 0.6, Some("alpha"));
        let id_beta = add(&mut m, "beta memory", 0.6, Some("beta"));

        let cfg = KnowledgeCompileConfig::for_test();

        let alpha_cands = select_candidates(&mut m, "alpha", &cfg).unwrap();
        let beta_cands = select_candidates(&mut m, "beta", &cfg).unwrap();

        let alpha_ids: Vec<&str> = alpha_cands.iter().map(|c| c.id.as_str()).collect();
        let beta_ids: Vec<&str> = beta_cands.iter().map(|c| c.id.as_str()).collect();

        assert!(alpha_ids.contains(&id_alpha.as_str()));
        assert!(!alpha_ids.contains(&id_beta.as_str()));
        assert!(beta_ids.contains(&id_beta.as_str()));
        assert!(!beta_ids.contains(&id_alpha.as_str()));
    }
}
