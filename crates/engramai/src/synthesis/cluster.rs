//! Cluster discovery module for the synthesis engine.
//!
//! Uses 4 zero-LLM signals to find groups of related memories:
//! 1. Hebbian link weights
//! 2. Entity overlap (Jaccard index)
//! 3. Embedding similarity (cosine)
//! 4. Temporal proximity (exponential decay)

use std::collections::{HashMap, HashSet};

use crate::embeddings::EmbeddingProvider;
use crate::storage::Storage;
use crate::synthesis::types::*;
use crate::types::MemoryRecord;

/// Compute pairwise signals between two memories.
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

/// Discover clusters of related memories.
///
/// Performance optimization: only compute pairwise scores for memory pairs
/// that share at least one signal (Hebbian link OR shared entity), not all×all.
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
            !m.access_times.is_empty() // accessed at least once (access_count > 0)
                && m.importance >= config.min_importance
                && !is_synthesis_output(m) // not already a synthesis
        })
        .collect();

    if candidates.len() < config.min_cluster_size {
        return Ok(Vec::new());
    }

    let candidate_ids: HashSet<&str> = candidates.iter().map(|m| m.id.as_str()).collect();
    let records: HashMap<String, &MemoryRecord> =
        candidates.iter().map(|m| (m.id.clone(), *m)).collect();

    // Step 2: Build signal maps (pre-compute for O(n) instead of O(n²) queries)
    // Hebbian: query all links involving candidates
    let mut hebbian_map: HashMap<(String, String), f64> = HashMap::new();
    for m in &candidates {
        if let Ok(links) = storage.get_hebbian_links_weighted(&m.id) {
            for (neighbor, weight) in links {
                if candidate_ids.contains(neighbor.as_str()) {
                    let key = if m.id < neighbor {
                        (m.id.clone(), neighbor)
                    } else {
                        (neighbor, m.id.clone())
                    };
                    hebbian_map.entry(key).or_insert(weight);
                }
            }
        }
    }

    // Entity: get entities for each candidate
    let mut entity_map: HashMap<String, HashSet<String>> = HashMap::new();
    for m in &candidates {
        if let Ok(entities) = storage.get_entity_ids_for_memory(&m.id) {
            entity_map.insert(m.id.clone(), entities.into_iter().collect());
        }
    }

    // Embeddings: load all at once if model provided
    let embedding_map: HashMap<String, Vec<f32>> = if let Some(model) = embedding_model {
        storage
            .get_all_embeddings(model)?
            .into_iter()
            .filter(|(id, _)| candidate_ids.contains(id.as_str()))
            .collect()
    } else {
        HashMap::new()
    };

    // Step 3: Build candidate pairs (OPTIMIZATION: only pairs with ≥1 signal)
    let mut candidate_pairs: HashSet<(String, String)> = HashSet::new();

    // From Hebbian links
    for key in hebbian_map.keys() {
        candidate_pairs.insert(key.clone());
    }

    // From shared entities
    let mut entity_to_memories: HashMap<&str, Vec<&str>> = HashMap::new();
    for (mem_id, entities) in &entity_map {
        for ent_id in entities {
            entity_to_memories
                .entry(ent_id.as_str())
                .or_default()
                .push(mem_id.as_str());
        }
    }
    for mems in entity_to_memories.values() {
        for i in 0..mems.len() {
            for j in (i + 1)..mems.len() {
                let (a, b) = if mems[i] < mems[j] {
                    (mems[i], mems[j])
                } else {
                    (mems[j], mems[i])
                };
                candidate_pairs.insert((a.to_string(), b.to_string()));
            }
        }
    }

    // Step 4: Compute scores and build edge list
    let mut edges: Vec<(String, String, f64)> = Vec::new();
    for (id_a, id_b) in &candidate_pairs {
        let signals = compute_pairwise_signals(
            storage,
            id_a,
            id_b,
            &hebbian_map,
            &entity_map,
            &embedding_map,
            &records,
            config,
        );
        let score = compute_composite_score(&signals, &config.weights);
        if score >= config.cluster_threshold {
            edges.push((id_a.clone(), id_b.clone(), score));
        }
    }

    // Step 5: Connected components
    let clusters = connected_components(&edges, &candidate_ids, config);

    // Step 6: Build MemoryCluster structs
    let mut result: Vec<MemoryCluster> = Vec::new();
    for members in &clusters {
        if let Some(cluster) =
            build_memory_cluster(members, &edges, &records, &entity_map, config)
        {
            result.push(cluster);
        }
    }

    // Sort by quality descending
    result.sort_by(|a, b| {
        b.quality_score
            .partial_cmp(&a.quality_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(result)
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

/// Find connected components from an edge list.
/// Returns Vec of member sets, filtered by min/max cluster size.
fn connected_components(
    edges: &[(String, String, f64)],
    all_ids: &HashSet<&str>,
    config: &ClusterDiscoveryConfig,
) -> Vec<Vec<String>> {
    // Union-Find implementation
    let id_list: Vec<String> = all_ids.iter().map(|s| s.to_string()).collect();
    let id_to_idx: HashMap<&str, usize> = id_list
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let mut parent: Vec<usize> = (0..id_list.len()).collect();

    fn find(parent: &mut Vec<usize>, i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    fn union(parent: &mut Vec<usize>, a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    for (a, b, _) in edges {
        if let (Some(&ia), Some(&ib)) = (id_to_idx.get(a.as_str()), id_to_idx.get(b.as_str())) {
            union(&mut parent, ia, ib);
        }
    }

    // Group by root
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    for (i, id) in id_list.iter().enumerate() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(id.clone());
    }

    // Filter by size and handle splitting
    let mut result = Vec::new();
    for (_, mut members) in groups {
        if members.len() < config.min_cluster_size {
            continue; // too small
        }
        if members.len() > config.max_cluster_size {
            // Split: for simplicity, take the top max_cluster_size by their edge connectivity
            // TODO: implement proper recursive splitting with higher threshold
            members.sort();
            members.truncate(config.max_cluster_size);
        }
        members.sort();
        result.push(members);
    }

    result
}

/// Build a MemoryCluster struct from a set of member IDs.
fn build_memory_cluster(
    members: &[String],
    edges: &[(String, String, f64)],
    _records: &HashMap<String, &MemoryRecord>,
    entity_map: &HashMap<String, HashSet<String>>,
    config: &ClusterDiscoveryConfig,
) -> Option<MemoryCluster> {
    if members.len() < config.min_cluster_size {
        return None;
    }

    let member_set: HashSet<&str> = members.iter().map(|s| s.as_str()).collect();

    // Compute quality = average pairwise score among members
    let mut total_score = 0.0;
    let mut pair_count = 0usize;
    let mut per_member_avg: HashMap<&str, (f64, usize)> = HashMap::new();

    for (a, b, score) in edges {
        if member_set.contains(a.as_str()) && member_set.contains(b.as_str()) {
            total_score += score;
            pair_count += 1;
            {
                let entry = per_member_avg.entry(a.as_str()).or_insert((0.0, 0));
                entry.0 += score;
                entry.1 += 1;
            }
            {
                let entry = per_member_avg.entry(b.as_str()).or_insert((0.0, 0));
                entry.0 += score;
                entry.1 += 1;
            }
        }
    }

    let quality_score = if pair_count > 0 {
        total_score / pair_count as f64
    } else {
        0.0
    };

    // Find centroid (member with highest avg relatedness)
    let centroid_id = per_member_avg
        .iter()
        .max_by(|a, b| {
            let avg_a = a.1 .0 / a.1 .1.max(1) as f64;
            let avg_b = b.1 .0 / b.1 .1.max(1) as f64;
            avg_a
                .partial_cmp(&avg_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(id, _)| id.to_string())
        .unwrap_or_else(|| members[0].clone());

    // Compute signals summary — estimate dominant signal from entity coverage
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

    // Determine dominant signal based on weighted contributions
    let hebbian_c = config.weights.hebbian;
    let entity_c = config.weights.entity * entity_contribution_est;
    let embedding_c = config.weights.embedding;
    let temporal_c = config.weights.temporal;

    let dominant_signal = if hebbian_c >= entity_c && hebbian_c >= embedding_c && hebbian_c >= temporal_c {
        ClusterSignal::Hebbian
    } else if entity_c >= embedding_c && entity_c >= temporal_c {
        ClusterSignal::Entity
    } else if embedding_c >= temporal_c {
        ClusterSignal::Embedding
    } else {
        ClusterSignal::Temporal
    };

    let signals_summary = SignalsSummary {
        dominant_signal,
        hebbian_contribution: hebbian_c,
        entity_contribution: entity_c,
        embedding_contribution: embedding_c,
        temporal_contribution: temporal_c,
    };

    // Deterministic cluster ID: hash of sorted member IDs
    let mut sorted_members = members.to_vec();
    sorted_members.sort();
    let id = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        sorted_members.hash(&mut hasher);
        format!("cluster-{:016x}", hasher.finish())
    };

    Some(MemoryCluster {
        id,
        members: sorted_members,
        quality_score,
        centroid_id,
        signals_summary,
    })
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

    // Compute emotional salience for each cluster and optionally boost quality
    let mut cluster_salience: Vec<(usize, f64)> = Vec::with_capacity(clusters.len());

    for (i, cluster) in clusters.iter_mut().enumerate() {
        let salience = compute_emotional_salience(cluster, members_map);

        // Boost quality_score by emotional factor
        if config.emotional_boost_weight > 0.0 && salience > 0.0 {
            let boost = 1.0 + config.emotional_boost_weight * salience;
            cluster.quality_score *= boost;
        }

        cluster_salience.push((i, salience));
    }

    // Re-sort: emotional salience desc, then quality desc
    if config.prioritize_emotional {
        let mut indexed: Vec<(usize, f64, f64)> = cluster_salience
            .iter()
            .map(|(i, sal)| (*i, *sal, clusters[*i].quality_score))
            .collect();
        indexed.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.2.partial_cmp(&a.2)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        let reordered: Vec<MemoryCluster> =
            indexed.into_iter().map(|(i, _, _)| clusters[i].clone()).collect();
        return reordered;
    }

    clusters
}

/// Compute the average emotional salience of a cluster's members.
///
/// Salience = mean of |emotional_valence| across members.
/// Members without emotional data contribute 0.0.
fn compute_emotional_salience(
    cluster: &MemoryCluster,
    members_map: &HashMap<String, &MemoryRecord>,
) -> f64 {
    if cluster.members.is_empty() {
        return 0.0;
    }

    let total: f64 = cluster
        .members
        .iter()
        .map(|id| {
            members_map
                .get(id.as_str())
                .and_then(|m| m.metadata.as_ref())
                .and_then(|meta| meta.get("emotional_valence"))
                .and_then(|v| v.as_f64())
                .map(|v| v.abs())
                .unwrap_or(0.0)
        })
        .sum();

    total / cluster.members.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_composite_score_defaults() {
        let weights = ClusterWeights::default();
        let signals = PairwiseSignals {
            hebbian_weight: Some(5.0),
            entity_overlap: 0.5,
            embedding_similarity: 0.8,
            temporal_proximity: 0.9,
        };
        let score = compute_composite_score(&signals, &weights);
        // hebbian: 0.4 * (5.0/10.0).min(1.0) = 0.4 * 0.5 = 0.20
        // entity:  0.3 * 0.5                                = 0.15
        // embed:   0.2 * 0.8                                = 0.16
        // temporal: 0.1 * 0.9                               = 0.09
        // total                                             = 0.60
        assert!((score - 0.60).abs() < 1e-9);
    }

    #[test]
    fn test_composite_score_no_hebbian() {
        let weights = ClusterWeights::default();
        let signals = PairwiseSignals {
            hebbian_weight: None,
            entity_overlap: 1.0,
            embedding_similarity: 1.0,
            temporal_proximity: 1.0,
        };
        let score = compute_composite_score(&signals, &weights);
        // 0.0 + 0.3 + 0.2 + 0.1 = 0.6
        assert!((score - 0.6).abs() < 1e-9);
    }

    #[test]
    fn test_composite_score_hebbian_clamped() {
        let weights = ClusterWeights::default();
        let signals = PairwiseSignals {
            hebbian_weight: Some(20.0), // 20/10 = 2.0, clamped to 1.0
            entity_overlap: 0.0,
            embedding_similarity: 0.0,
            temporal_proximity: 0.0,
        };
        let score = compute_composite_score(&signals, &weights);
        // 0.4 * 1.0 = 0.4
        assert!((score - 0.4).abs() < 1e-9);
    }

    #[test]
    fn test_composite_score_all_zero() {
        let weights = ClusterWeights::default();
        let signals = PairwiseSignals {
            hebbian_weight: None,
            entity_overlap: 0.0,
            embedding_similarity: 0.0,
            temporal_proximity: 0.0,
        };
        let score = compute_composite_score(&signals, &weights);
        assert!((score - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_is_synthesis_output_true() {
        let mut record = make_test_record("test-1");
        record.metadata = Some(serde_json::json!({"is_synthesis": true}));
        assert!(is_synthesis_output(&record));
    }

    #[test]
    fn test_is_synthesis_output_false() {
        let record = make_test_record("test-2");
        assert!(!is_synthesis_output(&record));
    }

    #[test]
    fn test_is_synthesis_output_no_metadata() {
        let mut record = make_test_record("test-3");
        record.metadata = None;
        assert!(!is_synthesis_output(&record));
    }

    #[test]
    fn test_is_synthesis_output_wrong_type() {
        let mut record = make_test_record("test-4");
        record.metadata = Some(serde_json::json!({"is_synthesis": "yes"}));
        assert!(!is_synthesis_output(&record));
    }

    #[test]
    fn test_connected_components_simple_triangle() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 3,
            max_cluster_size: 15,
            ..Default::default()
        };
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.5),
            ("b".to_string(), "c".to_string(), 0.6),
            ("a".to_string(), "c".to_string(), 0.4),
        ];
        let ids: HashSet<&str> = ["a", "b", "c"].into_iter().collect();

        let components = connected_components(&edges, &ids, &config);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn test_connected_components_two_clusters() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 2,
            max_cluster_size: 15,
            ..Default::default()
        };
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.5),
            ("c".to_string(), "d".to_string(), 0.6),
        ];
        let ids: HashSet<&str> = ["a", "b", "c", "d"].into_iter().collect();

        let mut components = connected_components(&edges, &ids, &config);
        components.sort_by(|a, b| a[0].cmp(&b[0]));
        assert_eq!(components.len(), 2);
        assert_eq!(components[0], vec!["a", "b"]);
        assert_eq!(components[1], vec!["c", "d"]);
    }

    #[test]
    fn test_connected_components_filters_small() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 3,
            max_cluster_size: 15,
            ..Default::default()
        };
        // Only 2 connected nodes — below min_cluster_size of 3
        let edges = vec![("a".to_string(), "b".to_string(), 0.5)];
        let ids: HashSet<&str> = ["a", "b", "c"].into_iter().collect();

        let components = connected_components(&edges, &ids, &config);
        // Both the pair {a,b} and the singleton {c} are < 3
        assert_eq!(components.len(), 0);
    }

    #[test]
    fn test_connected_components_truncates_large() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 2,
            max_cluster_size: 3,
            ..Default::default()
        };
        let edges = vec![
            ("a".to_string(), "b".to_string(), 0.5),
            ("b".to_string(), "c".to_string(), 0.6),
            ("c".to_string(), "d".to_string(), 0.4),
            ("d".to_string(), "e".to_string(), 0.3),
        ];
        let ids: HashSet<&str> = ["a", "b", "c", "d", "e"].into_iter().collect();

        let components = connected_components(&edges, &ids, &config);
        assert_eq!(components.len(), 1);
        assert!(components[0].len() <= 3);
    }

    #[test]
    fn test_build_memory_cluster_basic() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 2,
            ..Default::default()
        };
        let r1 = make_test_record("m1");
        let r2 = make_test_record("m2");
        let r3 = make_test_record("m3");
        let records: HashMap<String, &MemoryRecord> = [
            ("m1".to_string(), &r1),
            ("m2".to_string(), &r2),
            ("m3".to_string(), &r3),
        ]
        .into_iter()
        .collect();
        let entity_map: HashMap<String, HashSet<String>> = HashMap::new();
        let edges = vec![
            ("m1".to_string(), "m2".to_string(), 0.5),
            ("m2".to_string(), "m3".to_string(), 0.6),
        ];
        let members = vec!["m1".to_string(), "m2".to_string(), "m3".to_string()];

        let cluster =
            build_memory_cluster(&members, &edges, &records, &entity_map, &config).unwrap();

        assert_eq!(cluster.members, vec!["m1", "m2", "m3"]);
        assert!(cluster.quality_score > 0.0);
        assert!(cluster.id.starts_with("cluster-"));
        // m2 has edges to both m1 and m3: avg = (0.5+0.6)/2 = 0.55
        // m3 has one edge (0.6): avg = 0.6/1 = 0.6
        // m1 has one edge (0.5): avg = 0.5/1 = 0.5
        // So m3 is the centroid (highest avg)
        assert_eq!(cluster.centroid_id, "m3");
    }

    #[test]
    fn test_build_memory_cluster_too_small() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 5,
            ..Default::default()
        };
        let r1 = make_test_record("m1");
        let records: HashMap<String, &MemoryRecord> =
            [("m1".to_string(), &r1)].into_iter().collect();
        let entity_map: HashMap<String, HashSet<String>> = HashMap::new();
        let edges = vec![];
        let members = vec!["m1".to_string()];

        let cluster = build_memory_cluster(&members, &edges, &records, &entity_map, &config);
        assert!(cluster.is_none());
    }

    /// Helper: create a minimal MemoryRecord for testing.
    fn make_test_record(id: &str) -> MemoryRecord {
        use crate::types::{MemoryLayer, MemoryType};
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
        rec.memory_type = crate::types::MemoryType::Emotional;
        if let Some(v) = valence {
            rec.metadata = Some(serde_json::json!({"emotional_valence": v}));
        }
        rec
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
            make_cluster_with_quality("c1", &["a"], 0.9), // high quality, no emotion
            make_cluster_with_quality("c2", &["b"], 0.3), // low quality, high emotion
        ];
        let rec_a = make_record_with_emotion("a", None);
        let rec_b = make_record_with_emotion("b", Some(0.9));
        let members_map: HashMap<String, &MemoryRecord> =
            [("a".to_string(), &rec_a), ("b".to_string(), &rec_b)]
                .into_iter()
                .collect();
        let result = apply_emotional_modulation(clusters, &members_map, &config);
        // c2 (emotional) should come first despite lower quality
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
}
