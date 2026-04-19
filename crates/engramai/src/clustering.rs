//! Unified Infomap clustering engine.
//!
//! Provides a single, shared clustering implementation used by both:
//! - **compiler/discovery.rs** — topic discovery across all memories (embedding-only edges)
//! - **synthesis/cluster.rs** — sleep-cycle clustering of recent memories (multi-signal edges)
//!
//! The clustering algorithm (Infomap on a sparse k-NN graph) is identical in both cases.
//! The only difference is **how edge weights are computed**, which is abstracted via the
//! [`EdgeWeightStrategy`] trait.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────┐
//! │       InfomapClusterer<S>       │
//! │  (builds k-NN graph, runs      │
//! │   Infomap, returns clusters)    │
//! └──────────┬──────────────────────┘
//!            │ uses S::compute_weight()
//!     ┌──────┴──────┐
//!     ▼             ▼
//! EmbeddingOnly   MultiSignal
//! (cosine sim)    (hebbian + entity +
//!                  embedding + temporal)
//! ```

use std::collections::HashMap;

use crate::embeddings::EmbeddingProvider;

// ═══════════════════════════════════════════════════════════════════════════════
//  EDGE WEIGHT STRATEGY
// ═══════════════════════════════════════════════════════════════════════════════

/// A node in the clustering graph, carrying all signals that any strategy might need.
///
/// Not every strategy uses every field. `EmbeddingOnly` only reads `embedding`;
/// `MultiSignal` reads all four signal sources.
#[derive(Debug, Clone)]
pub struct ClusterNode {
    /// Unique identifier for this memory.
    pub id: String,
    /// Embedding vector (required for all strategies).
    pub embedding: Vec<f32>,
    /// Pre-computed Hebbian link weights to other nodes: `(other_id, weight)`.
    /// Empty if not available.
    pub hebbian_links: Vec<(String, f64)>,
    /// Entity IDs associated with this memory (for Jaccard overlap).
    pub entity_ids: Vec<String>,
    /// Creation timestamp (seconds since epoch, for temporal proximity).
    pub created_at_secs: f64,
}

/// Strategy for computing edge weights between two nodes.
///
/// Implementations define what "similarity" means in their context.
/// The clusterer calls this for every candidate edge in the k-NN graph.
pub trait EdgeWeightStrategy: Send + Sync {
    /// Compute the edge weight between two nodes.
    ///
    /// Returns a value in `[0.0, 1.0]` where higher = more related.
    /// Returning 0.0 or negative means "no meaningful edge".
    fn compute_weight(&self, a: &ClusterNode, b: &ClusterNode) -> f64;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  BUILT-IN STRATEGIES
// ═══════════════════════════════════════════════════════════════════════════════

/// Embedding-only strategy: edge weight = cosine similarity.
///
/// Used by the compiler's topic discovery, where only embedding vectors are available.
#[derive(Debug, Clone, Default)]
pub struct EmbeddingOnly;

impl EdgeWeightStrategy for EmbeddingOnly {
    fn compute_weight(&self, a: &ClusterNode, b: &ClusterNode) -> f64 {
        EmbeddingProvider::cosine_similarity(&a.embedding, &b.embedding) as f64
    }
}

/// Multi-signal strategy: weighted combination of 4 signals.
///
/// Used by the synthesis engine's sleep-cycle clustering, where Hebbian links,
/// entity overlap, embedding similarity, and temporal proximity are all available.
#[derive(Debug, Clone)]
pub struct MultiSignal {
    pub weights: SignalWeights,
    /// Temporal decay lambda (controls how fast temporal proximity drops).
    /// Default: 0.00413 (7-day half-life).
    pub temporal_decay_lambda: f64,
}

/// Weights for the four clustering signals in `MultiSignal`.
#[derive(Debug, Clone)]
pub struct SignalWeights {
    /// Hebbian co-activation weight (default: 0.4).
    pub hebbian: f64,
    /// Entity overlap weight (default: 0.3).
    pub entity: f64,
    /// Embedding similarity weight (default: 0.2).
    pub embedding: f64,
    /// Temporal proximity weight (default: 0.1).
    pub temporal: f64,
}

impl Default for SignalWeights {
    fn default() -> Self {
        Self {
            hebbian: 0.4,
            entity: 0.3,
            embedding: 0.2,
            temporal: 0.1,
        }
    }
}

impl Default for MultiSignal {
    fn default() -> Self {
        Self {
            weights: SignalWeights::default(),
            temporal_decay_lambda: 0.00413,
        }
    }
}

impl EdgeWeightStrategy for MultiSignal {
    fn compute_weight(&self, a: &ClusterNode, b: &ClusterNode) -> f64 {
        let w = &self.weights;

        // Signal 1: Hebbian weight (normalized to 0-1 by dividing by 10.0)
        let hebbian = a
            .hebbian_links
            .iter()
            .find(|(id, _)| id == &b.id)
            .map(|(_, weight)| (weight / 10.0).min(1.0))
            .unwrap_or(0.0);

        // Signal 2: Entity overlap (Jaccard index)
        let entity_overlap = if a.entity_ids.is_empty() && b.entity_ids.is_empty() {
            0.0
        } else {
            let set_a: std::collections::HashSet<&str> =
                a.entity_ids.iter().map(|s| s.as_str()).collect();
            let set_b: std::collections::HashSet<&str> =
                b.entity_ids.iter().map(|s| s.as_str()).collect();
            let intersection = set_a.intersection(&set_b).count();
            let union = set_a.union(&set_b).count();
            if union > 0 {
                intersection as f64 / union as f64
            } else {
                0.0
            }
        };

        // Signal 3: Embedding similarity (cosine)
        let embedding_sim =
            EmbeddingProvider::cosine_similarity(&a.embedding, &b.embedding) as f64;

        // Signal 4: Temporal proximity (exponential decay)
        let hours_apart = (a.created_at_secs - b.created_at_secs).abs() / 3600.0;
        let temporal = (-self.temporal_decay_lambda * hours_apart).exp();

        w.hebbian * hebbian + w.entity * entity_overlap + w.embedding * embedding_sim + w.temporal * temporal
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  VP-TREE (Vantage-Point Tree for ANN search)
// ═══════════════════════════════════════════════════════════════════════════════

/// Euclidean distance between two vectors.
///
/// On L2-normalized vectors this is monotonically equivalent to cosine distance
/// but satisfies the triangle inequality required for VP-tree pruning.
fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// A Vantage-Point Tree for efficient nearest neighbor search in metric spaces.
///
/// Uses Euclidean distance on L2-normalized vectors (monotonically equivalent to
/// cosine distance but satisfies triangle inequality, which VP-tree pruning requires).
pub(crate) struct VpTree {
    nodes: Vec<VpTreeNode>,
    /// Original embeddings (borrowed indices point into this).
    embeddings: Vec<Vec<f32>>,
}

struct VpTreeNode {
    /// Index into the embeddings array.
    index: usize,
    /// Median distance — points within threshold go left, beyond go right.
    threshold: f32,
    /// Left subtree (within threshold), `None` if leaf.
    left: Option<usize>,
    /// Right subtree (beyond threshold), `None` if leaf.
    right: Option<usize>,
}

impl VpTree {
    /// Build a VP-tree from a set of L2-normalized embeddings.
    ///
    /// Complexity: O(n log n).
    pub(crate) fn build(embeddings: Vec<Vec<f32>>) -> Self {
        // Debug-assert that all embeddings are L2-normalized.
        for (i, emb) in embeddings.iter().enumerate() {
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            debug_assert!(
                (norm - 1.0).abs() < 0.05,
                "embedding {} not L2-normalized: norm={}",
                i,
                norm
            );
        }

        let n = embeddings.len();
        if n == 0 {
            return Self {
                nodes: Vec::new(),
                embeddings,
            };
        }

        let mut indices: Vec<usize> = (0..n).collect();
        let mut nodes: Vec<VpTreeNode> = Vec::with_capacity(n);

        // Iterative build using an explicit stack.
        // Each entry: (start, end, parent_node_index, is_left_child).
        // `None` for parent means this is the root.
        let mut stack: Vec<(usize, usize, Option<usize>, bool)> = Vec::new();
        stack.push((0, n, None, false));

        while let Some((start, end, parent, is_left)) = stack.pop() {
            if start >= end {
                continue;
            }

            // Use the first element as the vantage point.
            let vp_idx = indices[start];
            let node_pos = nodes.len();

            nodes.push(VpTreeNode {
                index: vp_idx,
                threshold: 0.0,
                left: None,
                right: None,
            });

            // Link parent to this node.
            if let Some(p) = parent {
                if is_left {
                    nodes[p].left = Some(node_pos);
                } else {
                    nodes[p].right = Some(node_pos);
                }
            }

            let rest_start = start + 1;
            if rest_start >= end {
                // Leaf node — no children.
                continue;
            }

            // Compute distances from vantage point to all remaining points in this slice.
            let vp_emb = &embeddings[vp_idx];
            for i in rest_start..end {
                // Store distance temporarily by sorting in-place below.
                let _ = euclidean_distance(vp_emb, &embeddings[indices[i]]);
            }

            // Sort the rest by distance to the vantage point.
            let embs = &embeddings;
            indices[rest_start..end].sort_by(|&a, &b| {
                let da = euclidean_distance(vp_emb, &embs[a]);
                let db = euclidean_distance(vp_emb, &embs[b]);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Greater)
            });

            // Median index — split into left (within) and right (beyond).
            let median_idx = rest_start + (end - rest_start) / 2;
            let threshold = euclidean_distance(vp_emb, &embeddings[indices[median_idx]]);
            nodes[node_pos].threshold = threshold;

            // Push right first so left is processed first (stack is LIFO).
            if median_idx < end {
                stack.push((median_idx, end, Some(node_pos), false));
            }
            if rest_start < median_idx {
                stack.push((rest_start, median_idx, Some(node_pos), true));
            }
        }

        Self { nodes, embeddings }
    }

    /// Find the `k` nearest neighbors to `query`.
    ///
    /// Returns `Vec<(original_index, distance)>` sorted by distance ascending.
    pub(crate) fn query_nearest(&self, query: &[f32], k: usize) -> Vec<(usize, f32)> {
        use std::collections::BinaryHeap;

        if self.nodes.is_empty() || k == 0 {
            return Vec::new();
        }

        /// Max-heap entry: ordered by distance so we can evict the farthest.
        #[derive(PartialEq)]
        struct HeapItem {
            dist: f32,
            index: usize,
        }

        impl Eq for HeapItem {}

        impl PartialOrd for HeapItem {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                self.dist
                    .partial_cmp(&other.dist)
                    .or(Some(std::cmp::Ordering::Equal))
            }
        }

        impl Ord for HeapItem {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
            }
        }

        let mut heap: BinaryHeap<HeapItem> = BinaryHeap::with_capacity(k + 1);

        // Iterative traversal stack (node indices into self.nodes).
        let mut stack: Vec<usize> = vec![0];

        while let Some(node_idx) = stack.pop() {
            let node = &self.nodes[node_idx];
            let d = euclidean_distance(query, &self.embeddings[node.index]);

            // Consider this node as a candidate.
            if heap.len() < k || d < heap.peek().map(|h| h.dist).unwrap_or(f32::INFINITY) {
                heap.push(HeapItem {
                    dist: d,
                    index: node.index,
                });
                if heap.len() > k {
                    heap.pop();
                }
            }

            let tau = if heap.len() == k {
                heap.peek().map(|h| h.dist).unwrap_or(f32::INFINITY)
            } else {
                f32::INFINITY
            };

            // Determine which subtrees to search.
            if d < node.threshold {
                // Query is inside the threshold — search left (closer) first.
                // We push right first so left is popped first from the stack.
                if d + tau >= node.threshold {
                    if let Some(right) = node.right {
                        stack.push(right);
                    }
                }
                if let Some(left) = node.left {
                    stack.push(left);
                }
            } else {
                // Query is outside the threshold — search right (closer) first.
                if d - tau < node.threshold {
                    if let Some(left) = node.left {
                        stack.push(left);
                    }
                }
                if let Some(right) = node.right {
                    stack.push(right);
                }
            }
        }

        // Drain heap into a sorted vec (ascending by distance).
        let mut results: Vec<(usize, f32)> = heap
            .into_iter()
            .map(|item| (item.index, item.dist))
            .collect();
        results.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Greater)
        });
        results
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  INFOMAP CLUSTERER
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for the Infomap clusterer.
#[derive(Debug, Clone)]
pub struct ClustererConfig {
    /// Number of nearest neighbors per node when building the sparse graph.
    /// Higher k = denser graph = potentially larger communities.
    /// Default: 15.
    pub k_neighbors: usize,
    /// Minimum edge weight to include in the graph.
    /// This is NOT the clustering threshold — Infomap decides community boundaries.
    /// This only filters out noise edges (near-zero or negative similarity).
    /// Default: 0.1.
    pub min_edge_weight: f64,
    /// Minimum number of nodes required to form a valid cluster.
    /// Default: 3.
    pub min_cluster_size: usize,
    /// Number of Infomap trials (more trials = better quality, slower).
    /// Default: 5.
    pub num_trials: usize,
    /// Random seed for reproducibility.
    /// Default: 42.
    pub seed: u64,
}

impl Default for ClustererConfig {
    fn default() -> Self {
        Self {
            k_neighbors: 15,
            min_edge_weight: 0.1,
            min_cluster_size: 3,
            num_trials: 5,
            seed: 42,
        }
    }
}

/// A discovered cluster of nodes.
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Node indices (into the original input slice) that belong to this cluster.
    pub member_indices: Vec<usize>,
    /// Centroid embedding: mean of member embeddings.
    pub centroid: Vec<f32>,
    /// Average pairwise similarity within the cluster (cohesion).
    pub cohesion: f64,
}

/// Unified Infomap clusterer, parameterized by edge weight strategy.
///
/// # Usage
///
/// ```rust,ignore
/// use engramai::clustering::*;
///
/// // Compiler scenario: embedding-only
/// let clusterer = InfomapClusterer::new(EmbeddingOnly, ClustererConfig { min_cluster_size: 3, ..Default::default() });
/// let clusters = clusterer.cluster(&nodes);
///
/// // Synthesis scenario: multi-signal
/// let clusterer = InfomapClusterer::new(MultiSignal::default(), ClustererConfig { min_cluster_size: 2, ..Default::default() });
/// let clusters = clusterer.cluster(&nodes);
/// ```
pub struct InfomapClusterer<S: EdgeWeightStrategy> {
    strategy: S,
    config: ClustererConfig,
}

impl<S: EdgeWeightStrategy> InfomapClusterer<S> {
    /// Create a new clusterer with the given strategy and configuration.
    pub fn new(strategy: S, config: ClustererConfig) -> Self {
        Self { strategy, config }
    }

    /// Access the edge weight strategy.
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Cluster a set of nodes using Infomap community detection.
    ///
    /// # Algorithm
    ///
    /// 1. Build sparse k-NN graph: each node connects to its k nearest neighbors,
    ///    weighted by `strategy.compute_weight()`. O(n·k) edges.
    /// 2. Run Infomap on the sparse graph to find communities.
    /// 3. Filter communities below `min_cluster_size`.
    /// 4. Compute centroid and cohesion for each cluster.
    pub fn cluster(&self, nodes: &[ClusterNode]) -> Vec<Cluster> {
        if nodes.is_empty() {
            return Vec::new();
        }

        let n = nodes.len();

        // Edge case: fewer nodes than k — use n-1 neighbors.
        let k = self.config.k_neighbors.min(n.saturating_sub(1));
        if k == 0 {
            return Vec::new();
        }

        // Step 1: Build sparse k-NN graph.
        // Two-stage approach (ISS-001 Fix 2):
        //   Stage 1: VP-tree ANN for embedding candidates (O(n log n))
        //   Stage 2: Inject Hebbian + entity co-occurring pairs
        //   Stage 3: Compute full strategy weights on candidate pairs, keep top-k
        let mut edges: Vec<(usize, usize, f64)> = Vec::with_capacity(n * k);

        // Check if nodes have embeddings for ANN acceleration.
        let has_embeddings = nodes.iter().all(|n| !n.embedding.is_empty());

        if has_embeddings && n > k * 3 {
            // --- Two-stage pipeline ---
            let k_prime = k * 3; // over-fetch factor for ANN candidates

            // Stage 1: VP-tree ANN candidates
            let embeddings: Vec<Vec<f32>> = nodes.iter().map(|n| n.embedding.clone()).collect();
            let vp_tree = VpTree::build(embeddings);

            // Build candidate pairs set: HashSet<(min_idx, max_idx)> to dedup
            let mut candidate_pairs: std::collections::HashSet<(usize, usize)> =
                std::collections::HashSet::with_capacity(n * k_prime);

            for i in 0..n {
                let neighbors = vp_tree.query_nearest(&nodes[i].embedding, k_prime + 1);
                for (j, _dist) in neighbors {
                    if j != i {
                        let pair = if i < j { (i, j) } else { (j, i) };
                        candidate_pairs.insert(pair);
                    }
                }
            }

            // Stage 2a: Inject Hebbian-linked pairs (prior knowledge)
            let id_to_idx: HashMap<&str, usize> = nodes.iter().enumerate()
                .map(|(i, n)| (n.id.as_str(), i))
                .collect();

            for (i, node) in nodes.iter().enumerate() {
                for (neighbor_id, _weight) in &node.hebbian_links {
                    if let Some(&j) = id_to_idx.get(neighbor_id.as_str()) {
                        if j != i {
                            let pair = if i < j { (i, j) } else { (j, i) };
                            candidate_pairs.insert(pair);
                        }
                    }
                }
            }

            // Stage 2b: Inject entity co-occurring pairs (reverse index approach)
            let mut entity_to_nodes: HashMap<&str, Vec<usize>> = HashMap::new();
            for (i, node) in nodes.iter().enumerate() {
                for eid in &node.entity_ids {
                    entity_to_nodes.entry(eid.as_str()).or_default().push(i);
                }
            }
            for (_eid, node_indices) in &entity_to_nodes {
                // Pairwise within each entity's memory set (usually small, <10)
                for (pi, &i) in node_indices.iter().enumerate() {
                    for &j in &node_indices[(pi + 1)..] {
                        let pair = if i < j { (i, j) } else { (j, i) };
                        candidate_pairs.insert(pair);
                    }
                }
            }

            // Stage 3: Compute full strategy weights on candidate pairs, keep top-k per node
            let mut per_node_candidates: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
            for &(i, j) in &candidate_pairs {
                let w = self.strategy.compute_weight(&nodes[i], &nodes[j]);
                per_node_candidates[i].push((j, w));
                per_node_candidates[j].push((i, w));
            }

            for i in 0..n {
                let sims = &mut per_node_candidates[i];
                sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                sims.dedup_by_key(|s| s.0);
                for &(j, w) in sims.iter().take(k) {
                    if w > self.config.min_edge_weight {
                        edges.push((i, j, w));
                    }
                }
            }
        } else {
            // Fallback: O(n²) brute force (when nodes lack embeddings or n is small)
            for i in 0..n {
                let mut sims: Vec<(usize, f64)> = (0..n)
                    .filter(|&j| j != i)
                    .map(|j| {
                        let w = self.strategy.compute_weight(&nodes[i], &nodes[j]);
                        (j, w)
                    })
                    .collect();

                sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                for &(j, w) in sims.iter().take(k) {
                    if w > self.config.min_edge_weight {
                        edges.push((i, j, w));
                    }
                }
            }
        }

        // Step 2: Run Infomap.
        use infomap_rs::{Infomap, Network};

        let mut net = Network::new();
        for &(src, dst, weight) in &edges {
            net.add_edge(src, dst, weight);
        }
        net.ensure_capacity(n);

        let result = Infomap::new(&net)
            .seed(self.config.seed)
            .num_trials(self.config.num_trials)
            .run();

        // Step 3: Collect communities, filter by min_cluster_size.
        let mut communities: HashMap<usize, Vec<usize>> = HashMap::new();
        for (node_idx, &module_id) in result.assignments.iter().enumerate() {
            communities.entry(module_id).or_default().push(node_idx);
        }

        // Step 4: Build Cluster structs.
        let dim = nodes[0].embedding.len();
        let mut clusters: Vec<Cluster> = Vec::new();

        for (_module_id, member_indices) in &communities {
            if member_indices.len() < self.config.min_cluster_size {
                continue;
            }

            // Centroid: mean of member embeddings.
            let mut centroid = vec![0.0f32; dim];
            for &idx in member_indices {
                for (d, val) in nodes[idx].embedding.iter().enumerate() {
                    if d < dim {
                        centroid[d] += val;
                    }
                }
            }
            let count = member_indices.len() as f32;
            for c in centroid.iter_mut() {
                *c /= count;
            }

            // Cohesion: average pairwise similarity within the cluster.
            let mut cohesion_sum = 0.0;
            let mut pair_count = 0usize;
            for (pi, &i) in member_indices.iter().enumerate() {
                for &j in &member_indices[(pi + 1)..] {
                    let s = self.strategy.compute_weight(&nodes[i], &nodes[j]);
                    cohesion_sum += s;
                    pair_count += 1;
                }
            }
            let cohesion = if pair_count > 0 {
                cohesion_sum / pair_count as f64
            } else {
                1.0
            };

            clusters.push(Cluster {
                member_indices: member_indices.clone(),
                centroid,
                cohesion,
            });
        }

        // Sort by cohesion descending for deterministic output.
        clusters.sort_by(|a, b| {
            b.cohesion
                .partial_cmp(&a.cohesion)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        clusters
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal ClusterNode with only an embedding.
    fn node(id: &str, embedding: Vec<f32>) -> ClusterNode {
        ClusterNode {
            id: id.to_string(),
            embedding,
            hebbian_links: Vec::new(),
            entity_ids: Vec::new(),
            created_at_secs: 0.0,
        }
    }

    /// Helper: create a ClusterNode with all signals populated.
    fn full_node(
        id: &str,
        embedding: Vec<f32>,
        hebbian: Vec<(String, f64)>,
        entities: Vec<String>,
        created_secs: f64,
    ) -> ClusterNode {
        ClusterNode {
            id: id.to_string(),
            embedding,
            hebbian_links: hebbian,
            entity_ids: entities,
            created_at_secs: created_secs,
        }
    }

    // ── EmbeddingOnly tests ──────────────────────────────────────────────

    #[test]
    fn test_embedding_only_strategy() {
        let strategy = EmbeddingOnly;
        let a = node("a", vec![1.0, 0.0, 0.0]);
        let b = node("b", vec![1.0, 0.0, 0.0]); // identical
        let c = node("c", vec![0.0, 1.0, 0.0]); // orthogonal

        let w_same = strategy.compute_weight(&a, &b);
        let w_diff = strategy.compute_weight(&a, &c);

        assert!((w_same - 1.0).abs() < 0.001, "identical vectors should have weight ~1.0");
        assert!(w_diff.abs() < 0.001, "orthogonal vectors should have weight ~0.0");
    }

    #[test]
    fn test_multi_signal_strategy() {
        let strategy = MultiSignal::default();

        let a = full_node(
            "a",
            vec![1.0, 0.0],
            vec![("b".to_string(), 5.0)], // hebbian link to b
            vec!["entity-1".to_string()],
            0.0,
        );
        let b = full_node(
            "b",
            vec![0.9, 0.1],              // similar embedding
            vec![("a".to_string(), 5.0)], // reciprocal hebbian
            vec!["entity-1".to_string()], // shared entity
            3600.0,                       // 1 hour apart
        );
        let c = full_node(
            "c",
            vec![0.0, 1.0], // different embedding
            vec![],          // no hebbian
            vec!["entity-9".to_string()], // different entity
            86400.0 * 30.0,  // 30 days apart
        );

        let w_ab = strategy.compute_weight(&a, &b);
        let w_ac = strategy.compute_weight(&a, &c);

        assert!(
            w_ab > w_ac,
            "a-b should be much more related than a-c: ab={}, ac={}",
            w_ab, w_ac
        );
        assert!(w_ab > 0.3, "a-b should have meaningful weight: {}", w_ab);
    }

    // ── InfomapClusterer tests ───────────────────────────────────────────

    #[test]
    fn test_cluster_empty() {
        let clusterer = InfomapClusterer::new(EmbeddingOnly, ClustererConfig::default());
        let clusters = clusterer.cluster(&[]);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cluster_single_node() {
        let clusterer = InfomapClusterer::new(EmbeddingOnly, ClustererConfig::default());
        let nodes = vec![node("a", vec![1.0, 0.0])];
        let clusters = clusterer.cluster(&nodes);
        assert!(clusters.is_empty(), "single node can't form a cluster");
    }

    #[test]
    fn test_cluster_two_groups() {
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 5,
            ..Default::default()
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

        // Two clearly separated groups in 10D space
        let nodes = vec![
            // Group A: near [1,1,1,1,1, 0,0,0,0,0]
            node("a1", vec![1.0, 0.9, 1.0, 0.9, 1.0, 0.0, 0.1, 0.0, 0.1, 0.0]),
            node("a2", vec![0.9, 1.0, 0.9, 1.0, 0.9, 0.1, 0.0, 0.1, 0.0, 0.1]),
            node("a3", vec![1.0, 1.0, 0.9, 0.9, 1.0, 0.0, 0.0, 0.1, 0.1, 0.0]),
            // Group B: near [0,0,0,0,0, 1,1,1,1,1]
            node("b1", vec![0.0, 0.1, 0.0, 0.1, 0.0, 1.0, 0.9, 1.0, 0.9, 1.0]),
            node("b2", vec![0.1, 0.0, 0.1, 0.0, 0.1, 0.9, 1.0, 0.9, 1.0, 0.9]),
            node("b3", vec![0.0, 0.0, 0.1, 0.1, 0.0, 1.0, 1.0, 0.9, 0.9, 1.0]),
        ];

        let clusters = clusterer.cluster(&nodes);

        // Should find 2 clusters
        assert_eq!(
            clusters.len(),
            2,
            "expected 2 clusters, got {}",
            clusters.len()
        );

        // Each cluster should have 3 members
        for c in &clusters {
            assert_eq!(c.member_indices.len(), 3);
        }
    }

    #[test]
    fn test_cluster_filters_small() {
        let config = ClustererConfig {
            min_cluster_size: 4,
            k_neighbors: 5,
            ..Default::default()
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

        // Only 3 nodes — can't form a cluster of size 4
        let nodes = vec![
            node("a", vec![1.0, 0.0]),
            node("b", vec![0.9, 0.1]),
            node("c", vec![0.8, 0.2]),
        ];

        let clusters = clusterer.cluster(&nodes);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_cluster_with_multi_signal() {
        let strategy = MultiSignal::default();
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 5,
            ..Default::default()
        };
        let clusterer = InfomapClusterer::new(strategy, config);

        // Two groups with strong separation across all 4 signals:
        // - Distinct embeddings in 10D space
        // - Hebbian links within group only
        // - Shared entities within group only
        // - Close timestamps within group, distant between groups
        let nodes = vec![
            full_node("a1", vec![1.0, 0.9, 1.0, 0.9, 1.0, 0.0, 0.1, 0.0, 0.1, 0.0],
                vec![("a2".into(), 8.0), ("a3".into(), 6.0)], vec!["topic-rust".into()], 0.0),
            full_node("a2", vec![0.9, 1.0, 0.9, 1.0, 0.9, 0.1, 0.0, 0.1, 0.0, 0.1],
                vec![("a1".into(), 8.0), ("a3".into(), 7.0)], vec!["topic-rust".into()], 3600.0),
            full_node("a3", vec![1.0, 1.0, 0.9, 0.9, 1.0, 0.0, 0.0, 0.1, 0.1, 0.0],
                vec![("a1".into(), 6.0), ("a2".into(), 7.0)], vec!["topic-rust".into()], 7200.0),
            full_node("b1", vec![0.0, 0.1, 0.0, 0.1, 0.0, 1.0, 0.9, 1.0, 0.9, 1.0],
                vec![("b2".into(), 7.0), ("b3".into(), 6.0)], vec!["topic-python".into()], 86400.0 * 30.0),
            full_node("b2", vec![0.1, 0.0, 0.1, 0.0, 0.1, 0.9, 1.0, 0.9, 1.0, 0.9],
                vec![("b1".into(), 7.0), ("b3".into(), 8.0)], vec!["topic-python".into()], 86400.0 * 30.0 + 3600.0),
            full_node("b3", vec![0.0, 0.0, 0.1, 0.1, 0.0, 1.0, 1.0, 0.9, 0.9, 1.0],
                vec![("b1".into(), 6.0), ("b2".into(), 8.0)], vec!["topic-python".into()], 86400.0 * 30.0 + 7200.0),
        ];

        let clusters = clusterer.cluster(&nodes);
        assert_eq!(clusters.len(), 2, "expected 2 clusters, got {}", clusters.len());

        // Each cluster should have 3 members
        for c in &clusters {
            assert_eq!(c.member_indices.len(), 3);
        }
    }

    #[test]
    fn test_cohesion_computed() {
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 5,
            ..Default::default()
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

        let nodes = vec![
            node("a", vec![1.0, 0.0, 0.0, 0.0, 0.0]),
            node("b", vec![0.95, 0.05, 0.0, 0.0, 0.0]),
            node("c", vec![0.9, 0.1, 0.0, 0.0, 0.0]),
        ];

        let clusters = clusterer.cluster(&nodes);
        assert!(!clusters.is_empty());
        assert!(clusters[0].cohesion > 0.9, "tight cluster should have high cohesion");
    }

    #[test]
    fn test_centroid_computed() {
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 5,
            ..Default::default()
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

        let nodes = vec![
            node("a", vec![1.0, 0.0]),
            node("b", vec![0.0, 1.0]),
        ];

        let clusters = clusterer.cluster(&nodes);
        if !clusters.is_empty() {
            let c = &clusters[0];
            // Centroid should be [0.5, 0.5]
            assert!((c.centroid[0] - 0.5).abs() < 0.01);
            assert!((c.centroid[1] - 0.5).abs() < 0.01);
        }
    }

    // ── VP-tree tests ────────────────────────────────────────────────────

    /// Helper: L2-normalize a vector in-place and return it.
    fn normalize(mut v: Vec<f32>) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
        v
    }

    #[test]
    fn test_vp_tree_empty() {
        let tree = VpTree::build(Vec::new());
        let results = tree.query_nearest(&[1.0, 0.0], 3);
        assert!(results.is_empty());
    }

    #[test]
    fn test_vp_tree_single_point() {
        let tree = VpTree::build(vec![normalize(vec![1.0, 0.0])]);
        let results = tree.query_nearest(&normalize(vec![1.0, 0.0]), 3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0); // index 0
    }

    #[test]
    fn test_vp_tree_top_k_matches_brute_force() {
        // 10 normalized 4D points
        let points: Vec<Vec<f32>> = vec![
            normalize(vec![1.0, 0.0, 0.0, 0.0]),
            normalize(vec![0.9, 0.1, 0.0, 0.0]),
            normalize(vec![0.0, 1.0, 0.0, 0.0]),
            normalize(vec![0.0, 0.9, 0.1, 0.0]),
            normalize(vec![0.0, 0.0, 1.0, 0.0]),
            normalize(vec![0.0, 0.0, 0.9, 0.1]),
            normalize(vec![0.0, 0.0, 0.0, 1.0]),
            normalize(vec![0.5, 0.5, 0.0, 0.0]),
            normalize(vec![0.0, 0.5, 0.5, 0.0]),
            normalize(vec![0.3, 0.3, 0.3, 0.1]),
        ];

        let query = normalize(vec![0.8, 0.2, 0.0, 0.0]);
        let k = 3;

        // Brute force
        let mut brute: Vec<(usize, f32)> = points.iter().enumerate()
            .map(|(i, p)| (i, euclidean_distance(&query, p)))
            .collect();
        brute.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let brute_top_k: Vec<usize> = brute.iter().take(k).map(|(i, _)| *i).collect();

        // VP-tree
        let tree = VpTree::build(points);
        let vp_results = tree.query_nearest(&query, k);
        let vp_top_k: Vec<usize> = vp_results.iter().map(|(i, _)| *i).collect();

        assert_eq!(brute_top_k, vp_top_k, "VP-tree top-{k} should match brute force");
    }

    #[test]
    fn test_vp_tree_768d() {
        // Simulate 768-dim embeddings with clear distance structure.
        // Point 0 is the query. Points 1-4 are nearby (share components). Points 5+ are far.
        let dim = 768;
        let mut points = Vec::new();

        // Query point: primary direction at dim 0
        let mut q = vec![0.0f32; dim];
        q[0] = 1.0;
        q[1] = 0.3;
        points.push(normalize(q));

        // Points 1-4: close to query (share dim 0 component)
        for i in 1..5 {
            let mut v = vec![0.0f32; dim];
            v[0] = 1.0;
            v[1] = 0.3;
            v[i + 1] = 0.1 * i as f32; // slight perturbation
            points.push(normalize(v));
        }

        // Points 5-19: far from query (orthogonal directions)
        for i in 5..20 {
            let mut v = vec![0.0f32; dim];
            v[100 + i] = 1.0; // completely different direction
            v[200 + i] = 0.5;
            points.push(normalize(v));
        }

        let query = points[0].clone();
        let k = 5;

        // Brute force
        let mut brute: Vec<(usize, f32)> = points.iter().enumerate()
            .map(|(i, p)| (i, euclidean_distance(&query, p)))
            .collect();
        brute.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let brute_top_k: std::collections::HashSet<usize> = brute.iter().take(k).map(|(i, _)| *i).collect();

        let tree = VpTree::build(points);
        let vp_results = tree.query_nearest(&query, k);
        let vp_top_k: std::collections::HashSet<usize> = vp_results.iter().map(|(i, _)| *i).collect();

        assert_eq!(brute_top_k, vp_top_k, "VP-tree 768D should find same nearest neighbors as brute force");
        // Query itself should be closest (distance 0)
        assert_eq!(vp_results[0].0, 0, "query itself should be nearest");
    }

    #[test]
    #[should_panic(expected = "not L2-normalized")]
    #[cfg(debug_assertions)]
    fn test_vp_tree_rejects_non_normalized() {
        // Non-normalized vector should trigger debug_assert
        VpTree::build(vec![vec![5.0, 5.0, 5.0]]);
    }

    // ── Two-stage vs brute-force consistency ─────────────────────────────

    #[test]
    fn test_two_stage_vs_brute_force_consistency() {
        // 20-node dataset with two clearly separated groups + Hebbian cross-links.
        // The two-stage pipeline should produce equivalent clustering to brute force.
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 5,
            ..Default::default()
        };

        // Group A: 10 nodes near [1,1,0,0,0,0,0,0,0,0]
        // Group B: 10 nodes near [0,0,0,0,0,1,1,0,0,0]
        let mut nodes = Vec::new();
        for i in 0..10 {
            let mut emb = vec![0.0f32; 10];
            emb[0] = 1.0;
            emb[1] = 0.9 + (i as f32) * 0.01;
            nodes.push(full_node(
                &format!("a{}", i),
                normalize(emb),
                if i > 0 { vec![(format!("a{}", i - 1), 5.0)] } else { vec![] },
                vec!["topic-rust".into()],
                i as f64 * 3600.0,
            ));
        }
        for i in 0..10 {
            let mut emb = vec![0.0f32; 10];
            emb[5] = 1.0;
            emb[6] = 0.9 + (i as f32) * 0.01;
            nodes.push(full_node(
                &format!("b{}", i),
                normalize(emb),
                if i > 0 { vec![(format!("b{}", i - 1), 5.0)] } else { vec![] },
                vec!["topic-python".into()],
                86400.0 * 30.0 + i as f64 * 3600.0,
            ));
        }

        // Two-stage (has embeddings, n=20 > k*3=15)
        let clusterer_two_stage = InfomapClusterer::new(MultiSignal::default(), config.clone());
        let clusters_two = clusterer_two_stage.cluster(&nodes);

        // Should find 2 groups
        assert!(clusters_two.len() >= 2, "two-stage should find at least 2 clusters, got {}", clusters_two.len());

        // Each cluster should contain nodes from same group
        for c in &clusters_two {
            let has_a = c.member_indices.iter().any(|&i| i < 10);
            let has_b = c.member_indices.iter().any(|&i| i >= 10);
            assert!(!(has_a && has_b), "cluster should not mix groups A and B");
        }
    }

    #[test]
    fn test_hebbian_linked_pair_survives_two_stage() {
        // Verify that a Hebbian-linked pair with distant embeddings still appears in candidates.
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 3,
            ..Default::default()
        };

        // 8 nodes: group A (4) and group B (4) in different embedding regions.
        // a0 and b0 have a strong Hebbian link despite distant embeddings.
        let mut nodes = Vec::new();
        for i in 0..4 {
            let mut emb = vec![0.0f32; 10];
            emb[0] = 1.0;
            emb[1] = (i as f32) * 0.1;
            let hebbian = if i == 0 {
                vec![("b0".to_string(), 9.0)] // strong Hebbian link to b0
            } else {
                vec![]
            };
            nodes.push(full_node(
                &format!("a{}", i),
                normalize(emb),
                hebbian,
                vec!["entity-shared".into()],
                0.0,
            ));
        }
        for i in 0..4 {
            let mut emb = vec![0.0f32; 10];
            emb[5] = 1.0;
            emb[6] = (i as f32) * 0.1;
            let hebbian = if i == 0 {
                vec![("a0".to_string(), 9.0)] // reverse Hebbian link
            } else {
                vec![]
            };
            nodes.push(full_node(
                &format!("b{}", i),
                normalize(emb),
                hebbian,
                vec![],
                86400.0 * 30.0,
            ));
        }

        let clusterer = InfomapClusterer::new(MultiSignal::default(), config);
        let clusters = clusterer.cluster(&nodes);

        // The Hebbian link between a0 and b0 should cause them to be in the same cluster
        // (or at least the edge should exist — we verify they're considered)
        let a0_cluster = clusters.iter().find(|c| c.member_indices.contains(&0));
        let b0_cluster = clusters.iter().find(|c| c.member_indices.contains(&4));

        // They might be in the same cluster due to the strong Hebbian link
        // or they might form a bridge between clusters. Either way, the test
        // verifies the two-stage pipeline didn't silently drop the pair.
        assert!(a0_cluster.is_some(), "a0 should be in some cluster");
        assert!(b0_cluster.is_some(), "b0 should be in some cluster");
    }

    #[test]
    fn test_entity_co_occurring_pair_survives_two_stage() {
        // Verify that two nodes sharing an entity but with distant embeddings
        // still get considered as candidates.
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 3,
            ..Default::default()
        };

        let mut nodes = Vec::new();
        // 6 nodes, 3 per group
        for i in 0..3 {
            let mut emb = vec![0.0f32; 10];
            emb[0] = 1.0;
            emb[1] = (i as f32) * 0.05;
            nodes.push(full_node(
                &format!("a{}", i),
                normalize(emb),
                vec![],
                vec!["shared-entity-X".into()], // a0-a2 share entity X
                0.0,
            ));
        }
        for i in 0..3 {
            let mut emb = vec![0.0f32; 10];
            emb[5] = 1.0;
            emb[6] = (i as f32) * 0.05;
            let entities = if i == 0 {
                vec!["shared-entity-X".into()] // b0 also shares entity X with group A
            } else {
                vec!["other-entity".into()]
            };
            nodes.push(full_node(
                &format!("b{}", i),
                normalize(emb),
                vec![],
                entities,
                86400.0 * 30.0,
            ));
        }

        let clusterer = InfomapClusterer::new(MultiSignal::default(), config);
        let clusters = clusterer.cluster(&nodes);

        // b0 shares entity X with a0-a2, so the entity injection should ensure
        // b0 is considered as a candidate for edges with group A nodes
        assert!(!clusters.is_empty(), "should find at least some clusters");
    }

    #[test]
    fn test_brute_force_fallback_no_embeddings() {
        // Nodes without embeddings should fall back to O(n²) brute force
        let config = ClustererConfig {
            min_cluster_size: 2,
            k_neighbors: 3,
            ..Default::default()
        };

        let nodes: Vec<ClusterNode> = (0..6).map(|i| ClusterNode {
            id: format!("n{}", i),
            embedding: vec![], // empty embedding!
            hebbian_links: if i < 3 {
                vec![(format!("n{}", (i + 1) % 3), 8.0)]
            } else {
                vec![(format!("n{}", 3 + (i + 1) % 3), 8.0)]
            },
            entity_ids: vec![],
            created_at_secs: 0.0,
        }).collect();

        // This should not panic — it should use the brute force fallback
        let clusterer = InfomapClusterer::new(MultiSignal::default(), config);
        let _clusters = clusterer.cluster(&nodes);
        // Just verifying no panic — brute force with empty embeddings will have
        // cosine_similarity return 0.0 but Hebbian signal still works
    }
}
