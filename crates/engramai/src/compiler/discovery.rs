//! Topic Discovery — discovers topic candidates from memory clusters.
//!
//! Uses the unified [`InfomapClusterer`](crate::clustering::InfomapClusterer) with
//! [`EmbeddingOnly`](crate::clustering::EmbeddingOnly) strategy for edge weights.
//!
//! This module is a thin adapter: it converts compiler-specific types (`TopicCandidate`,
//! `TopicPage`) to/from the shared clustering types, and adds compiler-specific logic
//! (overlap detection, LLM labeling).

use std::collections::HashSet;

use crate::clustering::{ClusterNode, ClustererConfig, EmbeddingOnly, InfomapClusterer};

use super::llm::LlmProvider;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  CONSTANTS
// ═══════════════════════════════════════════════════════════════════════════════

/// Number of nearest neighbors per memory node when building the sparse graph.
const KNN_K: usize = 15;

/// Minimum edge weight (cosine similarity) to include in the graph.
const MIN_EDGE_WEIGHT: f64 = 0.1;

// ═══════════════════════════════════════════════════════════════════════════════
//  TOPIC DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════════

/// Discovers topic candidates from a set of memories using Infomap community
/// detection on a sparse k-nearest-neighbor graph.
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

    /// Discover topic candidates from memories using the unified Infomap clusterer.
    ///
    /// This is a thin wrapper around [`InfomapClusterer<EmbeddingOnly>`] that converts
    /// the output into `TopicCandidate` structs.
    pub fn discover(
        &self,
        memories: &[(String, Vec<f32>)], // (memory_id, embedding)
    ) -> Vec<TopicCandidate> {
        if memories.is_empty() {
            return Vec::new();
        }

        // Convert to ClusterNodes (embedding-only, other fields empty).
        let nodes: Vec<ClusterNode> = memories
            .iter()
            .map(|(id, embedding)| ClusterNode {
                id: id.clone(),
                embedding: embedding.clone(),
                hebbian_links: Vec::new(),
                entity_ids: Vec::new(),
                created_at_secs: 0.0,
            })
            .collect();

        // Run the unified clusterer.
        let config = ClustererConfig {
            k_neighbors: KNN_K,
            min_edge_weight: MIN_EDGE_WEIGHT,
            min_cluster_size: self.min_cluster_size,
            num_trials: 5,
            seed: 42,
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);
        let clusters = clusterer.cluster(&nodes);

        // Convert Cluster → TopicCandidate.
        clusters
            .into_iter()
            .map(|c| {
                let memory_ids: Vec<String> =
                    c.member_indices.iter().map(|&i| memories[i].0.clone()).collect();

                TopicCandidate {
                    memories: memory_ids,
                    centroid_embedding: c.centroid,
                    cohesion_score: c.cohesion,
                    suggested_title: None,
                }
            })
            .collect()
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
            Err(_) => Ok(Self::fallback_label(memory_contents, candidate)),
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
        // 6 memories forming 2 clusters in 10D space.
        // Cluster A: m1, m2, m3 — variations around [1,1,1,1,1, 0,0,0,0,0]
        // Cluster B: m4, m5, m6 — variations around [0,0,0,0,0, 1,1,1,1,1]
        let memories = vec![
            ("m1".to_string(), vec![1.0, 0.9, 1.0, 0.9, 1.0, 0.0, 0.1, 0.0, 0.1, 0.0]),
            ("m2".to_string(), vec![0.9, 1.0, 0.9, 1.0, 0.9, 0.1, 0.0, 0.1, 0.0, 0.1]),
            ("m3".to_string(), vec![1.0, 1.0, 0.9, 0.9, 1.0, 0.0, 0.0, 0.1, 0.1, 0.0]),
            ("m4".to_string(), vec![0.0, 0.1, 0.0, 0.1, 0.0, 1.0, 0.9, 1.0, 0.9, 1.0]),
            ("m5".to_string(), vec![0.1, 0.0, 0.1, 0.0, 0.1, 0.9, 1.0, 0.9, 1.0, 0.9]),
            ("m6".to_string(), vec![0.0, 0.0, 0.1, 0.1, 0.0, 1.0, 1.0, 0.9, 0.9, 1.0]),
        ];

        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&memories);

        assert_eq!(candidates.len(), 2, "expected 2 topic candidates, got {}", candidates.len());

        // Check that each cluster has 3 members.
        for c in &candidates {
            assert_eq!(c.memories.len(), 3, "each cluster should have 3 members");
        }

        // All 6 memory IDs should appear exactly once across both clusters.
        let mut all_ids: Vec<&str> = candidates.iter()
            .flat_map(|c| c.memories.iter().map(|s| s.as_str()))
            .collect();
        all_ids.sort();
        assert_eq!(all_ids, vec!["m1", "m2", "m3", "m4", "m5", "m6"]);
    }

    #[test]
    fn test_discover_empty() {
        let discovery = TopicDiscovery::new(2);
        let candidates = discovery.discover(&[]);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_discover_single() {
        let discovery = TopicDiscovery::new(2);
        let memories = vec![("m1".to_string(), vec![1.0, 0.0])];
        let candidates = discovery.discover(&memories);
        assert!(candidates.is_empty(), "single memory can't form a cluster");
    }

    #[test]
    fn test_discover_respects_min_cluster_size() {
        let discovery = TopicDiscovery::new(3);
        let memories = vec![
            ("m1".to_string(), vec![1.0, 0.0]),
            ("m2".to_string(), vec![0.9, 0.1]),
        ];
        let candidates = discovery.discover(&memories);
        // With min_cluster_size=3, 2 memories can't form a cluster.
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_discover_cohesion_populated() {
        let discovery = TopicDiscovery::new(2);
        let memories = vec![
            ("m1".to_string(), vec![1.0, 0.0, 0.0, 0.0, 0.0]),
            ("m2".to_string(), vec![0.95, 0.05, 0.0, 0.0, 0.0]),
            ("m3".to_string(), vec![0.9, 0.1, 0.0, 0.0, 0.0]),
        ];
        let candidates = discovery.discover(&memories);
        assert!(!candidates.is_empty());
        assert!(candidates[0].cohesion_score > 0.0, "cohesion should be populated");
    }

    #[test]
    fn test_discover_centroid_populated() {
        let discovery = TopicDiscovery::new(2);
        let memories = vec![
            ("m1".to_string(), vec![1.0, 0.0]),
            ("m2".to_string(), vec![0.0, 1.0]),
        ];
        let candidates = discovery.discover(&memories);
        if !candidates.is_empty() {
            let c = &candidates[0];
            assert!(!c.centroid_embedding.is_empty(), "centroid should be populated");
        }
    }

    #[test]
    fn test_label_cluster_success() {
        let discovery = TopicDiscovery::new(2);
        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![0.5, 0.5],
            cohesion_score: 0.8,
            suggested_title: None,
        };
        let contents = vec![
            ("m1".to_string(), "Rust is a systems language".to_string()),
            ("m2".to_string(), "Rust has ownership semantics".to_string()),
        ];
        let llm = MockLlmProvider::success("Rust Programming");

        let label = discovery.label_cluster(&candidate, &contents, &llm).unwrap();
        assert_eq!(label, "Rust Programming");
    }

    #[test]
    fn test_label_cluster_fallback() {
        let discovery = TopicDiscovery::new(2);
        let candidate = TopicCandidate {
            memories: vec!["m1".to_string()],
            centroid_embedding: vec![],
            cohesion_score: 0.5,
            suggested_title: None,
        };
        let contents = vec![
            ("m1".to_string(), "The quick brown fox jumps over the lazy dog".to_string()),
        ];
        let llm = MockLlmProvider::failure();

        let label = discovery.label_cluster(&candidate, &contents, &llm).unwrap();
        assert_eq!(label, "The quick brown fox jumps");
    }

    #[test]
    fn test_overlap_detection() {
        let discovery = TopicDiscovery::new(2);

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            centroid_embedding: vec![],
            cohesion_score: 0.5,
            suggested_title: None,
        };

        // Existing page with significant overlap
        let existing = vec![make_topic_page("t1", vec!["m1", "m2", "m4"])];

        let result = discovery.detect_overlap(&candidate, &existing);
        // Jaccard: intersection={m1,m2}=2, union={m1,m2,m3,m4}=4, 2/4=0.5 > 0.3
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "t1");
    }

    #[test]
    fn test_no_overlap_detection() {
        let discovery = TopicDiscovery::new(2);

        let candidate = TopicCandidate {
            memories: vec!["m1".to_string(), "m2".to_string()],
            centroid_embedding: vec![],
            cohesion_score: 0.5,
            suggested_title: None,
        };

        // Existing page with no overlap
        let existing = vec![make_topic_page("t1", vec!["m5", "m6", "m7", "m8"])];

        let result = discovery.detect_overlap(&candidate, &existing);
        assert!(result.is_none());
    }
}
