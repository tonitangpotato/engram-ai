//! Regulation Layer — Generate advisory actions from interoceptive state.
//!
//! Pure rule-based (no LLM). Examines domain states and signal patterns
//! to produce [`RegulationAction`] suggestions. The caller (RustClaw hooks)
//! decides whether to act on them.
//!
//! Design reference: INTEROCEPTIVE-LAYER.md §5.2 Layer 3

use crate::interoceptive::types::{
    AlertSeverity, DomainState, InteroceptiveState, RegulationAction,
};

/// Configuration for regulation thresholds.
#[derive(Debug, Clone)]
pub struct RegulationConfig {
    /// Valence below this triggers SoulUpdateSuggestion.
    pub negative_valence_threshold: f64,
    /// Minimum signals with negative valence before triggering.
    pub negative_valence_min_signals: u64,
    /// Confidence below this triggers RetrievalAdjustment.
    pub low_confidence_threshold: f64,
    /// Action success rate below this triggers BehaviorShift.
    pub low_success_threshold: f64,
    /// Anomaly level above this across multiple domains triggers Alert.
    pub high_anomaly_threshold: f64,
    /// Alignment below this triggers SoulUpdateSuggestion.
    pub low_alignment_threshold: f64,
    /// Minimum domains with high anomaly to trigger a high-severity alert.
    pub multi_anomaly_domain_count: usize,
}

impl Default for RegulationConfig {
    fn default() -> Self {
        Self {
            negative_valence_threshold: -0.3,
            negative_valence_min_signals: 5,
            low_confidence_threshold: 0.4,
            low_success_threshold: 0.3,
            high_anomaly_threshold: 2.0,
            low_alignment_threshold: 0.3,
            multi_anomaly_domain_count: 2,
        }
    }
}

/// Evaluate the current interoceptive state and generate regulation actions.
///
/// Returns a list of advisory actions. Empty list = system is nominal.
pub fn evaluate(state: &InteroceptiveState, config: &RegulationConfig) -> Vec<RegulationAction> {
    let mut actions = Vec::new();

    for ds in state.domain_states.values() {
        check_negative_valence(ds, config, &mut actions);
        check_low_confidence(ds, config, &mut actions);
        check_low_success_rate(ds, config, &mut actions);
        check_low_alignment(ds, config, &mut actions);
    }

    check_multi_domain_anomaly(state, config, &mut actions);

    actions
}

/// Rule 1: Persistent negative valence → suggest SOUL update.
fn check_negative_valence(
    ds: &DomainState,
    config: &RegulationConfig,
    actions: &mut Vec<RegulationAction>,
) {
    if ds.valence_trend < config.negative_valence_threshold
        && ds.signal_count >= config.negative_valence_min_signals
    {
        actions.push(RegulationAction::SoulUpdateSuggestion {
            domain: ds.domain.clone(),
            reason: format!(
                "Persistent negative trend in '{}': valence {:.2} over {} signals",
                ds.domain, ds.valence_trend, ds.signal_count
            ),
            valence_trend: ds.valence_trend,
        });
    }
}

/// Rule 2: Low confidence → suggest expanding retrieval.
fn check_low_confidence(
    ds: &DomainState,
    config: &RegulationConfig,
    actions: &mut Vec<RegulationAction>,
) {
    if ds.confidence < config.low_confidence_threshold
        && ds.signal_count >= 3 // need enough data
    {
        actions.push(RegulationAction::RetrievalAdjustment {
            expand_search: true,
            reason: format!(
                "Low confidence in '{}': {:.0}% — consider broadening recall",
                ds.domain,
                ds.confidence * 100.0
            ),
        });
    }
}

/// Rule 3: Low action success rate → suggest behavior shift.
fn check_low_success_rate(
    ds: &DomainState,
    config: &RegulationConfig,
    actions: &mut Vec<RegulationAction>,
) {
    if ds.action_success_rate < config.low_success_threshold
        && ds.signal_count >= 5 // need enough data
    {
        actions.push(RegulationAction::BehaviorShift {
            action: ds.domain.clone(),
            recommendation: format!(
                "Success rate for '{}' is {:.0}% — consider changing approach",
                ds.domain,
                ds.action_success_rate * 100.0
            ),
            success_rate: ds.action_success_rate,
        });
    }
}

/// Rule 4: Low alignment → suggest SOUL update.
fn check_low_alignment(
    ds: &DomainState,
    config: &RegulationConfig,
    actions: &mut Vec<RegulationAction>,
) {
    if ds.alignment_score < config.low_alignment_threshold
        && ds.signal_count >= 5
    {
        actions.push(RegulationAction::SoulUpdateSuggestion {
            domain: ds.domain.clone(),
            reason: format!(
                "Low drive alignment in '{}': {:.0}% — activities may not match SOUL drives",
                ds.domain,
                ds.alignment_score * 100.0
            ),
            valence_trend: ds.valence_trend,
        });
    }
}

/// Rule 5: Multiple domains with high anomaly → alert.
fn check_multi_domain_anomaly(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    actions: &mut Vec<RegulationAction>,
) {
    let anomalous_domains: Vec<String> = state
        .domain_states
        .values()
        .filter(|ds| ds.anomaly_level > config.high_anomaly_threshold)
        .map(|ds| ds.domain.clone())
        .collect();

    if anomalous_domains.len() >= config.multi_anomaly_domain_count {
        actions.push(RegulationAction::Alert {
            severity: AlertSeverity::High,
            message: format!(
                "Multiple domains showing anomalous patterns: {}",
                anomalous_domains.join(", ")
            ),
            domains: anomalous_domains,
        });
    } else if anomalous_domains.len() == 1 {
        actions.push(RegulationAction::Alert {
            severity: AlertSeverity::Medium,
            message: format!(
                "Anomalous pattern in domain '{}'",
                anomalous_domains[0]
            ),
            domains: anomalous_domains,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use chrono::Utc;

    fn make_state(domains: Vec<DomainState>) -> InteroceptiveState {
        let mut domain_states = HashMap::new();
        for ds in domains {
            domain_states.insert(ds.domain.clone(), ds);
        }
        InteroceptiveState {
            domain_states,
            global_arousal: 0.3,
            buffer_size: 10,
            active_markers: vec![],
            timestamp: Utc::now(),
        }
    }

    fn domain_with(
        name: &str,
        valence: f64,
        confidence: f64,
        success: f64,
        alignment: f64,
        anomaly: f64,
        signals: u64,
    ) -> DomainState {
        DomainState {
            domain: name.to_string(),
            valence_trend: valence,
            anomaly_level: anomaly,
            action_success_rate: success,
            alignment_score: alignment,
            confidence,
            signal_count: signals,
            last_updated: Utc::now(),
        }
    }

    #[test]
    fn no_actions_for_healthy_state() {
        let state = make_state(vec![
            domain_with("coding", 0.5, 0.8, 0.7, 0.8, 0.5, 20),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.is_empty(), "got: {:?}", actions);
    }

    #[test]
    fn negative_valence_triggers_soul_update() {
        let state = make_state(vec![
            domain_with("coding", -0.5, 0.7, 0.6, 0.7, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "coding")));
    }

    #[test]
    fn negative_valence_needs_min_signals() {
        let state = make_state(vec![
            domain_with("coding", -0.5, 0.7, 0.6, 0.7, 0.5, 2), // only 2 signals
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        // Should NOT trigger because signal_count < 5
        assert!(!actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { .. })));
    }

    #[test]
    fn low_confidence_triggers_retrieval_adjustment() {
        let state = make_state(vec![
            domain_with("research", 0.2, 0.25, 0.6, 0.7, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::RetrievalAdjustment { expand_search: true, .. })));
    }

    #[test]
    fn low_success_triggers_behavior_shift() {
        let state = make_state(vec![
            domain_with("testing", 0.1, 0.7, 0.15, 0.7, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::BehaviorShift { .. })));
    }

    #[test]
    fn low_alignment_triggers_soul_update() {
        let state = make_state(vec![
            domain_with("social", 0.0, 0.7, 0.6, 0.15, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "social")));
    }

    #[test]
    fn multi_domain_anomaly_triggers_alert() {
        let state = make_state(vec![
            domain_with("coding", 0.1, 0.7, 0.6, 0.7, 2.5, 10),
            domain_with("trading", 0.1, 0.7, 0.6, 0.7, 3.0, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::Alert { severity: AlertSeverity::High, .. })));
    }

    #[test]
    fn single_domain_anomaly_triggers_medium_alert() {
        let state = make_state(vec![
            domain_with("coding", 0.1, 0.7, 0.6, 0.7, 2.5, 10),
            domain_with("trading", 0.1, 0.7, 0.6, 0.7, 0.5, 10), // normal
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(a, RegulationAction::Alert { severity: AlertSeverity::Medium, .. })));
    }

    #[test]
    fn multiple_issues_generate_multiple_actions() {
        // Domain with BOTH low confidence AND negative valence.
        let state = make_state(vec![
            domain_with("debugging", -0.6, 0.2, 0.6, 0.7, 0.5, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.len() >= 2, "expected >=2 actions, got {}: {:?}", actions.len(), actions);
    }
}
