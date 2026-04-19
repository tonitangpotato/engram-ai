//! Cluster discovery module for the synthesis engine.
//!
//! Uses the unified [`InfomapClusterer`](crate::clustering::InfomapClusterer) with
//! [`MultiSignal`](crate::clustering::MultiSignal) strategy for edge weights.
//! This combines 4 zero-LLM signals:
//!
//! 1. Hebbian link weights
//! 2. Entity overlap (Jaccard index)
//! 3. Embedding similarity (cosine)
//! 4. Temporal proximity (exponential decay)
//!
//! This module is an adapter: it loads signal data from Storage, builds
//! [`ClusterNode`]s, runs the shared clusterer, and converts results back
//! to synthesis-specific [`MemoryCluster`] structs.

use std::collections::{HashMap, HashSet};

use crate::clustering::{ClusterNode, ClustererConfig, InfomapClusterer, MultiSignal, SignalWeights};
use crate::storage::Storage;
use crate::synthesis::types::*;
use crate::types::MemoryRecord;

/// Compute pairwise signals between two memories.
///
/// This remains public for use by the gate and other modules that need
/// to inspect individual pair signals.
pub fn compute_pairwise_signals(
    _storage: &Storage,
    id_a: &str,
    id_b: &str,
    hebbian_map: &HashMap<(String, String), f64>,
    entity_map: &HashMap<String, HashSet<String>>,
    embedding_map: &HashMap<String, Vec<f32>>,
    records: &HashMap<String, &MemoryRecord>,
    config: &ClusterDiscoveryConfig,
) -> PairwiseSignals {
    use crate::embeddings::EmbeddingProvider;

    // Signal 1: Hebbian weight (normalized to 0-1 by dividing by 10.0)
    let key = if id_a < id_b {
        (id_a.to_string(), id_b.to_string())
    } else {
        (id_b.to_string(), id_a.to_string())
    };
    let hebbian_weight = hebbian_map.get(&key).copied();

    // Signal 2: Entity overlap (Jaccard index)
    let entities_a = entity_map.get(id_a);
    let entities_b = entity_map.get(id_b);
    let entity_overlap = match (entities_a, entities_b) {
        (Some(a), Some(b)) if !a.is_empty() || !b.is_empty() => {
            let intersection = a.intersection(b).count();
            let union = a.union(b).count();
            if union > 0 {
                intersection as f64 / union as f64
            } else {
                0.0
            }
        }
        _ => 0.0,
    };

    // Signal 3: Embedding similarity (cosine)
    let embedding_similarity = match (embedding_map.get(id_a), embedding_map.get(id_b)) {
        (Some(emb_a), Some(emb_b)) => EmbeddingProvider::cosine_similarity(emb_a, emb_b) as f64,
        _ => 0.0,
    };

    // Signal 4: Temporal proximity
    let temporal_proximity = match (records.get(id_a), records.get(id_b)) {
        (Some(a), Some(b)) => {
            let hours_apart =
                (a.created_at - b.created_at).num_seconds().unsigned_abs() as f64 / 3600.0;
            (-config.temporal_decay_lambda * hours_apart).exp()
        }
        _ => 0.0,
    };

    PairwiseSignals {
        hebbian_weight,
        entity_overlap,
        embedding_similarity,
        temporal_proximity,
    }
}

/// Compute the weighted composite score from pairwise signals.
pub fn compute_composite_score(signals: &PairwiseSignals, weights: &ClusterWeights) -> f64 {
    let hebbian_norm = signals
        .hebbian_weight
        .map(|w| (w / 10.0).min(1.0))
        .unwrap_or(0.0);
    weights.hebbian * hebbian_norm
        + weights.entity * signals.entity_overlap
        + weights.embedding * signals.embedding_similarity
        + weights.temporal * signals.temporal_proximity
}

/// Discover clusters of related memories using Infomap community detection.
///
/// Loads all signal data from Storage, builds `ClusterNode`s with full signal
/// information, and delegates to the unified `InfomapClusterer<MultiSignal>`.
pub fn discover_clusters(
    storage: &Storage,
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<Vec<MemoryCluster>, Box<dyn std::error::Error>> {
    // Step 1: Get candidate memories (pre-filter)
    let all_memories = storage.all()?;
    let candidates: Vec<&MemoryRecord> = all_memories
        .iter()
        .filter(|m| {
            !m.access_times.is_empty() // accessed at least once
                && m.importance >= config.min_importance
                && !is_synthesis_output(m)
        })
        .collect();

    if candidates.len() < config.min_cluster_size {
        return Ok(Vec::new());
    }

    let candidate_ids: HashSet<&str> = candidates.iter().map(|m| m.id.as_str()).collect();

    // Step 2: Build signal maps (pre-compute for efficient node construction)
    // Bulk-load all data in 2 SQL queries instead of 26k per-ID queries (ISS-001 Fix 1)

    // Hebbian links — single bulk query, then filter to candidates
    let all_hebbian = storage.get_all_hebbian_links_bulk()?;
    let hebbian_map: std::collections::HashMap<String, Vec<(String, f64)>> = all_hebbian
        .into_iter()
        .filter(|(id, _)| candidate_ids.contains(id.as_str()))
        .map(|(id, links)| {
            let filtered: Vec<(String, f64)> = links
                .into_iter()
                .filter(|(neighbor, _)| candidate_ids.contains(neighbor.as_str()))
                .collect();
            (id, filtered)
        })
        .collect();

    // Entity IDs per memory — single bulk query, then filter to candidates
    let all_entities = storage.get_all_memory_entities_bulk()?;
    let entity_map: HashMap<String, Vec<String>> = all_entities
        .into_iter()
        .filter(|(id, _)| candidate_ids.contains(id.as_str()))
        .collect();

    // Embeddings
    let embedding_map: HashMap<String, Vec<f32>> = if let Some(model) = embedding_model {
        storage
            .get_all_embeddings(model)?
            .into_iter()
            .filter(|(id, _)| candidate_ids.contains(id.as_str()))
            .collect()
    } else {
        HashMap::new()
    };

    // Step 3: Build ClusterNodes
    let nodes: Vec<ClusterNode> = candidates
        .iter()
        .map(|m| {
            let embedding = embedding_map
                .get(&m.id)
                .cloned()
                .unwrap_or_default();
            let hebbian_links = hebbian_map
                .get(&m.id)
                .cloned()
                .unwrap_or_default();
            let entity_ids = entity_map
                .get(&m.id)
                .cloned()
                .unwrap_or_default();
            let created_at_secs = m.created_at.timestamp() as f64;

            ClusterNode {
                id: m.id.clone(),
                embedding,
                hebbian_links,
                entity_ids,
                created_at_secs,
            }
        })
        .collect();

    // Step 4: Run the unified clusterer with MultiSignal strategy
    let strategy = MultiSignal {
        weights: SignalWeights {
            hebbian: config.weights.hebbian,
            entity: config.weights.entity,
            embedding: config.weights.embedding,
            temporal: config.weights.temporal,
        },
        temporal_decay_lambda: config.temporal_decay_lambda,
    };
    let clusterer_config = ClustererConfig {
        k_neighbors: 15,
        min_edge_weight: 0.1,
        min_cluster_size: config.min_cluster_size,
        num_trials: 5,
        seed: 42,
    };
    let clusterer = InfomapClusterer::new(strategy, clusterer_config);
    let raw_clusters = clusterer.cluster(&nodes);

    // Step 5: Convert to MemoryCluster structs with synthesis-specific metadata
    let mut result: Vec<MemoryCluster> = Vec::new();

    for cluster in raw_clusters {
        let members: Vec<String> = cluster
            .member_indices
            .iter()
            .map(|&i| nodes[i].id.clone())
            .collect();

        // Enforce max_cluster_size
        let mut sorted_members = members;
        sorted_members.sort();
        if sorted_members.len() > config.max_cluster_size {
            sorted_members.truncate(config.max_cluster_size);
        }

        if sorted_members.len() < config.min_cluster_size {
            continue;
        }

        // Find centroid: member with highest average weight to others
        let member_set: HashSet<&str> = sorted_members.iter().map(|s| s.as_str()).collect();
        let member_nodes: Vec<&ClusterNode> = nodes
            .iter()
            .filter(|n| member_set.contains(n.id.as_str()))
            .collect();

        let centroid_id = find_centroid(&member_nodes, clusterer.strategy());
        let quality_score = cluster.cohesion;

        // Compute signals summary
        let signals_summary = compute_signals_summary(&sorted_members, &entity_map, config);

        // Deterministic cluster ID
        let id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            sorted_members.hash(&mut hasher);
            format!("cluster-{:016x}", hasher.finish())
        };

        result.push(MemoryCluster {
            id,
            members: sorted_members,
            quality_score,
            centroid_id,
            signals_summary,
        });
    }

    // Sort by quality descending
    result.sort_by(|a, b| {
        b.quality_score
            .partial_cmp(&a.quality_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(result)
}

/// Find the centroid node (highest average weight to other members).
fn find_centroid<S: crate::clustering::EdgeWeightStrategy>(
    members: &[&ClusterNode],
    strategy: &S,
) -> String {
    if members.is_empty() {
        return String::new();
    }
    if members.len() == 1 {
        return members[0].id.clone();
    }

    let mut best_id = members[0].id.clone();
    let mut best_avg = f64::NEG_INFINITY;

    for (i, &node_a) in members.iter().enumerate() {
        let mut sum = 0.0;
        let mut count = 0usize;
        for (j, &node_b) in members.iter().enumerate() {
            if i != j {
                sum += strategy.compute_weight(node_a, node_b);
                count += 1;
            }
        }
        let avg = if count > 0 { sum / count as f64 } else { 0.0 };
        if avg > best_avg {
            best_avg = avg;
            best_id = node_a.id.clone();
        }
    }

    best_id
}

/// Compute the signals summary for a cluster.
fn compute_signals_summary(
    members: &[String],
    entity_map: &HashMap<String, Vec<String>>,
    config: &ClusterDiscoveryConfig,
) -> SignalsSummary {
    let entity_pairs = members
        .iter()
        .filter(|m| {
            entity_map
                .get(m.as_str())
                .map(|e| !e.is_empty())
                .unwrap_or(false)
        })
        .count();
    let entity_contribution_est = entity_pairs as f64 / members.len().max(1) as f64;

    let hebbian_c = config.weights.hebbian;
    let entity_c = config.weights.entity * entity_contribution_est;
    let embedding_c = config.weights.embedding;
    let temporal_c = config.weights.temporal;

    let dominant_signal =
        if hebbian_c >= entity_c && hebbian_c >= embedding_c && hebbian_c >= temporal_c {
            ClusterSignal::Hebbian
        } else if entity_c >= embedding_c && entity_c >= temporal_c {
            ClusterSignal::Entity
        } else if embedding_c >= temporal_c {
            ClusterSignal::Embedding
        } else {
            ClusterSignal::Temporal
        };

    SignalsSummary {
        dominant_signal,
        hebbian_contribution: hebbian_c,
        entity_contribution: entity_c,
        embedding_contribution: embedding_c,
        temporal_contribution: temporal_c,
    }
}

/// Check if a memory is a synthesis output (via metadata).
fn is_synthesis_output(record: &MemoryRecord) -> bool {
    record
        .metadata
        .as_ref()
        .and_then(|m| m.get("is_synthesis"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ===========================================================================
// §2.4 — Emotional Modulation
// ===========================================================================

/// Apply emotional modulation to discovered clusters.
///
/// - Boosts quality scores for clusters with emotionally salient members
/// - Re-sorts clusters by emotional salience (descending), then quality
/// - Flags clusters that contain emotional context for prompt inclusion
///
/// When `config.prioritize_emotional` is false and `config.emotional_boost_weight` is 0.0,
/// this is effectively a no-op (returns clusters unchanged).
pub fn apply_emotional_modulation(
    mut clusters: Vec<MemoryCluster>,
    members_map: &HashMap<String, &MemoryRecord>,
    config: &EmotionalModulationConfig,
) -> Vec<MemoryCluster> {
    if !config.prioritize_emotional && config.emotional_boost_weight == 0.0 {
        return clusters;
    }

    let mut cluster_salience: Vec<(usize, f64)> = Vec::with_capacity(clusters.len());

    for (i, cluster) in clusters.iter_mut().enumerate() {
        let salience = compute_emotional_salience(cluster, members_map);

        if config.emotional_boost_weight > 0.0 && salience > 0.0 {
            let boost = 1.0 + config.emotional_boost_weight * salience;
            cluster.quality_score *= boost;
        }

        cluster_salience.push((i, salience));
    }

    if config.prioritize_emotional {
        let mut indexed: Vec<(usize, &MemoryCluster)> =
            clusters.iter().enumerate().collect();
        indexed.sort_by(|a, b| {
            let sa = cluster_salience
                .iter()
                .find(|x| x.0 == a.0)
                .map(|x| x.1)
                .unwrap_or(0.0);
            let sb = cluster_salience
                .iter()
                .find(|x| x.0 == b.0)
                .map(|x| x.1)
                .unwrap_or(0.0);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.1.quality_score
                        .partial_cmp(&a.1.quality_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let order: Vec<usize> = indexed.iter().map(|(i, _)| *i).collect();
        let old = clusters.clone();
        for (new_pos, &old_pos) in order.iter().enumerate() {
            clusters[new_pos] = old[old_pos].clone();
        }
    }

    clusters
}

/// Compute the emotional salience of a cluster (average absolute valence of emotional members).
pub fn compute_emotional_salience(
    cluster: &MemoryCluster,
    members_map: &HashMap<String, &MemoryRecord>,
) -> f64 {
    let mut total_valence = 0.0;
    let mut emotional_count = 0usize;

    for member_id in &cluster.members {
        if let Some(record) = members_map.get(member_id) {
            if let Some(metadata) = &record.metadata {
                if let Some(valence) = metadata.get("emotional_valence").and_then(|v| v.as_f64()) {
                    total_valence += valence.abs();
                    emotional_count += 1;
                }
            }
        }
    }

    if emotional_count > 0 {
        total_valence / emotional_count as f64
    } else {
        0.0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MemoryLayer, MemoryType};

    /// Helper: create a minimal MemoryRecord for testing.
    fn make_test_record(id: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: format!("Test memory {}", id),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
            access_times: vec![chrono::Utc::now()],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            metadata: Some(serde_json::json!({})),
        }
    }

    fn make_cluster_with_quality(id: &str, members: &[&str], quality: f64) -> MemoryCluster {
        MemoryCluster {
            id: id.to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            quality_score: quality,
            centroid_id: members[0].to_string(),
            signals_summary: SignalsSummary {
                dominant_signal: ClusterSignal::Hebbian,
                hebbian_contribution: 0.4,
                entity_contribution: 0.3,
                embedding_contribution: 0.2,
                temporal_contribution: 0.1,
            },
        }
    }

    fn make_record_with_emotion(id: &str, valence: Option<f64>) -> MemoryRecord {
        let mut rec = make_test_record(id);
        rec.memory_type = MemoryType::Emotional;
        if let Some(v) = valence {
            rec.metadata = Some(serde_json::json!({"emotional_valence": v}));
        }
        rec
    }

    #[test]
    fn test_composite_score_basic() {
        let signals = PairwiseSignals {
            hebbian_weight: Some(5.0),
            entity_overlap: 0.5,
            embedding_similarity: 0.8,
            temporal_proximity: 0.9,
        };
        let weights = ClusterWeights::default();
        let score = compute_composite_score(&signals, &weights);

        // hebbian: 0.4 * (5.0/10.0) = 0.4 * 0.5 = 0.20
        // entity:  0.3 * 0.5 = 0.15
        // embed:   0.2 * 0.8 = 0.16
        // temporal: 0.1 * 0.9 = 0.09
        // total = 0.60
        assert!((score - 0.60).abs() < 0.01, "expected ~0.60, got {}", score);
    }

    #[test]
    fn test_composite_score_no_hebbian() {
        let signals = PairwiseSignals {
            hebbian_weight: None,
            entity_overlap: 1.0,
            embedding_similarity: 1.0,
            temporal_proximity: 1.0,
        };
        let weights = ClusterWeights::default();
        let score = compute_composite_score(&signals, &weights);

        // hebbian: 0
        // entity: 0.3, embed: 0.2, temporal: 0.1
        // total = 0.6
        assert!((score - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_emotional_modulation_noop_when_disabled() {
        let config = EmotionalModulationConfig {
            emotional_boost_weight: 0.0,
            prioritize_emotional: false,
            include_emotion_in_prompt: false,
        };
        let clusters = vec![
            make_cluster_with_quality("c1", &["a", "b"], 0.5),
            make_cluster_with_quality("c2", &["c", "d"], 0.8),
        ];
        let members_map = HashMap::new();
        let result = apply_emotional_modulation(clusters.clone(), &members_map, &config);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].quality_score, 0.5);
        assert_eq!(result[1].quality_score, 0.8);
    }

    #[test]
    fn test_emotional_modulation_boosts_quality() {
        let config = EmotionalModulationConfig {
            emotional_boost_weight: 0.5,
            prioritize_emotional: false,
            include_emotion_in_prompt: true,
        };
        let clusters = vec![make_cluster_with_quality("c1", &["a", "b"], 0.6)];
        let rec_a = make_record_with_emotion("a", Some(0.8));
        let rec_b = make_record_with_emotion("b", Some(0.4));
        let members_map: HashMap<String, &MemoryRecord> =
            [("a".to_string(), &rec_a), ("b".to_string(), &rec_b)]
                .into_iter()
                .collect();
        let result = apply_emotional_modulation(clusters, &members_map, &config);
        // salience = (0.8 + 0.4) / 2 = 0.6
        // boost = 1.0 + 0.5 * 0.6 = 1.3
        // quality = 0.6 * 1.3 = 0.78
        assert!((result[0].quality_score - 0.78).abs() < 0.001);
    }

    #[test]
    fn test_emotional_modulation_prioritizes_emotional_clusters() {
        let config = EmotionalModulationConfig {
            emotional_boost_weight: 0.0,
            prioritize_emotional: true,
            include_emotion_in_prompt: true,
        };
        let clusters = vec![
            make_cluster_with_quality("c1", &["a"], 0.9),
            make_cluster_with_quality("c2", &["b"], 0.3),
        ];
        let rec_a = make_record_with_emotion("a", None);
        let rec_b = make_record_with_emotion("b", Some(0.9));
        let members_map: HashMap<String, &MemoryRecord> =
            [("a".to_string(), &rec_a), ("b".to_string(), &rec_b)]
                .into_iter()
                .collect();
        let result = apply_emotional_modulation(clusters, &members_map, &config);
        assert_eq!(result[0].id, "c2");
        assert_eq!(result[1].id, "c1");
    }

    #[test]
    fn test_emotional_salience_no_emotion() {
        let cluster = make_cluster_with_quality("c1", &["a", "b"], 0.5);
        let rec_a = make_record_with_emotion("a", None);
        let rec_b = make_record_with_emotion("b", None);
        let members_map: HashMap<String, &MemoryRecord> =
            [("a".to_string(), &rec_a), ("b".to_string(), &rec_b)]
                .into_iter()
                .collect();
        let salience = compute_emotional_salience(&cluster, &members_map);
        assert_eq!(salience, 0.0);
    }

    #[test]
    fn test_is_synthesis_output() {
        let mut rec = make_test_record("test");
        assert!(!is_synthesis_output(&rec));

        rec.metadata = Some(serde_json::json!({"is_synthesis": true}));
        assert!(is_synthesis_output(&rec));

        rec.metadata = Some(serde_json::json!({"is_synthesis": false}));
        assert!(!is_synthesis_output(&rec));
    }
}
