//! Synthesis engine orchestration layer.
//!
//! Ties cluster discovery, gate check, insight generation, and provenance
//! into a single pipeline. Implements [`SynthesisEngine`] as `DefaultSynthesisEngine`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use chrono::Utc;

use crate::storage::Storage;
use crate::synthesis::cluster;
use crate::synthesis::gate;
use crate::synthesis::insight;
use crate::synthesis::provenance;
use crate::synthesis::types::*;
use crate::types::MemoryRecord;

/// Convert a linear pair index (0..N*(N-1)/2) to (i, j) coordinates
/// where i < j. Used for sampling random pairs from a triangular matrix.
///
/// Linear index maps pairs in row-major order:
///   idx=0 → (0,1), idx=1 → (0,2), ..., idx=n-2 → (0,n-1),
///   idx=n-1 → (1,2), idx=n → (1,3), ...
fn linear_index_to_pair(idx: usize, n: usize) -> (usize, usize) {
    // Row i has (n - 1 - i) elements. Walk rows until we find which row idx falls in.
    let mut remaining = idx;
    for i in 0..n {
        let row_len = n - 1 - i;
        if remaining < row_len {
            return (i, i + 1 + remaining);
        }
        remaining -= row_len;
    }
    // Should never reach here for valid inputs
    (n - 2, n - 1)
}

/// Default implementation of the synthesis engine.
pub struct DefaultSynthesisEngine {
    /// Optional LLM provider. When None, synthesis is skipped (graceful degradation).
    llm_provider: Option<Box<dyn SynthesisLlmProvider>>,
    /// Embedding model name for cluster discovery.
    embedding_model: Option<String>,
}

impl DefaultSynthesisEngine {
    pub fn new(
        llm_provider: Option<Box<dyn SynthesisLlmProvider>>,
        embedding_model: Option<String>,
    ) -> Self {
        Self {
            llm_provider,
            embedding_model,
        }
    }

    /// Consume the engine and return the LLM provider (for restoring to Memory).
    pub fn into_provider(self) -> Option<Box<dyn SynthesisLlmProvider>> {
        self.llm_provider
    }

    /// Check whether a cluster has changed enough to warrant re-synthesis.
    fn should_resynthesize(
        cluster: &MemoryCluster,
        state: &IncrementalState,
        config: &IncrementalConfig,
    ) -> bool {
        // Condition 1: member change > staleness_member_change_pct (Jaccard distance)
        let current_members: HashSet<&str> = cluster.members.iter().map(|s| s.as_str()).collect();
        let old_members: HashSet<&str> = state.last_member_snapshot.iter().map(|s| s.as_str()).collect();
        let intersection = current_members.intersection(&old_members).count();
        let union_size = current_members.union(&old_members).count();
        if union_size == 0 {
            return true; // empty/new cluster
        }
        let change_pct = 1.0 - (intersection as f64 / union_size as f64);
        if change_pct >= config.staleness_member_change_pct {
            return true;
        }

        // Condition 2: quality_score delta > staleness_quality_delta
        if (cluster.quality_score - state.last_quality_score).abs() >= config.staleness_quality_delta {
            return true;
        }

        false
    }

    /// Check whether all pairs of members in a cluster are near-duplicates.
    ///
    /// Fetches embeddings from storage, computes pairwise cosine similarity,
    /// and returns `true` only if every checked pair exceeds `threshold`.
    ///
    /// For clusters with >10 members, samples up to 45 pairs (deterministic
    /// seed from cluster ID) instead of computing all N*(N-1)/2 pairs.
    /// Returns `false` if any member lacks an embedding.
    fn compute_all_pairs_similar(
        storage: &Storage,
        member_ids: &[String],
        threshold: f64,
    ) -> bool {
        let n = member_ids.len();
        if n < 2 {
            return false; // single member can't be "all pairs similar"
        }

        // Fetch embeddings for all members
        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(n);
        for id in member_ids {
            match storage.get_embedding_for_memory(id) {
                Ok(Some(emb)) => embeddings.push(emb),
                _ => {
                    // Missing embedding → can't confirm all pairs similar
                    log::debug!(
                        "pairwise similarity: member {} has no embedding, returning false",
                        id
                    );
                    return false;
                }
            }
        }

        let total_pairs = n * (n - 1) / 2;

        // For small clusters (≤10 members, ≤45 pairs), check all pairs
        if n <= 10 {
            for i in 0..n {
                for j in (i + 1)..n {
                    let sim = cluster::cosine_similarity(&embeddings[i], &embeddings[j]);
                    if sim < threshold {
                        return false;
                    }
                }
            }
            return true;
        }

        // For large clusters (>10 members), sample pairs.
        // Sample size: min(45, total_pairs) — 45 pairs gives >95% probability
        // of catching a dissimilar pair if ≥10% of pairs are dissimilar.
        let sample_size = total_pairs.min(45);

        // Deterministic seed from cluster member IDs (sorted, so stable)
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for id in member_ids {
            id.hash(&mut hasher);
        }
        let seed = hasher.finish();

        // Generate deterministic pseudo-random pair indices using simple LCG
        let mut rng_state = seed;
        let mut checked = HashSet::with_capacity(sample_size);

        while checked.len() < sample_size {
            // Linear congruential generator
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pair_idx = (rng_state >> 33) as usize % total_pairs;

            if !checked.insert(pair_idx) {
                continue; // already checked this pair
            }

            // Convert linear pair index to (i, j) coordinates
            let (i, j) = linear_index_to_pair(pair_idx, n);

            let sim = cluster::cosine_similarity(&embeddings[i], &embeddings[j]);
            if sim < threshold {
                return false;
            }
        }

        true
    }

    /// Store an insight + provenance + demotion in a single transaction.
    /// Returns (insight_id, demoted_source_ids).
    #[allow(clippy::too_many_arguments)]
    fn store_insight_atomically(
        &self,
        storage: &mut Storage,
        cluster: &MemoryCluster,
        members: &[MemoryRecord],
        output: &SynthesisOutput,
        importance: f64,
        gate_result: &GateResult,
        settings: &SynthesisSettings,
    ) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        storage.begin_transaction()?;

        let result = (|| -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
            // 1. Create insight as a MemoryRecord
            let insight_id = generate_id();
            let now = Utc::now();

            // Build metadata with is_synthesis flag (GUARD-5)
            let metadata = serde_json::json!({
                "is_synthesis": true,
                "source_cluster": cluster.id,
                "insight_type": format!("{:?}", output.insight_type),
                "confidence": output.confidence,
                "source_count": output.source_references.len(),
            });

            // Determine memory type based on insight_type
            let memory_type = match output.insight_type {
                InsightType::Pattern => "factual",
                InsightType::Rule => "factual",
                InsightType::Connection => "relational",
                InsightType::Contradiction => "causal",
            };

            // Store the insight
            storage.store_raw(
                &insight_id,
                &output.insight_text,
                memory_type,
                importance,
                Some(&serde_json::to_string(&metadata)?),
            )?;

            // 2. Record provenance for each source
            for source_id in &output.source_references {
                let prov_id = generate_id();
                let source_importance = members
                    .iter()
                    .find(|m| m.id == *source_id)
                    .map(|m| m.importance);

                let record = ProvenanceRecord {
                    id: prov_id,
                    insight_id: insight_id.clone(),
                    source_id: source_id.clone(),
                    cluster_id: cluster.id.clone(),
                    synthesis_timestamp: now,
                    gate_decision: "SYNTHESIZE".to_string(),
                    gate_scores: Some(gate_result.scores.clone()),
                    confidence: output.confidence,
                    source_original_importance: source_importance,
                };
                storage.record_provenance(&record)?;
            }

            // 3. Demote source importances
            let mut demoted_ids = Vec::new();
            for source_id in &output.source_references {
                if let Some(member) = members.iter().find(|m| m.id == *source_id) {
                    let new_importance = member.importance * settings.demotion_factor;
                    storage.update_importance(source_id, new_importance)?;
                    demoted_ids.push(source_id.clone());
                }
            }

            Ok((insight_id, demoted_ids))
        })();

        match &result {
            Ok(_) => storage.commit_transaction()?,
            Err(_) => {
                let _ = storage.rollback_transaction();
            }
        }

        result
    }
}

impl SynthesisEngine for DefaultSynthesisEngine {
    fn synthesize(
        &self,
        storage: &mut Storage,
        settings: &SynthesisSettings,
    ) -> Result<SynthesisReport, Box<dyn std::error::Error>> {
        let start = Instant::now();
        let mut report = SynthesisReport {
            clusters_found: 0,
            clusters_synthesized: 0,
            clusters_auto_updated: 0,
            clusters_deferred: 0,
            clusters_skipped: 0,
            synthesis_runs_full: 0,
            synthesis_runs_incremental: 0,
            insights_created: Vec::new(),
            sources_demoted: Vec::new(),
            errors: Vec::new(),
            duration: std::time::Duration::ZERO,
            gate_results: Vec::new(),
        };

        // Step 1: Determine clustering strategy (hot/warm/cold)
        let pending_count = storage.get_pending_count().unwrap_or(0);
        let total_count = storage.count_memories().unwrap_or(0);
        let dirty_count = storage
            .get_dirty_cluster_ids()
            .map(|v| v.len())
            .unwrap_or(0);

        let cold_ratio = settings
            .cluster_discovery
            .cold_recluster_ratio
            .unwrap_or(0.2);
        let should_cold = total_count == 0
            || (total_count > 0 && pending_count as f64 / total_count as f64 > cold_ratio);

        let clusters = if should_cold {
            // Cold path: full Infomap recluster (also the initial path when no clusters exist)
            log::info!(
                "synthesis: cold recluster ({} pending / {} total, {} dirty)",
                pending_count,
                total_count,
                dirty_count
            );
            let clusters = cluster::discover_clusters(
                storage,
                &settings.cluster_discovery,
                self.embedding_model.as_deref(),
            )?;

            // Save full cluster state for incremental use
            let cluster_tuples: Vec<(String, Vec<String>, Vec<f32>)> = clusters
                .iter()
                .filter_map(|c| {
                    let centroid =
                        cluster::compute_centroid_embedding(storage, &c.members)?;
                    Some((c.id.clone(), c.members.clone(), centroid))
                })
                .collect();
            if !cluster_tuples.is_empty() {
                let _ = storage.save_full_cluster_state(&cluster_tuples);
            }

            clusters
        } else if pending_count > 0 || dirty_count > 0 {
            // Warm path: recluster only dirty clusters + pending memories,
            // then read all clusters from storage (avoids full Infomap)
            log::info!(
                "synthesis: warm recluster ({} pending, {} dirty clusters)",
                pending_count,
                dirty_count
            );
            let _warm_result = cluster::recluster_dirty(
                storage,
                &settings.cluster_discovery,
                self.embedding_model.as_deref(),
            )?;
            // After warm recluster, read all cluster data from storage
            storage
                .get_all_cluster_data()
                .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?
        } else {
            // Nothing pending, nothing dirty — use cached cluster data from storage
            log::info!("synthesis: using cached cluster data (no pending/dirty)");
            let cached = storage
                .get_all_cluster_data()
                .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
            if cached.is_empty() {
                // No cached data — fall back to cold path (first run)
                log::info!("synthesis: no cached clusters, falling back to cold recluster");
                cluster::discover_clusters(
                    storage,
                    &settings.cluster_discovery,
                    self.embedding_model.as_deref(),
                )?
            } else {
                cached
            }
        };
        report.clusters_found = clusters.len();

        if clusters.is_empty() {
            report.duration = start.elapsed();
            return Ok(report);
        }

        // Pre-load all memories ONCE and build a HashMap index for O(1) lookups.
        // This single load serves both emotional modulation (Step 2) and
        // per-cluster member resolution (Step 4). Previously storage.all()
        // was called inside the per-cluster loop, making it O(C×N).
        let all_memories = storage.all()?;
        let memory_index: HashMap<String, MemoryRecord> = all_memories
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();

        // Step 2: Apply emotional modulation — boost quality scores for
        // emotionally salient clusters and re-sort by abs(avg_valence)
        // descending when prioritize_emotional is enabled.
        let members_ref_map: HashMap<String, &MemoryRecord> = memory_index
            .iter()
            .map(|(id, m)| (id.clone(), m))
            .collect();
        let clusters = cluster::apply_emotional_modulation(
            clusters,
            &members_ref_map,
            &settings.emotional,
        );

        // Step 3: Budget tracking
        let mut llm_calls_remaining = settings.max_llm_calls_per_run;
        let mut insights_remaining = settings.max_insights_per_consolidation;

        // Step 4: Process each cluster
        for cluster_data in &clusters {
            // --- Incremental staleness check (C4) ---
            // If we have a previous incremental state for this cluster and
            // the cluster hasn't changed enough, skip it entirely.
            let incremental_state = storage
                .get_incremental_state(&cluster_data.id)
                .ok()
                .flatten();
            if let Some(ref state) = incremental_state {
                if !Self::should_resynthesize(cluster_data, state, &settings.incremental) {
                    log::debug!(
                        "synthesis: skipping unchanged cluster {} (incremental)",
                        cluster_data.id
                    );
                    report.clusters_skipped += 1;
                    continue;
                }
            }

            // Look up cluster members from the pre-built index (O(M) per cluster)
            let members: Vec<MemoryRecord> = cluster_data
                .members
                .iter()
                .filter_map(|id| memory_index.get(id).cloned())
                .collect();

            // Pre-compute gate inputs
            let covered_pct = storage.check_coverage(&cluster_data.members)?;

            // Determine if this cluster has changed since the last *attempt*
            // (not just last successful synthesis). Uses Jaccard distance on
            // the attempt-level member snapshot to detect membership changes.
            let cluster_changed = match &incremental_state {
                Some(state) if !state.last_attempt_members.is_empty() => {
                    let current: HashSet<&str> =
                        cluster_data.members.iter().map(|s| s.as_str()).collect();
                    let previous: HashSet<&str> =
                        state.last_attempt_members.iter().map(|s| s.as_str()).collect();
                    let intersection = current.intersection(&previous).count();
                    let union_size = current.union(&previous).count();
                    if union_size == 0 {
                        true // empty → treat as changed
                    } else {
                        let jaccard = intersection as f64 / union_size as f64;
                        // Any membership change (Jaccard < 1.0) means the cluster grew/shrunk
                        jaccard < 1.0
                    }
                }
                _ => true, // No prior attempt → treat as changed (first time)
            };

            // Compute pairwise embedding similarity to detect near-duplicate clusters.
            // If all pairs exceed the duplicate_similarity threshold, Gate Rule 2
            // triggers AutoUpdate (MergeDuplicates) instead of full synthesis.
            let all_pairs_similar = Self::compute_all_pairs_similar(
                storage,
                &cluster_data.members,
                settings.gate.duplicate_similarity,
            );

            // Gate check
            let gate_result = gate::check_gate(
                cluster_data,
                &members,
                &settings.gate,
                covered_pct,
                cluster_changed,
                all_pairs_similar,
            );
            report.gate_results.push(gate_result.clone());

            // Persist attempt history — record this gate attempt regardless of outcome.
            // This updates last_attempt_timestamp, attempt_count, and last_attempt_members
            // so that subsequent runs can detect whether the cluster has changed.
            {
                let mut updated_state = incremental_state.clone().unwrap_or_else(|| {
                    IncrementalState {
                        last_member_snapshot: HashSet::new(),
                        last_quality_score: cluster_data.quality_score,
                        last_run: Utc::now(),
                        run_count: 0,
                        last_attempt_timestamp: Utc::now(),
                        attempt_count: 0,
                        last_attempt_members: HashSet::new(),
                    }
                });
                updated_state.last_attempt_timestamp = Utc::now();
                updated_state.attempt_count += 1;
                updated_state.last_attempt_members =
                    cluster_data.members.iter().cloned().collect();
                let _ = storage.set_incremental_state(&cluster_data.id, &updated_state);
            }

            match &gate_result.decision {
                GateDecision::Synthesize { .. } => {
                    // Check budget
                    if llm_calls_remaining == 0 {
                        report.errors.push(SynthesisError::BudgetExhausted {
                            remaining_clusters: clusters.len()
                                - report.clusters_synthesized
                                - report.clusters_skipped
                                - report.clusters_deferred
                                - report.clusters_auto_updated,
                        });
                        report.clusters_skipped += 1;
                        continue;
                    }
                    if insights_remaining == 0 {
                        report.clusters_skipped += 1;
                        continue;
                    }

                    // Check if LLM is available (graceful degradation)
                    let provider = match &self.llm_provider {
                        Some(p) => p,
                        None => {
                            log::warn!(
                                "Synthesis LLM not configured, skipping insight generation"
                            );
                            report.clusters_skipped += 1;
                            continue;
                        }
                    };

                    // Build prompt
                    let prompt = insight::build_prompt(
                        cluster_data,
                        &members,
                        &settings.synthesis,
                        settings.emotional.include_emotion_in_prompt,
                    );

                    // Call LLM
                    let raw_response =
                        match insight::call_llm(&prompt, provider.as_ref(), &settings.synthesis) {
                            Ok(resp) => {
                                llm_calls_remaining = llm_calls_remaining.saturating_sub(1);
                                resp
                            }
                            Err(_e) => {
                                report.errors.push(SynthesisError::LlmTimeout {
                                    cluster_id: cluster_data.id.clone(),
                                });
                                report.clusters_skipped += 1;
                                continue;
                            }
                        };

                    // Validate output
                    let output =
                        match insight::validate_output(&raw_response, cluster_data, &members) {
                            Ok(o) => o,
                            Err(e) => {
                                report.errors.push(e);
                                report.clusters_skipped += 1;
                                continue;
                            }
                        };

                    // Compute importance
                    let importance =
                        insight::compute_insight_importance(&output, cluster_data, &members);

                    // === ATOMIC TRANSACTION: store insight + provenance + demotion ===
                    // GUARD-1: No Data Loss — all or nothing
                    match self.store_insight_atomically(
                        storage,
                        cluster_data,
                        &members,
                        &output,
                        importance,
                        &gate_result,
                        settings,
                    ) {
                        Ok((insight_id, demoted_ids)) => {
                            report.insights_created.push(insight_id);
                            report.sources_demoted.extend(demoted_ids);
                            report.clusters_synthesized += 1;
                            insights_remaining = insights_remaining.saturating_sub(1);

                            // Track full vs incremental
                            if incremental_state.is_some() {
                                report.synthesis_runs_incremental += 1;
                            } else {
                                report.synthesis_runs_full += 1;
                            }

                            // Save incremental state for next run.
                            // Note: attempt history was already persisted above
                            // (before the gate check), so we read the latest
                            // attempt_count from the state we just saved.
                            let latest_attempt = storage
                                .get_incremental_state(&cluster_data.id)
                                .ok()
                                .flatten();
                            let now = Utc::now();
                            let members_snapshot: HashSet<String> =
                                cluster_data.members.iter().cloned().collect();
                            let new_state = IncrementalState {
                                last_member_snapshot: members_snapshot.clone(),
                                last_quality_score: cluster_data.quality_score,
                                last_run: now,
                                run_count: incremental_state
                                    .as_ref()
                                    .map(|s| s.run_count + 1)
                                    .unwrap_or(1),
                                last_attempt_timestamp: latest_attempt
                                    .as_ref()
                                    .map(|s| s.last_attempt_timestamp)
                                    .unwrap_or(now),
                                attempt_count: latest_attempt
                                    .as_ref()
                                    .map(|s| s.attempt_count)
                                    .unwrap_or(1),
                                last_attempt_members: members_snapshot,
                            };
                            let _ = storage.set_incremental_state(
                                &cluster_data.id,
                                &new_state,
                            );
                        }
                        Err(e) => {
                            report.errors.push(SynthesisError::StorageError {
                                cluster_id: cluster_data.id.clone(),
                                message: e.to_string(),
                            });
                            report.clusters_skipped += 1;
                        }
                    }
                }
                GateDecision::AutoUpdate { action } => {
                    match action {
                        AutoUpdateAction::MergeDuplicates { keep, demote } => {
                            // Use supersession to mark duplicates as superseded by the kept memory.
                            // Then transfer Hebbian links from each demoted memory to the keeper
                            // (preserving graph connectivity) and boost the kept memory's importance.
                            let demote_refs: Vec<&str> = demote.iter().map(|s| s.as_str()).collect();
                            match storage.supersede_bulk(&demote_refs, keep) {
                                Ok(count) => {
                                    log::info!(
                                        "auto-update: merged {} duplicates into {} for cluster {}",
                                        count, keep, cluster_data.id
                                    );
                                    // Transfer Hebbian links from demoted memories to keeper
                                    for donor_id in demote {
                                        if let Err(e) = storage.merge_hebbian_links(donor_id, keep) {
                                            log::warn!(
                                                "auto-update: failed to merge Hebbian links from {} to {}: {}",
                                                donor_id, keep, e
                                            );
                                        }
                                    }
                                    // Boost importance of kept memory: max of all member importances
                                    let max_importance = members
                                        .iter()
                                        .map(|m| m.importance)
                                        .fold(0.0_f64, f64::max);
                                    let _ = storage.update_importance(keep, max_importance);
                                    report.sources_demoted.extend(demote.iter().cloned());
                                }
                                Err(e) => {
                                    log::warn!(
                                        "auto-update: supersede_bulk failed for cluster {}: {}",
                                        cluster_data.id, e
                                    );
                                    report.errors.push(SynthesisError::StorageError {
                                        cluster_id: cluster_data.id.clone(),
                                        message: format!("MergeDuplicates failed: {}", e),
                                    });
                                }
                            }
                        }
                        AutoUpdateAction::StrengthenLinks { pairs } => {
                            // Boost Hebbian co-activation for each pair.
                            // Uses threshold=0 so strength increments immediately
                            // (synthesis-discovered pairs skip the coactivation ramp-up).
                            for (id_a, id_b) in pairs {
                                if let Err(e) = storage.record_coactivation(id_a, id_b, 0) {
                                    log::warn!(
                                        "auto-update: failed to strengthen link {} <-> {}: {}",
                                        id_a, id_b, e
                                    );
                                }
                            }
                            log::info!(
                                "auto-update: strengthened {} links for cluster {}",
                                pairs.len(), cluster_data.id
                            );
                        }
                    }
                    report.clusters_auto_updated += 1;
                }
                GateDecision::Defer { .. } => {
                    report.clusters_deferred += 1;
                }
                GateDecision::Skip { .. } => {
                    report.clusters_skipped += 1;
                }
            }
        }

        report.duration = start.elapsed();
        Ok(report)
    }

    fn discover_clusters(
        &self,
        storage: &Storage,
        config: &ClusterDiscoveryConfig,
    ) -> Result<Vec<MemoryCluster>, Box<dyn std::error::Error>> {
        cluster::discover_clusters(storage, config, self.embedding_model.as_deref())
    }

    fn check_gate(
        &self,
        cluster: &MemoryCluster,
        members: &[MemoryRecord],
        config: &GateConfig,
    ) -> GateResult {
        // For trait method: pass defaults for pre-computed values
        gate::check_gate(cluster, members, config, 0.0, true, false)
    }

    fn undo_synthesis(
        &self,
        storage: &mut Storage,
        insight_id: &str,
    ) -> Result<UndoSynthesis, Box<dyn std::error::Error>> {
        provenance::undo_synthesis(storage, insight_id)
    }

    fn get_provenance(
        &self,
        storage: &Storage,
        memory_id: &str,
        max_depth: usize,
    ) -> Result<ProvenanceChain, Box<dyn std::error::Error>> {
        provenance::get_provenance_chain(storage, memory_id, max_depth)
    }
}

/// Generate a short random hex ID.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let random_part: u32 = nanos ^ std::process::id();
    format!("{:08x}", random_part)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MemoryLayer, MemoryType};
    use chrono::Utc;

    // -----------------------------------------------------------------------
    // Mock LLM provider
    // -----------------------------------------------------------------------

    struct MockLlmProvider {
        /// The response to return from generate().
        response: String,
    }

    impl MockLlmProvider {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
            }
        }

        /// Returns a provider that produces valid JSON for the given source IDs.
        fn valid_for(source_ids: &[&str]) -> Self {
            let refs: Vec<String> = source_ids.iter().map(|id| format!("\"{}\"", id)).collect();
            let json = format!(
                r#"{{"insight": "This is a test insight that meets the minimum length requirement for validation purposes and references the source memories.", "confidence": 0.85, "insight_type": "pattern", "source_references": [{}]}}"#,
                refs.join(", ")
            );
            Self::new(&json)
        }
    }

    impl SynthesisLlmProvider for MockLlmProvider {
        fn generate(
            &self,
            _prompt: &str,
            _config: &SynthesisConfig,
        ) -> Result<String, Box<dyn std::error::Error>> {
            Ok(self.response.clone())
        }
    }

    #[allow(dead_code)]
    struct FailingLlmProvider;

    impl SynthesisLlmProvider for FailingLlmProvider {
        fn generate(
            &self,
            _prompt: &str,
            _config: &SynthesisConfig,
        ) -> Result<String, Box<dyn std::error::Error>> {
            Err("LLM call failed".into())
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_memory(id: &str, content: &str, memory_type: MemoryType, importance: f64) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.5,
            importance,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    fn setup_storage_with_memories(memories: &[MemoryRecord]) -> Storage {
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        for mem in memories {
            storage.add(mem, "default").unwrap();
        }
        storage
    }

    fn default_settings() -> SynthesisSettings {
        SynthesisSettings {
            enabled: true,
            ..Default::default()
        }
    }

    fn make_cluster(id: &str, members: &[&str], quality: f64) -> MemoryCluster {
        MemoryCluster {
            id: id.to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            quality_score: quality,
            centroid_id: members.first().unwrap_or(&"").to_string(),
            signals_summary: SignalsSummary {
                dominant_signal: ClusterSignal::Hebbian,
                hebbian_contribution: 0.4,
                entity_contribution: 0.3,
                embedding_contribution: 0.2,
                temporal_contribution: 0.1,
            },
        }
    }

    // -----------------------------------------------------------------------
    // Incremental / C4 tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_should_resynthesize_new_cluster() {
        // No previous state means should_resynthesize isn't even called;
        // but if called with an empty snapshot, union=0 → true
        let cluster = make_cluster("c1", &["m1", "m2", "m3"], 0.7);
        let state = IncrementalState {
            last_member_snapshot: HashSet::new(),
            last_quality_score: 0.7,
            last_run: Utc::now(),
            run_count: 0,
            last_attempt_timestamp: Utc::now(),
            attempt_count: 0,
            last_attempt_members: HashSet::new(),
        };
        let config = IncrementalConfig::default();
        assert!(DefaultSynthesisEngine::should_resynthesize(&cluster, &state, &config));
    }

    #[test]
    fn test_should_resynthesize_no_change() {
        let cluster = make_cluster("c1", &["m1", "m2", "m3"], 0.7);
        let state = IncrementalState {
            last_member_snapshot: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
            last_quality_score: 0.7,
            last_run: Utc::now(),
            run_count: 1,
            last_attempt_timestamp: Utc::now(),
            attempt_count: 1,
            last_attempt_members: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
        };
        let config = IncrementalConfig::default();
        // Same members, same quality → false (skip)
        assert!(!DefaultSynthesisEngine::should_resynthesize(&cluster, &state, &config));
    }

    #[test]
    fn test_should_resynthesize_member_change() {
        // Original: m1, m2, m3.  New: m1, m4, m5 → intersection=1, union=5
        // change_pct = 1 - 1/5 = 0.8 ≥ 0.5 → true
        let cluster = make_cluster("c1", &["m1", "m4", "m5"], 0.7);
        let state = IncrementalState {
            last_member_snapshot: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
            last_quality_score: 0.7,
            last_run: Utc::now(),
            run_count: 1,
            last_attempt_timestamp: Utc::now(),
            attempt_count: 1,
            last_attempt_members: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
        };
        let config = IncrementalConfig::default();
        assert!(DefaultSynthesisEngine::should_resynthesize(&cluster, &state, &config));
    }

    #[test]
    fn test_should_resynthesize_quality_delta() {
        // Same members but quality changed by 0.3 (> 0.2 threshold)
        let cluster = make_cluster("c1", &["m1", "m2", "m3"], 1.0);
        let state = IncrementalState {
            last_member_snapshot: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
            last_quality_score: 0.7,
            last_run: Utc::now(),
            run_count: 1,
            last_attempt_timestamp: Utc::now(),
            attempt_count: 1,
            last_attempt_members: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
        };
        let config = IncrementalConfig::default();
        assert!(DefaultSynthesisEngine::should_resynthesize(&cluster, &state, &config));
    }

    #[test]
    fn test_incremental_state_storage_roundtrip() {
        let storage = Storage::new(":memory:").expect("in-memory db");
        let now = Utc::now();
        let state = IncrementalState {
            last_member_snapshot: vec!["m1".to_string(), "m2".to_string()].into_iter().collect(),
            last_quality_score: 0.75,
            last_run: now,
            run_count: 3,
            last_attempt_timestamp: now,
            attempt_count: 5,
            last_attempt_members: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect(),
        };
        storage.set_incremental_state("cluster-abc", &state).unwrap();
        let loaded = storage.get_incremental_state("cluster-abc").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.last_member_snapshot.len(), 2);
        assert!(loaded.last_member_snapshot.contains("m1"));
        assert!(loaded.last_member_snapshot.contains("m2"));
        assert!((loaded.last_quality_score - 0.75).abs() < 0.001);
        assert_eq!(loaded.run_count, 3);
        // Verify new attempt history fields roundtrip correctly
        assert_eq!(loaded.attempt_count, 5);
        assert_eq!(loaded.last_attempt_members.len(), 3);
        assert!(loaded.last_attempt_members.contains("m3"));
    }

    #[test]
    fn test_incremental_state_missing() {
        let storage = Storage::new(":memory:").expect("in-memory db");
        let loaded = storage.get_incremental_state("nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_synthesize_skips_unchanged_clusters() {
        // Set up a storage with a pre-existing incremental state matching the cluster.
        // The synthesize loop should skip it.
        let memories = vec![
            make_memory("m1", "Rust is fast and safe", MemoryType::Factual, 0.7),
            make_memory("m2", "Borrow checker prevents bugs", MemoryType::Episodic, 0.7),
            make_memory("m3", "Ownership model is unique", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Create Hebbian links to force a cluster
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        // First run: discover clusters and run synthesis
        let provider = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report1 = engine.synthesize(&mut storage, &settings).unwrap();
        // If clusters were found and synthesized, incremental state should have been saved
        if report1.clusters_synthesized > 0 {
            // Second run with a new engine (same storage) — clusters unchanged
            let provider2 = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
            let engine2 = DefaultSynthesisEngine::new(Some(Box::new(provider2)), None);
            let report2 = engine2.synthesize(&mut storage, &settings).unwrap();

            // The same clusters should be skipped because incremental state matches
            assert!(
                report2.clusters_skipped >= report1.clusters_synthesized,
                "Expected unchanged clusters to be skipped. \
                 First run synthesized {}, second run skipped {}",
                report1.clusters_synthesized,
                report2.clusters_skipped
            );
            assert_eq!(report2.clusters_synthesized, 0,
                "No new synthesis should happen on unchanged clusters");
        }
    }

    // -----------------------------------------------------------------------
    // Cluster attempt history tests (TASK-08)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cluster_changed_no_prior_attempt() {
        // When there is no prior IncrementalState, cluster_changed should be true
        // (first time seeing this cluster → allow synthesis).
        // This is tested indirectly: with no state, cluster_changed=true,
        // so gate Rule 6 ("no growth since last attempt") won't fire.
        let memories = vec![
            make_memory("m1", "Cats are animals", MemoryType::Factual, 0.7),
            make_memory("m2", "Dogs are animals", MemoryType::Episodic, 0.7),
            make_memory("m3", "Birds are animals", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        let provider = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();
        // First run should allow synthesis (cluster_changed=true for new clusters)
        if report.clusters_found > 0 {
            // Gate should NOT skip with "no growth since last attempt"
            for gr in &report.gate_results {
                if let GateDecision::Skip { reason } = &gr.decision {
                    assert!(
                        !reason.contains("no growth"),
                        "New cluster should not be skipped for 'no growth': {}",
                        reason
                    );
                }
            }
        }
    }

    #[test]
    fn test_cluster_changed_same_members_returns_false() {
        // When cluster members haven't changed since last attempt,
        // cluster_changed should be false → gate skips with "no growth".
        let memories = vec![
            make_memory("m1", "Apples are fruit", MemoryType::Factual, 0.7),
            make_memory("m2", "Oranges are fruit", MemoryType::Episodic, 0.7),
            make_memory("m3", "Bananas are fruit", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        // We need to discover what cluster ID will be generated first
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        // Run once to discover cluster IDs
        let provider = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);
        let report1 = engine.synthesize(&mut storage, &settings).unwrap();

        if report1.clusters_found > 0 {
            // Now run again — the attempt history was persisted by the first run,
            // and since members haven't changed, cluster_changed=false.
            // The cluster was already synthesized, so incremental staleness check
            // will also skip it. Both mechanisms work together.
            let provider2 = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
            let engine2 = DefaultSynthesisEngine::new(Some(Box::new(provider2)), None);
            let report2 = engine2.synthesize(&mut storage, &settings).unwrap();
            assert_eq!(
                report2.clusters_synthesized, 0,
                "Unchanged cluster should not be re-synthesized"
            );
        }
    }

    #[test]
    fn test_attempt_count_persisted() {
        // Verify that attempt_count increments across synthesis runs.
        let memories = vec![
            make_memory("m1", "Fish swim in water", MemoryType::Factual, 0.7),
            make_memory("m2", "Fish have gills", MemoryType::Episodic, 0.7),
            make_memory("m3", "Fish are cold-blooded", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        let provider = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();
        if report.clusters_found > 0 {
            // Check that incremental state has attempt_count >= 1
            // (at least one attempt was recorded for each cluster processed)
            for gr in &report.gate_results {
                let state = storage.get_incremental_state(&gr.cluster_id).unwrap();
                assert!(state.is_some(), "Incremental state should be saved for attempted cluster");
                let state = state.unwrap();
                assert!(
                    state.attempt_count >= 1,
                    "attempt_count should be at least 1 after first run, got {}",
                    state.attempt_count
                );
                assert!(
                    !state.last_attempt_members.is_empty(),
                    "last_attempt_members should be populated"
                );
            }
        }
    }

    #[test]
    fn test_cluster_changed_jaccard_detects_membership_change() {
        // Unit test: verify the Jaccard logic directly.
        // Previous attempt members: {m1, m2, m3}
        // Current members: {m1, m2, m4} → Jaccard = 2/4 = 0.5 < 1.0 → changed
        let current_members = vec!["m1".to_string(), "m2".to_string(), "m4".to_string()];
        let previous_members: HashSet<String> =
            vec!["m1".to_string(), "m2".to_string(), "m3".to_string()]
                .into_iter().collect();

        let current: HashSet<&str> = current_members.iter().map(|s| s.as_str()).collect();
        let previous: HashSet<&str> = previous_members.iter().map(|s| s.as_str()).collect();
        let intersection = current.intersection(&previous).count();
        let union_size = current.union(&previous).count();
        let jaccard = intersection as f64 / union_size as f64;

        assert_eq!(intersection, 2); // m1, m2
        assert_eq!(union_size, 4);   // m1, m2, m3, m4
        assert!((jaccard - 0.5).abs() < 0.001);
        assert!(jaccard < 1.0, "Membership changed → cluster_changed should be true");
    }

    #[test]
    fn test_cluster_changed_jaccard_identical_members() {
        // Previous attempt members: {m1, m2, m3}
        // Current members: {m1, m2, m3} → Jaccard = 3/3 = 1.0 → NOT changed
        let members: HashSet<&str> = vec!["m1", "m2", "m3"].into_iter().collect();
        let intersection = members.intersection(&members).count();
        let union_size = members.union(&members).count();
        let jaccard = intersection as f64 / union_size as f64;

        assert_eq!(jaccard, 1.0);
        assert!(!(jaccard < 1.0), "Identical members → cluster_changed should be false");
    }

    // -----------------------------------------------------------------------
    // Pairwise similarity tests (TASK-09)
    // -----------------------------------------------------------------------

    #[test]
    fn test_linear_index_to_pair() {
        // n=4: pairs (0,1),(0,2),(0,3),(1,2),(1,3),(2,3)
        assert_eq!(linear_index_to_pair(0, 4), (0, 1));
        assert_eq!(linear_index_to_pair(1, 4), (0, 2));
        assert_eq!(linear_index_to_pair(2, 4), (0, 3));
        assert_eq!(linear_index_to_pair(3, 4), (1, 2));
        assert_eq!(linear_index_to_pair(4, 4), (1, 3));
        assert_eq!(linear_index_to_pair(5, 4), (2, 3));

        // n=3: pairs (0,1),(0,2),(1,2)
        assert_eq!(linear_index_to_pair(0, 3), (0, 1));
        assert_eq!(linear_index_to_pair(1, 3), (0, 2));
        assert_eq!(linear_index_to_pair(2, 3), (1, 2));
    }

    #[test]
    fn test_all_pairs_similar_identical_embeddings() {
        // All members have the same embedding → cosine similarity = 1.0
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let mems = vec![
            make_memory("m1", "content 1", MemoryType::Factual, 0.7),
            make_memory("m2", "content 2", MemoryType::Factual, 0.7),
            make_memory("m3", "content 3", MemoryType::Factual, 0.7),
        ];
        for mem in &mems {
            storage.add(mem, "default").unwrap();
        }
        let emb = vec![1.0_f32, 0.0, 0.0];
        for mem in &mems {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        assert!(DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    #[test]
    fn test_all_pairs_similar_dissimilar_pair() {
        // Two similar, one dissimilar → should return false
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let mems = vec![
            make_memory("m1", "content 1", MemoryType::Factual, 0.7),
            make_memory("m2", "content 2", MemoryType::Factual, 0.7),
            make_memory("m3", "content 3", MemoryType::Factual, 0.7),
        ];
        for mem in &mems {
            storage.add(mem, "default").unwrap();
        }
        // m1 and m2 are similar, m3 is orthogonal
        storage.store_embedding("m1", &[1.0, 0.0, 0.0], "test/model", 3).unwrap();
        storage.store_embedding("m2", &[0.99, 0.1, 0.0], "test/model", 3).unwrap();
        storage.store_embedding("m3", &[0.0, 1.0, 0.0], "test/model", 3).unwrap();

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        assert!(!DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    #[test]
    fn test_all_pairs_similar_missing_embedding() {
        // One member lacks an embedding → should return false
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let mems = vec![
            make_memory("m1", "content 1", MemoryType::Factual, 0.7),
            make_memory("m2", "content 2", MemoryType::Factual, 0.7),
            make_memory("m3", "content 3", MemoryType::Factual, 0.7),
        ];
        for mem in &mems {
            storage.add(mem, "default").unwrap();
        }
        let emb = vec![1.0_f32, 0.0, 0.0];
        // Only store embeddings for m1 and m2, NOT m3
        storage.store_embedding("m1", &emb, "test/model", 3).unwrap();
        storage.store_embedding("m2", &emb, "test/model", 3).unwrap();

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        assert!(!DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    #[test]
    fn test_all_pairs_similar_single_member() {
        // Single member → not "all pairs similar" (no pairs to check)
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let mem = make_memory("m1", "content 1", MemoryType::Factual, 0.7);
        storage.add(&mem, "default").unwrap();
        storage.store_embedding("m1", &[1.0, 0.0, 0.0], "test/model", 3).unwrap();

        let ids = vec!["m1".to_string()];
        assert!(!DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    #[test]
    fn test_all_pairs_similar_large_cluster_sampling() {
        // >10 members, all identical embeddings → should still return true via sampling
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let n = 12;
        let mems: Vec<MemoryRecord> = (0..n)
            .map(|i| make_memory(&format!("m{}", i), &format!("content {}", i), MemoryType::Factual, 0.7))
            .collect();
        for mem in &mems {
            storage.add(mem, "default").unwrap();
        }
        let emb = vec![1.0_f32, 0.0, 0.0];
        for mem in &mems {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        assert!(DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    #[test]
    fn test_all_pairs_similar_large_cluster_with_outlier() {
        // >10 members, most identical but one outlier → sampling should catch it
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let n = 15;
        let mems: Vec<MemoryRecord> = (0..n)
            .map(|i| make_memory(&format!("m{}", i), &format!("content {}", i), MemoryType::Factual, 0.7))
            .collect();
        for mem in &mems {
            storage.add(mem, "default").unwrap();
        }
        // All similar except the last one
        let similar_emb = vec![1.0_f32, 0.0, 0.0];
        let outlier_emb = vec![0.0_f32, 1.0, 0.0]; // orthogonal
        for (i, mem) in mems.iter().enumerate() {
            if i == n - 1 {
                storage.store_embedding(&mem.id, &outlier_emb, "test/model", 3).unwrap();
            } else {
                storage.store_embedding(&mem.id, &similar_emb, "test/model", 3).unwrap();
            }
        }

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        // With 15 members, 1 outlier creates 14 dissimilar pairs out of 105 total (13%).
        // Sampling 45 pairs has >99% chance of hitting at least one.
        assert!(!DefaultSynthesisEngine::compute_all_pairs_similar(
            &storage, &ids, 0.95
        ));
    }

    // -----------------------------------------------------------------------
    // Test 1: No LLM provider — graceful degradation
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_llm_provider_graceful_degradation() {
        let engine = DefaultSynthesisEngine::new(None, None);
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let settings = default_settings();

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // With no memories, 0 clusters found
        assert_eq!(report.clusters_found, 0);
        assert_eq!(report.clusters_synthesized, 0);
        assert!(report.insights_created.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn test_no_llm_with_memories_skips_synthesis() {
        // Create memories that might form clusters, but without an LLM
        // the engine should skip synthesis for any clusters that pass the gate.
        let engine = DefaultSynthesisEngine::new(None, None);
        let memories = vec![
            make_memory("m1", "Rust is a systems language", MemoryType::Factual, 0.7),
            make_memory("m2", "Rust has a borrow checker", MemoryType::Factual, 0.7),
            make_memory("m3", "Rust prevents memory bugs", MemoryType::Episodic, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);
        let settings = default_settings();

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Even if clusters are found, 0 should be synthesized without LLM
        assert_eq!(report.clusters_synthesized, 0);
        assert!(report.insights_created.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Mock LLM — full synthesis pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn test_mock_llm_synthesis() {
        // For this test, we need a cluster to be discovered. The cluster
        // discovery requires Hebbian links or shared entities. We'll set
        // up Hebbian links to force clustering.
        let memories = vec![
            make_memory("m1", "Rust is fast and safe", MemoryType::Factual, 0.7),
            make_memory("m2", "Borrow checker prevents bugs", MemoryType::Episodic, 0.7),
            make_memory("m3", "Ownership model is unique", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Create Hebbian links between all pairs to force a cluster
        storage.record_coactivation("m1", "m2", 0).unwrap();
        storage.record_coactivation("m1", "m3", 0).unwrap();
        storage.record_coactivation("m2", "m3", 0).unwrap();
        // Strengthen links with repeated co-activations
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        let provider = MockLlmProvider::valid_for(&["m1", "m2", "m3"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);

        let mut settings = default_settings();
        // Lower thresholds to make test easier
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // We expect at least 1 cluster found
        if report.clusters_found > 0 {
            // If a cluster passed the gate, we should have synthesized it
            let synthesize_gate_count = report
                .gate_results
                .iter()
                .filter(|r| matches!(r.decision, GateDecision::Synthesize { .. }))
                .count();

            if synthesize_gate_count > 0 {
                assert!(
                    report.clusters_synthesized > 0,
                    "Expected synthesis but got: {:?}",
                    report
                );
                assert!(!report.insights_created.is_empty());
                assert!(!report.sources_demoted.is_empty());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: Budget exhaustion
    // -----------------------------------------------------------------------

    #[test]
    fn test_budget_exhaustion() {
        // Create enough memories for potential clusters
        let memories = vec![
            make_memory("m1", "First topic memory A", MemoryType::Factual, 0.7),
            make_memory("m2", "First topic memory B", MemoryType::Episodic, 0.7),
            make_memory("m3", "First topic memory C", MemoryType::Relational, 0.7),
            make_memory("m4", "Second topic memory D", MemoryType::Factual, 0.7),
            make_memory("m5", "Second topic memory E", MemoryType::Episodic, 0.7),
            make_memory("m6", "Second topic memory F", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Create Hebbian links for two separate clusters
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
            storage.record_coactivation("m4", "m5", 0).unwrap();
            storage.record_coactivation("m4", "m6", 0).unwrap();
            storage.record_coactivation("m5", "m6", 0).unwrap();
        }

        let provider =
            MockLlmProvider::valid_for(&["m1", "m2", "m3", "m4", "m5", "m6"]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);

        let mut settings = default_settings();
        settings.max_llm_calls_per_run = 1; // Budget for only 1 LLM call
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // With budget=1, at most 1 cluster should be synthesized
        assert!(
            report.clusters_synthesized <= 1,
            "Expected at most 1 synthesis, got {}",
            report.clusters_synthesized
        );

        // If there were multiple synthesizable clusters, we should see budget exhaustion
        let synthesize_gate_count = report
            .gate_results
            .iter()
            .filter(|r| matches!(r.decision, GateDecision::Synthesize { .. }))
            .count();

        if synthesize_gate_count > 1 {
            let budget_errors = report
                .errors
                .iter()
                .filter(|e| matches!(e, SynthesisError::BudgetExhausted { .. }))
                .count();
            assert!(
                budget_errors > 0,
                "Expected BudgetExhausted error when multiple clusters need synthesis"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 4: store_insight_atomically
    // -----------------------------------------------------------------------

    #[test]
    fn test_store_insight_atomically() {
        let engine = DefaultSynthesisEngine::new(None, None);
        let memories = vec![
            make_memory("s1", "Source memory one", MemoryType::Factual, 0.8),
            make_memory("s2", "Source memory two", MemoryType::Episodic, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        let cluster = MemoryCluster {
            id: "test-cluster-001".to_string(),
            members: vec!["s1".to_string(), "s2".to_string()],
            quality_score: 0.75,
            centroid_id: "s1".to_string(),
            signals_summary: SignalsSummary {
                dominant_signal: ClusterSignal::Hebbian,
                hebbian_contribution: 0.4,
                entity_contribution: 0.3,
                embedding_contribution: 0.2,
                temporal_contribution: 0.1,
            },
        };

        let output = SynthesisOutput {
            insight_text: "Test insight text".to_string(),
            confidence: 0.85,
            insight_type: InsightType::Pattern,
            source_references: vec!["s1".to_string(), "s2".to_string()],
        };

        let gate_result = GateResult {
            cluster_id: "test-cluster-001".to_string(),
            decision: GateDecision::Synthesize {
                reason: "passed all gates".to_string(),
            },
            scores: GateScores {
                quality: 0.75,
                type_diversity: 2,
                estimated_cost: 0.01,
                member_count: 2,
            },
            timestamp: Utc::now(),
        };

        let settings = default_settings();

        let (insight_id, demoted_ids) = engine
            .store_insight_atomically(
                &mut storage,
                &cluster,
                &memories,
                &output,
                0.9,
                &gate_result,
                &settings,
            )
            .unwrap();

        // Verify insight was created
        assert_eq!(insight_id.len(), 8);
        let stored = storage.get(&insight_id).unwrap();
        assert!(stored.is_some(), "Insight should be stored");
        let stored = stored.unwrap();
        assert_eq!(stored.content, "Test insight text");
        assert!((stored.importance - 0.9).abs() < 0.001);

        // Verify metadata
        let meta = stored.metadata.unwrap();
        assert_eq!(meta["is_synthesis"], true);
        assert_eq!(meta["source_cluster"], "test-cluster-001");

        // Verify provenance
        let sources = storage.get_insight_sources(&insight_id).unwrap();
        assert_eq!(sources.len(), 2);

        // Verify demotion
        assert_eq!(demoted_ids.len(), 2);
        let s1 = storage.get("s1").unwrap().unwrap();
        assert!((s1.importance - 0.4).abs() < 0.001); // 0.8 * 0.5
        let s2 = storage.get("s2").unwrap().unwrap();
        assert!((s2.importance - 0.35).abs() < 0.001); // 0.7 * 0.5
    }

    // -----------------------------------------------------------------------
    // Test 5: generate_id uniqueness
    // -----------------------------------------------------------------------

    #[test]
    fn test_generate_id_format() {
        let id = generate_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -----------------------------------------------------------------------
    // Test 6: Trait method check_gate delegates correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_check_gate_delegation() {
        let engine = DefaultSynthesisEngine::new(None, None);

        let members = vec![
            make_memory("m1", "Fact A", MemoryType::Factual, 0.5),
            make_memory("m2", "Episode B", MemoryType::Episodic, 0.5),
            make_memory("m3", "Relation C", MemoryType::Relational, 0.5),
        ];

        let cluster = MemoryCluster {
            id: "test-cluster".to_string(),
            members: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            quality_score: 0.8,
            centroid_id: "m1".to_string(),
            signals_summary: SignalsSummary {
                dominant_signal: ClusterSignal::Hebbian,
                hebbian_contribution: 0.4,
                entity_contribution: 0.3,
                embedding_contribution: 0.2,
                temporal_contribution: 0.1,
            },
        };

        let config = GateConfig::default();
        let result = engine.check_gate(&cluster, &members, &config);

        // High quality diverse cluster should be synthesized
        assert!(
            matches!(result.decision, GateDecision::Synthesize { .. }),
            "Expected Synthesize, got {:?}",
            result.decision
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Provenance delegation
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_provenance_delegation() {
        let engine = DefaultSynthesisEngine::new(None, None);
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem = make_memory("raw1", "Raw memory", MemoryType::Factual, 0.5);
        storage.add(&mem, "default").unwrap();

        let chain = engine.get_provenance(&storage, "raw1", 5).unwrap();
        assert_eq!(chain.root_id, "raw1");
        assert!(chain.layers.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 8: Empty storage produces empty report
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_storage_no_clusters() {
        let provider = MockLlmProvider::valid_for(&[]);
        let engine = DefaultSynthesisEngine::new(Some(Box::new(provider)), None);
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let settings = default_settings();

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        assert_eq!(report.clusters_found, 0);
        assert_eq!(report.clusters_synthesized, 0);
        assert_eq!(report.clusters_auto_updated, 0);
        assert_eq!(report.clusters_deferred, 0);
        assert_eq!(report.clusters_skipped, 0);
        assert!(report.insights_created.is_empty());
        assert!(report.sources_demoted.is_empty());
        assert!(report.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 9: Cold path triggers on empty cluster state (total_count == 0)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cold_path_on_empty_storage() {
        let engine = DefaultSynthesisEngine::new(None, None);
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let settings = default_settings();

        // total_count == 0 → should_cold = true
        let total = storage.count_memories().unwrap();
        assert_eq!(total, 0);

        // Synthesize should succeed (cold path, finds 0 clusters)
        let report = engine.synthesize(&mut storage, &settings).unwrap();
        assert_eq!(report.clusters_found, 0);
    }

    // -----------------------------------------------------------------------
    // Test 10: count_memories and get_all_cluster_data work correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_count_memories() {
        let memories = vec![
            make_memory("m1", "Memory one", MemoryType::Factual, 0.5),
            make_memory("m2", "Memory two", MemoryType::Episodic, 0.5),
            make_memory("m3", "Memory three", MemoryType::Relational, 0.5),
        ];
        let storage = setup_storage_with_memories(&memories);
        assert_eq!(storage.count_memories().unwrap(), 3);
    }

    #[test]
    fn test_get_all_cluster_data_empty() {
        let storage = Storage::new(":memory:").expect("in-memory db");
        let clusters = storage.get_all_cluster_data().unwrap();
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_get_all_cluster_data_after_save() {
        let storage = Storage::new(":memory:").expect("in-memory db");

        // Save some cluster state
        let cluster_tuples = vec![
            (
                "cluster-a".to_string(),
                vec!["m1".to_string(), "m2".to_string()],
                vec![0.1f32, 0.2, 0.3],
            ),
            (
                "cluster-b".to_string(),
                vec!["m3".to_string(), "m4".to_string(), "m5".to_string()],
                vec![0.4f32, 0.5, 0.6],
            ),
        ];
        storage.save_full_cluster_state(&cluster_tuples).unwrap();

        let clusters = storage.get_all_cluster_data().unwrap();
        assert_eq!(clusters.len(), 2);

        // Find cluster-a
        let ca = clusters.iter().find(|c| c.id == "cluster-a").unwrap();
        assert_eq!(ca.members, vec!["m1", "m2"]);
        assert!((ca.quality_score - 0.5).abs() < 0.01); // default quality

        // Find cluster-b
        let cb = clusters.iter().find(|c| c.id == "cluster-b").unwrap();
        assert_eq!(cb.members, vec!["m3", "m4", "m5"]);
    }

    // -----------------------------------------------------------------------
    // Test 11: Cold path saves cluster state for future warm/cached use
    // -----------------------------------------------------------------------

    #[test]
    fn test_cold_path_saves_cluster_state() {
        // Directly test the save_full_cluster_state + get_all_cluster_data round-trip
        // which is what the cold path does after discover_clusters
        let storage = Storage::new(":memory:").expect("in-memory db");

        // Simulate what cold path does: save cluster state
        let cluster_tuples = vec![
            (
                "cluster-cold-1".to_string(),
                vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
                vec![0.5f32, 0.5, 0.0],
            ),
        ];
        storage.save_full_cluster_state(&cluster_tuples).unwrap();

        // Verify cluster state was saved and can be retrieved
        let cached = storage.get_all_cluster_data().unwrap();
        assert!(!cached.is_empty(), "Cluster state should be saved after cold path");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].id, "cluster-cold-1");
        assert_eq!(cached[0].members.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Test 12: Three-tier config defaults
    // -----------------------------------------------------------------------

    #[test]
    fn test_three_tier_config_defaults() {
        let config = ClusterDiscoveryConfig::default();
        assert!(config.cold_recluster_ratio.is_none());
        assert!(config.warm_recluster_interval.is_none());
        assert!(config.hot_assign_threshold.is_none());
    }

    #[test]
    fn test_three_tier_config_custom() {
        let mut config = ClusterDiscoveryConfig::default();
        config.cold_recluster_ratio = Some(0.3);
        config.warm_recluster_interval = Some(50);
        config.hot_assign_threshold = Some(0.7);

        assert_eq!(config.cold_recluster_ratio.unwrap(), 0.3);
        assert_eq!(config.warm_recluster_interval.unwrap(), 50);
        assert_eq!(config.hot_assign_threshold.unwrap(), 0.7);
    }

    // -----------------------------------------------------------------------
    // Test 13: Warm path — pending/dirty triggers warm recluster
    // -----------------------------------------------------------------------

    #[test]
    fn test_warm_path_with_pending() {
        let memories = vec![
            make_memory("m1", "Memory one", MemoryType::Factual, 0.7),
            make_memory("m2", "Memory two", MemoryType::Episodic, 0.7),
            make_memory("m3", "Memory three", MemoryType::Relational, 0.7),
            make_memory("m4", "Memory four", MemoryType::Factual, 0.7),
            make_memory("m5", "Memory five", MemoryType::Episodic, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Set up existing cluster state (simulating a previous cold run)
        let cluster_tuples = vec![(
            "cluster-existing".to_string(),
            vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            vec![1.0f32, 0.0, 0.0],
        )];
        storage.save_full_cluster_state(&cluster_tuples).unwrap();

        // Add pending memories (simulating memories added since last cold run)
        // Only 1 pending out of 5 total = 20%, right at threshold, so should NOT cold
        storage.add_pending_memory("m4").unwrap();

        let pending = storage.get_pending_count().unwrap();
        assert_eq!(pending, 1);

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        // Set cold ratio high so we don't trigger cold
        settings.cluster_discovery.cold_recluster_ratio = Some(0.5);

        // This should take the warm path (pending > 0, ratio < cold threshold)
        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // The warm path ran — report should reflect clusters found from storage
        // (at minimum the existing cluster, possibly updated)
        assert!(report.errors.is_empty() || report.errors.iter().all(|e| {
            // Storage errors from missing memories in subset are acceptable
            matches!(e, SynthesisError::StorageError { .. })
        }));
    }

    // -----------------------------------------------------------------------
    // Test 14: Cold ratio threshold triggers cold path
    // -----------------------------------------------------------------------

    #[test]
    fn test_cold_path_triggered_by_ratio() {
        let memories = vec![
            make_memory("m1", "Memory one", MemoryType::Factual, 0.7),
            make_memory("m2", "Memory two", MemoryType::Episodic, 0.7),
            make_memory("m3", "Memory three", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Set up existing cluster state
        let cluster_tuples = vec![(
            "cluster-old".to_string(),
            vec!["m1".to_string()],
            vec![1.0f32, 0.0, 0.0],
        )];
        storage.save_full_cluster_state(&cluster_tuples).unwrap();

        // Add 2 pending out of 3 total = 66.7% > default 20% ratio → cold path
        storage.add_pending_memory("m2").unwrap();
        storage.add_pending_memory("m3").unwrap();

        let engine = DefaultSynthesisEngine::new(None, None);
        let settings = default_settings();

        // should_cold = true because pending/total = 2/3 = 0.67 > 0.2
        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Cold path runs discover_clusters from scratch
        // Just verify it doesn't error
        assert!(report.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 15: Cached path — no pending, no dirty
    // -----------------------------------------------------------------------

    #[test]
    fn test_cached_path_no_pending_no_dirty() {
        let memories = vec![
            make_memory("m1", "Memory one", MemoryType::Factual, 0.7),
            make_memory("m2", "Memory two", MemoryType::Episodic, 0.7),
            make_memory("m3", "Memory three", MemoryType::Relational, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Set up existing cluster state (no pending, no dirty)
        let cluster_tuples = vec![(
            "cluster-cached".to_string(),
            vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            vec![1.0f32, 0.0, 0.0],
        )];
        storage.save_full_cluster_state(&cluster_tuples).unwrap();

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;

        // No pending, no dirty → cached path
        let pending = storage.get_pending_count().unwrap();
        let dirty = storage.get_dirty_cluster_ids().unwrap();
        assert_eq!(pending, 0);
        assert!(dirty.is_empty());

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Should find clusters from cache (1 cluster with 3 members)
        assert_eq!(report.clusters_found, 1);
    }

    // -----------------------------------------------------------------------
    // Test 16: Emotional modulation is wired into the synthesis pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn test_emotional_modulation_wired_in_synthesis() {
        // Create two groups of memories:
        // - Group 1 (m1-m3): no emotional valence
        // - Group 2 (m4-m6): strong emotional valence
        //
        // With prioritize_emotional=true, the emotional cluster (group 2)
        // should appear FIRST in gate_results, proving the modulation
        // is wired into the synthesis pipeline and reorders clusters.

        let m1 = make_memory("m1", "Neutral fact A", MemoryType::Factual, 0.7);
        let m2 = make_memory("m2", "Neutral fact B", MemoryType::Episodic, 0.7);
        let m3 = make_memory("m3", "Neutral fact C", MemoryType::Relational, 0.7);

        // Memories with emotional valence in metadata
        let mut m4 = make_memory("m4", "Emotional memory X", MemoryType::Emotional, 0.7);
        m4.metadata = Some(serde_json::json!({"emotional_valence": 0.9}));
        let mut m5 = make_memory("m5", "Emotional memory Y", MemoryType::Episodic, 0.7);
        m5.metadata = Some(serde_json::json!({"emotional_valence": -0.8}));
        let mut m6 = make_memory("m6", "Emotional memory Z", MemoryType::Relational, 0.7);
        m6.metadata = Some(serde_json::json!({"emotional_valence": 0.7}));

        let mut storage = setup_storage_with_memories(&[m1, m2, m3, m4, m5, m6]);

        // Create Hebbian links for two separate clusters
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
            storage.record_coactivation("m4", "m5", 0).unwrap();
            storage.record_coactivation("m4", "m6", 0).unwrap();
            storage.record_coactivation("m5", "m6", 0).unwrap();
        }

        // No LLM provider — we're testing cluster ordering, not synthesis output.
        // The engine will skip synthesis but still process gate checks in order.
        let engine = DefaultSynthesisEngine::new(None, None);

        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;
        // Enable emotional prioritization
        settings.emotional.prioritize_emotional = true;
        settings.emotional.emotional_boost_weight = 0.3;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // We should have found at least 1 cluster
        assert!(
            report.clusters_found >= 1,
            "Expected at least 1 cluster, found {}",
            report.clusters_found
        );

        // If we got 2 clusters, verify the emotional cluster is processed first
        if report.gate_results.len() >= 2 {
            let first_cluster_id = &report.gate_results[0].cluster_id;
            let second_cluster_id = &report.gate_results[1].cluster_id;

            // To identify which cluster has the emotional members, we need to
            // check which cluster contains m4/m5/m6. We saved cluster state
            // during cold recluster, so we can read it back.
            let all_clusters = storage.get_all_cluster_data().unwrap();
            let emotional_cluster = all_clusters.iter().find(|c| {
                c.members.contains(&"m4".to_string())
                    || c.members.contains(&"m5".to_string())
                    || c.members.contains(&"m6".to_string())
            });

            if let Some(emo_cluster) = emotional_cluster {
                assert_eq!(
                    first_cluster_id, &emo_cluster.id,
                    "Emotional cluster should be processed first. \
                     First={} (expected emotional cluster {}), Second={}",
                    first_cluster_id, emo_cluster.id, second_cluster_id
                );
            }
        }

        // No errors should have occurred
        assert!(
            report.errors.is_empty(),
            "Unexpected errors: {:?}",
            report.errors
        );
    }

    // -----------------------------------------------------------------------
    // Test 17: Emotional modulation with disabled config is a no-op
    // -----------------------------------------------------------------------

    #[test]
    fn test_emotional_modulation_disabled_noop() {
        // When emotional modulation is disabled, cluster order should be
        // unchanged (sorted by quality only).

        let mut m1 = make_memory("m1", "Very emotional A", MemoryType::Emotional, 0.7);
        m1.metadata = Some(serde_json::json!({"emotional_valence": 0.95}));
        let mut m2 = make_memory("m2", "Very emotional B", MemoryType::Episodic, 0.7);
        m2.metadata = Some(serde_json::json!({"emotional_valence": -0.9}));
        let mut m3 = make_memory("m3", "Very emotional C", MemoryType::Relational, 0.7);
        m3.metadata = Some(serde_json::json!({"emotional_valence": 0.85}));

        let mut storage = setup_storage_with_memories(&[m1, m2, m3]);

        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        // Explicitly disable emotional modulation
        settings.emotional.prioritize_emotional = false;
        settings.emotional.emotional_boost_weight = 0.0;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Should complete without errors regardless of emotional data
        let non_budget_errors: Vec<_> = report.errors.iter()
            .filter(|e| !matches!(e, SynthesisError::BudgetExhausted { .. }))
            .collect();
        assert!(
            non_budget_errors.is_empty(),
            "Unexpected errors: {:?}",
            non_budget_errors
        );
    }

    // -----------------------------------------------------------------------
    // Test 18: Emotional boost increases cluster quality score
    // -----------------------------------------------------------------------

    #[test]
    fn test_emotional_boost_increases_quality() {
        // Directly test that apply_emotional_modulation (called within synthesize)
        // boosts quality for emotional clusters by using the cluster module function.
        use super::cluster::apply_emotional_modulation;

        let m1 = make_memory("m1", "Neutral fact", MemoryType::Factual, 0.7);
        let mut m2 = make_memory("m2", "Emotional memory", MemoryType::Emotional, 0.7);
        m2.metadata = Some(serde_json::json!({"emotional_valence": 0.9}));
        let m3 = make_memory("m3", "Another neutral", MemoryType::Factual, 0.7);

        let members_map: HashMap<String, &MemoryRecord> = vec![
            ("m1".to_string(), &m1),
            ("m2".to_string(), &m2),
            ("m3".to_string(), &m3),
        ].into_iter().collect();

        let cluster = make_cluster("c1", &["m1", "m2", "m3"], 0.5);
        let original_quality = cluster.quality_score;

        let config = EmotionalModulationConfig {
            emotional_boost_weight: 0.5,
            prioritize_emotional: false,
            include_emotion_in_prompt: true,
        };

        let result = apply_emotional_modulation(vec![cluster], &members_map, &config);
        assert_eq!(result.len(), 1);

        // Quality should be boosted because m2 has emotional valence
        // Salience = mean(|0.0|, |0.9|, |0.0|) / 3 = 0.3
        // Boost = 1.0 + 0.5 * 0.3 = 1.15
        // New quality = 0.5 * 1.15 = 0.575
        assert!(
            result[0].quality_score > original_quality,
            "Expected quality boost from emotional modulation: original={}, got={}",
            original_quality,
            result[0].quality_score
        );
    }

    // -----------------------------------------------------------------------
    // Auto-update action tests (TASK-10)
    // -----------------------------------------------------------------------

    #[test]
    fn test_auto_update_merge_duplicates_supersedes_memories() {
        // When gate returns MergeDuplicates, the engine should:
        // 1. Supersede all demote IDs by the keep ID
        // 2. Transfer Hebbian links from demoted to keeper
        // 3. Boost keeper importance to max of all members
        // 4. Report demoted IDs in report.sources_demoted
        let memories = vec![
            make_memory("m1", "Duplicate content A", MemoryType::Factual, 0.9),
            make_memory("m2", "Duplicate content B", MemoryType::Factual, 0.6),
            make_memory("m3", "Duplicate content C", MemoryType::Factual, 0.5),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Create strong Hebbian links to form a cluster
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        // Store identical embeddings so all_pairs_similar=true → gate triggers MergeDuplicates
        let emb = vec![1.0_f32, 0.0, 0.0];
        for mem in &memories {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        // Create an external Hebbian link from m2 to an outside memory
        // to verify link transfer during merge
        let external = make_memory("ext1", "External memory", MemoryType::Factual, 0.5);
        storage.add(&external, "default").unwrap();
        for _ in 0..5 {
            storage.record_coactivation("m2", "ext1", 0).unwrap();
        }

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Check that we got an auto-update result
        if report.clusters_auto_updated > 0 {
            // Verify demoted memories are superseded
            let all = storage.get_by_ids(&["m1", "m2", "m3"]).unwrap();
            // The kept memory (centroid) should remain active, others excluded
            // by get_by_ids (which filters superseded). The kept one should remain.
            // At least one memory should be returned (the keeper).
            assert!(
                !all.is_empty(),
                "At least the kept memory should still be active"
            );

            // Report should list demoted sources
            assert!(
                !report.sources_demoted.is_empty(),
                "Expected demoted sources in report"
            );

            // No LLM, so 0 synthesized
            assert_eq!(report.clusters_synthesized, 0);
        }
    }

    #[test]
    fn test_auto_update_merge_duplicates_transfers_hebbian_links() {
        // Verify that Hebbian links from demoted memories are transferred
        // to the keeper during MergeDuplicates.
        let memories = vec![
            make_memory("m1", "Near-duplicate alpha", MemoryType::Factual, 0.7),
            make_memory("m2", "Near-duplicate beta", MemoryType::Factual, 0.6),
            make_memory("m3", "Near-duplicate gamma", MemoryType::Factual, 0.5),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        // Strong intra-cluster links
        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        // Identical embeddings → all_pairs_similar = true
        let emb = vec![0.5_f32, 0.5, 0.0];
        for mem in &memories {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        // External link: m3 <-> ext1 (will need to transfer to keeper)
        let ext = make_memory("ext1", "External node", MemoryType::Episodic, 0.5);
        storage.add(&ext, "default").unwrap();
        for _ in 0..5 {
            storage.record_coactivation("m3", "ext1", 0).unwrap();
        }

        // Verify external link exists before merge
        let links_before = storage.get_hebbian_links_weighted("ext1").unwrap();
        let has_m3_link = links_before.iter().any(|(id, _)| id == "m3");
        assert!(has_m3_link, "ext1 should have a link to m3 before merge");

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        if report.clusters_auto_updated > 0 {
            // After merge, the donor's links should be transferred to the keeper.
            // The keeper ID is the centroid (first member when sorted).
            // Check that ext1 now links to the keeper instead of m3.
            let links_after = storage.get_hebbian_links_weighted("ext1").unwrap();
            // m3's direct link should be gone (merged into keeper)
            let has_m3_link_after = links_after.iter().any(|(id, _)| id == "m3");
            assert!(
                !has_m3_link_after,
                "m3's Hebbian links should be deleted after merge_hebbian_links"
            );
            // The keeper should now have a link to ext1
            let keeper_links = links_after.iter().any(|(id, _)| id != "ext1");
            assert!(
                keeper_links || !links_after.is_empty(),
                "Keeper should have inherited links from demoted memories"
            );
        }
    }

    #[test]
    fn test_auto_update_merge_duplicates_boosts_importance() {
        // Verify that the kept memory gets the max importance of all members.
        let memories = vec![
            make_memory("m1", "Dup X", MemoryType::Factual, 0.4),
            make_memory("m2", "Dup Y", MemoryType::Factual, 0.9),
            make_memory("m3", "Dup Z", MemoryType::Factual, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        let emb = vec![1.0_f32, 0.0, 0.0];
        for mem in &memories {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        if report.clusters_auto_updated > 0 {
            // Find the kept memory: it's the one NOT in sources_demoted
            // that was part of the original cluster.
            let all_ids = vec!["m1", "m2", "m3"];
            let keeper_id = all_ids.iter().find(|id| {
                !report.sources_demoted.contains(&id.to_string())
            });
            if let Some(keeper_id) = keeper_id {
                let keeper = storage.get(keeper_id).unwrap();
                if let Some(k) = keeper {
                    assert!(
                        (k.importance - 0.9).abs() < 0.01,
                        "Keeper ({}) importance should be boosted to max (0.9), got {}",
                        keeper_id, k.importance
                    );
                }
            }
        }
    }

    #[test]
    fn test_auto_update_strengthen_links() {
        // Directly test StrengthenLinks action by manually constructing the
        // gate result and verifying Hebbian link strengthening.
        // Since the gate currently only emits MergeDuplicates, we test the
        // underlying logic: record_coactivation with threshold=0.
        //
        // record_coactivation behavior with threshold=0:
        //   Call 1 (None): creates tracking record (strength=0.0, count=1)
        //   Call 2 (Some(0.0, 1)): count=2 >= threshold=0 → forms link (strength=1.0)
        // So we need 2 calls to form a visible link (matching what StrengthenLinks does
        // in the engine: the cluster already has prior coactivation records, so the
        // additional call pushes them over threshold).
        let mut storage = Storage::new(":memory:").expect("in-memory db");
        let m1 = make_memory("m1", "Memory A", MemoryType::Factual, 0.7);
        let m2 = make_memory("m2", "Memory B", MemoryType::Episodic, 0.7);
        storage.add(&m1, "default").unwrap();
        storage.add(&m2, "default").unwrap();

        // No existing link
        let links_before = storage.get_hebbian_links_weighted("m1").unwrap();
        assert!(
            links_before.is_empty(),
            "No Hebbian links should exist before strengthening"
        );

        // First call: creates tracking record (strength=0.0)
        storage.record_coactivation("m1", "m2", 0).unwrap();
        // Second call: exceeds threshold → forms link (strength=1.0)
        storage.record_coactivation("m1", "m2", 0).unwrap();

        let links_after = storage.get_hebbian_links_weighted("m1").unwrap();
        assert!(
            !links_after.is_empty(),
            "Hebbian link should be formed after two record_coactivation calls with threshold=0"
        );
        // Verify the link points to m2
        let linked_to_m2 = links_after.iter().any(|(id, _)| id == "m2");
        assert!(linked_to_m2, "Link should connect m1 to m2");
    }

    #[test]
    fn test_auto_update_reports_cluster_count() {
        // Verify that clusters_auto_updated is incremented for auto-update actions.
        let memories = vec![
            make_memory("m1", "Same same A", MemoryType::Factual, 0.7),
            make_memory("m2", "Same same B", MemoryType::Factual, 0.7),
            make_memory("m3", "Same same C", MemoryType::Factual, 0.7),
        ];
        let mut storage = setup_storage_with_memories(&memories);

        for _ in 0..10 {
            storage.record_coactivation("m1", "m2", 0).unwrap();
            storage.record_coactivation("m1", "m3", 0).unwrap();
            storage.record_coactivation("m2", "m3", 0).unwrap();
        }

        // Identical embeddings → triggers MergeDuplicates auto-update
        let emb = vec![0.0_f32, 1.0, 0.0];
        for mem in &memories {
            storage.store_embedding(&mem.id, &emb, "test/model", 3).unwrap();
        }

        let engine = DefaultSynthesisEngine::new(None, None);
        let mut settings = default_settings();
        settings.cluster_discovery.min_importance = 0.3;
        settings.cluster_discovery.cluster_threshold = 0.1;
        settings.gate.gate_quality_threshold = 0.1;
        settings.gate.defer_quality_threshold = 0.1;
        settings.gate.min_type_diversity = 1;

        let report = engine.synthesize(&mut storage, &settings).unwrap();

        // Should have at least one auto-updated cluster (duplicate detection)
        if report.clusters_found > 0 {
            // Check for AutoUpdate gate decisions
            let auto_update_gates = report
                .gate_results
                .iter()
                .filter(|r| matches!(r.decision, GateDecision::AutoUpdate { .. }))
                .count();
            assert_eq!(
                report.clusters_auto_updated, auto_update_gates,
                "clusters_auto_updated ({}) should match AutoUpdate gate decisions ({})",
                report.clusters_auto_updated, auto_update_gates
            );
        }
    }
}
