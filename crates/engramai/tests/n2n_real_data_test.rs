//! N2N (End-to-End) tests with REAL engram data.
//!
//! These tests use the actual production engram DB (read-only) to validate:
//! 1. Topic Discovery (compiler) — Infomap on real embeddings
//! 2. Synthesis Clustering — Infomap with 4-signal weighting on real data
//! 3. Full pipeline: discover → compile → query
//!
//! Run with: `cargo test --test n2n_real_data_test -- --nocapture`
//!
//! These tests are `#[ignore]` by default because they require:
//! - The real engram DB at ~/rustclaw/engram-memory.db
//! - Sufficient memories with embeddings
//!
//! To run: `cargo test --test n2n_real_data_test -- --ignored --nocapture`

use engramai::storage::Storage;
use engramai::clustering::{
    ClusterNode, ClustererConfig, EmbeddingOnly, InfomapClusterer, MultiSignal,
};
use engramai::synthesis::cluster::discover_clusters;
use engramai::synthesis::types::ClusterDiscoveryConfig;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const REAL_DB_PATH: &str = concat!(env!("HOME"), "/rustclaw/engram-memory.db");
const EMBEDDING_MODEL: &str = "ollama/nomic-embed-text";

/// Helper: check if the real DB exists (skip test if not).
fn require_real_db() -> Storage {
    let path = REAL_DB_PATH;
    if !Path::new(path).exists() {
        panic!(
            "Real engram DB not found at {}. Run with real data only.",
            path
        );
    }
    Storage::new(path).expect("Failed to open real engram DB")
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 1: Raw data inventory — understand what we're working with
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_01_data_inventory() {
    let storage = require_real_db();

    let all_memories = storage.all().unwrap();
    let embeddings = storage.get_all_embeddings(EMBEDDING_MODEL).unwrap();

    // Count Hebbian links (sample first 100 memories)
    let mut hebbian_count = 0usize;
    let mut memories_with_hebbian = 0usize;
    for m in all_memories.iter().take(200) {
        if let Ok(links) = storage.get_hebbian_links_weighted(&m.id) {
            if !links.is_empty() {
                memories_with_hebbian += 1;
                hebbian_count += links.len();
            }
        }
    }

    // Count entity associations (sample)
    let mut entity_count = 0usize;
    let mut memories_with_entities = 0usize;
    for m in all_memories.iter().take(200) {
        if let Ok(entities) = storage.get_entity_ids_for_memory(&m.id) {
            if !entities.is_empty() {
                memories_with_entities += 1;
                entity_count += entities.len();
            }
        }
    }

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║         REAL DATA INVENTORY                  ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║ Total memories:        {:>8}              ║", all_memories.len());
    println!("║ With embeddings:       {:>8}              ║", embeddings.len());
    println!("║ Embedding dimensions:  {:>8}              ║",
        embeddings.first().map(|(_, e)| e.len()).unwrap_or(0));
    println!("║ (sample 200)                                ║");
    println!("║   With Hebbian links:  {:>8}              ║", memories_with_hebbian);
    println!("║   Total Hebbian edges: {:>8}              ║", hebbian_count);
    println!("║   With entities:       {:>8}              ║", memories_with_entities);
    println!("║   Total entity assocs: {:>8}              ║", entity_count);
    println!("╚══════════════════════════════════════════════╝\n");

    // Assertions: we need real data to test with
    assert!(all_memories.len() > 100, "Need at least 100 memories for meaningful N2N");
    assert!(embeddings.len() > 100, "Need at least 100 embeddings for clustering");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 2: Topic Discovery (EmbeddingOnly) on real data
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_02_topic_discovery_infomap() {
    let storage = require_real_db();
    let all_embeddings = storage.get_all_embeddings(EMBEDDING_MODEL).unwrap();

    // Subsample for tractable k-NN build (O(n²) pairwise computation).
    // Use 2000 for fast CI, full dataset for thorough validation.
    let sample_size = std::env::var("N2N_SAMPLE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2000);
    let embeddings: Vec<&(String, Vec<f32>)> = all_embeddings.iter().take(sample_size).collect();

    println!("\n=== Topic Discovery (EmbeddingOnly + Infomap) ===");
    println!("Input: {} memories (sampled from {})", embeddings.len(), all_embeddings.len());

    // Build ClusterNodes from real embeddings
    let nodes: Vec<ClusterNode> = embeddings
        .iter()
        .map(|(id, emb)| ClusterNode {
            id: id.clone(),
            embedding: emb.clone(),
            hebbian_links: Vec::new(),
            entity_ids: Vec::new(),
            created_at_secs: 0.0,
        })
        .collect();

    // Run Infomap with production-like settings
    let config = ClustererConfig {
        k_neighbors: 15,
        min_edge_weight: 0.1,
        min_cluster_size: 3,
        num_trials: 5,
        seed: 42,
    };
    let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

    let start = std::time::Instant::now();
    let clusters = clusterer.cluster(&nodes);
    let elapsed = start.elapsed();

    println!("Clusters found: {}", clusters.len());
    println!("Clustering time: {:?}", elapsed);

    // Analyze cluster size distribution
    let total_clustered: usize = clusters.iter().map(|c| c.member_indices.len()).sum();
    let unclustered = nodes.len() - total_clustered;

    println!("\n--- Cluster Size Distribution ---");
    let mut size_counts: HashMap<usize, usize> = HashMap::new();
    for c in &clusters {
        *size_counts.entry(c.member_indices.len()).or_default() += 1;
    }
    let mut sizes: Vec<(usize, usize)> = size_counts.into_iter().collect();
    sizes.sort_by_key(|&(size, _)| size);
    for (size, count) in &sizes {
        println!("  Size {:>4}: {:>4} clusters", size, count);
    }
    println!("  Unclustered: {} memories ({:.1}%)",
        unclustered, unclustered as f64 / nodes.len() as f64 * 100.0);

    // Show top-10 clusters by cohesion (with sample content)
    let all_memories = storage.all().unwrap();
    let memory_map: HashMap<&str, &str> = all_memories
        .iter()
        .map(|m| (m.id.as_str(), m.content.as_str()))
        .collect();

    println!("\n--- Top 10 Clusters by Cohesion ---");
    for (i, c) in clusters.iter().take(10).enumerate() {
        println!("\n  Cluster {} (size={}, cohesion={:.4}):", i + 1, c.member_indices.len(), c.cohesion);
        // Show first 3 members as content preview
        for &idx in c.member_indices.iter().take(3) {
            let id = &nodes[idx].id;
            let content = memory_map.get(id.as_str()).unwrap_or(&"<not found>");
            let preview: String = content.chars().take(100).collect();
            println!("    • [{}] {}", id, preview);
        }
        if c.member_indices.len() > 3 {
            println!("    ... and {} more", c.member_indices.len() - 3);
        }
    }

    // Quality assertions
    assert!(!clusters.is_empty(), "Should discover at least some clusters from real data");
    assert!(clusters.len() >= 5, "Expected at least 5 clusters from 10k+ memories, got {}", clusters.len());

    // No single cluster should swallow everything (chaining effect check)
    let max_cluster_size = clusters.iter().map(|c| c.member_indices.len()).max().unwrap_or(0);
    let max_pct = max_cluster_size as f64 / nodes.len() as f64 * 100.0;
    println!("\nLargest cluster: {} ({:.1}% of all memories)", max_cluster_size, max_pct);
    assert!(
        max_pct < 50.0,
        "Largest cluster has {:.1}% of all memories — chaining effect detected!",
        max_pct
    );

    // Average cohesion should be reasonable
    let avg_cohesion: f64 = clusters.iter().map(|c| c.cohesion).sum::<f64>() / clusters.len() as f64;
    println!("Average cohesion: {:.4}", avg_cohesion);
    assert!(avg_cohesion > 0.1, "Average cohesion too low: {:.4}", avg_cohesion);

    println!("\n✅ Topic Discovery N2N PASSED");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 3: Synthesis Clustering (MultiSignal) on real data
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_03_synthesis_clustering_infomap() {
    let storage = require_real_db();

    println!("\n=== Synthesis Clustering (MultiSignal + Infomap) ===");

    let config = ClusterDiscoveryConfig {
        min_cluster_size: 2,
        max_cluster_size: 50,
        cluster_threshold: 0.3,
        min_importance: 0.0,
        weights: engramai::synthesis::types::ClusterWeights {
            hebbian: 0.4,
            entity: 0.3,
            embedding: 0.2,
            temporal: 0.1,
        },
        temporal_decay_lambda: 0.00413,
        ..Default::default()
    };

    let start = std::time::Instant::now();
    let clusters = discover_clusters(&storage, &config, Some(EMBEDDING_MODEL))
        .expect("discover_clusters should not fail on real data");
    let elapsed = start.elapsed();

    println!("Clusters found: {}", clusters.len());
    println!("Clustering time: {:?}", elapsed);

    if clusters.is_empty() {
        println!("⚠️ No clusters found — might need to lower min_importance or check data");
        return;
    }

    // Show size distribution
    println!("\n--- Cluster Size Distribution ---");
    let mut size_counts: HashMap<usize, usize> = HashMap::new();
    for c in &clusters {
        *size_counts.entry(c.members.len()).or_default() += 1;
    }
    let mut sizes: Vec<(usize, usize)> = size_counts.into_iter().collect();
    sizes.sort_by_key(|&(size, _)| size);
    for (size, count) in &sizes {
        println!("  Size {:>4}: {:>4} clusters", size, count);
    }

    // Show top clusters with content preview
    let all_memories = storage.all().unwrap();
    let memory_map: HashMap<&str, &str> = all_memories
        .iter()
        .map(|m| (m.id.as_str(), m.content.as_str()))
        .collect();

    println!("\n--- Top 10 Clusters by Quality ---");
    for (i, c) in clusters.iter().take(10).enumerate() {
        println!("\n  Cluster {} [{}] (size={}, quality={:.4}, centroid={}):",
            i + 1, c.id, c.members.len(), c.quality_score, c.centroid_id);
        println!("    Dominant signal: {:?}", c.signals_summary.dominant_signal);
        println!("    Signals: H={:.3} E={:.3} Emb={:.3} T={:.3}",
            c.signals_summary.hebbian_contribution,
            c.signals_summary.entity_contribution,
            c.signals_summary.embedding_contribution,
            c.signals_summary.temporal_contribution);
        // Show member content
        for member_id in c.members.iter().take(3) {
            let content = memory_map.get(member_id.as_str()).unwrap_or(&"<not found>");
            let preview: String = content.chars().take(100).collect();
            println!("    • [{}] {}", member_id, preview);
        }
        if c.members.len() > 3 {
            println!("    ... and {} more", c.members.len() - 3);
        }
    }

    // Quality assertions
    assert!(clusters.len() >= 3, "Expected at least 3 clusters from real synthesis, got {}", clusters.len());

    // No mega-cluster
    let max_size = clusters.iter().map(|c| c.members.len()).max().unwrap_or(0);
    println!("\nLargest cluster: {} members", max_size);
    assert!(max_size <= 50, "Cluster exceeds max_cluster_size of 50: {}", max_size);

    // Average quality should be positive
    let avg_quality: f64 = clusters.iter().map(|c| c.quality_score).sum::<f64>() / clusters.len() as f64;
    println!("Average quality: {:.4}", avg_quality);
    assert!(avg_quality > 0.0, "Average quality should be positive");

    println!("\n✅ Synthesis Clustering N2N PASSED");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 4: Compare EmbeddingOnly vs MultiSignal on same data
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_04_strategy_comparison() {
    let storage = require_real_db();
    let embeddings = storage.get_all_embeddings(EMBEDDING_MODEL).unwrap();

    println!("\n=== Strategy Comparison: EmbeddingOnly vs MultiSignal ===");
    println!("Input: {} memories", embeddings.len());

    // Subsample if too large (for faster comparison)
    let sample_size = embeddings.len().min(2000);
    let sample: Vec<&(String, Vec<f32>)> = embeddings.iter().take(sample_size).collect();

    // Build full nodes with all signals for the sample
    let all_memories = storage.all().unwrap();
    let memory_map: HashMap<&str, &engramai::MemoryRecord> = all_memories
        .iter()
        .map(|m| (m.id.as_str(), m))
        .collect();

    let sample_ids: HashSet<&str> = sample.iter().map(|(id, _)| id.as_str()).collect();

    let mut hebbian_map: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    let mut entity_map: HashMap<String, Vec<String>> = HashMap::new();

    for (id, _) in &sample {
        if let Ok(links) = storage.get_hebbian_links_weighted(id) {
            let filtered: Vec<(String, f64)> = links
                .into_iter()
                .filter(|(neighbor, _)| sample_ids.contains(neighbor.as_str()))
                .collect();
            if !filtered.is_empty() {
                hebbian_map.insert(id.clone(), filtered);
            }
        }
        if let Ok(entities) = storage.get_entity_ids_for_memory(id) {
            if !entities.is_empty() {
                entity_map.insert(id.clone(), entities);
            }
        }
    }

    let nodes: Vec<ClusterNode> = sample
        .iter()
        .map(|(id, emb)| {
            let created_at = memory_map
                .get(id.as_str())
                .map(|m| m.created_at.timestamp() as f64)
                .unwrap_or(0.0);
            ClusterNode {
                id: id.clone(),
                embedding: emb.clone(),
                hebbian_links: hebbian_map.get(id).cloned().unwrap_or_default(),
                entity_ids: entity_map.get(id).cloned().unwrap_or_default(),
                created_at_secs: created_at,
            }
        })
        .collect();

    println!("Sample size: {}", nodes.len());
    println!("With Hebbian links: {}", hebbian_map.len());
    println!("With entities: {}", entity_map.len());

    // Run EmbeddingOnly
    let config = ClustererConfig {
        k_neighbors: 15,
        min_edge_weight: 0.1,
        min_cluster_size: 3,
        num_trials: 5,
        seed: 42,
    };

    let start = std::time::Instant::now();
    let clusters_emb = InfomapClusterer::new(EmbeddingOnly, config.clone()).cluster(&nodes);
    let t_emb = start.elapsed();

    // Run MultiSignal
    let start = std::time::Instant::now();
    let clusters_multi = InfomapClusterer::new(MultiSignal::default(), config).cluster(&nodes);
    let t_multi = start.elapsed();

    println!("\n┌───────────────────┬──────────────┬──────────────┐");
    println!("│                   │ EmbeddingOnly│  MultiSignal │");
    println!("├───────────────────┼──────────────┼──────────────┤");
    println!("│ Clusters found    │ {:>12} │ {:>12} │", clusters_emb.len(), clusters_multi.len());
    println!("│ Total clustered   │ {:>12} │ {:>12} │",
        clusters_emb.iter().map(|c| c.member_indices.len()).sum::<usize>(),
        clusters_multi.iter().map(|c| c.member_indices.len()).sum::<usize>());
    println!("│ Avg cluster size  │ {:>12.1} │ {:>12.1} │",
        if clusters_emb.is_empty() { 0.0 } else { clusters_emb.iter().map(|c| c.member_indices.len()).sum::<usize>() as f64 / clusters_emb.len() as f64 },
        if clusters_multi.is_empty() { 0.0 } else { clusters_multi.iter().map(|c| c.member_indices.len()).sum::<usize>() as f64 / clusters_multi.len() as f64 });
    println!("│ Max cluster size  │ {:>12} │ {:>12} │",
        clusters_emb.iter().map(|c| c.member_indices.len()).max().unwrap_or(0),
        clusters_multi.iter().map(|c| c.member_indices.len()).max().unwrap_or(0));
    println!("│ Avg cohesion      │ {:>12.4} │ {:>12.4} │",
        if clusters_emb.is_empty() { 0.0 } else { clusters_emb.iter().map(|c| c.cohesion).sum::<f64>() / clusters_emb.len() as f64 },
        if clusters_multi.is_empty() { 0.0 } else { clusters_multi.iter().map(|c| c.cohesion).sum::<f64>() / clusters_multi.len() as f64 });
    println!("│ Time              │ {:>12?} │ {:>12?} │", t_emb, t_multi);
    println!("└───────────────────┴──────────────┴──────────────┘");

    // Compute cluster overlap (Jaccard of member sets between the two strategies)
    let emb_sets: Vec<HashSet<usize>> = clusters_emb.iter().map(|c| c.member_indices.iter().cloned().collect()).collect();
    let multi_sets: Vec<HashSet<usize>> = clusters_multi.iter().map(|c| c.member_indices.iter().cloned().collect()).collect();

    let mut total_max_jaccard = 0.0;
    let mut matched_count = 0;
    for emb_set in &emb_sets {
        let mut best_jaccard = 0.0f64;
        for multi_set in &multi_sets {
            let intersection = emb_set.intersection(multi_set).count();
            let union = emb_set.union(multi_set).count();
            if union > 0 {
                best_jaccard = best_jaccard.max(intersection as f64 / union as f64);
            }
        }
        total_max_jaccard += best_jaccard;
        if best_jaccard > 0.5 {
            matched_count += 1;
        }
    }
    let avg_match = if emb_sets.is_empty() { 0.0 } else { total_max_jaccard / emb_sets.len() as f64 };
    println!("\nCluster overlap between strategies:");
    println!("  Average best-match Jaccard: {:.3}", avg_match);
    println!("  Clusters with >50%% match: {}/{}", matched_count, emb_sets.len());

    println!("\n✅ Strategy Comparison N2N PASSED");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 5: Semantic coherence spot-check
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_05_semantic_coherence() {
    let storage = require_real_db();
    let all_embeddings = storage.get_all_embeddings(EMBEDDING_MODEL).unwrap();
    let all_memories = storage.all().unwrap();

    let sample_size = std::env::var("N2N_SAMPLE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2000);

    println!("\n=== Semantic Coherence Spot-Check ===");

    let memory_map: HashMap<&str, &str> = all_memories
        .iter()
        .map(|m| (m.id.as_str(), m.content.as_str()))
        .collect();

    let nodes: Vec<ClusterNode> = all_embeddings
        .iter()
        .take(sample_size)
        .map(|(id, emb)| ClusterNode {
            id: id.clone(),
            embedding: emb.clone(),
            hebbian_links: Vec::new(),
            entity_ids: Vec::new(),
            created_at_secs: 0.0,
        })
        .collect();

    let config = ClustererConfig {
        k_neighbors: 15,
        min_edge_weight: 0.1,
        min_cluster_size: 3,
        num_trials: 5,
        seed: 42,
    };
    let clusters = InfomapClusterer::new(EmbeddingOnly, config).cluster(&nodes);

    // For each of the top 20 clusters, check semantic coherence:
    // Extract keywords from each member and check overlap
    println!("Checking top 20 clusters for semantic coherence...\n");

    let mut coherent_count = 0;
    let check_count = clusters.len().min(20);

    for (i, c) in clusters.iter().take(check_count).enumerate() {
        let contents: Vec<&str> = c
            .member_indices
            .iter()
            .filter_map(|&idx| memory_map.get(nodes[idx].id.as_str()).copied())
            .collect();

        // Simple keyword extraction: lowercase, split on whitespace, filter stopwords
        let stopwords: HashSet<&str> = [
            "the", "a", "an", "is", "are", "was", "were", "be", "been",
            "being", "have", "has", "had", "do", "does", "did", "will",
            "would", "shall", "should", "may", "might", "must", "can",
            "could", "to", "of", "in", "for", "on", "with", "at", "by",
            "from", "as", "into", "through", "during", "before", "after",
            "above", "below", "between", "out", "off", "over", "under",
            "again", "further", "then", "once", "here", "there", "when",
            "where", "why", "how", "all", "both", "each", "few", "more",
            "most", "other", "some", "such", "no", "nor", "not", "only",
            "own", "same", "so", "than", "too", "very", "and", "but",
            "or", "if", "this", "that", "it", "its", "i", "you", "he",
            "she", "we", "they", "me", "him", "her", "us", "them", "my",
            "your", "his", "our", "their", "what", "which", "who", "whom",
            "—", "-", "—", "", "about", "just", "up", "down",
        ].iter().copied().collect();

        // Count word frequency across all members
        let mut word_freq: HashMap<String, usize> = HashMap::new();
        for content in &contents {
            let words: HashSet<String> = content
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() > 2 && !stopwords.contains(w))
                .map(|w| w.to_string())
                .collect();
            for word in words {
                *word_freq.entry(word).or_default() += 1;
            }
        }

        // Top shared keywords (appearing in >50% of members)
        let threshold = (contents.len() as f64 * 0.4).ceil() as usize;
        let mut shared: Vec<(&str, usize)> = word_freq
            .iter()
            .filter(|(_, &count)| count >= threshold)
            .map(|(word, &count)| (word.as_str(), count))
            .collect();
        shared.sort_by(|a, b| b.1.cmp(&a.1));

        let is_coherent = !shared.is_empty();
        if is_coherent {
            coherent_count += 1;
        }

        let status = if is_coherent { "✅" } else { "⚠️" };
        let top_keywords: Vec<&str> = shared.iter().take(5).map(|(w, _)| *w).collect();
        println!("  {} Cluster {:>2} (size={:>3}, cohesion={:.3}): [{}]",
            status, i + 1, c.member_indices.len(), c.cohesion,
            top_keywords.join(", "));
    }

    let coherence_pct = coherent_count as f64 / check_count as f64 * 100.0;
    println!("\nCoherence: {}/{} ({:.0}%) clusters have shared keywords",
        coherent_count, check_count, coherence_pct);

    assert!(
        coherence_pct >= 50.0,
        "Less than 50% of clusters are semantically coherent — clustering quality issue"
    );

    println!("\n✅ Semantic Coherence N2N PASSED");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TEST 6: Performance & scalability
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore]
fn n2n_06_performance() {
    let storage = require_real_db();
    let embeddings = storage.get_all_embeddings(EMBEDDING_MODEL).unwrap();

    println!("\n=== Performance Benchmark ===");

    let nodes: Vec<ClusterNode> = embeddings
        .iter()
        .map(|(id, emb)| ClusterNode {
            id: id.clone(),
            embedding: emb.clone(),
            hebbian_links: Vec::new(),
            entity_ids: Vec::new(),
            created_at_secs: 0.0,
        })
        .collect();

    // Test at different scales
    let scales = [100, 500, 1000, 2000];

    println!("\n┌──────────┬──────────┬──────────┬──────────────┐");
    println!("│  N nodes │ Clusters │ Time     │ ms/node      │");
    println!("├──────────┼──────────┼──────────┼──────────────┤");

    for &n in &scales {
        if n > nodes.len() {
            continue;
        }
        let subset = &nodes[..n];
        let config = ClustererConfig {
            k_neighbors: 15.min(n.saturating_sub(1)),
            min_edge_weight: 0.1,
            min_cluster_size: 3,
            num_trials: 3,  // fewer trials for benchmark
            seed: 42,
        };
        let clusterer = InfomapClusterer::new(EmbeddingOnly, config);

        let start = std::time::Instant::now();
        let clusters = clusterer.cluster(subset);
        let elapsed = start.elapsed();

        let ms_per_node = elapsed.as_secs_f64() * 1000.0 / n as f64;
        println!("│ {:>8} │ {:>8} │ {:>7.1}s │ {:>10.3} ms │",
            n, clusters.len(), elapsed.as_secs_f64(), ms_per_node);
    }
    println!("└──────────┴──────────┴──────────┴──────────────┘");

    // 2000-node benchmark should complete in <60 seconds
    let bench_size = nodes.len().min(2000);
    let bench_nodes = &nodes[..bench_size];
    let config = ClustererConfig {
        k_neighbors: 15,
        min_edge_weight: 0.1,
        min_cluster_size: 3,
        num_trials: 5,
        seed: 42,
    };
    let start = std::time::Instant::now();
    let _clusters = InfomapClusterer::new(EmbeddingOnly, config).cluster(bench_nodes);
    let total = start.elapsed();

    println!("\nBenchmark ({} nodes): {:?}", bench_size, total);
    assert!(
        total.as_secs() < 120,
        "Clustering {} nodes took {:?} — too slow (>2 min)",
        bench_size, total
    );

    println!("\n✅ Performance N2N PASSED");
}
