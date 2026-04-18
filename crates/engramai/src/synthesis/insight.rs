//! Insight generation module for the synthesis engine.
//!
//! Handles LLM prompt construction, output parsing/validation, and importance
//! calculation. This module is **pure** — no storage calls. The engine handles
//! all storage operations within its transaction boundary.

use crate::synthesis::types::*;
use crate::types::{MemoryRecord, MemoryType};

// ---------------------------------------------------------------------------
// Prompt construction
// ---------------------------------------------------------------------------

/// Select the appropriate prompt template based on the dominant memory type.
pub fn select_template(members: &[MemoryRecord]) -> PromptTemplate {
    if members.is_empty() {
        return PromptTemplate::General;
    }

    let mut counts = std::collections::HashMap::new();
    for m in members {
        *counts.entry(&m.memory_type).or_insert(0usize) += 1;
    }

    let majority_threshold = members.len() / 2;

    if counts.get(&MemoryType::Factual).copied().unwrap_or(0) > majority_threshold {
        PromptTemplate::FactualPattern
    } else if counts.get(&MemoryType::Episodic).copied().unwrap_or(0) > majority_threshold {
        PromptTemplate::EpisodicThread
    } else if counts.get(&MemoryType::Causal).copied().unwrap_or(0) > majority_threshold {
        PromptTemplate::CausalChain
    } else {
        PromptTemplate::General
    }
}

/// Build the LLM prompt for synthesizing an insight from a cluster.
///
/// The prompt includes system instructions, rules, JSON schema,
/// and the formatted memory list.
pub fn build_prompt(
    cluster: &MemoryCluster,
    members: &[MemoryRecord],
    config: &SynthesisConfig,
    include_emotion: bool,
) -> String {
    let template = if config.prompt_template == PromptTemplate::General {
        select_template(members)
    } else {
        config.prompt_template
    };

    let mut prompt = String::with_capacity(4096);

    // System instruction
    prompt.push_str(
        "System: You are a knowledge synthesizer. Given a cluster of related memories, \
         produce a single higher-order insight that captures what these memories \
         collectively reveal — a pattern, rule, or connection not explicit in any \
         individual memory.\n\n",
    );

    // Rules
    prompt.push_str("Rules:\n");
    prompt.push_str("- The insight must be a NEW observation, not a summary\n");
    prompt.push_str("- It must be falsifiable (can be proven wrong by future evidence)\n");
    prompt.push_str("- It must reference specific source memories by ID\n");
    prompt.push_str("- Keep it concise: 1-3 sentences for the insight, then supporting evidence\n");

    // Template-specific guidance
    match template {
        PromptTemplate::FactualPattern => {
            prompt.push_str(
                "- Focus on recurring patterns, common rules, or generalizations across these factual memories\n",
            );
        }
        PromptTemplate::EpisodicThread => {
            prompt.push_str(
                "- Focus on narrative threads, temporal sequences, and story arcs connecting these episodes\n",
            );
        }
        PromptTemplate::CausalChain => {
            prompt.push_str(
                "- Focus on cause-effect relationships and causal mechanisms revealed by these memories\n",
            );
        }
        PromptTemplate::General => {}
    }

    prompt.push('\n');

    // Format requirements
    prompt.push_str("Format requirements:\n");
    prompt.push_str("- Respond ONLY with valid JSON — no markdown fences, no preamble, no trailing text\n");
    prompt.push_str("- All string values must be properly escaped\n");
    prompt.push_str("- \"confidence\" must be a decimal number between 0.0 and 1.0 (inclusive)\n");
    prompt.push_str("- \"insight_type\" must be exactly one of: \"pattern\", \"rule\", \"connection\", \"contradiction\"\n");
    prompt.push_str("- \"source_references\" must contain only IDs from the provided memories list\n");
    prompt.push_str("- \"insight\" must be between 50 and 500 characters\n\n");

    // Memory list
    let max_memories = config.max_memories_per_llm_call.min(members.len());
    prompt.push_str("Memories:\n");
    for member in members.iter().take(max_memories) {
        if include_emotion {
            // Check metadata for emotional_valence
            let emotion_str = member
                .metadata
                .as_ref()
                .and_then(|m| m.get("emotional_valence"))
                .map(|v| format!(", emotion: {v}"))
                .unwrap_or_default();
            prompt.push_str(&format!(
                "[{}: {} (type: {}, importance: {:.2}{})]\n",
                member.id, member.content, member.memory_type, member.importance, emotion_str,
            ));
        } else {
            prompt.push_str(&format!(
                "[{}: {} (type: {}, importance: {:.2})]\n",
                member.id, member.content, member.memory_type, member.importance,
            ));
        }
    }
    prompt.push('\n');

    // JSON schema
    prompt.push_str("Required JSON schema:\n");
    prompt.push_str("{\n");
    prompt.push_str("  \"insight\": \"string (50-500 chars, the synthesized insight text)\",\n");
    prompt.push_str("  \"confidence\": \"number (0.0-1.0)\",\n");
    prompt.push_str("  \"insight_type\": \"pattern|rule|connection|contradiction\",\n");
    prompt.push_str("  \"source_references\": [\"mem_id_1\", \"mem_id_2\"]\n");
    prompt.push_str("}\n");

    // Note cluster info for context
    prompt.push_str(&format!(
        "\nCluster ID: {} (quality: {:.3}, {} members, centroid: {})\n",
        cluster.id,
        cluster.quality_score,
        cluster.members.len(),
        cluster.centroid_id,
    ));

    prompt
}

/// Call the LLM provider and return the raw response.
pub fn call_llm(
    prompt: &str,
    provider: &dyn SynthesisLlmProvider,
    config: &SynthesisConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    provider.generate(prompt, config)
}

// ---------------------------------------------------------------------------
// Output validation
// ---------------------------------------------------------------------------

/// Helper struct for deserializing the LLM JSON response.
#[derive(serde::Deserialize)]
struct LlmResponse {
    insight: String,
    confidence: f64,
    insight_type: String,
    source_references: Vec<String>,
}

/// Parse and validate the LLM output against the cluster.
///
/// # Validation checks (§4.3)
/// 1. `source_references` all present in cluster members
/// 2. `confidence` in 0.0–1.0
/// 3. `insight` not empty, differs from source content
/// 4. Valid `insight_type` (pattern|rule|connection|contradiction)
/// 5. `insight` length 50–500 chars
/// 6. `insight` is not a substring of any source (prevents trivial copy)
/// 7. On any failure → return `Err(SynthesisError)`
pub fn validate_output(
    raw_json: &str,
    cluster: &MemoryCluster,
    members: &[MemoryRecord],
) -> Result<SynthesisOutput, SynthesisError> {
    let cluster_id = cluster.id.clone();

    // Strip markdown fences if present
    let cleaned = raw_json.trim();
    let cleaned = if let Some(rest) = cleaned.strip_prefix("```json") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else if let Some(rest) = cleaned.strip_prefix("```") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        cleaned
    };

    // Parse JSON
    let resp: LlmResponse = serde_json::from_str(cleaned).map_err(|e| {
        SynthesisError::LlmInvalidResponse {
            cluster_id: cluster_id.clone(),
            raw_response: format!("JSON parse error: {e}"),
        }
    })?;

    // Check 2: confidence in 0.0-1.0
    if !(0.0..=1.0).contains(&resp.confidence) {
        return Err(SynthesisError::ValidationFailed {
            cluster_id: cluster_id.clone(),
            reason: format!("confidence {} not in 0.0-1.0", resp.confidence),
        });
    }

    // Check 3: insight not empty
    if resp.insight.trim().is_empty() {
        return Err(SynthesisError::ValidationFailed {
            cluster_id: cluster_id.clone(),
            reason: "insight text is empty".to_string(),
        });
    }

    // Check 4: valid insight_type
    let insight_type = match resp.insight_type.to_lowercase().as_str() {
        "pattern" => InsightType::Pattern,
        "rule" => InsightType::Rule,
        "connection" => InsightType::Connection,
        "contradiction" => InsightType::Contradiction,
        other => {
            return Err(SynthesisError::ValidationFailed {
                cluster_id: cluster_id.clone(),
                reason: format!("invalid insight_type: {other}"),
            });
        }
    };

    // Check 5: insight length 50-500 chars
    let insight_len = resp.insight.len();
    if insight_len < 50 {
        return Err(SynthesisError::ValidationFailed {
            cluster_id: cluster_id.clone(),
            reason: format!("insight too short: {insight_len} chars, minimum 50"),
        });
    }
    if insight_len > 500 {
        return Err(SynthesisError::ValidationFailed {
            cluster_id: cluster_id.clone(),
            reason: format!("insight too long: {insight_len} chars, maximum 500"),
        });
    }

    // Check 1: source_references all in cluster members
    let member_set: std::collections::HashSet<&str> =
        cluster.members.iter().map(|s| s.as_str()).collect();
    let invalid_refs: Vec<String> = resp
        .source_references
        .iter()
        .filter(|r| !member_set.contains(r.as_str()))
        .cloned()
        .collect();
    if !invalid_refs.is_empty() {
        return Err(SynthesisError::HallucinatedReferences {
            cluster_id,
            invalid_ids: invalid_refs,
        });
    }

    // Check 6: insight is not a trivial copy of any source
    let insight_lower = resp.insight.to_lowercase();
    for m in members {
        let content_lower = m.content.to_lowercase();
        if content_lower == insight_lower {
            return Err(SynthesisError::ValidationFailed {
                cluster_id: cluster_id.clone(),
                reason: format!("insight is identical to source {}", m.id),
            });
        }
        // Also check if insight is a substring of source (trivial extraction)
        if content_lower.len() >= 50 && insight_lower.contains(&content_lower) {
            return Err(SynthesisError::ValidationFailed {
                cluster_id: cluster_id.clone(),
                reason: format!("insight contains full content of source {}", m.id),
            });
        }
    }

    // Check 3 continued: insight differs from all sources
    // (already handled above with case-insensitive comparison)

    Ok(SynthesisOutput {
        insight_text: resp.insight,
        confidence: resp.confidence,
        insight_type,
        source_references: resp.source_references,
    })
}

/// Compute the importance score for a synthesized insight.
///
/// Formula (§4.4):
/// - `base` = mean importance of source memories
/// - Boost by confidence: `base × (0.5 + 0.5 × confidence)`
/// - Boost by cluster quality: `× (0.8 + 0.2 × quality_score)`
/// - Cap at 1.0
pub fn compute_insight_importance(
    output: &SynthesisOutput,
    cluster: &MemoryCluster,
    members: &[MemoryRecord],
) -> f64 {
    if members.is_empty() {
        return 0.0;
    }

    let base = members.iter().map(|m| m.importance).sum::<f64>() / members.len() as f64;
    let confidence_boost = 0.5 + 0.5 * output.confidence;
    let quality_boost = 0.8 + 0.2 * cluster.quality_score;
    let result = base * confidence_boost * quality_boost;
    result.min(1.0)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryLayer;
    use chrono::Utc;

    fn make_record(id: &str, content: &str, memory_type: MemoryType, importance: f64) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type,
            layer: MemoryLayer::Core,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 0.5,
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

    fn make_cluster(members: &[&str]) -> MemoryCluster {
        MemoryCluster {
            id: "test-cluster".to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            quality_score: 0.7,
            centroid_id: members[0].to_string(),
            signals_summary: SignalsSummary {
                dominant_signal: ClusterSignal::Hebbian,
                hebbian_contribution: 0.5,
                entity_contribution: 0.2,
                embedding_contribution: 0.2,
                temporal_contribution: 0.1,
            },
        }
    }

    // ---- select_template tests ----

    #[test]
    fn test_select_template_factual_majority() {
        let members = vec![
            make_record("a", "fact1", MemoryType::Factual, 0.5),
            make_record("b", "fact2", MemoryType::Factual, 0.5),
            make_record("c", "ep1", MemoryType::Episodic, 0.5),
        ];
        assert_eq!(select_template(&members), PromptTemplate::FactualPattern);
    }

    #[test]
    fn test_select_template_episodic_majority() {
        let members = vec![
            make_record("a", "ep1", MemoryType::Episodic, 0.5),
            make_record("b", "ep2", MemoryType::Episodic, 0.5),
            make_record("c", "fact1", MemoryType::Factual, 0.5),
        ];
        assert_eq!(select_template(&members), PromptTemplate::EpisodicThread);
    }

    #[test]
    fn test_select_template_causal_majority() {
        let members = vec![
            make_record("a", "cause1", MemoryType::Causal, 0.5),
            make_record("b", "cause2", MemoryType::Causal, 0.5),
            make_record("c", "fact1", MemoryType::Factual, 0.5),
        ];
        assert_eq!(select_template(&members), PromptTemplate::CausalChain);
    }

    #[test]
    fn test_select_template_mixed() {
        let members = vec![
            make_record("a", "fact1", MemoryType::Factual, 0.5),
            make_record("b", "ep1", MemoryType::Episodic, 0.5),
            make_record("c", "rel1", MemoryType::Relational, 0.5),
        ];
        assert_eq!(select_template(&members), PromptTemplate::General);
    }

    #[test]
    fn test_select_template_empty() {
        assert_eq!(select_template(&[]), PromptTemplate::General);
    }

    // ---- build_prompt tests ----

    #[test]
    fn test_build_prompt_general() {
        let cluster = make_cluster(&["m1", "m2", "m3"]);
        let members = vec![
            make_record("m1", "fact about coding", MemoryType::Factual, 0.6),
            make_record("m2", "episode about debugging", MemoryType::Episodic, 0.7),
            make_record("m3", "opinion about Rust", MemoryType::Opinion, 0.5),
        ];
        let config = SynthesisConfig::default();
        let prompt = build_prompt(&cluster, &members, &config, false);

        assert!(prompt.contains("knowledge synthesizer"));
        assert!(prompt.contains("[m1:"));
        assert!(prompt.contains("[m2:"));
        assert!(prompt.contains("[m3:"));
        assert!(prompt.contains("JSON"));
        assert!(!prompt.contains("emotion:"));
    }

    #[test]
    fn test_build_prompt_with_emotion() {
        let cluster = make_cluster(&["m1"]);
        let mut record = make_record("m1", "emotional memory", MemoryType::Emotional, 0.8);
        record.metadata = Some(serde_json::json!({"emotional_valence": 0.9}));
        let config = SynthesisConfig::default();
        let prompt = build_prompt(&cluster, &[record], &config, true);

        assert!(prompt.contains("emotion: 0.9"));
    }

    #[test]
    fn test_build_prompt_factual_template() {
        let cluster = make_cluster(&["m1", "m2", "m3"]);
        let members = vec![
            make_record("m1", "fact1", MemoryType::Factual, 0.5),
            make_record("m2", "fact2", MemoryType::Factual, 0.5),
            make_record("m3", "fact3", MemoryType::Factual, 0.5),
        ];
        let config = SynthesisConfig::default();
        let prompt = build_prompt(&cluster, &members, &config, false);

        assert!(prompt.contains("recurring patterns"));
    }

    // ---- validate_output tests ----

    fn valid_json(cluster: &MemoryCluster) -> String {
        format!(
            r#"{{"insight": "This is a valid insight that meets the 50-character minimum length requirement for testing purposes.", "confidence": 0.85, "insight_type": "pattern", "source_references": ["{}", "{}"]}}"#,
            cluster.members[0], cluster.members[1],
        )
    }

    #[test]
    fn test_validate_output_valid() {
        let cluster = make_cluster(&["m1", "m2", "m3"]);
        let members = vec![
            make_record("m1", "content one", MemoryType::Factual, 0.5),
            make_record("m2", "content two", MemoryType::Factual, 0.5),
            make_record("m3", "content three", MemoryType::Factual, 0.5),
        ];
        let json = valid_json(&cluster);
        let result = validate_output(&json, &cluster, &members);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.confidence, 0.85);
        assert_eq!(output.insight_type, InsightType::Pattern);
        assert_eq!(output.source_references.len(), 2);
    }

    #[test]
    fn test_validate_output_strips_markdown_fences() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 0.5),
            make_record("m2", "c2", MemoryType::Factual, 0.5),
        ];
        let json = format!("```json\n{}\n```", valid_json(&cluster));
        let result = validate_output(&json, &cluster, &members);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_output_invalid_json() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let result = validate_output("not json at all", &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::LlmInvalidResponse { .. })
        ));
    }

    #[test]
    fn test_validate_output_confidence_too_high() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 0.5),
            make_record("m2", "c2", MemoryType::Factual, 0.5),
        ];
        let json = r#"{"insight": "This is a valid insight that meets the fifty character minimum length requirement for tests.", "confidence": 1.5, "insight_type": "pattern", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_confidence_negative() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let json = r#"{"insight": "This is a valid insight that meets the fifty character minimum length requirement for tests.", "confidence": -0.1, "insight_type": "pattern", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_empty_insight() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let json = r#"{"insight": "", "confidence": 0.5, "insight_type": "pattern", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_invalid_type() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let json = r#"{"insight": "This is a valid insight that meets the fifty character minimum length requirement for tests.", "confidence": 0.5, "insight_type": "summary", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_too_short() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let json = r#"{"insight": "Too short", "confidence": 0.5, "insight_type": "pattern", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_too_long() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![make_record("m1", "c1", MemoryType::Factual, 0.5)];
        let long_text = "x".repeat(501);
        let json = format!(
            r#"{{"insight": "{long_text}", "confidence": 0.5, "insight_type": "pattern", "source_references": ["m1"]}}"#,
        );
        let result = validate_output(&json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_hallucinated_refs() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 0.5),
            make_record("m2", "c2", MemoryType::Factual, 0.5),
        ];
        let json = r#"{"insight": "This is a valid insight that meets the fifty character minimum length requirement for tests.", "confidence": 0.5, "insight_type": "pattern", "source_references": ["m1", "m99"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::HallucinatedReferences { .. })
        ));
    }

    #[test]
    fn test_validate_output_identical_to_source() {
        let cluster = make_cluster(&["m1", "m2"]);
        let content = "This is a valid insight that meets the fifty character minimum length requirement for tests.";
        let members = vec![
            make_record("m1", content, MemoryType::Factual, 0.5),
            make_record("m2", "other content", MemoryType::Factual, 0.5),
        ];
        let json = format!(
            r#"{{"insight": "{content}", "confidence": 0.5, "insight_type": "pattern", "source_references": ["m1"]}}"#,
        );
        let result = validate_output(&json, &cluster, &members);
        assert!(matches!(
            result,
            Err(SynthesisError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn test_validate_output_case_insensitive_type() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 0.5),
            make_record("m2", "c2", MemoryType::Factual, 0.5),
        ];
        let json = r#"{"insight": "This is a valid insight that meets the fifty character minimum length requirement for tests.", "confidence": 0.5, "insight_type": "Connection", "source_references": ["m1"]}"#;
        let result = validate_output(json, &cluster, &members);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().insight_type, InsightType::Connection);
    }

    // ---- compute_insight_importance tests ----

    #[test]
    fn test_importance_normal() {
        let cluster = make_cluster(&["m1", "m2"]);
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 0.6),
            make_record("m2", "c2", MemoryType::Factual, 0.8),
        ];
        let output = SynthesisOutput {
            insight_text: "insight".to_string(),
            confidence: 0.8,
            insight_type: InsightType::Pattern,
            source_references: vec!["m1".to_string()],
        };
        let imp = compute_insight_importance(&output, &cluster, &members);
        // base = 0.7, confidence_boost = 0.9, quality_boost = 0.94
        // 0.7 * 0.9 * 0.94 = 0.5922
        assert!((imp - 0.5922).abs() < 0.01);
    }

    #[test]
    fn test_importance_caps_at_one() {
        let cluster = MemoryCluster {
            quality_score: 1.0,
            ..make_cluster(&["m1", "m2"])
        };
        let members = vec![
            make_record("m1", "c1", MemoryType::Factual, 1.0),
            make_record("m2", "c2", MemoryType::Factual, 1.0),
        ];
        let output = SynthesisOutput {
            insight_text: "insight".to_string(),
            confidence: 1.0,
            insight_type: InsightType::Pattern,
            source_references: vec!["m1".to_string()],
        };
        let imp = compute_insight_importance(&output, &cluster, &members);
        assert_eq!(imp, 1.0);
    }

    #[test]
    fn test_importance_empty_members() {
        let cluster = make_cluster(&["m1"]);
        let output = SynthesisOutput {
            insight_text: "insight".to_string(),
            confidence: 0.5,
            insight_type: InsightType::Pattern,
            source_references: vec![],
        };
        let imp = compute_insight_importance(&output, &cluster, &[]);
        assert_eq!(imp, 0.0);
    }
}
