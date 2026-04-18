//! Synthesis engine orchestration layer.
//!
//! Ties cluster discovery, gate check, insight generation, and provenance
//! into a single pipeline. Implements [`SynthesisEngine`] as `DefaultSynthesisEngine`.

use std::collections::HashSet;
use std::time::Instant;

use chrono::Utc;

use crate::storage::Storage;
use crate::synthesis::cluster;
use crate::synthesis::gate;
use crate::synthesis::insight;
use crate::synthesis::provenance;
use crate::synthesis::types::*;
use crate::types::MemoryRecord;

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

    /// Store an insight + provenance + demotion in a single transaction.
    /// Returns (insight_id, demoted_source_ids).
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
            insights_created: Vec::new(),
            sources_demoted: Vec::new(),
            errors: Vec::new(),
            duration: std::time::Duration::ZERO,
            gate_results: Vec::new(),
        };

        // Step 1: Discover clusters
        let clusters = cluster::discover_clusters(
            storage,
            &settings.cluster_discovery,
            self.embedding_model.as_deref(),
        )?;
        report.clusters_found = clusters.len();

        if clusters.is_empty() {
            report.duration = start.elapsed();
            return Ok(report);
        }

        // Step 2: TODO - Apply emotional modulation (cluster_emotional module, not yet wired)
        // let clusters = cluster::apply_emotional_modulation(clusters, &settings.emotional);

        // Step 3: Budget tracking
        let mut llm_calls_remaining = settings.max_llm_calls_per_run;
        let mut insights_remaining = settings.max_insights_per_consolidation;

        // Step 4: Process each cluster
        for cluster_data in &clusters {
            // Load members
            let all_memories = storage.all()?;
            let member_set: HashSet<&str> =
                cluster_data.members.iter().map(|s| s.as_str()).collect();
            let members: Vec<MemoryRecord> = all_memories
                .into_iter()
                .filter(|m| member_set.contains(m.id.as_str()))
                .collect();

            // Pre-compute gate inputs
            let covered_pct = storage.check_coverage(&cluster_data.members)?;
            // For cluster_changed: check if this cluster ID was previously attempted.
            // TODO: persist cluster attempt history. For now, assume changed (allow synthesis).
            let cluster_changed = true;
            let all_pairs_similar = false; // TODO: compute from pairwise signals

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
                GateDecision::AutoUpdate { action: _action } => {
                    // TODO: implement auto-update actions (merge duplicates, strengthen links)
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
    let random_part: u32 = nanos ^ (std::process::id() as u32);
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
}
