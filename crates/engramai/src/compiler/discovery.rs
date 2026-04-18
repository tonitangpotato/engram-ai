//! Topic Discovery — discovers topic candidates from memory clusters.
//!
//! Uses embeddings-based agglomerative clustering (single-linkage) to find
//! groups of related memories, and optional LLM labeling to suggest topic titles.

use std::collections::HashSet;

use crate::embeddings::EmbeddingProvider;
use super::llm::LlmProvider;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using embedding-based
/// agglomerative clustering.
pub struct TopicDiscovery {
    /// Minimum number of memories required to form a valid cluster.
    min_cluster_size: usize,
    /// Jaccard similarity threshold for overlap detection with existing topics.
    overlap_threshold: f64,
}

impl TopicDiscovery {
    /// Create a new `TopicDiscovery` with the given minimum cluster size.
    ///
    /// The overlap threshold defaults to 0.3 (matching `TopicCandidate::overlaps_with`).
    pub fn new(min_cluster_size: usize) -> Self {
        Self {
            min_cluster_size,
            overlap_threshold: 0.3,
        }
    }

    /// Discover topic candidates from memories using embedding-based clustering.
    ///
    /// # Algorithm
    ///
    /// 1. Compute pairwise cosine similarity between all memory embeddings
    /// 2. Build clusters using simple agglomerative clustering (single-linkage)
    /// 3. Filter clusters below `min_cluster_size`
    /// 4. For each cluster, create a `TopicCandidate` with:
    ///    - `memories`: list of memory IDs in the cluster
    ///    - `centroid_embedding`: mean of cluster member embeddings
    ///    - `cohesion_score`: average intra-cluster similarity
    ///    - `suggested_title`: `None` (can be filled by `label_cluster`)
    pub fn discover(
        &self,
        memories: &[(String, Vec<f32>)], // (memory_id, embedding)
    ) -> Vec<TopicCandidate> {
        if memories.is_empty() {
            return Vec::new();
        }

        let n = memories.len();

        // Step 1: Compute pairwise cosine similarity matrix (upper triangular).
        let mut sim_matrix = vec![vec![0.0f64; n]; n];
        for i in 0..n {
            sim_matrix[i][i] = 1.0;
            for j in (i + 1)..n {
                let s = EmbeddingProvider::cosine_similarity(&memories[i].1, &memories[j].1) as f64;
                sim_matrix[i][j] = s;
                sim_matrix[j][i] = s;
            }
        }

        // Step 2: Agglomerative single-linkage clustering.
        // Each memory starts as its own cluster.
        let similarity_threshold = 0.5;
        let mut cluster_ids: Vec<usize> = (0..n).collect();
        let mut next_merge = true;

        while next_merge {
            next_merge = false;
            let mut best_sim = f64::NEG_INFINITY;
            let mut best_pair: (usize, usize) = (0, 0);

            // Find the pair of distinct clusters with the highest max-link similarity.
            // Collect unique cluster IDs.
            let unique_clusters: Vec<usize> = {
                let mut s: Vec<usize> = cluster_ids.clone();
                s.sort_unstable();
                s.dedup();
                s
            };

            if unique_clusters.len() < 2 {
                break;
            }

            for ci_idx in 0..unique_clusters.len() {
                for cj_idx in (ci_idx + 1)..unique_clusters.len() {
                    let ci = unique_clusters[ci_idx];
                    let cj = unique_clusters[cj_idx];

                    // Single-linkage: max similarity between any member of ci and any member of cj.
                    let mut max_link = f64::NEG_INFINITY;
                    for (i, &cid_i) in cluster_ids.iter().enumerate() {
                        if cid_i != ci {
                            continue;
                        }
                        for (j, &cid_j) in cluster_ids.iter().enumerate() {
                            if cid_j != cj {
                                continue;
                            }
                            if sim_matrix[i][j] > max_link {
                                max_link = sim_matrix[i][j];
                            }
                        }
                    }

                    if max_link > best_sim {
                        best_sim = max_link;
                        best_pair = (ci, cj);
                    }
                }
            }

            // Merge if above threshold.
            if best_sim > similarity_threshold {
                let (merge_from, merge_to) = (best_pair.1, best_pair.0);
                for cid in cluster_ids.iter_mut() {
                    if *cid == merge_from {
                        *cid = merge_to;
                    }
                }
                next_merge = true;
            }
        }

        // Step 3: Collect clusters and filter by min_cluster_size.
        let unique_clusters: HashSet<usize> = cluster_ids.iter().copied().collect();
        let mut candidates = Vec::new();

        for cluster_id in unique_clusters {
            let member_indices: Vec<usize> = cluster_ids
                .iter()
                .enumerate()
                .filter(|(_, &cid)| cid == cluster_id)
                .map(|(i, _)| i)
                .collect();

            if member_indices.len() < self.min_cluster_size {
                continue;
            }

            // Step 4: Build TopicCandidate.
            let memory_ids: Vec<String> = member_indices
                .iter()
                .map(|&i| memories[i].0.clone())
                .collect();

            // Centroid: mean of embeddings.
            let dim = memories[0].1.len();
            let mut centroid = vec![0.0f32; dim];
            for &idx in &member_indices {
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

            // Cohesion: average intra-cluster pairwise similarity.
            let mut cohesion_sum = 0.0;
            let mut pair_count = 0usize;
            for (pi, &i) in member_indices.iter().enumerate() {
                for &j in &member_indices[(pi + 1)..] {
                    cohesion_sum += sim_matrix[i][j];
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

        // Sort candidates by cohesion descending for deterministic output.
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
    fn test_discover_basic() {
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

        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&memories);

        assert_eq!(candidates.len(), 2, "expected 2 clusters, got {}", candidates.len());

        // Each cluster should have 3 members.
        let mut sizes: Vec<usize> = candidates.iter().map(|c| c.memories.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![3, 3]);

        // Verify that clusters are internally consistent:
        // m1, m2, m3 should be in the same cluster; m4, m5, m6 in another.
        let cluster_a_ids: HashSet<&str> = candidates[0].memories.iter().map(|s| s.as_str()).collect();
        let cluster_b_ids: HashSet<&str> = candidates[1].memories.iter().map(|s| s.as_str()).collect();

        let group_a: HashSet<&str> = ["m1", "m2", "m3"].into_iter().collect();
        let group_b: HashSet<&str> = ["m4", "m5", "m6"].into_iter().collect();

        assert!(
            (cluster_a_ids == group_a && cluster_b_ids == group_b)
                || (cluster_a_ids == group_b && cluster_b_ids == group_a),
            "clusters should separate the two groups"
        );

        // Cohesion scores should be positive.
        for c in &candidates {
            assert!(c.cohesion_score > 0.0, "cohesion should be > 0");
        }

        // Suggested titles should be None.
        for c in &candidates {
            assert!(c.suggested_title.is_none());
        }
    }

    #[test]
    fn test_discover_min_cluster_size() {
        // 3 memories: 2 similar, 1 outlier. min_cluster_size = 2.
        // The outlier should be filtered out.
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.0, 0.0]),
            ("m2".to_string(), vec![0.95, 0.1, 0.0]),
            ("m3".to_string(), vec![0.0, 0.0, 1.0]), // outlier
        ];

        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&memories);

        assert_eq!(candidates.len(), 1, "expected 1 cluster (outlier filtered)");
        assert_eq!(candidates[0].memories.len(), 2);

        let ids: HashSet<&str> = candidates[0].memories.iter().map(|s| s.as_str()).collect();
        assert!(ids.contains("m1"));
        assert!(ids.contains("m2"));
        assert!(!ids.contains("m3"));
    }

    #[test]
    fn test_discover_empty() {
        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&[]);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_discover_single_cluster() {
        // All memories are very similar → 1 big cluster.
        let memories = vec![
            ("m1".to_string(), vec![1.0f32, 0.1, 0.0]),
            ("m2".to_string(), vec![0.95, 0.15, 0.0]),
            ("m3".to_string(), vec![0.9, 0.2, 0.0]),
            ("m4".to_string(), vec![0.85, 0.25, 0.05]),
        ];

        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&memories);

        assert_eq!(candidates.len(), 1, "expected 1 single cluster");
        assert_eq!(candidates[0].memories.len(), 4);
    }

    #[test]
    fn test_detect_overlap() {
        let discovery = TopicDiscovery::new(2);

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            centroid_embedding: vec![1.0, 0.0, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        // Existing topic with significant overlap (m1, m2 shared out of 4 union).
        let existing = vec![make_topic_page("t1", vec!["m1", "m2", "m4"])];

        // Jaccard = |{m1,m2}| / |{m1,m2,m3,m4}| = 2/4 = 0.5 > 0.3
        let result = discovery.detect_overlap(&candidate, &existing);
        assert!(result.is_some(), "expected overlap with t1");
        assert_eq!(result.unwrap().0, "t1");
    }

    #[test]
    fn test_detect_no_overlap() {
        let discovery = TopicDiscovery::new(2);

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            centroid_embedding: vec![1.0, 0.0, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        // Existing topic with no overlap at all.
        let existing = vec![make_topic_page("t1", vec!["m10", "m11", "m12"])];

        // Jaccard = 0/6 = 0.0, well below threshold.
        let result = discovery.detect_overlap(&candidate, &existing);
        assert!(result.is_none(), "expected no overlap");
    }

    #[test]
    fn test_label_cluster_success() {
        let discovery = TopicDiscovery::new(2);
        let llm = MockLlmProvider::success("Rust Programming Patterns");

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![1.0, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        let contents = vec![
            ("m1".to_string(), "Rust ownership and borrowing rules".to_string()),
            ("m2".to_string(), "Pattern matching in Rust with enums".to_string()),
        ];

        let label = discovery.label_cluster(&candidate, &contents, &llm).unwrap();
        assert_eq!(label, "Rust Programming Patterns");
    }

    #[test]
    fn test_label_cluster_fallback() {
        let discovery = TopicDiscovery::new(2);
        let llm = MockLlmProvider::failure();

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![1.0, 0.0],
            cohesion_score: 0.9,
            suggested_title: None,
        };

        let contents = vec![
            ("m1".to_string(), "Short note".to_string()),
            (
                "m2".to_string(),
                "This is a much longer memory content that has many words in it".to_string(),
            ),
        ];

        let label = discovery.label_cluster(&candidate, &contents, &llm).unwrap();
        // Should be first 5 words of longest memory.
        assert_eq!(label, "This is a much longer");
    }
}
