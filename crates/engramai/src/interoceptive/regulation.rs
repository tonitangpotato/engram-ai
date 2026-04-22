//! Regulation Layer — Generate advisory actions from interoceptive state.
//!
//! Adaptive σ-based regulation: uses rolling baselines to detect deviations
//! relative to the system's own history, not hardcoded thresholds.
//!
//! Cold-start behavior: when baselines haven't calibrated yet (<20 samples),
//! falls back to conservative hardcoded thresholds to provide basic safety.
//!
//! Design reference: INTEROCEPTIVE-LAYER.md §5.2 Layer 3

use crate::interoceptive::hub::InteroceptiveHub;
use crate::interoceptive::types::{
    AlertSeverity, DeviationLevel, DomainState, HeartbeatAdjustDirection, IdentityAspect,
    InteroceptiveState, RegulationAction,
};

/// Configuration for regulation — primarily controls cold-start fallback
/// thresholds used when baselines haven't calibrated yet.
#[derive(Debug, Clone)]
pub struct RegulationConfig {
    // ── Cold-start fallback thresholds ───────────────────────────────
    // Used ONLY when adaptive baselines are uncalibrated.

    /// Valence below this triggers SoulUpdateSuggestion (cold-start).
    pub fallback_negative_valence: f64,
    /// Minimum signals with negative valence before triggering (cold-start).
    pub fallback_min_signals: u64,
    /// Confidence below this triggers RetrievalAdjustment (cold-start).
    pub fallback_low_confidence: f64,
    /// Action success rate below this triggers BehaviorShift (cold-start).
    pub fallback_low_success: f64,
    /// Anomaly level above this triggers Alert (cold-start).
    pub fallback_high_anomaly: f64,
    /// Alignment below this triggers SoulUpdateSuggestion (cold-start).
    pub fallback_low_alignment: f64,

    // ── Adaptive thresholds ──────────────────────────────────────────
    // σ-multipliers for when baselines ARE calibrated.

    /// Minimum σ deviation to trigger actions (default: 2.5).
    pub action_sigma: f64,
    /// Minimum σ for high-severity alerts (default: 3.5).
    pub alert_sigma: f64,

    /// Minimum domains with High+ deviation for multi-domain alert.
    pub multi_anomaly_domain_count: usize,
}

impl Default for RegulationConfig {
    fn default() -> Self {
        Self {
            // Conservative cold-start fallbacks — deliberately lenient
            // to avoid false positives during warm-up.
            fallback_negative_valence: -0.5,
            fallback_min_signals: 10,
            fallback_low_confidence: 0.3,
            fallback_low_success: 0.2,
            fallback_high_anomaly: 2.0,
            fallback_low_alignment: 0.2,

            // Adaptive thresholds.
            action_sigma: 2.5,
            alert_sigma: 3.5,

            multi_anomaly_domain_count: 2,
        }
    }
}

/// Evaluate the current interoceptive state with adaptive baselines.
///
/// When baselines are calibrated, uses σ-deviation for all thresholds.
/// When uncalibrated, falls back to conservative hardcoded values.
///
/// Returns a list of advisory actions. Empty list = system is nominal.
pub fn evaluate(state: &InteroceptiveState, config: &RegulationConfig) -> Vec<RegulationAction> {
    evaluate_with_hub(state, config, None)
}

/// Evaluate with access to the hub's adaptive baselines.
///
/// This is the primary entry point when the hub is available.
pub fn evaluate_with_hub(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
) -> Vec<RegulationAction> {
    let mut actions = Vec::new();

    for ds in state.domain_states.values() {
        check_valence(ds, config, hub, &mut actions);
        check_confidence(ds, config, hub, &mut actions);
        check_success_rate(ds, config, hub, &mut actions);
        check_alignment(ds, config, hub, &mut actions);
    }

    check_multi_domain_anomaly(state, config, hub, &mut actions);
    check_identity_evolution(state, config, hub, &mut actions);
    check_heartbeat_frequency(state, config, hub, &mut actions);

    actions
}

// ── Individual checks ─────────────────────────────────────────────────

/// Valence check: is the domain trending significantly more negative than its baseline?
fn check_valence(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let should_trigger = match hub {
        Some(hub) => {
            // Adaptive: check if current valence is a significant negative deviation.
            // We look at any valence-contributing source's baseline.
            let dev = hub.deviation_level("accumulator", &ds.domain, ds.valence_trend);
            match dev {
                DeviationLevel::Uncalibrated => {
                    // Fallback to hardcoded.
                    ds.valence_trend < config.fallback_negative_valence
                        && ds.signal_count >= config.fallback_min_signals
                }
                _ => {
                    // Adaptive: trigger if valence is negative AND deviating significantly
                    // from baseline. We need both conditions because a system that normally
                    // runs at -0.1 valence shouldn't alert on -0.15.
                    ds.valence_trend < 0.0 && dev.is_actionable()
                }
            }
        }
        None => {
            // No hub → pure fallback.
            ds.valence_trend < config.fallback_negative_valence
                && ds.signal_count >= config.fallback_min_signals
        }
    };

    if should_trigger {
        let sigma_info = hub.and_then(|h| {
            h.baseline("accumulator", &ds.domain)
                .and_then(|bl| bl.sigma_deviation(ds.valence_trend))
        });
        actions.push(RegulationAction::SoulUpdateSuggestion {
            domain: ds.domain.clone(),
            reason: format!(
                "Negative valence trend in '{}': {:.2}{}",
                ds.domain,
                ds.valence_trend,
                sigma_info
                    .map(|s| format!(" ({:.1}σ from baseline)", s))
                    .unwrap_or_default(),
            ),
            valence_trend: ds.valence_trend,
        });
    }
}

/// Confidence check: is confidence significantly lower than this domain's normal?
fn check_confidence(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let should_trigger = match hub {
        Some(hub) => {
            let dev = hub.deviation_level("confidence", &ds.domain, ds.confidence);
            match dev {
                DeviationLevel::Uncalibrated => {
                    ds.confidence < config.fallback_low_confidence && ds.signal_count >= 3
                }
                _ => {
                    // Confidence is low AND significantly below this domain's norm.
                    ds.confidence < 0.5 && dev.is_actionable()
                }
            }
        }
        None => ds.confidence < config.fallback_low_confidence && ds.signal_count >= 3,
    };

    if should_trigger {
        let sigma_info = hub.and_then(|h| {
            h.baseline("confidence", &ds.domain)
                .and_then(|bl| bl.sigma_deviation(ds.confidence))
        });
        actions.push(RegulationAction::RetrievalAdjustment {
            expand_search: true,
            reason: format!(
                "Low confidence in '{}': {:.0}%{}",
                ds.domain,
                ds.confidence * 100.0,
                sigma_info
                    .map(|s| format!(" ({:.1}σ below baseline)", s))
                    .unwrap_or_default(),
            ),
        });
    }
}

/// Success rate check: is the action success rate significantly worse than normal?
fn check_success_rate(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let should_trigger = match hub {
        Some(hub) => {
            let dev = hub.deviation_level("feedback", &ds.domain, ds.action_success_rate);
            match dev {
                DeviationLevel::Uncalibrated => {
                    ds.action_success_rate < config.fallback_low_success
                        && ds.signal_count >= 5
                }
                _ => {
                    // Success rate is below 50% AND significantly below baseline.
                    ds.action_success_rate < 0.5 && dev.is_actionable()
                }
            }
        }
        None => {
            ds.action_success_rate < config.fallback_low_success && ds.signal_count >= 5
        }
    };

    if should_trigger {
        let sigma_info = hub.and_then(|h| {
            h.baseline("feedback", &ds.domain)
                .and_then(|bl| bl.sigma_deviation(ds.action_success_rate))
        });
        actions.push(RegulationAction::BehaviorShift {
            action: ds.domain.clone(),
            recommendation: format!(
                "Success rate for '{}' is {:.0}%{} — change approach, don't retry same strategy",
                ds.domain,
                ds.action_success_rate * 100.0,
                sigma_info
                    .map(|s| format!(" ({:.1}σ below baseline)", s))
                    .unwrap_or_default(),
            ),
            success_rate: ds.action_success_rate,
        });
    }
}

/// Alignment check: is alignment significantly below this domain's norm?
fn check_alignment(
    ds: &DomainState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let should_trigger = match hub {
        Some(hub) => {
            let dev = hub.deviation_level("alignment", &ds.domain, ds.alignment_score);
            match dev {
                DeviationLevel::Uncalibrated => {
                    ds.alignment_score < config.fallback_low_alignment
                        && ds.signal_count >= 5
                }
                _ => {
                    ds.alignment_score < 0.4 && dev.is_actionable()
                }
            }
        }
        None => {
            ds.alignment_score < config.fallback_low_alignment && ds.signal_count >= 5
        }
    };

    if should_trigger {
        actions.push(RegulationAction::SoulUpdateSuggestion {
            domain: ds.domain.clone(),
            reason: format!(
                "Low drive alignment in '{}': {:.0}% — activities may not match SOUL drives",
                ds.domain,
                ds.alignment_score * 100.0,
            ),
            valence_trend: ds.valence_trend,
        });
    }
}

/// Identity evolution: detect stable behavioral patterns that indicate
/// the agent's identity has evolved (new capabilities, traits, specializations).
///
/// Unlike SoulUpdateSuggestion (negative trends → update values/principles),
/// this detects positive/distinctive stable patterns → update self-description.
///
/// Detection criteria:
/// - Domain must be calibrated (enough history to judge stability)
/// - Positive valence trend sustained (not a temporary spike)
/// - Low variance in recent behavior (pattern is stable, not noisy)
/// - High signal count (pattern has been observed many times)
fn check_identity_evolution(
    state: &InteroceptiveState,
    _config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let hub = match hub {
        Some(h) => h,
        None => return, // Identity evolution requires calibrated baselines; no hub → skip.
    };

    for ds in state.domain_states.values() {
        // Gate 1: Need substantial history (at least 30 signals).
        if ds.signal_count < 30 {
            continue;
        }

        // Gate 2: Domain must have calibrated baselines.
        if !hub.is_domain_calibrated(&ds.domain) {
            continue;
        }

        // ── Detect capability / domain specialization ────────────────
        // A domain with consistently high success rate AND positive valence
        // suggests the agent has developed expertise in this area.
        // Check that this high performance is the baseline (stable), not an outlier.
        // The hub baselines track raw signal valence, so we compare valence against
        // the accumulator baseline for stability. We also check that feedback baseline
        // exists and is calibrated (meaning consistent performance history).
        if ds.action_success_rate > 0.75 && ds.valence_trend > 0.2 {
            // Valence should be within normal range of the accumulator baseline.
            let valence_dev =
                hub.deviation_level("accumulator", &ds.domain, ds.valence_trend);

            let valence_is_stable = matches!(
                valence_dev,
                DeviationLevel::Normal | DeviationLevel::Elevated
            );

            // Feedback baseline must exist and be calibrated (proves consistent history).
            let feedback_calibrated = hub
                .baseline("feedback", &ds.domain)
                .is_some_and(|bl| bl.is_calibrated());

            if valence_is_stable && feedback_calibrated {
                let (aspect, observation, suggestion) = if ds.action_success_rate > 0.9 {
                    (
                        IdentityAspect::DomainSpecialization,
                        format!(
                            "Consistently high performance in '{}': {:.0}% success rate over {} signals with stable positive trend",
                            ds.domain,
                            ds.action_success_rate * 100.0,
                            ds.signal_count,
                        ),
                        format!(
                            "Strong domain specialization in '{}' — consider adding to IDENTITY.md capabilities",
                            ds.domain,
                        ),
                    )
                } else {
                    (
                        IdentityAspect::Capability,
                        format!(
                            "Reliable capability in '{}': {:.0}% success rate, positive valence ({:.2})",
                            ds.domain,
                            ds.action_success_rate * 100.0,
                            ds.valence_trend,
                        ),
                        format!(
                            "Developed capability in '{}' — stable, positive performance pattern",
                            ds.domain,
                        ),
                    )
                };

                actions.push(RegulationAction::IdentityEvolutionSuggestion {
                    aspect,
                    observation,
                    domains: vec![ds.domain.clone()],
                    confidence: ds.action_success_rate * ds.confidence,
                    suggestion,
                });
            }
        }

        // ── Detect behavioral pattern ────────────────────────────────
        // A domain with very high confidence AND high alignment means the agent
        // has a stable, well-aligned work pattern in this area.
        if ds.confidence > 0.8 && ds.alignment_score > 0.7 && ds.valence_trend > 0.0 {
            // Check that valence is stable (within normal range of baseline).
            let valence_dev = hub.deviation_level("accumulator", &ds.domain, ds.valence_trend);

            let valence_stable = matches!(
                valence_dev,
                DeviationLevel::Normal | DeviationLevel::Elevated
            );

            // Confidence and alignment baselines must be calibrated.
            let conf_calibrated = hub
                .baseline("confidence", &ds.domain)
                .is_some_and(|bl| bl.is_calibrated());
            let align_calibrated = hub
                .baseline("alignment", &ds.domain)
                .is_some_and(|bl| bl.is_calibrated());

            if valence_stable && conf_calibrated && align_calibrated {
                actions.push(RegulationAction::IdentityEvolutionSuggestion {
                    aspect: IdentityAspect::BehavioralPattern,
                    observation: format!(
                        "Stable high-confidence ({:.0}%), well-aligned ({:.0}%) work pattern in '{}'",
                        ds.confidence * 100.0,
                        ds.alignment_score * 100.0,
                        ds.domain,
                    ),
                    domains: vec![ds.domain.clone()],
                    confidence: ds.confidence * ds.alignment_score,
                    suggestion: format!(
                        "Established work pattern in '{}' — high confidence and strong drive alignment",
                        ds.domain,
                    ),
                });
            }
        }
    }
}

/// Multi-domain anomaly: are multiple domains simultaneously deviating?
fn check_multi_domain_anomaly(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    let anomalous_domains: Vec<String> = state
        .domain_states
        .values()
        .filter(|ds| {
            match hub {
                Some(hub) => {
                    // Adaptive: check if anomaly_level deviates from this domain's norm.
                    let dev = hub.deviation_level("anomaly", &ds.domain, ds.anomaly_level);
                    match dev {
                        DeviationLevel::Uncalibrated => {
                            ds.anomaly_level > config.fallback_high_anomaly
                        }
                        _ => dev.is_actionable(),
                    }
                }
                None => ds.anomaly_level > config.fallback_high_anomaly,
            }
        })
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

/// Heartbeat frequency adjustment: should we poll more or less often?
///
/// Increase frequency when:
/// - Multiple domains show elevated anomaly levels (needs closer monitoring)
/// - Global arousal is high (system is stressed)
/// - Negative valence sustained across domains
///
/// Decrease frequency when:
/// - All domains are stable (low anomaly, positive valence, calibrated baselines)
/// - Global arousal is low
/// - System has been running without issues for a while (high signal count, stable)
///
/// Neutral (no action) when the system is in normal operating range.
fn check_heartbeat_frequency(
    state: &InteroceptiveState,
    config: &RegulationConfig,
    hub: Option<&InteroceptiveHub>,
    actions: &mut Vec<RegulationAction>,
) {
    // Need at least 2 domains with data to make frequency decisions.
    // Single-domain systems don't have enough signal diversity.
    let active_domains: Vec<&DomainState> = state
        .domain_states
        .values()
        .filter(|ds| ds.signal_count >= 5 && ds.domain != "_global")
        .collect();

    if active_domains.len() < 2 {
        return;
    }

    // ── Check for INCREASE (more frequent) ──────────────────────────
    //
    // Count domains that are "troubled": elevated anomaly OR significantly
    // negative valence. Use adaptive baselines when available.

    let troubled_domains: Vec<String> = active_domains
        .iter()
        .filter(|ds| {
            let anomaly_elevated = match hub {
                Some(hub) => {
                    let dev = hub.deviation_level("anomaly", &ds.domain, ds.anomaly_level);
                    match dev {
                        DeviationLevel::Uncalibrated => ds.anomaly_level > config.fallback_high_anomaly,
                        _ => dev.is_elevated(),
                    }
                }
                None => ds.anomaly_level > config.fallback_high_anomaly,
            };

            let valence_negative = match hub {
                Some(hub) => {
                    let dev = hub.deviation_level("accumulator", &ds.domain, ds.valence_trend);
                    match dev {
                        DeviationLevel::Uncalibrated => {
                            ds.valence_trend < config.fallback_negative_valence
                        }
                        _ => ds.valence_trend < 0.0 && dev.is_elevated(),
                    }
                }
                None => ds.valence_trend < config.fallback_negative_valence,
            };

            anomaly_elevated || valence_negative
        })
        .map(|ds| ds.domain.clone())
        .collect();

    // If ≥2 domains are troubled, or global arousal is high → increase frequency.
    let high_global_arousal = state.global_arousal > 0.6;
    let should_increase = troubled_domains.len() >= 2
        || (!troubled_domains.is_empty() && high_global_arousal);

    if should_increase {
        // Severity determines multiplier: more troubled domains → more aggressive.
        let multiplier = if troubled_domains.len() >= 3 || state.global_arousal > 0.8 {
            0.25 // 4x frequency
        } else if troubled_domains.len() >= 2 {
            0.5 // 2x frequency
        } else {
            0.67 // ~1.5x frequency
        };

        actions.push(RegulationAction::HeartbeatFrequencyAdjustment {
            direction: HeartbeatAdjustDirection::Increase,
            interval_multiplier: multiplier,
            reason: format!(
                "Elevated stress in {} domain(s): {} (global arousal: {:.0}%)",
                troubled_domains.len(),
                troubled_domains.join(", "),
                state.global_arousal * 100.0,
            ),
            domains: troubled_domains,
        });
        return; // Don't emit both increase and decrease in the same cycle.
    }

    // ── Check for DECREASE (less frequent) ──────────────────────────
    //
    // All domains must be calm: positive valence, low anomaly, calibrated baselines.
    // Additionally, require enough history (calibrated baselines) to be confident
    // that stability isn't just "we haven't collected enough data yet."

    let hub = match hub {
        Some(h) => h,
        None => return, // Decrease requires calibrated baselines; no hub → skip.
    };

    let all_calm = active_domains.iter().all(|ds| {
        // Valence within normal range (not deviating negatively).
        let valence_ok = {
            let dev = hub.deviation_level("accumulator", &ds.domain, ds.valence_trend);
            match dev {
                DeviationLevel::Uncalibrated => false, // Not calibrated → not confident.
                DeviationLevel::Normal => ds.valence_trend >= 0.0, // Normal AND non-negative.
                _ => false, // Elevated/High/Extreme deviations → not calm.
            }
        };

        // Anomaly within normal range.
        let anomaly_ok = {
            let dev = hub.deviation_level("anomaly", &ds.domain, ds.anomaly_level);
            matches!(dev, DeviationLevel::Normal)
        };

        // Requires sufficient history.
        let has_history = ds.signal_count >= 30 && hub.is_domain_calibrated(&ds.domain);

        valence_ok && anomaly_ok && has_history
    });

    if all_calm {
        let stable_domains: Vec<String> = active_domains
            .iter()
            .map(|ds| ds.domain.clone())
            .collect();

        actions.push(RegulationAction::HeartbeatFrequencyAdjustment {
            direction: HeartbeatAdjustDirection::Decrease,
            interval_multiplier: 2.0, // Half the frequency.
            reason: format!(
                "All {} domains stable with calibrated baselines — system is nominal",
                stable_domains.len(),
            ),
            domains: stable_domains,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interoceptive::types::{InteroceptiveSignal, SignalSource};
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

    // ── Fallback tests (no hub) ───────────────────────────────────────

    #[test]
    fn no_actions_for_healthy_state() {
        let state = make_state(vec![
            domain_with("coding", 0.5, 0.8, 0.7, 0.8, 0.5, 20),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.is_empty(), "got: {:?}", actions);
    }

    #[test]
    fn fallback_negative_valence_triggers_soul_update() {
        let state = make_state(vec![
            domain_with("coding", -0.7, 0.7, 0.6, 0.7, 0.5, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(
            a,
            RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "coding"
        )));
    }

    #[test]
    fn fallback_needs_min_signals() {
        let state = make_state(vec![
            domain_with("coding", -0.7, 0.7, 0.6, 0.7, 0.5, 3), // only 3 signals
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        // Should NOT trigger because signal_count < fallback_min_signals (10)
        assert!(!actions.iter().any(|a| matches!(
            a,
            RegulationAction::SoulUpdateSuggestion { .. }
        )));
    }

    #[test]
    fn fallback_low_confidence_triggers_retrieval() {
        let state = make_state(vec![
            domain_with("research", 0.2, 0.2, 0.6, 0.7, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(
            a,
            RegulationAction::RetrievalAdjustment { expand_search: true, .. }
        )));
    }

    #[test]
    fn fallback_low_success_triggers_behavior_shift() {
        let state = make_state(vec![
            domain_with("testing", 0.1, 0.7, 0.1, 0.7, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(
            a,
            RegulationAction::BehaviorShift { .. }
        )));
    }

    #[test]
    fn fallback_low_alignment_triggers_soul_update() {
        let state = make_state(vec![
            domain_with("social", 0.0, 0.7, 0.6, 0.1, 0.5, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(
            a,
            RegulationAction::SoulUpdateSuggestion { domain, .. } if domain == "social"
        )));
    }

    #[test]
    fn fallback_multi_domain_anomaly_triggers_high_alert() {
        let state = make_state(vec![
            domain_with("coding", 0.1, 0.7, 0.6, 0.7, 3.0, 10),
            domain_with("trading", 0.1, 0.7, 0.6, 0.7, 3.0, 10),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(actions.iter().any(|a| matches!(
            a,
            RegulationAction::Alert { severity: AlertSeverity::High, .. }
        )));
    }

    // ── Adaptive tests (with hub) ─────────────────────────────────────

    fn build_calibrated_hub(domain: &str, normal_valence: f64, n: u64) -> InteroceptiveHub {
        let mut hub = InteroceptiveHub::new();
        // Feed enough signals to calibrate baselines.
        for i in 0..n {
            // Small variation around the normal value.
            let jitter = ((i % 5) as f64 - 2.0) * 0.05;
            let val = normal_valence + jitter;
            let sig = InteroceptiveSignal::new(
                SignalSource::Accumulator,
                Some(domain.into()),
                val,
                0.3,
            );
            hub.process_signal(sig);

            // Also feed confidence, feedback, alignment signals for those baselines.
            for source in [SignalSource::Confidence, SignalSource::Feedback, SignalSource::Alignment] {
                let sig = InteroceptiveSignal::new(
                    source,
                    Some(domain.into()),
                    0.5 + jitter,
                    0.2,
                );
                hub.process_signal(sig);
            }
        }
        hub
    }

    #[test]
    fn adaptive_no_action_when_within_baseline() {
        let hub = build_calibrated_hub("coding", 0.3, 30);
        let state = make_state(vec![
            // Valence 0.2 — slightly below baseline of ~0.3 but within normal range.
            domain_with("coding", 0.2, 0.7, 0.6, 0.7, 0.5, 30),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        // Should NOT trigger because 0.2 is within normal σ range of baseline ~0.3.
        assert!(
            !actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { .. })),
            "got: {:?}",
            actions,
        );
    }

    #[test]
    fn adaptive_triggers_on_large_deviation() {
        let hub = build_calibrated_hub("coding", 0.3, 30);
        let state = make_state(vec![
            // Valence -0.8 — way below baseline of ~0.3 (>3σ).
            domain_with("coding", -0.8, 0.7, 0.6, 0.7, 0.5, 30),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { .. })),
            "large deviation should trigger, got: {:?}",
            actions,
        );
    }

    #[test]
    fn adaptive_message_includes_sigma() {
        let hub = build_calibrated_hub("coding", 0.3, 30);
        let state = make_state(vec![
            domain_with("coding", -0.8, 0.7, 0.6, 0.7, 0.5, 30),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        // The reason string should mention σ.
        let soul_action = actions.iter().find(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { .. }));
        if let Some(RegulationAction::SoulUpdateSuggestion { reason, .. }) = soul_action {
            assert!(reason.contains("σ"), "reason should mention σ: {}", reason);
        }
    }

    #[test]
    fn adaptive_falls_back_when_uncalibrated() {
        let hub = InteroceptiveHub::new(); // empty, no baselines
        let state = make_state(vec![
            // Very negative valence with enough signals for fallback threshold.
            domain_with("coding", -0.7, 0.7, 0.6, 0.7, 0.5, 15),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        // Should trigger via fallback (no calibrated baseline).
        assert!(
            actions.iter().any(|a| matches!(a, RegulationAction::SoulUpdateSuggestion { .. })),
            "uncalibrated should fall back to hardcoded, got: {:?}",
            actions,
        );
    }

    #[test]
    fn multiple_issues_generate_multiple_actions() {
        // Domain with BOTH low confidence AND negative valence.
        let state = make_state(vec![
            domain_with("debugging", -0.7, 0.15, 0.6, 0.7, 0.5, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(
            actions.len() >= 2,
            "expected >=2 actions, got {}: {:?}",
            actions.len(),
            actions,
        );
    }

    // ── Identity evolution tests ──────────────────────────────────────

    /// Build a hub where a domain has calibrated baselines at high performance levels.
    fn build_high_performance_hub(domain: &str, n: u64) -> InteroceptiveHub {
        let mut hub = InteroceptiveHub::new();
        for i in 0..n {
            let jitter = ((i % 5) as f64 - 2.0) * 0.02;

            // High positive valence (accumulator)
            hub.process_signal(InteroceptiveSignal::new(
                SignalSource::Accumulator,
                Some(domain.into()),
                0.6 + jitter, // ~0.6 valence baseline
                0.2,
            ));

            // High success rate (feedback): valence 0.8 → success_rate ~0.9
            hub.process_signal(InteroceptiveSignal::new(
                SignalSource::Feedback,
                Some(domain.into()),
                0.8 + jitter,
                0.2,
            ));

            // High confidence
            hub.process_signal(InteroceptiveSignal::new(
                SignalSource::Confidence,
                Some(domain.into()),
                0.7 + jitter,
                0.2,
            ));

            // High alignment
            hub.process_signal(InteroceptiveSignal::new(
                SignalSource::Alignment,
                Some(domain.into()),
                0.6 + jitter,
                0.2,
            ));
        }
        hub
    }

    #[test]
    fn identity_evolution_not_triggered_without_hub() {
        // High performance but no hub → no identity suggestions.
        let state = make_state(vec![
            domain_with("coding", 0.5, 0.9, 0.95, 0.8, 0.3, 50),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion { .. }
            )),
            "no identity evolution without hub: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_not_triggered_with_few_signals() {
        let hub = build_high_performance_hub("coding", 40);
        // Only 10 signals in state (below 30 threshold).
        let state = make_state(vec![
            domain_with("coding", 0.5, 0.9, 0.95, 0.8, 0.3, 10),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion { .. }
            )),
            "need 30+ signals: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_triggers_domain_specialization() {
        let hub = build_high_performance_hub("coding", 40);

        // Hub accumulator baseline mean ≈ 0.6 with tight variance.
        // DomainState valence must be within Normal/Elevated range of that baseline.
        let state = make_state(vec![
            domain_with("coding", 0.6, 0.85, 0.92, 0.75, 0.3, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion {
                    aspect: IdentityAspect::DomainSpecialization,
                    ..
                }
            )),
            "expected domain specialization, got: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_triggers_capability() {
        let hub = build_high_performance_hub("research", 40);
        // Success 0.82 (>0.75 but <0.9) + positive valence → Capability.
        // Hub accumulator baseline ≈ 0.6, so valence must be near 0.6.
        let state = make_state(vec![
            domain_with("research", 0.6, 0.7, 0.82, 0.6, 0.3, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion {
                    aspect: IdentityAspect::Capability,
                    ..
                }
            )),
            "expected capability, got: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_triggers_behavioral_pattern() {
        let hub = build_high_performance_hub("writing", 40);
        // High confidence (>0.8) + high alignment (>0.7) + positive valence → BehavioralPattern.
        // Hub accumulator baseline ≈ 0.6, so valence must be near 0.6.
        let state = make_state(vec![
            domain_with("writing", 0.6, 0.85, 0.6, 0.8, 0.3, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion {
                    aspect: IdentityAspect::BehavioralPattern,
                    ..
                }
            )),
            "expected behavioral pattern, got: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_not_triggered_by_mediocre_performance() {
        let hub = build_calibrated_hub("coding", 0.3, 30);
        // Average performance — nothing special.
        let state = make_state(vec![
            domain_with("coding", 0.1, 0.5, 0.5, 0.5, 0.5, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::IdentityEvolutionSuggestion { .. }
            )),
            "mediocre performance should not trigger identity evolution: {:?}",
            actions,
        );
    }

    #[test]
    fn identity_evolution_suggestion_has_valid_confidence() {
        let hub = build_high_performance_hub("coding", 40);
        let state = make_state(vec![
            domain_with("coding", 0.5, 0.85, 0.92, 0.75, 0.3, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        for action in &actions {
            if let RegulationAction::IdentityEvolutionSuggestion { confidence, .. } = action {
                assert!(*confidence > 0.0 && *confidence <= 1.0,
                    "confidence should be in (0, 1]: {}", confidence);
            }
        }
    }

    // ── Heartbeat frequency adjustment tests ──────────────────────────

    #[test]
    fn heartbeat_no_adjustment_with_single_domain() {
        // Need ≥2 active domains for heartbeat frequency decisions.
        let state = make_state(vec![
            domain_with("coding", -0.8, 0.3, 0.2, 0.3, 3.0, 20),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::HeartbeatFrequencyAdjustment { .. }
            )),
            "single domain should not trigger heartbeat adjustment: {:?}",
            actions,
        );
    }

    #[test]
    fn heartbeat_no_adjustment_with_few_signals() {
        // Domains with <5 signals are ignored.
        let state = make_state(vec![
            domain_with("coding", -0.8, 0.3, 0.2, 0.3, 3.0, 3),
            domain_with("research", -0.6, 0.4, 0.3, 0.3, 2.5, 2),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::HeartbeatFrequencyAdjustment { .. }
            )),
            "too few signals should not trigger: {:?}",
            actions,
        );
    }

    #[test]
    fn heartbeat_increase_when_two_domains_troubled_fallback() {
        // Two troubled domains (high anomaly, negative valence), no hub → fallback thresholds.
        let state = make_state(vec![
            domain_with("coding", -0.7, 0.3, 0.3, 0.3, 2.5, 15),
            domain_with("research", -0.6, 0.3, 0.3, 0.3, 2.5, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        let hb = actions.iter().find(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment { .. }
        ));
        assert!(hb.is_some(), "two troubled domains should increase frequency: {:?}", actions);
        if let Some(RegulationAction::HeartbeatFrequencyAdjustment {
            direction, interval_multiplier, ..
        }) = hb {
            assert_eq!(*direction, HeartbeatAdjustDirection::Increase);
            assert!(*interval_multiplier <= 0.5, "multiplier should be ≤0.5: {}", interval_multiplier);
        }
    }

    #[test]
    fn heartbeat_increase_with_high_global_arousal() {
        // One troubled domain + high global arousal → increase.
        let mut state = make_state(vec![
            domain_with("coding", -0.7, 0.3, 0.3, 0.3, 2.5, 15),
            domain_with("research", 0.3, 0.7, 0.7, 0.7, 0.5, 15),
        ]);
        state.global_arousal = 0.8;
        let actions = evaluate(&state, &RegulationConfig::default());
        let hb = actions.iter().find(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment {
                direction: HeartbeatAdjustDirection::Increase, ..
            }
        ));
        assert!(hb.is_some(), "one troubled + high arousal should increase: {:?}", actions);
    }

    #[test]
    fn heartbeat_increase_severe_with_three_troubled_domains() {
        // Three troubled domains → most aggressive multiplier (0.25).
        let state = make_state(vec![
            domain_with("coding", -0.8, 0.2, 0.2, 0.2, 3.0, 15),
            domain_with("research", -0.7, 0.3, 0.2, 0.2, 2.5, 15),
            domain_with("trading", -0.6, 0.3, 0.3, 0.3, 2.5, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        let hb = actions.iter().find(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment { .. }
        ));
        assert!(hb.is_some(), "three troubled → increase: {:?}", actions);
        if let Some(RegulationAction::HeartbeatFrequencyAdjustment {
            interval_multiplier, ..
        }) = hb {
            assert!(
                (*interval_multiplier - 0.25).abs() < f64::EPSILON,
                "3 troubled domains → 0.25 multiplier, got: {}",
                interval_multiplier,
            );
        }
    }

    /// Build a hub with all-stable signals for multiple domains.
    fn build_stable_multi_domain_hub(domains: &[&str], rounds: usize) -> InteroceptiveHub {
        let mut hub = InteroceptiveHub::new();
        for round in 0..rounds {
            for domain in domains {
                let jitter = ((round % 5) as f64 - 2.0) * 0.01;

                // Positive valence (accumulator)
                hub.process_signal(InteroceptiveSignal::new(
                    SignalSource::Accumulator,
                    Some((*domain).into()),
                    0.3 + jitter, // stable positive
                    0.1,
                ));

                // Low anomaly
                hub.process_signal(InteroceptiveSignal::new(
                    SignalSource::Anomaly,
                    Some((*domain).into()),
                    0.0 + jitter,
                    0.1, // low arousal → low anomaly
                ));
            }
        }
        hub
    }

    #[test]
    fn heartbeat_decrease_when_all_domains_stable() {
        let hub = build_stable_multi_domain_hub(&["coding", "research"], 40);
        // Anomaly_level must be near 0 to match hub's anomaly baseline (mean ~0.0).
        let state = make_state(vec![
            domain_with("coding", 0.3, 0.7, 0.7, 0.7, 0.0, 50),
            domain_with("research", 0.3, 0.7, 0.7, 0.7, 0.0, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        let hb = actions.iter().find(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment { .. }
        ));
        assert!(hb.is_some(), "all stable → decrease frequency: {:?}", actions);
        if let Some(RegulationAction::HeartbeatFrequencyAdjustment {
            direction, interval_multiplier, ..
        }) = hb {
            assert_eq!(*direction, HeartbeatAdjustDirection::Decrease);
            assert!((*interval_multiplier - 2.0).abs() < f64::EPSILON,
                "stable → 2.0x interval: {}", interval_multiplier);
        }
    }

    #[test]
    fn heartbeat_decrease_requires_hub() {
        // Stable state but no hub → no decrease (can't confirm calibration).
        let state = make_state(vec![
            domain_with("coding", 0.3, 0.7, 0.7, 0.7, 0.3, 50),
            domain_with("research", 0.3, 0.7, 0.7, 0.7, 0.3, 50),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::HeartbeatFrequencyAdjustment {
                    direction: HeartbeatAdjustDirection::Decrease, ..
                }
            )),
            "decrease needs hub for calibration check: {:?}",
            actions,
        );
    }

    #[test]
    fn heartbeat_no_decrease_when_uncalibrated() {
        // Hub with too few samples to be calibrated.
        let hub = build_stable_multi_domain_hub(&["coding", "research"], 5);
        let state = make_state(vec![
            domain_with("coding", 0.3, 0.7, 0.7, 0.7, 0.3, 50),
            domain_with("research", 0.3, 0.7, 0.7, 0.7, 0.3, 50),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                RegulationAction::HeartbeatFrequencyAdjustment {
                    direction: HeartbeatAdjustDirection::Decrease, ..
                }
            )),
            "uncalibrated baselines → no decrease: {:?}",
            actions,
        );
    }

    #[test]
    fn heartbeat_no_simultaneous_increase_and_decrease() {
        // Mixed state: one troubled, one stable. Should not emit both.
        let state = make_state(vec![
            domain_with("coding", -0.8, 0.3, 0.2, 0.3, 3.0, 15),
            domain_with("research", 0.5, 0.8, 0.8, 0.8, 0.3, 15),
        ]);
        let actions = evaluate(&state, &RegulationConfig::default());
        let increase = actions.iter().any(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment {
                direction: HeartbeatAdjustDirection::Increase, ..
            }
        ));
        let decrease = actions.iter().any(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment {
                direction: HeartbeatAdjustDirection::Decrease, ..
            }
        ));
        assert!(!(increase && decrease),
            "should never emit both increase and decrease: {:?}", actions);
    }

    #[test]
    fn heartbeat_increase_with_adaptive_baselines() {
        // Build a hub with stable baselines, then test with deviating state.
        let hub = build_stable_multi_domain_hub(&["coding", "research"], 40);
        // DomainState shows sudden negative deviation from the stable baseline.
        let state = make_state(vec![
            domain_with("coding", -0.5, 0.3, 0.3, 0.3, 2.0, 15),
            domain_with("research", -0.4, 0.4, 0.3, 0.3, 1.8, 15),
        ]);
        let actions = evaluate_with_hub(&state, &RegulationConfig::default(), Some(&hub));
        let hb = actions.iter().find(|a| matches!(
            a,
            RegulationAction::HeartbeatFrequencyAdjustment {
                direction: HeartbeatAdjustDirection::Increase, ..
            }
        ));
        assert!(hb.is_some(), "negative deviation from baseline → increase: {:?}", actions);
    }
}
