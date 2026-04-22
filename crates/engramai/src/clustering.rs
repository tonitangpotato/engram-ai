//! Unified Infomap clustering engine with pluggable edge-weight strategies.
//!
//! Both `compiler::discovery` (topic discovery, cosine-only edges) and
//! `synthesis::cluster` (4-signal weighted edges) use Infomap for community
//! detection. This module provides the shared engine and the
//! [`EdgeWeightStrategy`] trait so callers can plug in different edge-weight
//! computations without duplicating the Infomap orchestration logic.

use std::collections::HashMap;

use infomap_rs::{Infomap, Network};

// ═══════════════════════════════════════════════════════════════════════════════
//  EDGE WEIGHT STRATEGY
// ═══════════════════════════════════════════════════════════════════════════════

/// Strategy for computing the edge weight between two items.
///
/// Implementations decide *what* similarity signals to combine and *how* to
/// weight them. The clustering engine only cares about the final `f64` weight
/// per pair.
pub trait EdgeWeightStrategy {
    /// Type of item being clustered (e.g., memory ID, embedding, etc.).
    type Item;

    /// Compute the edge weight between two items.
    ///
    /// Returns `None` if the pair should not be connected (below threshold).
    fn edge_weight(&self, a: &Self::Item, b: &Self::Item) -> Option<f64>;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CLUSTERING ENGINE
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for the Infomap clustering engine.
#[derive(Debug, Clone)]
pub struct ClusteringConfig {
    /// Minimum community size to keep. Communities smaller than this are dropped.
    pub min_community_size: usize,
    /// Maximum community size. Communities larger than this are truncated.
    /// A future improvement would recursively sub-cluster.
    pub max_community_size: usize,
    /// Random seed for Infomap (default: 42).
    pub seed: u64,
}

impl Default for ClusteringConfig {
    fn default() -> Self {
        Self {
            min_community_size: 2,
            max_community_size: usize::MAX,
            seed: 42,
        }
    }
}

/// Result of clustering: a community of items.
#[derive(Debug, Clone)]
pub struct Community<T> {
    /// Items in this community.
    pub members: Vec<T>,
    /// Infomap module ID.
    pub module_id: usize,
}

/// Run Infomap community detection on a set of items using the given
/// edge-weight strategy.
///
/// 1. Computes all pairwise edge weights via `strategy.edge_weight()`
/// 2. Builds a weighted directed graph (undirected edges added both ways)
/// 3. Runs Infomap to find communities
/// 4. Filters by `config.min_community_size` / `max_community_size`
///
/// Returns communities as `Vec<Community<T>>` where `T` is cloned from
/// the input items.
pub fn cluster_with_infomap<T, S>(
    items: &[T],
    strategy: &S,
    config: &ClusteringConfig,
) -> Vec<Community<T>>
where
    T: Clone,
    S: EdgeWeightStrategy<Item = T>,
{
    let n = items.len();
    if n < 2 {
        return Vec::new();
    }

    // Build the Infomap network.
    let mut network = Network::with_capacity(n);
    network.ensure_capacity(n);
    let mut has_edges = false;

    for i in 0..n {
        for j in (i + 1)..n {
            if let Some(weight) = strategy.edge_weight(&items[i], &items[j]) {
                network.add_edge(i, j, weight);
                network.add_edge(j, i, weight);
                has_edges = true;
            }
        }
    }

    if !has_edges {
        return Vec::new();
    }

    // Run Infomap.
    let result = Infomap::new(&network).seed(config.seed).run();

    // Group by module.
    let mut modules: HashMap<usize, Vec<usize>> = HashMap::new();
    for (node_idx, &module_id) in result.assignments.iter().enumerate() {
        if node_idx < n {
            modules.entry(module_id).or_default().push(node_idx);
        }
    }

    // Build output communities.
    let mut communities = Vec::new();
    for (module_id, indices) in modules {
        if indices.len() < config.min_community_size {
            continue;
        }

        let mut members: Vec<T> = indices.iter().map(|&i| items[i].clone()).collect();
        if members.len() > config.max_community_size {
            members.truncate(config.max_community_size);
        }

        communities.push(Community {
            members,
            module_id,
        });
    }

    communities
}

// ═══════════════════════════════════════════════════════════════════════════════
//  BUILT-IN STRATEGIES
// ═══════════════════════════════════════════════════════════════════════════════

/// Cosine-similarity edge weight strategy.
///
/// Creates an edge when cosine similarity between two embedding vectors
/// meets or exceeds the threshold.
pub struct CosineStrategy {
    /// Minimum cosine similarity to create an edge.
    pub threshold: f64,
}

impl CosineStrategy {
    /// Create a new cosine strategy with the given threshold.
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

/// An item with an ID and embedding vector, used by `CosineStrategy`.
#[derive(Debug, Clone)]
pub struct EmbeddingItem {
    /// Unique identifier.
    pub id: String,
    /// Embedding vector.
    pub embedding: Vec<f32>,
}

impl EdgeWeightStrategy for CosineStrategy {
    type Item = EmbeddingItem;

    fn edge_weight(&self, a: &EmbeddingItem, b: &EmbeddingItem) -> Option<f64> {
        let sim = crate::embeddings::EmbeddingProvider::cosine_similarity(
            &a.embedding,
            &b.embedding,
        ) as f64;
        if sim >= self.threshold {
            Some(sim)
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_strategy_basic() {
        let items = vec![
            EmbeddingItem { id: "a".into(), embedding: vec![1.0, 0.0, 0.0] },
            EmbeddingItem { id: "b".into(), embedding: vec![0.95, 0.1, 0.0] },
            EmbeddingItem { id: "c".into(), embedding: vec![0.0, 1.0, 0.0] },
            EmbeddingItem { id: "d".into(), embedding: vec![0.1, 0.95, 0.0] },
        ];

        let strategy = CosineStrategy::new(0.3);
        let config = ClusteringConfig {
            min_community_size: 2,
            ..Default::default()
        };

        let communities = cluster_with_infomap(&items, &strategy, &config);
        assert_eq!(communities.len(), 2, "Expected 2 communities, got {}", communities.len());
    }

    #[test]
    fn test_empty_input() {
        let items: Vec<EmbeddingItem> = vec![];
        let strategy = CosineStrategy::new(0.3);
        let config = ClusteringConfig::default();
        let communities = cluster_with_infomap(&items, &strategy, &config);
        assert!(communities.is_empty());
    }

    #[test]
    fn test_no_edges_above_threshold() {
        let items = vec![
            EmbeddingItem { id: "a".into(), embedding: vec![1.0, 0.0, 0.0] },
            EmbeddingItem { id: "b".into(), embedding: vec![0.0, 1.0, 0.0] },
        ];

        // Threshold too high for these orthogonal vectors.
        let strategy = CosineStrategy::new(0.9);
        let config = ClusteringConfig::default();
        let communities = cluster_with_infomap(&items, &strategy, &config);
        assert!(communities.is_empty());
    }

    #[test]
    fn test_custom_strategy() {
        // Strategy that always connects pairs with weight 1.0
        struct AlwaysConnect;
        impl EdgeWeightStrategy for AlwaysConnect {
            type Item = String;
            fn edge_weight(&self, _a: &String, _b: &String) -> Option<f64> {
                Some(1.0)
            }
        }

        let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let config = ClusteringConfig {
            min_community_size: 2,
            ..Default::default()
        };

        let communities = cluster_with_infomap(&items, &AlwaysConnect, &config);
        assert_eq!(communities.len(), 1);
        assert_eq!(communities[0].members.len(), 3);
    }

    #[test]
    fn test_min_community_size_filter() {
        let items = vec![
            EmbeddingItem { id: "a".into(), embedding: vec![1.0, 0.0] },
            EmbeddingItem { id: "b".into(), embedding: vec![0.99, 0.01] },
        ];

        let strategy = CosineStrategy::new(0.3);
        let config = ClusteringConfig {
            min_community_size: 5, // too high
            ..Default::default()
        };

        let communities = cluster_with_infomap(&items, &strategy, &config);
        assert!(communities.is_empty());
    }
}
