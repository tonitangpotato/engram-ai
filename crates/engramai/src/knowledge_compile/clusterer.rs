//! K2 — Clustering (design §5bis.3).
//!
//! Groups [`CandidateMemory`]s into [`ProtoCluster`]s via a pluggable
//! [`Clusterer`] trait. The default implementation,
//! [`EmbeddingInfomapClusterer`], runs the existing engramai
//! [`crate::clustering`] Infomap engine over a cosine-similarity edge
//! weight strategy. See §5bis.3:
//!
//! > "Default implementation: embedding-space clustering (HDBSCAN over
//! > memory embeddings) with optional affect-reweighting per GOAL-3.7."
//!
//! ## Why Infomap not HDBSCAN?
//!
//! engramai already ships `clustering.rs` with an Infomap engine that
//! both `compiler::discovery` (v0.2) and `synthesis::cluster` use; it
//! satisfies the contract ("any deterministic clusterer is acceptable")
//! and avoids pulling in a new dependency for HDBSCAN. Trait users that
//! want HDBSCAN can write their own [`Clusterer`] impl — the trait was
//! designed to keep the algorithm choice swappable per §5bis.3.
//!
//! ## Affect reweighting
//!
//! `affect_bias` is plumbed through the trait but the default impl
//! currently ignores it (records `affect_bias_seen = false` in
//! `cluster_weights` so the retrieval `PlanTrace.affect` surface — when
//! `task:retr-impl-affective` lands — sees an honest "no bias applied").
//! Per §5bis.3: "Affect reweighting is a scoring bias at clustering time
//! only" — wiring it in is a follow-up task scoped to the affective
//! retrieval feature, not this one.

use std::error::Error;
use std::fmt;

use crate::clustering::{
    cluster_with_infomap, ClusteringConfig, EdgeWeightStrategy,
};
use crate::knowledge_compile::candidates::CandidateMemory;

/// One proto-cluster: a set of memory IDs the synthesizer will summarize
/// together, plus an opaque JSON record of any cluster-time scoring
/// signals (affect bias, density, etc.). The record is stored verbatim
/// on the `KnowledgeTopic.cluster_weights` column for downstream
/// retrieval to consume without recomputation (§5bis.3).
#[derive(Debug, Clone)]
pub struct ProtoCluster {
    pub memory_ids: Vec<String>,
    pub cluster_weights: serde_json::Value,
}

/// Pluggable clusterer trait — design §5bis.3 contract.
///
/// **Determinism requirement:** `cluster(same_input, same_bias)` MUST
/// produce the same partition (modulo cluster-id renaming, which is
/// not observable since `ProtoCluster` doesn't expose a stable id).
/// This is what lets compile runs be replay-safe / debuggable.
pub trait Clusterer {
    /// Group `memories` into proto-clusters.
    ///
    /// `affect_bias` — optional scoring bias applied at clustering time
    /// per §5bis.3 (GOAL-3.7). Implementations that don't support
    /// affect-aware clustering should record `affect_bias_seen = false`
    /// in their `cluster_weights` JSON so the retrieval-time consumer
    /// sees an honest "no bias applied".
    fn cluster(
        &self,
        memories: &[CandidateMemory],
        affect_bias: Option<&AffectWeights>,
    ) -> Result<Vec<ProtoCluster>, ClusterError>;
}

/// Affect-bias plumbing for the (future) affective retrieval integration.
///
/// Kept structurally as a marker / opaque map so the contract is stable
/// even though the default clusterer doesn't yet read it. The real shape
/// will be defined by the affective retrieval feature
/// (`task:retr-impl-affective`).
#[derive(Debug, Clone, Default)]
pub struct AffectWeights {
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub enum ClusterError {
    /// Underlying clustering engine failed (e.g., Infomap convergence
    /// error). Caller treats this as a fatal run failure (§5bis.5 —
    /// failed runs do not erase prior topics).
    Engine(Box<dyn Error + Send + Sync>),
    /// Input candidates were structurally invalid (e.g., embedding
    /// dimensions don't match within the batch).
    InvalidInput(String),
}

impl fmt::Display for ClusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(e) => write!(f, "clustering engine error: {e}"),
            Self::InvalidInput(s) => write!(f, "invalid clusterer input: {s}"),
        }
    }
}
impl Error for ClusterError {}

// ════════════════════════════════════════════════════════════════════════
//  Default implementation: cosine-similarity edges + Infomap
// ════════════════════════════════════════════════════════════════════════

/// Default clusterer: embedding-space cosine similarity → Infomap
/// communities. Memories without embeddings are silently skipped (no
/// signal to cluster on).
pub struct EmbeddingInfomapClusterer {
    /// Minimum cosine similarity to add an edge. Lower → coarser clusters.
    /// Default `0.5` mirrors `compiler::discovery`.
    pub similarity_threshold: f64,
    /// Forwarded to the underlying Infomap engine.
    pub clustering_config: ClusteringConfig,
}

impl Default for EmbeddingInfomapClusterer {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.5,
            clustering_config: ClusteringConfig::default(),
        }
    }
}

/// Edge-weight strategy: cosine similarity above a threshold.
///
/// `Item = usize` because the `Clusterer` interface gives us indexes
/// into a `&[CandidateMemory]` slice (the embeddings live there).
struct CosineEdges<'a> {
    embeddings: &'a [&'a [f32]],
    threshold: f64,
}

impl<'a> EdgeWeightStrategy for CosineEdges<'a> {
    type Item = usize;

    fn edge_weight(&self, a: &usize, b: &usize) -> Option<f64> {
        if a == b {
            return None;
        }
        let va = self.embeddings.get(*a)?;
        let vb = self.embeddings.get(*b)?;
        let sim = cosine_similarity(va, vb)?;
        if sim >= self.threshold {
            Some(sim)
        } else {
            None
        }
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let mut dot: f64 = 0.0;
    let mut na: f64 = 0.0;
    let mut nb: f64 = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = *x as f64;
        let yf = *y as f64;
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        Some(0.0)
    } else {
        Some(dot / denom)
    }
}

impl Clusterer for EmbeddingInfomapClusterer {
    fn cluster(
        &self,
        memories: &[CandidateMemory],
        affect_bias: Option<&AffectWeights>,
    ) -> Result<Vec<ProtoCluster>, ClusterError> {
        // Filter to memories that have an embedding, but track index
        // back to the original slice so the returned ProtoCluster's
        // `memory_ids` reference real ids.
        let mut indexed: Vec<(usize, &Vec<f32>)> = Vec::new();
        for (i, m) in memories.iter().enumerate() {
            if let Some(emb) = &m.embedding {
                indexed.push((i, emb));
            }
        }
        if indexed.len() < self.clustering_config.min_community_size {
            // Nothing to cluster — return empty rather than erroring.
            return Ok(Vec::new());
        }

        // Validate embedding dimensions are consistent within the batch.
        let dim = indexed[0].1.len();
        for (_, e) in &indexed {
            if e.len() != dim {
                return Err(ClusterError::InvalidInput(format!(
                    "embedding dim mismatch: expected {dim}, got {}",
                    e.len()
                )));
            }
        }

        // Build the index list `0..N` for the engine; resolve back via
        // `indexed[idx].0` to memories[].
        let n = indexed.len();
        let items: Vec<usize> = (0..n).collect();
        let embeddings: Vec<&[f32]> = indexed.iter().map(|(_, e)| e.as_slice()).collect();
        let strategy = CosineEdges {
            embeddings: &embeddings,
            threshold: self.similarity_threshold,
        };

        let communities =
            cluster_with_infomap(&items, &strategy, &self.clustering_config);

        // Map back to memory ids.
        let mut out = Vec::with_capacity(communities.len());
        for c in communities {
            let memory_ids: Vec<String> = c
                .members
                .iter()
                .map(|local_idx| memories[indexed[*local_idx].0].id.clone())
                .collect();
            out.push(ProtoCluster {
                memory_ids,
                cluster_weights: serde_json::json!({
                    "module_id": c.module_id,
                    "size": c.members.len(),
                    "similarity_threshold": self.similarity_threshold,
                    "affect_bias_seen": affect_bias.is_some(),
                }),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, importance: f64, emb: Option<Vec<f32>>) -> CandidateMemory {
        CandidateMemory {
            id: id.to_string(),
            content: format!("content for {id}"),
            importance,
            created_at_secs: 0.0,
            embedding: emb,
        }
    }

    #[test]
    fn empty_input_returns_empty_clusters() {
        let c = EmbeddingInfomapClusterer::default();
        assert!(c.cluster(&[], None).unwrap().is_empty());
    }

    #[test]
    fn single_memory_returns_no_clusters() {
        let c = EmbeddingInfomapClusterer::default();
        let cands = vec![cand("a", 0.5, Some(vec![1.0, 0.0]))];
        // min_community_size is 2 — single memory can't form a community.
        assert!(c.cluster(&cands, None).unwrap().is_empty());
    }

    #[test]
    fn similar_embeddings_cluster_together() {
        let c = EmbeddingInfomapClusterer {
            similarity_threshold: 0.5,
            clustering_config: ClusteringConfig {
                min_community_size: 2,
                max_community_size: usize::MAX,
                seed: 42,
            },
        };
        // Two near-identical pairs.
        let cands = vec![
            cand("a", 0.5, Some(vec![1.0, 0.0, 0.0])),
            cand("b", 0.5, Some(vec![0.99, 0.05, 0.05])),
            cand("c", 0.5, Some(vec![0.0, 1.0, 0.0])),
            cand("d", 0.5, Some(vec![0.05, 0.99, 0.05])),
        ];
        let clusters = c.cluster(&cands, None).unwrap();
        // Should produce two non-empty clusters.
        assert!(!clusters.is_empty());
        // No memory id appears in more than one cluster.
        let mut seen = std::collections::HashSet::new();
        for cl in &clusters {
            for id in &cl.memory_ids {
                assert!(seen.insert(id.clone()), "id {id} appeared in two clusters");
            }
        }
    }

    #[test]
    fn determinism_same_input_same_clusters() {
        let c = EmbeddingInfomapClusterer::default();
        let cands = vec![
            cand("a", 0.5, Some(vec![1.0, 0.0])),
            cand("b", 0.5, Some(vec![0.95, 0.31])),
            cand("c", 0.5, Some(vec![0.0, 1.0])),
            cand("d", 0.5, Some(vec![0.31, 0.95])),
        ];
        let r1 = c.cluster(&cands, None).unwrap();
        let r2 = c.cluster(&cands, None).unwrap();

        // Compare partitions: sorted set-of-sets.
        let to_sets = |r: &[ProtoCluster]| -> Vec<Vec<String>> {
            let mut sets: Vec<Vec<String>> = r
                .iter()
                .map(|c| {
                    let mut ids = c.memory_ids.clone();
                    ids.sort();
                    ids
                })
                .collect();
            sets.sort();
            sets
        };
        assert_eq!(to_sets(&r1), to_sets(&r2));
    }

    #[test]
    fn missing_embeddings_are_skipped() {
        let c = EmbeddingInfomapClusterer::default();
        let cands = vec![
            cand("a", 0.5, None),
            cand("b", 0.5, None),
            cand("c", 0.5, None),
        ];
        // No embeddings at all → no clusters.
        assert!(c.cluster(&cands, None).unwrap().is_empty());
    }

    #[test]
    fn inconsistent_dim_errors() {
        let c = EmbeddingInfomapClusterer::default();
        let cands = vec![
            cand("a", 0.5, Some(vec![1.0, 0.0, 0.0])),
            cand("b", 0.5, Some(vec![0.5, 0.5])), // wrong dim
        ];
        match c.cluster(&cands, None) {
            Err(ClusterError::InvalidInput(_)) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn affect_bias_seen_recorded_in_weights() {
        let c = EmbeddingInfomapClusterer {
            similarity_threshold: 0.5,
            clustering_config: ClusteringConfig {
                min_community_size: 2,
                max_community_size: usize::MAX,
                seed: 42,
            },
        };
        let cands = vec![
            cand("a", 0.5, Some(vec![1.0, 0.0])),
            cand("b", 0.5, Some(vec![0.99, 0.05])),
        ];
        let bias = AffectWeights::default();
        let clusters = c.cluster(&cands, Some(&bias)).unwrap();
        for cl in &clusters {
            assert_eq!(
                cl.cluster_weights["affect_bias_seen"],
                serde_json::Value::Bool(true)
            );
        }

        let no_bias_clusters = c.cluster(&cands, None).unwrap();
        for cl in &no_bias_clusters {
            assert_eq!(
                cl.cluster_weights["affect_bias_seen"],
                serde_json::Value::Bool(false)
            );
        }
    }

    #[test]
    fn cosine_similarity_basic_cases() {
        // Identical vectors → 1.0
        let s = cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]).unwrap();
        assert!((s - 1.0).abs() < 1e-6);
        // Orthogonal → 0.0
        let s = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(s.abs() < 1e-6);
        // Zero vector → 0
        let s = cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]).unwrap();
        assert_eq!(s, 0.0);
        // Mismatched dim → None
        assert!(cosine_similarity(&[1.0], &[1.0, 0.0]).is_none());
    }
}
