//! Cluster discovery module for the synthesis engine.
//!
//! Uses 4 zero-LLM signals to build a weighted similarity graph:
//! 1. Hebbian link weights
//! 2. Entity overlap (Jaccard index)
//! 3. Embedding similarity (cosine)
//! 4. Temporal proximity (exponential decay)
//!
//! Community detection is performed by **Infomap** (information-theoretic
//! clustering that minimises the map equation). This replaced the previous
//! Union-Find connected-components approach which suffered from the
//! single-linkage chaining effect — one weak bridge between two unrelated
//! groups was enough to merge them.

use std::collections::{HashMap, HashSet};

use infomap_rs::{Infomap, Network};

use crate::embeddings::EmbeddingProvider;
use crate::storage::Storage;
use crate::synthesis::types::*;
use crate::types::MemoryRecord;

/// Compute pairwise signals between two memories.
#[allow(clippy::too_many_arguments)]
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

// ===========================================================================
// VP-tree for approximate nearest-neighbor search
// ===========================================================================

// ===========================================================================
// Hot path: assign new memory to nearest cluster
// ===========================================================================

/// Result of hot-assigning a new memory to the nearest cluster.
#[derive(Debug, Clone)]
pub enum HotAssignResult {
    /// Memory was assigned to an existing cluster.
    Assigned {
        cluster_id: String,
        confidence: f64,
    },
    /// No cluster was close enough; memory is pending for warm/cold recluster.
    Pending,
    /// No clusters exist yet; memory is pending.
    NoClusters,
}

/// Result of the warm recluster path.
#[derive(Debug, Clone)]
pub enum WarmReclusterResult {
    /// Nothing to do — no dirty clusters and no pending memories.
    NothingToDo,
    /// Recluster completed.
    Reclustered {
        /// Number of dirty clusters that were reclustered.
        dirty_clusters: usize,
        /// Number of pending memories that were included.
        pending_count: usize,
        /// Number of new clusters produced.
        new_clusters: usize,
    },
}

/// Cosine similarity between two vectors.
/// For L2-normalized vectors, this is equivalent to dot product.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Hot path: assign a new memory to the nearest existing cluster.
///
/// Loads all cluster centroids from storage, finds the one with highest
/// cosine similarity to the new memory's embedding, and assigns if above
/// threshold. Otherwise marks as pending for warm/cold recluster.
///
/// Complexity: O(C) where C = number of clusters (typically < 100).
pub fn assign_new_memory(
    storage: &Storage,
    memory_id: &str,
    embedding: &[f32],
    config: &ClusterDiscoveryConfig,
) -> Result<HotAssignResult, Box<dyn std::error::Error>> {
    let centroids = storage.get_cluster_centroids()?;

    if centroids.is_empty() {
        storage.add_pending_memory(memory_id)?;
        return Ok(HotAssignResult::NoClusters);
    }

    // Find nearest centroid by cosine similarity
    let threshold = config.hot_assign_threshold.unwrap_or(0.6);
    let mut best_cluster: Option<(&str, f64)> = None;

    for (cluster_id, centroid) in &centroids {
        let sim = cosine_similarity(embedding, centroid);
        if let Some((_, best_sim)) = best_cluster {
            if sim > best_sim {
                best_cluster = Some((cluster_id.as_str(), sim));
            }
        } else {
            best_cluster = Some((cluster_id.as_str(), sim));
        }
    }

    if let Some((cluster_id, sim)) = best_cluster {
        if sim >= threshold {
            storage.assign_to_cluster(memory_id, cluster_id, "hot", sim)?;
            storage.update_centroid_incremental(cluster_id, embedding)?;
            storage.mark_cluster_dirty(cluster_id)?;
            Ok(HotAssignResult::Assigned {
                cluster_id: cluster_id.to_string(),
                confidence: sim,
            })
        } else {
            storage.add_pending_memory(memory_id)?;
            Ok(HotAssignResult::Pending)
        }
    } else {
        storage.add_pending_memory(memory_id)?;
        Ok(HotAssignResult::Pending)
    }
}

// ===========================================================================
// Warm path: recluster dirty clusters + pending memories
// ===========================================================================

/// Compute the mean embedding vector for a set of memory IDs.
/// Skips memories without embeddings. Returns None if no embeddings found.
pub(crate) fn compute_centroid_embedding(storage: &Storage, member_ids: &[String]) -> Option<Vec<f32>> {
    let mut sum: Vec<f64> = Vec::new();
    let mut count = 0usize;

    for mid in member_ids {
        if let Ok(Some(emb)) = storage.get_embedding_for_memory(mid) {
            if sum.is_empty() {
                sum = vec![0.0f64; emb.len()];
            }
            if emb.len() == sum.len() {
                for (s, e) in sum.iter_mut().zip(emb.iter()) {
                    *s += *e as f64;
                }
                count += 1;
            }
        }
    }

    if count == 0 {
        return None;
    }

    Some(sum.iter().map(|s| (*s / count as f64) as f32).collect())
}

/// Warm path: recluster dirty clusters and assign pending memories.
///
/// Collects all members of dirty clusters + pending memory IDs,
/// runs a local `discover_clusters_subset()` on just those memories,
/// replaces the old cluster assignments with new ones, and clears
/// dirty/pending flags.
///
/// Complexity: O(m log m) where m = dirty members + pending, typically m << n.
pub fn recluster_dirty(
    storage: &Storage,
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<WarmReclusterResult, Box<dyn std::error::Error>> {
    // 1. Get dirty cluster IDs + pending memory IDs
    let dirty_ids = storage.get_dirty_cluster_ids()?;
    let pending_ids = storage.get_pending_memory_ids()?;

    if dirty_ids.is_empty() && pending_ids.is_empty() {
        return Ok(WarmReclusterResult::NothingToDo);
    }

    let pending_count = pending_ids.len();

    // 2. Collect all involved memory IDs
    let mut involved_ids: Vec<String> = Vec::new();
    for cid in &dirty_ids {
        let members = storage.get_cluster_members(cid)?;
        involved_ids.extend(members);
    }
    involved_ids.extend(pending_ids);
    // Deduplicate (a pending memory might also be in a dirty cluster)
    involved_ids.sort();
    involved_ids.dedup();

    // 3. Run local discover_clusters on the subset
    let local_clusters = discover_clusters_subset(
        storage,
        &involved_ids,
        config,
        embedding_model,
    )?;

    // 4. Convert MemoryCluster → (cluster_id, member_ids, centroid_embedding)
    //    for replace_clusters storage API
    let new_cluster_data: Vec<(String, Vec<String>, Vec<f32>)> = local_clusters
        .iter()
        .filter_map(|mc| {
            let centroid = compute_centroid_embedding(storage, &mc.members)?;
            Some((mc.id.clone(), mc.members.clone(), centroid))
        })
        .collect();

    let new_clusters_count = new_cluster_data.len();

    // 5. Replace old clusters with new ones + clear dirty/pending
    storage.replace_clusters(&dirty_ids, &new_cluster_data)?;
    storage.clear_pending_and_dirty()?;

    Ok(WarmReclusterResult::Reclustered {
        dirty_clusters: dirty_ids.len(),
        pending_count,
        new_clusters: new_clusters_count,
    })
}

// ===========================================================================
// VP-tree for approximate nearest-neighbor search
// ===========================================================================

/// L2 (Euclidean) distance between two vectors.
fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Node in a Vantage-Point Tree.
struct VpNode {
    point_idx: usize,      // index into VpTree::points
    threshold: f32,        // median distance (split boundary)
    left: Option<usize>,   // index into VpTree::nodes
    right: Option<usize>,  // index into VpTree::nodes
}

/// Vantage-Point Tree for nearest-neighbor search on L2-normalized embeddings.
/// Distance metric: L2 (equivalent ranking to cosine for normalized vectors).
struct VpTree {
    nodes: Vec<VpNode>,
    points: Vec<(usize, Vec<f32>)>, // (original_index, embedding)
}

impl VpTree {
    /// Build a VP-tree from a set of points. O(n log n).
    fn build(points: &[(usize, &[f32])]) -> VpTree {
        let owned_points: Vec<(usize, Vec<f32>)> = points
            .iter()
            .map(|(idx, emb)| (*idx, emb.to_vec()))
            .collect();
        let mut tree = VpTree {
            nodes: Vec::new(),
            points: owned_points,
        };
        if tree.points.is_empty() {
            return tree;
        }
        let indices: Vec<usize> = (0..tree.points.len()).collect();
        tree.build_recursive(&indices);
        tree
    }

    fn build_recursive(&mut self, indices: &[usize]) -> Option<usize> {
        if indices.is_empty() {
            return None;
        }
        // Pick first element as vantage point
        let vp_idx = indices[0];
        let rest = &indices[1..];
        if rest.is_empty() {
            let node_idx = self.nodes.len();
            self.nodes.push(VpNode {
                point_idx: vp_idx,
                threshold: 0.0,
                left: None,
                right: None,
            });
            return Some(node_idx);
        }

        // Compute distances from vantage point to all others
        let vp_emb = self.points[vp_idx].1.clone();
        let mut dists: Vec<(usize, f32)> = rest
            .iter()
            .map(|&i| (i, l2_distance(&vp_emb, &self.points[i].1)))
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Median distance = threshold
        let median_pos = dists.len() / 2;
        let threshold = dists[median_pos].1;

        // Split: left = dist <= threshold, right = dist > threshold
        let left_indices: Vec<usize> = dists[..=median_pos].iter().map(|(i, _)| *i).collect();
        let right_indices: Vec<usize> = dists[median_pos + 1..].iter().map(|(i, _)| *i).collect();

        let node_idx = self.nodes.len();
        self.nodes.push(VpNode {
            point_idx: vp_idx,
            threshold,
            left: None,
            right: None,
        });

        let left = self.build_recursive(&left_indices);
        let right = self.build_recursive(&right_indices);
        self.nodes[node_idx].left = left;
        self.nodes[node_idx].right = right;

        Some(node_idx)
    }

}

impl VpTree {
    fn search_node(
        &self,
        node_idx: usize,
        query: &[f32],
        query_orig_idx: usize,
        k: usize,
        heap: &mut Vec<(f32, usize)>, // manually managed max-heap
    ) {
        let node = &self.nodes[node_idx];
        let candidate = &self.points[node.point_idx];
        let d = l2_distance(query, &candidate.1);

        // Skip self
        if candidate.0 != query_orig_idx {
            if heap.len() < k {
                heap.push((d, candidate.0));
                // Bubble up to maintain max-heap
                let mut i = heap.len() - 1;
                while i > 0 {
                    let parent = (i - 1) / 2;
                    if heap[i].0 > heap[parent].0 {
                        heap.swap(i, parent);
                        i = parent;
                    } else {
                        break;
                    }
                }
            } else if d < heap[0].0 {
                // Replace max element
                heap[0] = (d, candidate.0);
                // Sift down
                let mut i = 0;
                loop {
                    let left = 2 * i + 1;
                    let right = 2 * i + 2;
                    let mut largest = i;
                    if left < heap.len() && heap[left].0 > heap[largest].0 {
                        largest = left;
                    }
                    if right < heap.len() && heap[right].0 > heap[largest].0 {
                        largest = right;
                    }
                    if largest != i {
                        heap.swap(i, largest);
                        i = largest;
                    } else {
                        break;
                    }
                }
            }
        }

        if d <= node.threshold {
            // Query is inside: search left (closer) first
            if let Some(left) = node.left {
                self.search_node(left, query, query_orig_idx, k, heap);
            }
            // Search right if it could contain closer points
            let tau = if heap.len() < k { f32::INFINITY } else { heap[0].0 };
            if d + tau > node.threshold {
                if let Some(right) = node.right {
                    self.search_node(right, query, query_orig_idx, k, heap);
                }
            }
        } else {
            // Query is outside: search right (closer) first
            if let Some(right) = node.right {
                self.search_node(right, query, query_orig_idx, k, heap);
            }
            // Search left if it could contain closer points
            let tau = if heap.len() < k { f32::INFINITY } else { heap[0].0 };
            if d - tau <= node.threshold {
                if let Some(left) = node.left {
                    self.search_node(left, query, query_orig_idx, k, heap);
                }
            }
        }
    }

    fn query_k_nearest_impl(&self, query_idx: usize, k: usize) -> Vec<(usize, f32)> {
        if self.nodes.is_empty() || k == 0 {
            return Vec::new();
        }

        let query_internal = self
            .points
            .iter()
            .position(|(orig, _)| *orig == query_idx);
        let query_internal = match query_internal {
            Some(i) => i,
            None => return Vec::new(),
        };
        let query_emb = self.points[query_internal].1.clone();

        let mut heap: Vec<(f32, usize)> = Vec::with_capacity(k);
        self.search_node(0, &query_emb, query_idx, k, &mut heap);

        let mut result: Vec<(usize, f32)> = heap.into_iter().map(|(d, idx)| (idx, d)).collect();
        result.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        result
    }
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

    discover_clusters_inner(storage, &candidates, config, embedding_model)
}

/// Discover clusters from a subset of memories specified by ID.
///
/// Like [`discover_clusters`] but operates only on the given memory IDs
/// instead of loading all memories from storage. Used by the warm
/// (incremental) recluster path.
pub fn discover_clusters_subset(
    storage: &Storage,
    memory_ids: &[String],
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<Vec<MemoryCluster>, Box<dyn std::error::Error>> {
    // Load only the specified memories
    let records = storage.get_memories_by_ids(memory_ids)?;
    let candidates: Vec<&MemoryRecord> = records
        .iter()
        .filter(|m| !is_synthesis_output(m))
        .collect();

    if candidates.len() < config.min_cluster_size {
        return Ok(Vec::new());
    }

    discover_clusters_inner(storage, &candidates, config, embedding_model)
}

/// Shared implementation for cluster discovery (Steps 2-6).
///
/// Takes pre-filtered candidate memories and runs the full pipeline:
/// signal map construction, candidate pair generation, composite scoring,
/// Infomap community detection, and cluster building.
fn discover_clusters_inner(
    storage: &Storage,
    candidates: &[&MemoryRecord],
    config: &ClusterDiscoveryConfig,
    embedding_model: Option<&str>,
) -> Result<Vec<MemoryCluster>, Box<dyn std::error::Error>> {
    let candidate_ids: HashSet<&str> = candidates.iter().map(|m| m.id.as_str()).collect();
    let records: HashMap<String, &MemoryRecord> =
        candidates.iter().map(|m| (m.id.clone(), *m)).collect();

    // Step 2: Build signal maps (pre-compute for O(n) instead of O(n²) queries)
    // Hebbian: query all links involving candidates
    let mut hebbian_map: HashMap<(String, String), f64> = HashMap::new();
    for m in candidates {
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
    for m in candidates {
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

    // From embedding ANN (VP-tree k-nearest neighbors)
    if !embedding_map.is_empty() {
        // Build index mapping for VP-tree
        let embedding_ids: Vec<String> = embedding_map.keys().cloned().collect();
        let points: Vec<(usize, &[f32])> = embedding_ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| embedding_map.get(id).map(|emb| (i, emb.as_slice())))
            .collect();

        if points.len() >= 2 {
            let vp_tree = VpTree::build(&points);
            let ann_k = config.max_neighbors_per_node.unwrap_or_else(|| {
                let adaptive = (points.len() as f64).sqrt().round() as usize;
                adaptive.clamp(5, 30)
            });

            for (i, id) in embedding_ids.iter().enumerate() {
                if candidate_ids.contains(id.as_str()) {
                    let neighbors = vp_tree.query_k_nearest_impl(i, ann_k);
                    for (j, _dist) in &neighbors {
                        let neighbor_id = &embedding_ids[*j];
                        if candidate_ids.contains(neighbor_id.as_str()) {
                            let pair = if id < neighbor_id {
                                (id.clone(), neighbor_id.clone())
                            } else {
                                (neighbor_id.clone(), id.clone())
                            };
                            candidate_pairs.insert(pair);
                        }
                    }
                }
            }

            log::debug!(
                "ANN pairs added: k={}, embedding_count={}",
                ann_k,
                points.len()
            );
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

    // Step 5: Infomap community detection
    //
    // Sparsify the edge list before feeding to Infomap: keep only the top-K
    // strongest edges per node. This reduces Infomap runtime from O(E * trials)
    // to manageable levels for large graphs while preserving local structure.
    //
    // Adaptive default: clamp(sqrt(n), 5, 30) — scales with graph size.
    // Small graphs keep most edges; large graphs get aggressive pruning.
    let n = candidate_ids.len();
    let max_neighbors = config.max_neighbors_per_node.unwrap_or_else(|| {
        let adaptive = (n as f64).sqrt().round() as usize;
        adaptive.clamp(5, 30)
    });
    log::debug!(
        "cluster sparsification: nodes={}, edges_before={}, max_neighbors={} ({})",
        n,
        edges.len(),
        max_neighbors,
        if config.max_neighbors_per_node.is_some() { "manual" } else { "adaptive" },
    );
    let edges = sparsify_edges(edges, max_neighbors);
    let clusters = infomap_communities(&edges, &candidate_ids, config);


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

/// Sparsify edge list by keeping only the top-K strongest edges per node.
///
/// This preserves local neighborhood structure while dramatically reducing
/// the total edge count for Infomap. Each undirected edge (a,b) counts
/// towards both a's and b's quota.
fn sparsify_edges(
    mut edges: Vec<(String, String, f64)>,
    k: usize,
) -> Vec<(String, String, f64)> {
    // Sort edges by weight descending
    edges.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let mut neighbor_count: HashMap<String, usize> = HashMap::new();
    let mut kept = Vec::new();

    for (a, b, w) in edges {
        let count_a = neighbor_count.get(&a).copied().unwrap_or(0);
        let count_b = neighbor_count.get(&b).copied().unwrap_or(0);

        // Keep edge if either endpoint still has room
        if count_a < k || count_b < k {
            *neighbor_count.entry(a.clone()).or_insert(0) += 1;
            *neighbor_count.entry(b.clone()).or_insert(0) += 1;
            kept.push((a, b, w));
        }
    }

    kept
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

/// Find communities in the weighted edge graph using Infomap.
///
/// Infomap minimises the map equation (information-theoretic objective) to
/// discover natural community structure in the weighted graph.  Unlike
/// Union-Find connected-components, Infomap can split a single connected
/// component into multiple communities when there are clear bottlenecks
/// in the random-walk flow.
///
/// Returns Vec of member-ID vectors, filtered by min/max cluster size.
fn infomap_communities(
    edges: &[(String, String, f64)],
    all_ids: &HashSet<&str>,
    config: &ClusterDiscoveryConfig,
) -> Vec<Vec<String>> {
    if edges.is_empty() {
        return Vec::new();
    }

    // Map string IDs → contiguous indices for Infomap.
    let id_list: Vec<String> = all_ids.iter().map(|s| s.to_string()).collect();
    let id_to_idx: HashMap<&str, usize> = id_list
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    let mut network = Network::with_capacity(id_list.len());
    network.ensure_capacity(id_list.len());

    for (a, b, weight) in edges {
        if let (Some(&ia), Some(&ib)) = (id_to_idx.get(a.as_str()), id_to_idx.get(b.as_str())) {
            // Undirected: add both directions so Infomap treats it symmetrically.
            network.add_edge(ia, ib, *weight);
            network.add_edge(ib, ia, *weight);
        }
    }

    // Adaptive Infomap parameters:
    // - trials: 1 for sparse graphs (density < 5), 3 for dense graphs
    // - hierarchical: true for large graphs (>2000 nodes), false otherwise
    // Manual config overrides adaptive defaults.
    let edge_density = if id_list.is_empty() {
        0.0
    } else {
        edges.len() as f64 / id_list.len() as f64
    };
    let trials = config.infomap_trials.unwrap_or({
        if edge_density < 5.0 { 1 } else { 3 }
    });
    let hierarchical = config.infomap_hierarchical.unwrap_or({
        id_list.len() > 2000
    });

    log::debug!(
        "infomap params: nodes={}, edges={}, density={:.1}, trials={} ({}), hierarchical={} ({})",
        id_list.len(),
        edges.len(),
        edge_density,
        trials,
        if config.infomap_trials.is_some() { "manual" } else { "adaptive" },
        hierarchical,
        if config.infomap_hierarchical.is_some() { "manual" } else { "adaptive" },
    );
    let result = Infomap::new(&network)
        .seed(42)
        .num_trials(trials)
        .hierarchical(hierarchical)
        .run();

    // Group node indices by module assignment.
    let mut modules: HashMap<usize, Vec<usize>> = HashMap::new();
    for (node_idx, &module_id) in result.assignments.iter().enumerate() {
        if node_idx < id_list.len() {
            modules.entry(module_id).or_default().push(node_idx);
        }
    }

    // Convert back to string IDs, filter by size, handle splitting.
    let mut groups: Vec<Vec<String>> = Vec::new();
    for (_, member_indices) in modules {
        if member_indices.len() < config.min_cluster_size {
            continue;
        }

        let mut members: Vec<String> = member_indices
            .iter()
            .map(|&i| id_list[i].clone())
            .collect();
        members.sort();

        if members.len() > config.max_cluster_size {
            // Split oversized communities: take top max_cluster_size members.
            // A better approach would recursively sub-cluster, but matches
            // the previous behaviour for now.
            members.truncate(config.max_cluster_size);
        }

        groups.push(members);
    }

    groups
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
    fn test_infomap_communities_simple_triangle() {
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

        let components = infomap_communities(&edges, &ids, &config);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn test_infomap_communities_two_clusters() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 2,
            max_cluster_size: 15,
            ..Default::default()
        };
        // Two tight triangles with no bridge → Infomap finds 2 communities
        let edges = vec![
            ("a".to_string(), "b".to_string(), 1.0),
            ("b".to_string(), "c".to_string(), 1.0),
            ("a".to_string(), "c".to_string(), 1.0),
            ("d".to_string(), "e".to_string(), 1.0),
            ("e".to_string(), "f".to_string(), 1.0),
            ("d".to_string(), "f".to_string(), 1.0),
        ];
        let ids: HashSet<&str> = ["a", "b", "c", "d", "e", "f"].into_iter().collect();

        let mut components = infomap_communities(&edges, &ids, &config);
        components.sort_by(|a, b| a[0].cmp(&b[0]));
        assert_eq!(components.len(), 2);
        assert_eq!(components[0], vec!["a", "b", "c"]);
        assert_eq!(components[1], vec!["d", "e", "f"]);
    }

    #[test]
    fn test_infomap_communities_filters_small() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 3,
            max_cluster_size: 15,
            ..Default::default()
        };
        // Only 2 connected nodes — below min_cluster_size of 3
        let edges = vec![("a".to_string(), "b".to_string(), 0.5)];
        let ids: HashSet<&str> = ["a", "b", "c"].into_iter().collect();

        let components = infomap_communities(&edges, &ids, &config);
        // Both the pair {a,b} and the singleton {c} are < 3
        assert_eq!(components.len(), 0);
    }

    #[test]
    fn test_infomap_communities_truncates_large() {
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 2,
            max_cluster_size: 3,
            ..Default::default()
        };
        // One tight cluster of 5 nodes
        let edges = vec![
            ("a".to_string(), "b".to_string(), 1.0),
            ("b".to_string(), "c".to_string(), 1.0),
            ("c".to_string(), "d".to_string(), 1.0),
            ("d".to_string(), "e".to_string(), 1.0),
            ("a".to_string(), "c".to_string(), 1.0),
            ("a".to_string(), "d".to_string(), 1.0),
            ("a".to_string(), "e".to_string(), 1.0),
            ("b".to_string(), "d".to_string(), 1.0),
            ("b".to_string(), "e".to_string(), 1.0),
            ("c".to_string(), "e".to_string(), 1.0),
        ];
        let ids: HashSet<&str> = ["a", "b", "c", "d", "e"].into_iter().collect();

        let components = infomap_communities(&edges, &ids, &config);
        // Should produce at least one community, truncated to max_cluster_size=3
        for component in &components {
            assert!(component.len() <= 3);
        }
    }

    #[test]
    fn test_infomap_no_chaining_effect() {
        // Two tight clusters connected by a single weak edge.
        // Union-Find would merge them. Infomap should keep them separate.
        let config = ClusterDiscoveryConfig {
            min_cluster_size: 3,
            max_cluster_size: 15,
            ..Default::default()
        };
        let edges = vec![
            // Cluster 1: tight triangle
            ("a".to_string(), "b".to_string(), 1.0),
            ("b".to_string(), "c".to_string(), 1.0),
            ("a".to_string(), "c".to_string(), 1.0),
            // Cluster 2: tight triangle
            ("d".to_string(), "e".to_string(), 1.0),
            ("e".to_string(), "f".to_string(), 1.0),
            ("d".to_string(), "f".to_string(), 1.0),
            // Weak bridge
            ("c".to_string(), "d".to_string(), 0.05),
        ];
        let ids: HashSet<&str> = ["a", "b", "c", "d", "e", "f"].into_iter().collect();

        let components = infomap_communities(&edges, &ids, &config);

        // Should find 2 separate communities, not 1 merged blob.
        assert_eq!(
            components.len(), 2,
            "Expected 2 communities (Infomap should split at weak bridge), got {}",
            components.len()
        );
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
            superseded_by: None,
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

    // ===================================================================
    // VP-tree tests
    // ===================================================================

    #[test]
    fn test_l2_distance() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let d = l2_distance(&a, &b);
        assert!((d - std::f32::consts::SQRT_2).abs() < 1e-6);

        // Same point → 0
        assert!((l2_distance(&a, &a)).abs() < 1e-9);

        // Known distance
        let c = vec![3.0_f32, 4.0, 0.0];
        let origin = vec![0.0_f32, 0.0, 0.0];
        assert!((l2_distance(&c, &origin) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_vp_tree_build_and_query() {
        // 10 known 3D points
        let raw: Vec<Vec<f32>> = vec![
            vec![0.0, 0.0, 0.0], // 0
            vec![1.0, 0.0, 0.0], // 1
            vec![0.0, 1.0, 0.0], // 2
            vec![0.0, 0.0, 1.0], // 3
            vec![1.0, 1.0, 0.0], // 4
            vec![1.0, 0.0, 1.0], // 5
            vec![0.0, 1.0, 1.0], // 6
            vec![1.0, 1.0, 1.0], // 7
            vec![0.5, 0.5, 0.5], // 8  — center
            vec![0.1, 0.1, 0.1], // 9  — near origin
        ];
        let points: Vec<(usize, &[f32])> =
            raw.iter().enumerate().map(|(i, v)| (i, v.as_slice())).collect();
        let tree = VpTree::build(&points);

        // Query point 0 (origin), k=3
        // Distances from origin:
        //   9: sqrt(0.03) ≈ 0.173
        //   8: sqrt(0.75) ≈ 0.866
        //   1,2,3: 1.0 each
        // So k=3 nearest = [9, 8, one of {1,2,3}]
        let result = tree.query_k_nearest_impl(0, 3);
        assert_eq!(result.len(), 3);
        // First neighbor must be point 9
        assert_eq!(result[0].0, 9);
        // Second must be point 8
        assert_eq!(result[1].0, 8);
        // Third should be from {1, 2, 3} (all at distance 1.0)
        assert!([1, 2, 3].contains(&result[2].0));
    }

    #[test]
    fn test_vp_tree_single_point() {
        let raw = vec![vec![1.0_f32, 2.0, 3.0]];
        let points: Vec<(usize, &[f32])> = vec![(0, raw[0].as_slice())];
        let tree = VpTree::build(&points);
        let result = tree.query_k_nearest_impl(0, 3);
        // Only one point, and it's the query → empty
        assert!(result.is_empty());
    }

    #[test]
    fn test_vp_tree_two_points() {
        let raw = vec![vec![0.0_f32, 0.0], vec![1.0_f32, 0.0]];
        let points: Vec<(usize, &[f32])> = vec![(0, raw[0].as_slice()), (1, raw[1].as_slice())];
        let tree = VpTree::build(&points);

        let result = tree.query_k_nearest_impl(0, 5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 1);
        assert!((result[0].1 - 1.0).abs() < 1e-6);

        let result = tree.query_k_nearest_impl(1, 5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 0);
    }

    // =======================================================================
    // Cosine similarity tests
    // =======================================================================

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-9, "identical vectors should have similarity 1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-9, "orthogonal vectors should have similarity 0.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0_f32, 2.0, 3.0];
        let b = vec![-1.0_f32, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-9, "opposite vectors should have similarity -1.0, got {}", sim);
    }

    // =======================================================================
    // Hot path assign_new_memory tests
    // =======================================================================

    use crate::storage::Storage;

    fn test_storage() -> Storage {
        let s = Storage::new(":memory:").unwrap();
        s.init_cluster_tables().unwrap();
        s
    }

    #[test]
    fn test_assign_new_memory_no_clusters() {
        let storage = test_storage();
        let config = ClusterDiscoveryConfig::default();
        let embedding = vec![1.0_f32, 0.0, 0.0];

        let result = assign_new_memory(&storage, "mem-1", &embedding, &config).unwrap();
        match result {
            HotAssignResult::NoClusters => {} // expected
            other => panic!("expected NoClusters, got {:?}", other),
        }
    }

    #[test]
    fn test_assign_new_memory_assigned() {
        let storage = test_storage();
        let config = ClusterDiscoveryConfig::default();

        // Seed a centroid: [1.0, 0.0, 0.0]
        let centroid = vec![1.0_f32, 0.0, 0.0];
        storage.update_centroid_incremental("cluster-a", &centroid).unwrap();

        // New memory very similar to the centroid
        let embedding = vec![0.9_f32, 0.1, 0.0];
        let result = assign_new_memory(&storage, "mem-1", &embedding, &config).unwrap();
        match result {
            HotAssignResult::Assigned { cluster_id, confidence } => {
                assert_eq!(cluster_id, "cluster-a");
                assert!(confidence >= 0.6, "confidence {} should be >= 0.6", confidence);
            }
            other => panic!("expected Assigned, got {:?}", other),
        }
    }

    #[test]
    fn test_assign_new_memory_pending() {
        let storage = test_storage();
        let config = ClusterDiscoveryConfig::default();

        // Seed a centroid: [1.0, 0.0, 0.0]
        let centroid = vec![1.0_f32, 0.0, 0.0];
        storage.update_centroid_incremental("cluster-a", &centroid).unwrap();

        // New memory nearly orthogonal → below threshold
        let embedding = vec![0.0_f32, 1.0, 0.0];
        let result = assign_new_memory(&storage, "mem-2", &embedding, &config).unwrap();
        match result {
            HotAssignResult::Pending => {} // expected
            other => panic!("expected Pending, got {:?}", other),
        }
    }

    // =======================================================================
    // Warm path recluster_dirty tests
    // =======================================================================

    #[test]
    fn test_recluster_dirty_nothing_to_do() {
        let storage = test_storage();
        let config = ClusterDiscoveryConfig::default();
        let result = recluster_dirty(&storage, &config, None).unwrap();
        match result {
            WarmReclusterResult::NothingToDo => {} // expected
            other => panic!("expected NothingToDo, got {:?}", other),
        }
    }

    #[test]
    fn test_recluster_dirty_with_pending() {
        let mut storage = test_storage();
        let config = ClusterDiscoveryConfig {
            cluster_threshold: 0.1,
            min_cluster_size: 2,
            min_importance: 0.0,
            ..Default::default()
        };

        // Add 4 real MemoryRecord entries
        let ids: Vec<String> = (0..4).map(|i| format!("mem-{}", i)).collect();
        for id in &ids {
            let mut rec = make_test_record(id);
            rec.importance = 0.5;
            storage.add(&rec, "default").unwrap();
        }

        // Store embeddings — two pairs of similar vectors
        let embeddings: Vec<Vec<f32>> = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![0.0, 0.1, 0.9],
        ];
        for (id, emb) in ids.iter().zip(embeddings.iter()) {
            storage.store_embedding(id, emb, "test/model", emb.len()).unwrap();
        }

        // Add all as pending
        for id in &ids {
            storage.add_pending_memory(id).unwrap();
        }

        let result = recluster_dirty(&storage, &config, Some("test/model")).unwrap();
        match result {
            WarmReclusterResult::Reclustered { pending_count, .. } => {
                assert_eq!(pending_count, 4);
            }
            other => panic!("expected Reclustered, got {:?}", other),
        }

        // Verify pending is cleared
        let remaining = storage.get_pending_memory_ids().unwrap();
        assert!(remaining.is_empty(), "pending should be cleared after recluster");
    }

    #[test]
    fn test_compute_centroid_embedding() {
        let mut storage = test_storage();

        // Add two memories with embeddings
        let r1 = make_test_record("c-1");
        let r2 = make_test_record("c-2");
        storage.add(&r1, "default").unwrap();
        storage.add(&r2, "default").unwrap();

        let emb1 = vec![1.0_f32, 0.0, 0.0];
        let emb2 = vec![0.0_f32, 1.0, 0.0];
        storage.store_embedding("c-1", &emb1, "test/model", 3).unwrap();
        storage.store_embedding("c-2", &emb2, "test/model", 3).unwrap();

        let ids = vec!["c-1".to_string(), "c-2".to_string()];
        let centroid = compute_centroid_embedding(&storage, &ids).unwrap();
        // Mean of [1,0,0] and [0,1,0] = [0.5, 0.5, 0.0]
        assert!((centroid[0] - 0.5).abs() < 1e-6);
        assert!((centroid[1] - 0.5).abs() < 1e-6);
        assert!((centroid[2] - 0.0).abs() < 1e-6);

        // No embeddings → None
        let no_ids = vec!["nonexistent".to_string()];
        let result = compute_centroid_embedding(&storage, &no_ids);
        assert!(result.is_none());
    }

    #[test]
    fn test_vp_tree_excludes_self() {
        let raw = vec![
            vec![0.0_f32, 0.0],
            vec![1.0, 0.0],
            vec![2.0, 0.0],
            vec![3.0, 0.0],
        ];
        let points: Vec<(usize, &[f32])> =
            raw.iter().enumerate().map(|(i, v)| (i, v.as_slice())).collect();
        let tree = VpTree::build(&points);

        for i in 0..4 {
            let result = tree.query_k_nearest_impl(i, 10);
            // Must not contain self
            assert!(
                !result.iter().any(|(idx, _)| *idx == i),
                "query_k_nearest for point {} returned self in results",
                i
            );
            // Should have exactly 3 neighbors (all others)
            assert_eq!(result.len(), 3);
        }
    }
}
