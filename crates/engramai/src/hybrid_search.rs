//! Adaptive Hybrid Search — combines vector similarity with FTS for optimal retrieval.
//!
//! Uses both embedding-based semantic search and FTS5 keyword search,
//! combining scores with adaptive weights based on result overlap.
//!
//! When vector and FTS results agree (high Jaccard overlap), both signals
//! are strong. When they disagree, the system adapts weights.

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingProvider;
use crate::storage::Storage;
use crate::types::MemoryRecord;

/// Result from hybrid search with score breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResult {
    /// Memory ID
    pub id: String,
    /// Combined hybrid score (0.0-1.0)
    pub score: f64,
    /// Score from vector similarity (0.0-1.0)
    pub vector_score: f64,
    /// Score from FTS ranking (0.0-1.0)
    pub fts_score: f64,
    /// The memory record
    pub record: Option<MemoryRecord>,
}

/// Options for hybrid search.
#[derive(Debug, Clone)]
pub struct HybridSearchOpts {
    /// Weight for vector similarity (0.0-1.0)
    pub vector_weight: f64,
    /// Weight for FTS ranking (0.0-1.0)
    pub fts_weight: f64,
    /// Maximum results to return
    pub limit: usize,
    /// Namespace to search (None = "default", "*" = all)
    pub namespace: Option<String>,
    /// Include memory records in results
    pub include_records: bool,
}

impl Default for HybridSearchOpts {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            fts_weight: 0.3,
            limit: 10,
            namespace: None,
            include_records: true,
        }
    }
}

/// Perform hybrid search combining vector and FTS.
///
/// # Arguments
///
/// * `storage` - Storage backend
/// * `query_vector` - Query embedding vector (if available)
/// * `query_text` - Query text for FTS
/// * `opts` - Search options
/// * `model` - Embedding model identifier (e.g., "ollama/nomic-embed-text")
///
/// # Returns
///
/// Combined results sorted by hybrid score.
pub fn hybrid_search(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    opts: HybridSearchOpts,
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>> {
    let ns = opts.namespace.as_deref();
    let fetch_limit = opts.limit * 3; // Fetch more to combine
    
    // Get FTS results
    let fts_results = storage.search_fts_ns(query_text, fetch_limit, ns)?;
    let fts_count = fts_results.len();
    
    // Normalize FTS scores (rank-based, highest rank = highest score)
    let fts_scores: HashMap<String, f64> = fts_results
        .iter()
        .enumerate()
        .map(|(rank, record)| {
            // Inverse rank normalization: first result gets 1.0
            let score = 1.0 - (rank as f64 / fetch_limit.max(1) as f64);
            (record.id.clone(), score)
        })
        .collect();
    
    // Get vector results if query vector provided
    let vector_scores: HashMap<String, f64> = if let Some(qvec) = query_vector {
        let embeddings = storage.get_embeddings_in_namespace(ns, model)?;
        
        embeddings
            .iter()
            .map(|(id, emb)| {
                let sim = EmbeddingProvider::cosine_similarity(qvec, emb);
                // Normalize cosine similarity from [-1, 1] to [0, 1]
                let score = (sim + 1.0) / 2.0;
                (id.clone(), score as f64)
            })
            .collect()
    } else {
        HashMap::new()
    };
    
    // Combine all candidate IDs
    let all_ids: HashSet<String> = fts_scores.keys()
        .chain(vector_scores.keys())
        .cloned()
        .collect();
    
    // Calculate hybrid scores
    let mut results: Vec<HybridSearchResult> = all_ids
        .into_iter()
        .map(|id| {
            let vs = vector_scores.get(&id).copied().unwrap_or(0.0);
            let fs = fts_scores.get(&id).copied().unwrap_or(0.0);
            
            // Weighted combination
            let score = opts.vector_weight * vs + opts.fts_weight * fs;
            
            HybridSearchResult {
                id,
                score,
                vector_score: vs,
                fts_score: fs,
                record: None,
            }
        })
        .collect();
    
    // Sort by score descending
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    
    // Take top-k
    results.truncate(opts.limit);
    
    // Fetch records if requested
    if opts.include_records {
        for result in &mut results {
            result.record = storage.get(&result.id)?;
        }
    }
    
    log::debug!(
        "Hybrid search: {} FTS results, {} vector results, {} combined",
        fts_count,
        vector_scores.len(),
        results.len()
    );
    
    Ok(results)
}

/// Perform adaptive hybrid search with auto-tuned weights.
///
/// Automatically adjusts vector vs FTS weights based on the Jaccard overlap
/// of their result sets. High overlap → both signals are useful.
/// Low overlap → prefer the stronger signal.
///
/// # Arguments
///
/// * `storage` - Storage backend
/// * `query_vector` - Query embedding vector (if available)
/// * `query_text` - Query text for FTS
/// * `limit` - Maximum results to return
/// * `model` - Embedding model identifier
///
/// # Returns
///
/// Combined results with adaptively-weighted scores.
pub fn adaptive_hybrid_search(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    limit: usize,
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>> {
    let fetch_limit = limit * 3;
    
    // Get FTS results
    let fts_results = storage.search_fts_ns(query_text, fetch_limit, None)?;
    let fts_ids: HashSet<String> = fts_results.iter().map(|r| r.id.clone()).collect();
    
    // Get vector results
    let vector_scores: HashMap<String, f64> = if let Some(qvec) = query_vector {
        let embeddings = storage.get_embeddings_in_namespace(None, model)?;
        
        let mut scores: Vec<(String, f64)> = embeddings
            .iter()
            .map(|(id, emb)| {
                let sim = EmbeddingProvider::cosine_similarity(qvec, emb);
                let score = (sim + 1.0) / 2.0;
                (id.clone(), score as f64)
            })
            .collect();
        
        // Sort by score and take top-k
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(fetch_limit);
        
        scores.into_iter().collect()
    } else {
        HashMap::new()
    };
    
    let vector_ids: HashSet<String> = vector_scores.keys().cloned().collect();
    
    // Calculate Jaccard overlap
    let (vector_weight, fts_weight) = if vector_ids.is_empty() {
        // No vector results, use FTS only
        (0.0, 1.0)
    } else {
        let intersection = fts_ids.intersection(&vector_ids).count();
        let union = fts_ids.union(&vector_ids).count();
        
        let jaccard = if union > 0 {
            intersection as f64 / union as f64
        } else {
            0.0
        };
        
        // Adaptive weights based on overlap:
        // - High overlap (≥0.6): Both signals agree, use balanced weights
        // - Medium overlap (0.3-0.6): Slight preference for vector (semantic)
        // - Low overlap (<0.3): Strong preference for vector (FTS might be too literal)
        if jaccard >= 0.6 {
            (0.5, 0.5)
        } else if jaccard >= 0.3 {
            (0.6, 0.4)
        } else {
            (0.7, 0.3)
        }
    };
    
    log::debug!(
        "Adaptive weights: vector={:.2}, fts={:.2} (overlap={})",
        vector_weight,
        fts_weight,
        fts_ids.intersection(&vector_ids).count()
    );
    
    // Normalize FTS scores
    let fts_scores: HashMap<String, f64> = fts_results
        .iter()
        .enumerate()
        .map(|(rank, record)| {
            let score = 1.0 - (rank as f64 / fetch_limit.max(1) as f64);
            (record.id.clone(), score)
        })
        .collect();
    
    // Combine all IDs
    let all_ids: HashSet<String> = fts_scores.keys()
        .chain(vector_scores.keys())
        .cloned()
        .collect();
    
    // Calculate hybrid scores
    let mut results: Vec<HybridSearchResult> = all_ids
        .into_iter()
        .filter_map(|id| {
            let vs = vector_scores.get(&id).copied().unwrap_or(0.0);
            let fs = fts_scores.get(&id).copied().unwrap_or(0.0);
            
            // Skip if both scores are 0
            if vs == 0.0 && fs == 0.0 {
                return None;
            }
            
            let score = vector_weight * vs + fts_weight * fs;
            
            Some(HybridSearchResult {
                id,
                score,
                vector_score: vs,
                fts_score: fs,
                record: None,
            })
        })
        .collect();
    
    // Sort by score
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    
    // Fetch records
    for result in &mut results {
        result.record = storage.get(&result.id)?;
    }
    
    Ok(results)
}

/// Reciprocal Rank Fusion (RRF) for combining ranked lists.
///
/// An alternative to weighted linear combination that works well
/// when ranks are more meaningful than raw scores.
///
/// RRF(d) = Σ 1 / (k + rank_i(d))
/// where k is a constant (typically 60) and rank_i(d) is the rank of d in list i.
pub fn reciprocal_rank_fusion(
    storage: &Storage,
    query_vector: Option<&[f32]>,
    query_text: &str,
    limit: usize,
    k: f64, // RRF constant, typically 60
    model: &str,
) -> Result<Vec<HybridSearchResult>, Box<dyn std::error::Error>> {
    let fetch_limit = limit * 3;
    
    // Get FTS results
    let fts_results = storage.search_fts_ns(query_text, fetch_limit, None)?;
    let fts_ranks: HashMap<String, usize> = fts_results
        .iter()
        .enumerate()
        .map(|(rank, r)| (r.id.clone(), rank + 1)) // 1-indexed
        .collect();
    
    // Get vector results
    let vector_ranks: HashMap<String, usize> = if let Some(qvec) = query_vector {
        let embeddings = storage.get_embeddings_in_namespace(None, model)?;
        
        let mut scored: Vec<(String, f64)> = embeddings
            .iter()
            .map(|(id, emb)| {
                let sim = EmbeddingProvider::cosine_similarity(qvec, emb);
                (id.clone(), sim as f64)
            })
            .collect();
        
        // Sort by similarity descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        scored
            .into_iter()
            .enumerate()
            .map(|(rank, (id, _))| (id, rank + 1)) // 1-indexed
            .collect()
    } else {
        HashMap::new()
    };
    
    // Combine all IDs
    let all_ids: HashSet<String> = fts_ranks.keys()
        .chain(vector_ranks.keys())
        .cloned()
        .collect();
    
    // Calculate RRF scores
    let mut results: Vec<HybridSearchResult> = all_ids
        .into_iter()
        .map(|id| {
            let fts_contribution = fts_ranks.get(&id)
                .map(|&rank| 1.0 / (k + rank as f64))
                .unwrap_or(0.0);
            
            let vector_contribution = vector_ranks.get(&id)
                .map(|&rank| 1.0 / (k + rank as f64))
                .unwrap_or(0.0);
            
            let rrf_score = fts_contribution + vector_contribution;
            
            // Normalize FTS/vector scores for output
            let fts_score = fts_ranks.get(&id)
                .map(|&rank| 1.0 - (rank as f64 / fetch_limit as f64))
                .unwrap_or(0.0);
            
            let vector_score = vector_ranks.get(&id)
                .map(|&rank| 1.0 - (rank as f64 / fetch_limit as f64))
                .unwrap_or(0.0);
            
            HybridSearchResult {
                id,
                score: rrf_score,
                vector_score,
                fts_score,
                record: None,
            }
        })
        .collect();
    
    // Sort by RRF score
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    
    // Fetch records
    for result in &mut results {
        result.record = storage.get(&result.id)?;
    }
    
    Ok(results)
}

/// Calculate Jaccard similarity between two sets of IDs.
pub fn jaccard_similarity(set_a: &HashSet<String>, set_b: &HashSet<String>) -> f64 {
    if set_a.is_empty() && set_b.is_empty() {
        return 1.0; // Both empty = identical
    }
    
    let intersection = set_a.intersection(set_b).count();
    let union = set_a.union(set_b).count();
    
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_jaccard_similarity() {
        let a: HashSet<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["2", "3", "4"].iter().map(|s| s.to_string()).collect();
        
        let sim = jaccard_similarity(&a, &b);
        // Intersection: {2, 3} = 2, Union: {1, 2, 3, 4} = 4
        assert!((sim - 0.5).abs() < 0.01);
    }
    
    #[test]
    fn test_jaccard_identical() {
        let a: HashSet<String> = ["1", "2", "3"].iter().map(|s| s.to_string()).collect();
        let b = a.clone();
        
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.01);
    }
    
    #[test]
    fn test_jaccard_disjoint() {
        let a: HashSet<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["3", "4"].iter().map(|s| s.to_string()).collect();
        
        let sim = jaccard_similarity(&a, &b);
        assert!(sim.abs() < 0.01);
    }
    
    #[test]
    fn test_jaccard_empty() {
        let a: HashSet<String> = HashSet::new();
        let b: HashSet<String> = HashSet::new();
        
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.01);
    }
    
    #[test]
    fn test_hybrid_search_opts_default() {
        let opts = HybridSearchOpts::default();
        
        assert!((opts.vector_weight - 0.7).abs() < 0.01);
        assert!((opts.fts_weight - 0.3).abs() < 0.01);
        assert_eq!(opts.limit, 10);
        assert!(opts.include_records);
    }
}
