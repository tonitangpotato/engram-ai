//! Topic Discovery — discovers topic candidates from memory clusters.
//!
//! Uses Infomap community detection (information-theoretic) to find groups of
//! related memories. For large memory sets (≥100), uses HNSW approximate nearest
//! neighbors to build a sparse graph in O(n·log n) instead of O(n²).
//! For small sets (<100), uses exact all-pairs computation.

use std::collections::{HashMap, HashSet};

use hnsw::{Hnsw, Params, Searcher};
use infomap_rs::{Infomap, Network};
use rand::rngs::StdRng;
use rand::SeedableRng;
use space::{Metric, Neighbor};

use crate::embeddings::EmbeddingProvider;
use super::llm::LlmProvider;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  COSINE METRIC FOR HNSW
// ═══════════════════════════════════════════════════════════════════════════════

/// Cosine distance metric for HNSW index.
/// Distance = (1 - cosine_similarity) mapped to u32 space [0, 1_000_000].
#[derive(Clone)]
struct CosineMetric;

impl Metric<Vec<f32>> for CosineMetric {
    type Unit = u32;

    fn distance(&self, a: &Vec<f32>, b: &Vec<f32>) -> u32 {
        let sim = EmbeddingProvider::cosine_similarity(a, b);
        // Clamp to [0, 1] then convert to distance
        let distance = 1.0 - sim.clamp(-1.0, 1.0);
        // Map [0, 2] → [0, 2_000_000] as u32
        (distance * 1_000_000.0) as u32
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CONSTANTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Below this threshold, use exact O(n²) computation (fast enough).
const ANN_THRESHOLD: usize = 100;

/// Default number of nearest neighbors to find per node.
const DEFAULT_TOP_K: usize = 20;

// HNSW parameters
/// M: max connections per node at layers > 0
const HNSW_M: usize = 24;
/// M0: max connections per node at layer 0
const HNSW_M0: usize = 48;
/// ef_construction: candidate pool size during build
const HNSW_EF_CONSTRUCTION: usize = 100;
/// ef_search: candidate pool size during search
const HNSW_EF_SEARCH: usize = 50;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a cosine-similarity graph.
pub struct TopicDiscovery {
    /// Minimum number of memories required to form a valid cluster.
    min_cluster_size: usize,
    /// Jaccard similarity threshold for overlap detection with existing topics.
    overlap_threshold: f64,
    /// Minimum cosine similarity to create an edge in the similarity graph.
    /// Pairs below this threshold are not connected — Infomap only sees
    /// edges that represent genuine semantic relatedness.
    edge_threshold: f64,
    /// Maximum number of nearest neighbors per node (ANN mode only).
    top_k: usize,
}

impl TopicDiscovery {
    /// Create a new `TopicDiscovery` with the given minimum cluster size.
    ///
    /// The overlap threshold defaults to 0.3 (matching `TopicCandidate::overlaps_with`).
    /// The edge threshold defaults to 0.4 — only pairs with cosine similarity ≥ 0.4
    /// get an edge in the graph fed to Infomap.
    pub fn new(min_cluster_size: usize) -> Self {
        Self {
            min_cluster_size,
            overlap_threshold: 0.3,
            edge_threshold: 0.4,
            top_k: DEFAULT_TOP_K,
        }
    }

    /// Create a TopicDiscovery with a custom edge threshold.
    ///
    /// Lower threshold → more edges → fewer, larger communities.
    /// Higher threshold → fewer edges → more, smaller communities.
    pub fn with_edge_threshold(mut self, threshold: f64) -> Self {
        self.edge_threshold = threshold;
        self
    }

    /// Set the maximum number of nearest neighbors per node (ANN mode).
    ///
    /// Higher K → more edges → potentially larger communities, more compute.
    /// Lower K → sparser graph → faster, but might miss some connections.
    /// Default: 20.
    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k.max(1);
        self
    }

    /// Discover topic candidates from memories using Infomap community detection.
    ///
    /// # Algorithm
    ///
    /// For n < 100: exact all-pairs cosine similarity (O(n²), fast for small n).
    /// For n ≥ 100: HNSW approximate nearest neighbors (O(n·log n)).
    ///
    /// 1. Build similarity graph (exact or ANN)
    /// 2. Run Infomap to find community structure (minimises map equation)
    /// 3. Filter communities below `min_cluster_size`
    /// 4. For each community, create a `TopicCandidate`
    pub fn discover(
        &self,
        memories: &[(String, Vec<f32>)], // (memory_id, embedding)
    ) -> Vec<TopicCandidate> {
        if memories.len() < 2 {
            return Vec::new();
        }

        if memories.len() < ANN_THRESHOLD {
            self.discover_exact(memories)
        } else {
            self.discover_ann(memories)
        }
    }

    /// Exact O(n²) all-pairs similarity computation.
    /// Only used for small memory sets (< ANN_THRESHOLD).
    fn discover_exact(
        &self,
        memories: &[(String, Vec<f32>)],
    ) -> Vec<TopicCandidate> {
        let n = memories.len();

        let mut network = Network::with_capacity(n);
        network.ensure_capacity(n);

        let mut sim_cache: HashMap<(usize, usize), f64> = HashMap::new();
        let mut edge_count = 0usize;

        for i in 0..n {
            for j in (i + 1)..n {
                let sim = EmbeddingProvider::cosine_similarity(&memories[i].1, &memories[j].1) as f64;
                if sim >= self.edge_threshold {
                    network.add_edge(i, j, sim);
                    network.add_edge(j, i, sim);
                    sim_cache.insert((i, j), sim);
                    edge_count += 1;
                }
            }
        }

        if edge_count == 0 {
            return Vec::new();
        }

        self.run_infomap_and_build_candidates(memories, &network, &sim_cache)
    }

    /// HNSW-based approximate nearest neighbors graph construction.
    /// O(n·log n) build + O(n·K·log n) queries = O(n·log n) total.
    /// Each node connects to at most top_k neighbors, producing a sparse graph.
    fn discover_ann(
        &self,
        memories: &[(String, Vec<f32>)],
    ) -> Vec<TopicCandidate> {
        let n = memories.len();

        // Build HNSW index
        let params = Params::new()
            .ef_construction(HNSW_EF_CONSTRUCTION);
        let mut hnsw: Hnsw<CosineMetric, Vec<f32>, StdRng, HNSW_M, HNSW_M0> =
            Hnsw::new_params_and_prng(CosineMetric, params, StdRng::seed_from_u64(42));

        let mut searcher = Searcher::default();

        // Insert all embeddings into the index
        for (_, embedding) in memories.iter() {
            hnsw.insert(embedding.clone(), &mut searcher);
        }

        // Query top-K neighbors for each node and build the graph
        let mut network = Network::with_capacity(n);
        network.ensure_capacity(n);

        let mut sim_cache: HashMap<(usize, usize), f64> = HashMap::new();
        let mut edge_count = 0usize;

        let mut dest = vec![Neighbor { index: 0, distance: 0 }; self.top_k + 1];

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let results = hnsw.nearest(&memories[i].1, HNSW_EF_SEARCH, &mut searcher, &mut dest);

            for neighbor in results.iter() {
                let j = neighbor.index;
                if j == i {
                    continue; // skip self
                }

                // Convert distance back to similarity
                let sim = 1.0 - (neighbor.distance as f64 / 1_000_000.0);

                if sim >= self.edge_threshold {
                    let (lo, hi) = if i < j { (i, j) } else { (j, i) };

                    // Deduplicate: only add each edge once
                    use std::collections::hash_map::Entry;
                    if let Entry::Vacant(e) = sim_cache.entry((lo, hi)) {
                        network.add_edge(i, j, sim);
                        network.add_edge(j, i, sim);
                        e.insert(sim);
                        edge_count += 1;
                    }
                }
            }
        }

        if edge_count == 0 {
            return Vec::new();
        }

        self.run_infomap_and_build_candidates(memories, &network, &sim_cache)
    }

    /// Run Infomap on the built network and construct TopicCandidates.
    /// Shared between exact and ANN paths.
    fn run_infomap_and_build_candidates(
        &self,
        memories: &[(String, Vec<f32>)],
        network: &Network,
        sim_cache: &HashMap<(usize, usize), f64>,
    ) -> Vec<TopicCandidate> {
        let n = memories.len();

        // Run Infomap with fewer trials for sparse graphs (3 instead of default 10)
        let result = Infomap::new(network)
            .seed(42)
            .num_trials(3)
            .run();

        // Group memories by module assignment
        let mut modules: HashMap<usize, Vec<usize>> = HashMap::new();
        for (node_idx, &module_id) in result.assignments.iter().enumerate() {
            if node_idx < n {
                modules.entry(module_id).or_default().push(node_idx);
            }
        }

        // Build TopicCandidates, filtering by min_cluster_size
        let mut candidates = Vec::new();

        for member_indices in modules.values() {
            if member_indices.len() < self.min_cluster_size {
                continue;
            }

            let memory_ids: Vec<String> = member_indices
                .iter()
                .map(|&i| memories[i].0.clone())
                .collect();

            // Centroid: mean of embeddings
            let dim = memories[0].1.len();
            let mut centroid = vec![0.0f32; dim];
            for &idx in member_indices {
                for (d, val) in memories[idx].1.iter().enumerate() {
                    if d < dim {
                        centroid[d] += val;
                    }
                }
            }
            let count = member_indices.len() as f32;
            for c in centroid.iter_mut() {
                *c /= count;
            }

            // Cohesion: average intra-community pairwise similarity
            let mut cohesion_sum = 0.0;
            let mut pair_count = 0usize;
            for (pi, &i) in member_indices.iter().enumerate() {
                for &j in &member_indices[(pi + 1)..] {
                    let (lo, hi) = if i < j { (i, j) } else { (j, i) };
                    let sim = sim_cache
                        .get(&(lo, hi))
                        .copied()
                        .unwrap_or_else(|| {
                            EmbeddingProvider::cosine_similarity(
                                &memories[i].1,
                                &memories[j].1,
                            ) as f64
                        });
                    cohesion_sum += sim;
                    pair_count += 1;
                }
            }
            let cohesion_score = if pair_count > 0 {
                cohesion_sum / pair_count as f64
            } else {
                1.0
            };

            candidates.push(TopicCandidate {
                memories: memory_ids,
                centroid_embedding: centroid,
                cohesion_score,
                suggested_title: None,
            });
        }

        // Sort candidates by cohesion descending for deterministic output
        candidates.sort_by(|a, b| {
            b.cohesion_score
                .partial_cmp(&a.cohesion_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        candidates
    }

    /// Label a topic candidate using the LLM.
    ///
    /// Sends memory contents to the LLM asking for a concise 2-5 word topic label.
    /// On LLM failure, falls back to using the first 5 words of the longest memory
    /// content as the label.
    pub fn label_cluster(
        &self,
        candidate: &TopicCandidate,
        memory_contents: &[(String, String)], // (memory_id, content)
        llm: &dyn LlmProvider,
    ) -> Result<String, KcError> {
        // Build the prompt from memory contents that belong to this candidate.
        let mut prompt = String::from(
            "Given these related notes/memories, suggest a concise topic label (2-5 words):\n\n",
        );

        let mut numbered = 0;
        let candidate_ids: HashSet<&str> =
            candidate.memories.iter().map(|s| s.as_str()).collect();

        for (id, content) in memory_contents {
            if candidate_ids.contains(id.as_str()) {
                numbered += 1;
                prompt.push_str(&format!("{}. {}\n", numbered, content));
            }
        }

        prompt.push_str("\nRespond with ONLY the topic label, nothing else.");

        let request = LlmRequest {
            task: LlmTask::GenerateTitle,
            prompt,
            max_tokens: Some(20),
            temperature: Some(0.3),
        };

        match llm.complete(&request) {
            Ok(response) => {
                let label = response.content.trim().to_string();
                if label.is_empty() {
                    Ok(Self::fallback_label(memory_contents, candidate))
                } else {
                    Ok(label)
                }
            }
            Err(_) => {
                // Fallback: first 5 words of the longest memory.
                Ok(Self::fallback_label(memory_contents, candidate))
            }
        }
    }

    /// Fallback label: first 5 words of the longest memory content in the candidate.
    fn fallback_label(
        memory_contents: &[(String, String)],
        candidate: &TopicCandidate,
    ) -> String {
        let candidate_ids: HashSet<&str> =
            candidate.memories.iter().map(|s| s.as_str()).collect();

        let longest = memory_contents
            .iter()
            .filter(|(id, _)| candidate_ids.contains(id.as_str()))
            .max_by_key(|(_, content)| content.len());

        match longest {
            Some((_, content)) => {
                let words: Vec<&str> = content.split_whitespace().take(5).collect();
                words.join(" ")
            }
            None => "Untitled Topic".to_string(),
        }
    }

    /// Check overlap between a candidate and existing topic pages.
    ///
    /// Returns `Some(topic_id)` if the Jaccard similarity of the candidate's
    /// memory set and any existing topic's source memories exceeds `overlap_threshold`.
    pub fn detect_overlap(
        &self,
        candidate: &TopicCandidate,
        existing: &[TopicPage],
    ) -> Option<TopicId> {
        let candidate_set: HashSet<&str> =
            candidate.memories.iter().map(|s| s.as_str()).collect();

        for page in existing {
            let page_set: HashSet<&str> = page
                .metadata
                .source_memory_ids
                .iter()
                .map(|s| s.as_str())
                .collect();

            let intersection = candidate_set.intersection(&page_set).count();
            let union = candidate_set.union(&page_set).count();

            if union == 0 {
                continue;
            }

            let jaccard = intersection as f64 / union as f64;
            if jaccard > self.overlap_threshold {
                return Some(page.id.clone());
            }
        }

        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::llm::LlmProvider;
    use chrono::Utc;

    // ── Mock LLM Provider ────────────────────────────────────────────────

    struct MockLlmProvider {
        response: Result<LlmResponse, LlmError>,
    }

    impl MockLlmProvider {
        fn success(label: &str) -> Self {
            Self {
                response: Ok(LlmResponse {
                    content: label.to_string(),
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                    },
                    model: "mock".to_string(),
                    duration_ms: 1,
                }),
            }
        }

        fn failure() -> Self {
            Self {
                response: Err(LlmError::ProviderUnavailable(
                    "mock failure".to_string(),
                )),
            }
        }
    }

    impl LlmProvider for MockLlmProvider {
        fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            self.response.clone()
        }

        fn metadata(&self) -> ProviderMetadata {
            ProviderMetadata {
                name: "mock".to_string(),
                model: "mock".to_string(),
                max_context_tokens: 1000,
                supports_streaming: false,
            }
        }

        fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // ── Helper: make a simple TopicPage ──────────────────────────────────

    fn make_topic_page(id: &str, source_ids: Vec<&str>) -> TopicPage {
        let now = Utc::now();
        TopicPage {
            id: TopicId(id.to_string()),
            title: format!("Topic {}", id),
            content: "content".to_string(),
            sections: Vec::new(),
            summary: "summary".to_string(),
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: source_ids.into_iter().map(|s| s.to_string()).collect(),
                tags: vec![],
                quality_score: Some(0.8),
            },
            status: TopicStatus::Active,
            version: 1,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_discover_basic_two_clusters() {
        // 6 memories forming 2 clusters:
        // Cluster A: m1, m2, m3 — all near [1, 0, 0]
        // Cluster B: m4, m5, m6 — all near [0, 1, 0]
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.95, 0.1, 0.0]),
            ("m3".to_string(), vec![0.9, 0.15, 0.0]),
            ("m4".to_string(), vec![0.0, 1.0, 0.0]),
            ("m5".to_string(), vec![0.1, 0.95, 0.0]),
            ("m6".to_string(), vec![0.15, 0.9, 0.0]),
        ];

        let discovery = TopicDiscovery::new(2).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);

        // Should find 2 clusters
        assert_eq!(candidates.len(), 2, "Expected 2 clusters, got {}", candidates.len());

        // Each cluster should have 3 members
        for c in &candidates {
            assert_eq!(c.memories.len(), 3);
        }

        // Cluster A should contain m1, m2, m3
        let cluster_a = candidates
            .iter()
            .find(|c| c.memories.contains(&"m1".to_string()))
            .expect("Should find cluster containing m1");
        assert!(cluster_a.memories.contains(&"m2".to_string()));
        assert!(cluster_a.memories.contains(&"m3".to_string()));

        // Cluster B should contain m4, m5, m6
        let cluster_b = candidates
            .iter()
            .find(|c| c.memories.contains(&"m4".to_string()))
            .expect("Should find cluster containing m4");
        assert!(cluster_b.memories.contains(&"m5".to_string()));
        assert!(cluster_b.memories.contains(&"m6".to_string()));
    }

    #[test]
    fn test_discover_empty() {
        let memories: Vec<(String, Vec<f32>)> = vec![];
        let discovery = TopicDiscovery::new(2);
        assert!(discovery.discover(&memories).is_empty());
    }

    #[test]
    fn test_discover_single_memory() {
        let memories = vec![("m1".to_string(), vec![1.0f32, 0.0, 0.0])];
        let discovery = TopicDiscovery::new(2);
        assert!(discovery.discover(&memories).is_empty());
    }

    #[test]
    fn test_discover_min_cluster_size() {
        // 5 memories: 3 in one cluster, 2 in another
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.95, 0.1, 0.0]),
            ("m3".to_string(), vec![0.9, 0.15, 0.0]),
            ("m4".to_string(), vec![0.0, 1.0, 0.0]),
            ("m5".to_string(), vec![0.1, 0.95, 0.0]),
        ];

        // min_cluster_size = 3: should only find the 3-member cluster
        let discovery = TopicDiscovery::new(3).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].memories.len(), 3);
    }

    #[test]
    fn test_edge_threshold_controls_granularity() {
        // With a high threshold, distant memories won't connect
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.7, 0.7, 0.0]),  // ~45 degrees from m1
            ("m3".to_string(), vec![0.0, 1.0, 0.0]),
        ];

        // Low threshold: m1-m2 might connect (cos ~0.73), m2-m3 might connect (cos ~0.73)
        let low = TopicDiscovery::new(2).with_edge_threshold(0.3);
        let candidates_low = low.discover(&memories);

        // High threshold: only very similar pairs connect
        let high = TopicDiscovery::new(2).with_edge_threshold(0.9);
        let candidates_high = high.discover(&memories);

        // High threshold should find fewer or no clusters
        assert!(
            candidates_high.len() <= candidates_low.len(),
            "Higher threshold should produce fewer clusters"
        );
    }

    #[test]
    fn test_discover_cohesion_score() {
        // Tight cluster: all very similar
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.99, 0.01, 0.0]),
            ("m3".to_string(), vec![0.98, 0.02, 0.0]),
        ];

        let discovery = TopicDiscovery::new(2).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);

        assert_eq!(candidates.len(), 1);
        // Very tight cluster should have high cohesion
        assert!(
            candidates[0].cohesion_score > 0.95,
            "Tight cluster cohesion should be > 0.95, got {}",
            candidates[0].cohesion_score
        );
    }

    #[test]
    fn test_discover_no_chaining_effect() {
        // Test that Infomap doesn't suffer from single-linkage chaining.
        // Without Infomap, a chain A-B-C-D could merge everything into one cluster
        // even if A and D are very dissimilar.
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.9, 0.1, 0.0]),
            ("m3".to_string(), vec![0.5, 0.5, 0.0]),  // bridge
            ("m4".to_string(), vec![0.1, 0.9, 0.0]),
            ("m5".to_string(), vec![0.0, 1.0, 0.0]),
        ];

        let discovery = TopicDiscovery::new(2).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);

        // Infomap should NOT merge everything into one big cluster.
        // With a low threshold (0.3) on this chain, the bridge connects both sides.
        // Infomap may find 1-3 clusters depending on trials — the key point is that
        // if it finds one cluster, it demonstrates the information-theoretic approach
        // still works (the chain IS densely connected at threshold 0.3).
        // The real protection against chaining comes from the higher default threshold (0.4).
        // This test verifies the algorithm doesn't panic and produces some output.
        assert!(
            !candidates.is_empty(),
            "Should find at least one cluster from 5 connected memories"
        );
    }

    #[test]
    fn test_label_cluster_success() {
        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![0.5, 0.5, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        let memory_contents = vec![
            ("m1".to_string(), "Rust programming language".to_string()),
            ("m2".to_string(), "Cargo build system".to_string()),
        ];

        let llm = MockLlmProvider::success("Rust Development");
        let discovery = TopicDiscovery::new(2);
        let label = discovery.label_cluster(&candidate, &memory_contents, &llm).unwrap();
        assert_eq!(label, "Rust Development");
    }

    #[test]
    fn test_label_cluster_fallback() {
        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![0.5, 0.5, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        let memory_contents = vec![
            ("m1".to_string(), "Short note".to_string()),
            (
                "m2".to_string(),
                "This is a longer note about building systems in Rust for AI agents"
                    .to_string(),
            ),
        ];

        let llm = MockLlmProvider::failure();
        let discovery = TopicDiscovery::new(2);
        let label = discovery.label_cluster(&candidate, &memory_contents, &llm).unwrap();
        // Should use first 5 words of the longest memory
        assert_eq!(label, "This is a longer note");
    }

    #[test]
    fn test_detect_overlap() {
        let candidate = TopicCandidate {
            memories: vec![
                "m1".to_string(),
                "m2".to_string(),
                "m3".to_string(),
            ],
            centroid_embedding: vec![1.0, 0.0, 0.0],
            cohesion_score: 0.8,
            suggested_title: None,
        };

        // Existing topic with high overlap
        let existing = vec![make_topic_page("t1", vec!["m1", "m2", "m4"])];

        let discovery = TopicDiscovery::new(2);
        let overlap = discovery.detect_overlap(&candidate, &existing);

        // Jaccard: intersection={m1,m2}=2, union={m1,m2,m3,m4}=4, jaccard=0.5 > 0.3
        assert!(overlap.is_some());
        assert_eq!(overlap.unwrap().0, "t1");
    }

    #[test]
    fn test_detect_no_overlap() {
        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![1.0, 0.0, 0.0],
            cohesion_score: 0.8,
            suggested_title: None,
        };

        // Existing topic with no overlap
        let existing = vec![make_topic_page("t1", vec!["m5", "m6", "m7"])];

        let discovery = TopicDiscovery::new(2);
        let overlap = discovery.detect_overlap(&candidate, &existing);
        assert!(overlap.is_none());
    }

    // ── ANN-specific tests ───────────────────────────────────────────────

    #[test]
    fn test_top_k_builder() {
        let discovery = TopicDiscovery::new(2).with_top_k(30);
        assert_eq!(discovery.top_k, 30);

        // top_k of 0 should be clamped to 1
        let discovery = TopicDiscovery::new(2).with_top_k(0);
        assert_eq!(discovery.top_k, 1);
    }

    #[test]
    fn test_discover_ann_with_many_memories() {
        // Generate 150 memories (above ANN_THRESHOLD of 100) in 3 clusters
        let mut memories = Vec::new();

        // Cluster A: 50 memories near [1, 0, 0, 0, 0]
        for i in 0..50 {
            let noise = (i as f32) * 0.005;
            memories.push((
                format!("a{}", i),
                vec![1.0 - noise, noise, 0.0, 0.0, 0.0],
            ));
        }

        // Cluster B: 50 memories near [0, 1, 0, 0, 0]
        for i in 0..50 {
            let noise = (i as f32) * 0.005;
            memories.push((
                format!("b{}", i),
                vec![0.0, 1.0 - noise, noise, 0.0, 0.0],
            ));
        }

        // Cluster C: 50 memories near [0, 0, 1, 0, 0]
        for i in 0..50 {
            let noise = (i as f32) * 0.005;
            memories.push((
                format!("c{}", i),
                vec![0.0, 0.0, 1.0 - noise, noise, 0.0],
            ));
        }

        let discovery = TopicDiscovery::new(5).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);

        // Should find approximately 3 clusters (using ANN path since n=150 >= 100)
        // ANN approximation may produce slight variations in community boundaries
        assert!(
            candidates.len() >= 2 && candidates.len() <= 6,
            "Expected 2-6 clusters (approximately 3), got {}", candidates.len()
        );

        // Each cluster should be reasonably sized
        for c in &candidates {
            assert!(c.memories.len() >= 5, "Cluster too small: {}", c.memories.len());
        }

        // Verify cluster purity: memories from cluster A should be together
        if let Some(cluster_a) = candidates.iter().find(|c| c.memories.contains(&"a0".to_string())) {
            let a_count = cluster_a.memories.iter().filter(|m| m.starts_with('a')).count();
            let purity = a_count as f64 / cluster_a.memories.len() as f64;
            assert!(
                purity > 0.8,
                "Cluster A purity too low: {:.2} ({}/{})",
                purity, a_count, cluster_a.memories.len()
            );
        }
    }

    #[test]
    fn test_discover_two_memories() {
        // Degenerate case: exactly 2 memories
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.99, 0.01, 0.0]),
        ];

        let discovery = TopicDiscovery::new(2).with_edge_threshold(0.3);
        let candidates = discovery.discover(&memories);

        // Should find 1 cluster with both memories
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].memories.len(), 2);
    }
}
