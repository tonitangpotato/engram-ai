//! Gate check module for the synthesis engine.
//!
//! Evaluates each [`MemoryCluster`] and decides whether to synthesize, auto-update,
//! defer, or skip. This module is pure — no storage calls, no side effects.
//! All external context (coverage, growth, similarity) is passed in as parameters.

use std::collections::HashSet;

use chrono::Utc;

use crate::synthesis::types::*;
use crate::types::{MemoryRecord, MemoryType};

// ---------------------------------------------------------------------------
// Cost estimation
// ---------------------------------------------------------------------------

/// Price per token, hardcoded at ~$1/M tokens.
const PRICE_PER_TOKEN: f64 = 0.000001;

/// Estimate LLM cost for synthesizing a cluster.
///
/// `token_count ≈ sum of member content lengths / 4`
/// `cost = token_count * PRICE_PER_TOKEN`
pub fn estimate_cost(members: &[MemoryRecord]) -> f64 {
    let total_chars: usize = members.iter().map(|m| m.content.len()).sum();
    let token_count = total_chars as f64 / 4.0;
    token_count * PRICE_PER_TOKEN
}

// ---------------------------------------------------------------------------
// Gate scores
// ---------------------------------------------------------------------------

/// Compute gate scores for telemetry.
fn compute_gate_scores(cluster: &MemoryCluster, members: &[MemoryRecord]) -> GateScores {
    let type_diversity = members
        .iter()
        .map(|m| m.memory_type)
        .collect::<HashSet<MemoryType>>()
        .len();

    GateScores {
        quality: cluster.quality_score,
        type_diversity,
        estimated_cost: estimate_cost(members),
        member_count: cluster.members.len(),
    }
}

// ---------------------------------------------------------------------------
// Gate check — 9 rules
// ---------------------------------------------------------------------------

/// Check whether a cluster should be synthesized.
///
/// # Parameters
/// - `cluster`: The cluster to evaluate.
/// - `members`: Full memory records for each cluster member.
/// - `config`: Gate configuration thresholds.
/// - `covered_pct`: Fraction of members already in synthesis provenance (0.0–1.0).
/// - `cluster_changed`: `true` if the cluster has grown since last attempt.
/// - `all_pairs_similar`: `true` if all pairwise embedding similarities exceed
///   `config.duplicate_similarity` (caller pre-computes from clustering data).
pub fn check_gate(
    cluster: &MemoryCluster,
    members: &[MemoryRecord],
    config: &GateConfig,
    covered_pct: f64,
    cluster_changed: bool,
    all_pairs_similar: bool,
) -> GateResult {
    let scores = compute_gate_scores(cluster, members);
    let cluster_id = cluster.id.clone();
    let n = cluster.members.len();
    let q = cluster.quality_score;

    // Helper to build a GateResult with a given decision.
    let make_result = |decision: GateDecision| GateResult {
        cluster_id: cluster_id.clone(),
        decision,
        scores: scores.clone(),
        timestamp: Utc::now(),
    };

    // Rule 1: Minimum cluster size
    if n < config.min_cluster_size {
        return make_result(GateDecision::Skip {
            reason: format!(
                "too small: {} members, min {}",
                n, config.min_cluster_size
            ),
        });
    }

    // Rule 2: Near-duplicate detection
    if all_pairs_similar {
        let keep = cluster.centroid_id.clone();
        let demote: Vec<String> = cluster
            .members
            .iter()
            .filter(|id| **id != keep)
            .cloned()
            .collect();
        return make_result(GateDecision::AutoUpdate {
            action: AutoUpdateAction::MergeDuplicates { keep, demote },
        });
    }

    // Rule 3: Quality threshold
    if q < config.gate_quality_threshold {
        return make_result(GateDecision::Skip {
            reason: format!(
                "low quality: {:.3} < {}",
                q, config.gate_quality_threshold
            ),
        });
    }

    // Rule 4: Minimum-size cluster with borderline quality → defer
    if n == config.min_cluster_size && q < config.defer_quality_threshold {
        return make_result(GateDecision::Defer {
            reason: format!(
                "minimum size with low quality: {:.3} < {}, needs more memories",
                q, config.defer_quality_threshold
            ),
        });
    }

    // Rule 5: Already covered by existing synthesis
    if covered_pct >= 0.80 {
        return make_result(GateDecision::Skip {
            reason: format!(
                "already covered: {:.0}% of members have existing insights",
                covered_pct * 100.0
            ),
        });
    }

    // Rule 6: No growth since last attempt
    if !cluster_changed {
        return make_result(GateDecision::Skip {
            reason: "no growth since last attempt".to_string(),
        });
    }

    // Rule 7: Type diversity check
    let distinct_types: HashSet<MemoryType> =
        members.iter().map(|m| m.memory_type).collect();

    if distinct_types.len() < config.min_type_diversity {
        // Check exceptions: all Factual or all Episodic with entity_overlap > 0.5
        let single_type = *distinct_types.iter().next().unwrap();
        let has_entity_exception = matches!(
            single_type,
            MemoryType::Factual | MemoryType::Episodic
        ) && cluster.signals_summary.entity_contribution > 0.5;

        if !has_entity_exception {
            return make_result(GateDecision::Skip {
                reason: format!("homogeneous: only {}", single_type),
            });
        }
    }

    // Rule 8: Cost threshold for non-premium clusters
    let cost = scores.estimated_cost;
    if cost > config.cost_threshold && q < config.premium_threshold {
        return make_result(GateDecision::Skip {
            reason: format!(
                "cost {:.4} exceeds threshold {} for non-premium quality {:.3}",
                cost, config.cost_threshold, q
            ),
        });
    }

    // Rule 9: Passed all gates → synthesize
    make_result(GateDecision::Synthesize {
        reason: format!(
            "passed all gates: quality={:.3}, diversity={}, cost={:.4}",
            q, scores.type_diversity, cost
        ),
    })
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
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_member(id: &str, content: &str, memory_type: MemoryType) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.5,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            metadata: None,
        }
    }

    fn make_members(n: usize, memory_type: MemoryType) -> Vec<MemoryRecord> {
        (0..n)
            .map(|i| {
                make_member(
                    &format!("mem-{}", i),
                    &format!("Memory content number {}", i),
                    memory_type,
                )
            })
            .collect()
    }

    fn make_diverse_members(n: usize) -> Vec<MemoryRecord> {
        let types = [MemoryType::Factual, MemoryType::Episodic, MemoryType::Relational];
        (0..n)
            .map(|i| {
                make_member(
                    &format!("mem-{}", i),
                    &format!("Memory content number {}", i),
                    types[i % types.len()],
                )
            })
            .collect()
    }

    fn make_cluster(members: &[MemoryRecord], quality_score: f64) -> MemoryCluster {
        make_cluster_with_signals(members, quality_score, default_signals())
    }

    fn default_signals() -> SignalsSummary {
        SignalsSummary {
            dominant_signal: ClusterSignal::Hebbian,
            hebbian_contribution: 0.4,
            entity_contribution: 0.3,
            embedding_contribution: 0.2,
            temporal_contribution: 0.1,
        }
    }

    fn make_cluster_with_signals(
        members: &[MemoryRecord],
        quality_score: f64,
        signals_summary: SignalsSummary,
    ) -> MemoryCluster {
        let mut member_ids: Vec<String> = members.iter().map(|m| m.id.clone()).collect();
        member_ids.sort();
        let centroid_id = member_ids.first().cloned().unwrap_or_default();
        MemoryCluster {
            id: format!("cluster-{}", member_ids.join("-")),
            members: member_ids,
            quality_score,
            centroid_id,
            signals_summary,
        }
    }

    fn default_config() -> GateConfig {
        GateConfig::default()
    }

    // -----------------------------------------------------------------------
    // Rule 1: Minimum cluster size
    // -----------------------------------------------------------------------

    #[test]
    fn rule1_skip_too_small() {
        let members = make_diverse_members(2); // default min is 3
        let cluster = make_cluster(&members, 0.8);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("too small"), "reason: {}", reason);
                assert!(reason.contains("2 members"), "reason: {}", reason);
                assert!(reason.contains("min 3"), "reason: {}", reason);
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 2: Near-duplicate detection → AutoUpdate
    // -----------------------------------------------------------------------

    #[test]
    fn rule2_auto_update_duplicates() {
        let members = make_diverse_members(3);
        let cluster = make_cluster(&members, 0.95);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, true);

        match &result.decision {
            GateDecision::AutoUpdate { action } => match action {
                AutoUpdateAction::MergeDuplicates { keep, demote } => {
                    assert_eq!(keep, &cluster.centroid_id);
                    assert_eq!(demote.len(), 2);
                    assert!(!demote.contains(keep));
                }
                other => panic!("expected MergeDuplicates, got {:?}", other),
            },
            other => panic!("expected AutoUpdate, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 3: Low quality → Skip
    // -----------------------------------------------------------------------

    #[test]
    fn rule3_skip_low_quality() {
        let members = make_diverse_members(4);
        let cluster = make_cluster(&members, 0.2); // below 0.4 threshold
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("low quality"), "reason: {}", reason);
                assert!(reason.contains("0.200"), "reason: {}", reason);
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 4: Minimum size with borderline quality → Defer
    // -----------------------------------------------------------------------

    #[test]
    fn rule4_defer_minimum_size_low_quality() {
        let members = make_diverse_members(3); // exactly min_cluster_size
        let cluster = make_cluster(&members, 0.45); // above gate (0.4) but below defer (0.6)
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Defer { reason } => {
                assert!(reason.contains("minimum size"), "reason: {}", reason);
                assert!(reason.contains("0.450"), "reason: {}", reason);
            }
            other => panic!("expected Defer, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 5: Already covered → Skip
    // -----------------------------------------------------------------------

    #[test]
    fn rule5_skip_already_covered() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.85, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("already covered"), "reason: {}", reason);
                assert!(reason.contains("85%"), "reason: {}", reason);
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 6: No growth since last attempt → Skip
    // -----------------------------------------------------------------------

    #[test]
    fn rule6_skip_no_growth() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, false, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(
                    reason.contains("no growth"),
                    "reason: {}",
                    reason
                );
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 7: Homogeneous types → Skip
    // -----------------------------------------------------------------------

    #[test]
    fn rule7_skip_homogeneous() {
        // All Procedural — no exception applies
        let members = make_members(4, MemoryType::Procedural);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("homogeneous"), "reason: {}", reason);
                assert!(reason.contains("procedural"), "reason: {}", reason);
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 7 Exception: All Factual with entity_overlap > 0.5 → allow
    // -----------------------------------------------------------------------

    #[test]
    fn rule7_exception_factual_entity_overlap() {
        let members = make_members(4, MemoryType::Factual);
        let signals = SignalsSummary {
            dominant_signal: ClusterSignal::Entity,
            hebbian_contribution: 0.1,
            entity_contribution: 0.6, // > 0.5
            embedding_contribution: 0.2,
            temporal_contribution: 0.1,
        };
        let cluster = make_cluster_with_signals(&members, 0.7, signals);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        // Should NOT be skipped for homogeneity — exception applies
        match &result.decision {
            GateDecision::Skip { reason } if reason.contains("homogeneous") => {
                panic!("factual+entity exception should have allowed this cluster");
            }
            _ => {} // any other decision is fine (could be Synthesize or something else)
        }
    }

    // -----------------------------------------------------------------------
    // Rule 7 Exception: All Episodic with entity_overlap > 0.5 → allow
    // -----------------------------------------------------------------------

    #[test]
    fn rule7_exception_episodic_entity_overlap() {
        let members = make_members(4, MemoryType::Episodic);
        let signals = SignalsSummary {
            dominant_signal: ClusterSignal::Entity,
            hebbian_contribution: 0.1,
            entity_contribution: 0.6, // > 0.5
            embedding_contribution: 0.2,
            temporal_contribution: 0.1,
        };
        let cluster = make_cluster_with_signals(&members, 0.7, signals);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        // Should NOT be skipped for homogeneity — exception applies
        match &result.decision {
            GateDecision::Skip { reason } if reason.contains("homogeneous") => {
                panic!("episodic+entity exception should have allowed this cluster");
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Rule 8: Cost exceeds threshold for non-premium quality → Skip
    // -----------------------------------------------------------------------

    #[test]
    fn rule8_skip_expensive_non_premium() {
        // Create members with very long content to exceed cost threshold
        let types = [MemoryType::Factual, MemoryType::Episodic, MemoryType::Relational];
        let members: Vec<MemoryRecord> = (0..5)
            .map(|i| {
                make_member(
                    &format!("mem-{}", i),
                    // ~50k chars each → ~12500 tokens each → ~62500 tokens total
                    // cost = 62500 * 0.000001 = 0.0625 > 0.05 threshold
                    &"x".repeat(50_000),
                    types[i % types.len()],
                )
            })
            .collect();
        let cluster = make_cluster(&members, 0.7); // below premium_threshold (0.8)
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("cost"), "reason: {}", reason);
                assert!(reason.contains("non-premium"), "reason: {}", reason);
            }
            other => panic!("expected Skip for cost, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Rule 9: Passed all gates → Synthesize
    // -----------------------------------------------------------------------

    #[test]
    fn rule9_synthesize_passes_all() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        match &result.decision {
            GateDecision::Synthesize { reason } => {
                assert!(reason.contains("passed all gates"), "reason: {}", reason);
                assert!(reason.contains("quality=0.700"), "reason: {}", reason);
            }
            other => panic!("expected Synthesize, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Cost estimation
    // -----------------------------------------------------------------------

    #[test]
    fn test_estimate_cost() {
        // 3 members with 40-char content each = 120 chars → 30 tokens → $0.00003
        let members = make_members(3, MemoryType::Factual);
        let cost = estimate_cost(&members);
        assert!(cost > 0.0, "cost should be positive");

        // Each "Memory content number X" is ~24 chars, * 3 ≈ 72 chars → 18 tokens
        let expected_chars: usize = members.iter().map(|m| m.content.len()).sum();
        let expected = (expected_chars as f64 / 4.0) * PRICE_PER_TOKEN;
        assert!(
            (cost - expected).abs() < f64::EPSILON,
            "cost={}, expected={}",
            cost,
            expected
        );
    }

    #[test]
    fn test_estimate_cost_empty() {
        let members: Vec<MemoryRecord> = vec![];
        let cost = estimate_cost(&members);
        assert_eq!(cost, 0.0);
    }

    // -----------------------------------------------------------------------
    // Gate scores computation
    // -----------------------------------------------------------------------

    #[test]
    fn test_gate_scores() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.75);

        let scores = compute_gate_scores(&cluster, &members);

        assert_eq!(scores.quality, 0.75);
        assert_eq!(scores.member_count, 5);
        // 5 members cycling through 3 types → 3 distinct types
        assert_eq!(scores.type_diversity, 3);
        assert!(scores.estimated_cost > 0.0);
    }

    // -----------------------------------------------------------------------
    // Edge case: Premium quality bypasses cost gate
    // -----------------------------------------------------------------------

    #[test]
    fn edge_case_premium_quality_bypasses_cost() {
        let types = [MemoryType::Factual, MemoryType::Episodic, MemoryType::Relational];
        let members: Vec<MemoryRecord> = (0..5)
            .map(|i| {
                make_member(
                    &format!("mem-{}", i),
                    &"x".repeat(50_000), // expensive
                    types[i % types.len()],
                )
            })
            .collect();
        let cluster = make_cluster(&members, 0.85); // above premium_threshold (0.8)
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        // Should NOT be skipped for cost — premium quality bypasses Rule 8
        match &result.decision {
            GateDecision::Synthesize { reason } => {
                assert!(reason.contains("passed all gates"), "reason: {}", reason);
            }
            other => panic!("expected Synthesize for premium quality, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Edge case: Rule 4 doesn't trigger when size > min
    // -----------------------------------------------------------------------

    #[test]
    fn edge_case_defer_only_at_min_size() {
        // 4 members (above min of 3) with quality between gate and defer thresholds
        let members = make_diverse_members(4);
        let cluster = make_cluster(&members, 0.45);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.0, true, false);

        // Rule 4 only applies when n == min_cluster_size, so this should pass
        match &result.decision {
            GateDecision::Defer { .. } => {
                panic!("Defer should only apply at exactly min_cluster_size");
            }
            _ => {} // Synthesize or other is fine
        }
    }

    // -----------------------------------------------------------------------
    // Edge case: Exactly 80% coverage triggers skip
    // -----------------------------------------------------------------------

    #[test]
    fn edge_case_coverage_exactly_80_percent() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.80, true, false);

        match &result.decision {
            GateDecision::Skip { reason } => {
                assert!(reason.contains("already covered"), "reason: {}", reason);
            }
            other => panic!("expected Skip at 80% coverage, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Edge case: Coverage just below 80% does NOT skip
    // -----------------------------------------------------------------------

    #[test]
    fn edge_case_coverage_below_80_percent() {
        let members = make_diverse_members(5);
        let cluster = make_cluster(&members, 0.7);
        let config = default_config();

        let result = check_gate(&cluster, &members, &config, 0.79, true, false);

        match &result.decision {
            GateDecision::Skip { reason } if reason.contains("already covered") => {
                panic!("should not skip at 79% coverage");
            }
            _ => {} // anything else is fine
        }
    }
}
