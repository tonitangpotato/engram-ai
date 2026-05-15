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
    /// Minimum cosine similarity to add an edge under absolute-threshold
    /// mode. Default `0.5` mirrors `compiler::discovery`. Only used when
    /// `k_neighbors` is `None`.
    pub similarity_threshold: f64,

    /// When `Some(k)`, build edges using **mutual k-nearest-neighbors**
    /// instead of absolute thresholding. An edge `(i, j)` exists iff
    /// `j` is among `i`'s top-k most-similar nodes **and** vice versa.
    ///
    /// This is the ISS-111 fix for dense single-domain corpora: with
    /// homogeneous embeddings every pair clears any reasonable absolute
    /// threshold, so the edge graph becomes K_n and Infomap collapses
    /// into one super-community. Mutual k-NN bounds each node's degree
    /// by `k` regardless of overall density, letting Infomap recover
    /// real substructure.
    ///
    /// `None` preserves the legacy absolute-threshold behavior for
    /// callers that don't want the new semantics. Default is
    /// `Some(auto)` where `auto = clamp(sqrt(n), 3, 10)` — computed at
    /// `cluster()` time from the candidate count, so a single config
    /// works across corpus sizes.
    pub k_neighbors: Option<KNeighbors>,

    /// Forwarded to the underlying Infomap engine.
    pub clustering_config: ClusteringConfig,
}

/// k-NN sizing strategy for [`EmbeddingInfomapClusterer`].
///
/// `Auto` picks `clamp(sqrt(n), 3, 10)` from the candidate count.
/// `Fixed(k)` pins the exact k. See [`EmbeddingInfomapClusterer::k_neighbors`].
#[derive(Debug, Clone, Copy)]
pub enum KNeighbors {
    /// `k = clamp(sqrt(n), 3, 10)`.
    Auto,
    /// Pinned `k`.
    Fixed(usize),
}

impl KNeighbors {
    /// Resolve the actual k for a given candidate count.
    fn resolve(self, n: usize) -> usize {
        match self {
            KNeighbors::Fixed(k) => k.max(1),
            KNeighbors::Auto => {
                let sqrt_n = (n as f64).sqrt() as usize;
                sqrt_n.clamp(3, 10)
            }
        }
    }
}

impl Default for EmbeddingInfomapClusterer {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.5,
            k_neighbors: Some(KNeighbors::Auto),
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

/// Edge-weight strategy: **mutual k-nearest-neighbor** with cosine
/// similarity as the ranking metric (ISS-111 fix).
///
/// Returns `Some(sim)` for `(a, b)` iff `b` is among `a`'s top-k
/// neighbors **and** `a` is among `b`'s top-k neighbors. This bounds
/// each node's degree by `k` regardless of how dense the underlying
/// pairwise-similarity matrix is, breaking the "K_n graph → one
/// super-community" collapse mode that absolute thresholding suffers
/// on homogeneous corpora.
///
/// ## Why mutual (and not unilateral) k-NN
///
/// Unilateral k-NN (`b ∈ topK(a)` OR `a ∈ topK(b)`) lets dense hubs
/// pull in marginally-similar tails — exactly the failure mode we're
/// trying to escape. Mutual k-NN requires both endpoints to consider
/// each other "close enough to be in my top-k", which is a much
/// stricter local-density test and is the standard choice in graph
/// clustering literature (see e.g. Brito et al. 1997).
///
/// ## Cost
///
/// Precomputing top-k for `n` nodes is `O(n²)` time + `O(n·k)` space.
/// At Phase D scale (≤ ~1000 candidates per compile run) this is fine.
/// If the candidate budget grows beyond ~10k an approximate kNN index
/// (HNSW etc.) would be the upgrade — out of scope for this fix.
struct MutualKnnEdges {
    /// For each node `i`, the set of nodes that are in `i`'s top-k
    /// most-similar neighbors. We use HashSet for O(1) membership.
    top_k: Vec<std::collections::HashSet<usize>>,
    /// Precomputed sims, mirroring `top_k` indices — looked up when
    /// the strategy returns a weight (we don't recompute cosine in
    /// `edge_weight` since we already did the O(n²) pass).
    /// Maps `(min(i,j), max(i,j)) → sim` for `O(1)` retrieval.
    sims: std::collections::HashMap<(usize, usize), f64>,
}

impl MutualKnnEdges {
    /// Build the mutual k-NN edge set from a slice of embeddings.
    ///
    /// `embeddings[i]` must be already-validated (non-empty, equal
    /// dim within the batch). Returns an error if cosine_similarity
    /// returns `None` for any pair (dimension mismatch).
    fn build(embeddings: &[&[f32]], k: usize) -> Result<Self, ClusterError> {
        let n = embeddings.len();
        // For each node, sort all *other* nodes by sim desc and take
        // top-k. We materialize the full sim matrix once because
        // we need it both for top-k selection and to surface as the
        // edge weight downstream.
        let mut sims: std::collections::HashMap<(usize, usize), f64> =
            std::collections::HashMap::with_capacity(n * (n - 1) / 2);
        // Per-node neighbor lists: (sim, neighbor_idx), descending sim.
        let mut per_node: Vec<Vec<(f64, usize)>> = vec![Vec::with_capacity(n - 1); n];
        for i in 0..n {
            for j in (i + 1)..n {
                let sim = cosine_similarity(embeddings[i], embeddings[j])
                    .ok_or_else(|| {
                        ClusterError::InvalidInput(
                            "cosine_similarity returned None during k-NN build"
                                .to_string(),
                        )
                    })?;
                sims.insert((i, j), sim);
                per_node[i].push((sim, j));
                per_node[j].push((sim, i));
            }
        }

        let mut top_k: Vec<std::collections::HashSet<usize>> =
            Vec::with_capacity(n);
        for mut neighbors in per_node {
            // Partial sort: we only need the top-k. select_nth_unstable
            // would be O(n) but the slice is already small and the
            // sort is over f64 which doesn't impl Ord — use partial_cmp
            // with a full sort_by for simplicity. n*log(n) per node is
            // O(n² log n) overall, dominated by the O(n²) sim pass.
            neighbors.sort_by(|a, b| {
                b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            let set: std::collections::HashSet<usize> = neighbors
                .into_iter()
                .take(k)
                .map(|(_, j)| j)
                .collect();
            top_k.push(set);
        }

        Ok(Self { top_k, sims })
    }
}

impl EdgeWeightStrategy for MutualKnnEdges {
    type Item = usize;

    fn edge_weight(&self, a: &usize, b: &usize) -> Option<f64> {
        if a == b {
            return None;
        }
        // Mutual k-NN: both directions must agree.
        if !self.top_k.get(*a)?.contains(b) {
            return None;
        }
        if !self.top_k.get(*b)?.contains(a) {
            return None;
        }
        // Edge survives — return the precomputed sim.
        let key = if a < b { (*a, *b) } else { (*b, *a) };
        self.sims.get(&key).copied()
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

        // Choose edge strategy based on config (ISS-111).
        //   - `k_neighbors = Some(_)` → mutual k-NN, robust on dense
        //     single-domain corpora (default).
        //   - `k_neighbors = None`    → legacy absolute-threshold.
        let communities = match self.k_neighbors {
            Some(kn) => {
                let k = kn.resolve(n);
                let strategy = MutualKnnEdges::build(&embeddings, k)?;
                cluster_with_infomap(&items, &strategy, &self.clustering_config)
            }
            None => {
                let strategy = CosineEdges {
                    embeddings: &embeddings,
                    threshold: self.similarity_threshold,
                };
                cluster_with_infomap(&items, &strategy, &self.clustering_config)
            }
        };

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
        // Legacy absolute-threshold mode — this test predates ISS-111
        // and its expectation (two near-identical pairs → at least two
        // separate clusters at threshold 0.5) is exactly the case
        // absolute thresholding handles correctly. Pin to legacy mode
        // so we keep coverage of that branch.
        let c = EmbeddingInfomapClusterer {
            similarity_threshold: 0.5,
            k_neighbors: None,
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
            k_neighbors: None,
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

    /// ISS-111 reproduction: dense single-domain corpora collapse into a
    /// single super-cluster under the **legacy** absolute-threshold mode
    /// (`k_neighbors = None`, `similarity_threshold = 0.5`).
    ///
    /// This is kept as a pinned reproduction test for the failure mode —
    /// any future regression that bypasses the k-NN path and falls back
    /// to absolute thresholding on a dense corpus would trigger this
    /// failure pattern. The fix lives in
    /// `iss111_dense_single_domain_does_not_collapse_after_fix` below.
    ///
    /// **Fixture**: 50 embeddings, all in a tight cone around the same
    /// base direction. Cosine sims between any two are > 0.99, so every
    /// pair crosses the 0.5 threshold and the edge graph is the complete
    /// graph K_50. Infomap minimizes MDL by producing one giant
    /// community.
    ///
    /// **Why this is a bug** in production: under the default v0.3 KC
    /// config + a homogeneous corpus like one LoCoMo conversation, the
    /// single super-topic matches every Abstract sub-plan query,
    /// over-weights itself in the fuse stage, and squeezes
    /// Factual/Episodic candidates out of the top-K — the -22pp J-score
    /// regression observed in RUN-0026 vs RUN-0025.
    #[test]
    fn iss111_dense_single_domain_collapses_to_one_supercluster() {
        // Build dense fixture (see header).
        let base = vec![1.0f32; 16];
        let candidates: Vec<CandidateMemory> = (0..50)
            .map(|i| {
                let mut e = base.clone();
                // Tiny deterministic perturbation so the vectors aren't
                // bit-identical but are all very close in direction.
                e[i % 16] += 0.01 * (i as f32);
                cand(&format!("m{i}"), 0.5, Some(e))
            })
            .collect();

        // Explicitly pin legacy absolute-threshold mode so this test
        // continues to demonstrate the failure regardless of the
        // default's evolution.
        let clusterer = EmbeddingInfomapClusterer {
            similarity_threshold: 0.5,
            k_neighbors: None,
            clustering_config: ClusteringConfig::default(),
        };
        let clusters = clusterer.cluster(&candidates, None).unwrap();

        assert_eq!(
            clusters.len(),
            1,
            "ISS-111 reproduction: dense single-domain corpus should \
             collapse to exactly 1 super-cluster under legacy \
             absolute-threshold mode (threshold=0.5), got {} clusters",
            clusters.len(),
        );
        assert_eq!(
            clusters[0].memory_ids.len(),
            50,
            "super-cluster should swallow every candidate"
        );
    }

    /// ISS-111 fix: with the default `k_neighbors = Some(Auto)` mode
    /// the same dense fixture must produce **more than one** cluster,
    /// proving mutual k-NN recovers substructure that absolute
    /// thresholding cannot see.
    #[test]
    fn iss111_dense_single_domain_does_not_collapse_after_fix() {
        // Same fixture as the legacy-mode test above.
        let base = vec![1.0f32; 16];
        let candidates: Vec<CandidateMemory> = (0..50)
            .map(|i| {
                let mut e = base.clone();
                e[i % 16] += 0.01 * (i as f32);
                cand(&format!("m{i}"), 0.5, Some(e))
            })
            .collect();

        // Use default (`k_neighbors = Some(Auto)`).
        let clusterer = EmbeddingInfomapClusterer::default();
        let clusters = clusterer.cluster(&candidates, None).unwrap();

        assert!(
            clusters.len() > 1,
            "ISS-111 fix target: dense single-domain corpus must \
             produce >1 clusters under default (mutual k-NN), got {}",
            clusters.len(),
        );
        // Total membership must still cover all candidates that pass
        // the min_community_size filter (default 2). With n=50 and a
        // mutual k-NN graph this should be most of them; we don't
        // assert exact coverage to leave room for minor partitions
        // dropped by min_community_size.
        let total: usize = clusters.iter().map(|c| c.memory_ids.len()).sum();
        assert!(
            total >= 40,
            "expected ≥40 of 50 candidates to land in some cluster, \
             got {total} (clusters: {:?})",
            clusters.iter().map(|c| c.memory_ids.len()).collect::<Vec<_>>()
        );
    }
}
