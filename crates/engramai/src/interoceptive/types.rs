//! Interoceptive Layer — Unified signal types.
//!
//! Defines the common signal format that all internal monitoring sources
//! (anomaly, accumulator, feedback, confidence, alignment) emit into the
//! InteroceptiveHub.
//!
//! Neuroscience mapping:
//! - Craig (2002): interoceptive signals are the body's internal status reports
//! - Damasio (1994): somatic markers cache emotional associations with situations
//! - GWT (Baars 1988): signals broadcast to a global workspace for integration

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Signal Source ──────────────────────────────────────────────────────

/// Which internal monitoring subsystem produced this signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalSource {
    /// Anomaly/baseline tracker — z-score deviations from expected patterns.
    Anomaly,
    /// Emotional accumulator — running valence trends per domain.
    Accumulator,
    /// Behavior feedback — action success/failure rates.
    Feedback,
    /// Confidence scorer — metacognitive reliability assessment.
    Confidence,
    /// Drive alignment — how well current activity aligns with core drives.
    Alignment,
}

impl std::fmt::Display for SignalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anomaly => write!(f, "anomaly"),
            Self::Accumulator => write!(f, "accumulator"),
            Self::Feedback => write!(f, "feedback"),
            Self::Confidence => write!(f, "confidence"),
            Self::Alignment => write!(f, "alignment"),
        }
    }
}

// ── Interoceptive Signal ──────────────────────────────────────────────

/// A single internal status report from any monitoring subsystem.
///
/// This is the "lingua franca" that all signal sources convert their
/// native output into before feeding the InteroceptiveHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteroceptiveSignal {
    /// Which subsystem generated this signal.
    pub source: SignalSource,

    /// Domain this signal pertains to (e.g., "coding", "trading", "research").
    /// `None` for system-wide signals (e.g., global confidence).
    pub domain: Option<String>,

    /// Affective valence: -1.0 (very negative) to +1.0 (very positive).
    /// Anomaly: negative = deviation from baseline.
    /// Accumulator: direct emotional valence.
    /// Feedback: success_rate mapped to [-1, 1].
    /// Confidence: confidence mapped to [-1, 1].
    /// Alignment: alignment score mapped to [-1, 1].
    pub valence: f64,

    /// Arousal / urgency: 0.0 (calm/routine) to 1.0 (high alert).
    /// Anomaly: abs(z_score) / threshold (clamped to [0, 1]).
    /// Others: derived from rate of change or magnitude.
    pub arousal: f64,

    /// When this signal was generated.
    pub timestamp: DateTime<Utc>,

    /// Optional context about what triggered this signal.
    pub context: Option<SignalContext>,
}

impl InteroceptiveSignal {
    /// Create a new signal with required fields; context defaults to None.
    pub fn new(
        source: SignalSource,
        domain: Option<String>,
        valence: f64,
        arousal: f64,
    ) -> Self {
        Self {
            source,
            domain,
            valence: valence.clamp(-1.0, 1.0),
            arousal: arousal.clamp(0.0, 1.0),
            timestamp: Utc::now(),
            context: None,
        }
    }

    /// Attach context to this signal.
    pub fn with_context(mut self, ctx: SignalContext) -> Self {
        self.context = Some(ctx);
        self
    }

    /// Whether this signal indicates a negative state worth attending to.
    pub fn is_negative(&self) -> bool {
        self.valence < -0.3
    }

    /// Whether this signal indicates high arousal / urgency.
    pub fn is_urgent(&self) -> bool {
        self.arousal > 0.7
    }
}

// ── Signal Context ────────────────────────────────────────────────────

/// What triggered this signal — gives the hub richer information for
/// somatic marker formation and regulation decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalContext {
    /// An anomaly was detected on a specific metric.
    AnomalyDetected {
        metric: String,
        z_score: f64,
        baseline_mean: f64,
    },
    /// An emotional event was recorded.
    EmotionalEvent {
        event_description: String,
    },
    /// An action outcome was logged.
    ActionOutcome {
        action: String,
        success: bool,
        cumulative_score: f64,
    },
    /// A recall operation returned results with a confidence assessment.
    RecallConfidence {
        query: String,
        score: f64,
    },
    /// Content was evaluated against core drives.
    DriveAlignment {
        content_snippet: String,
        alignment_score: f64,
    },
}

// ── Domain State ──────────────────────────────────────────────────────

/// Aggregated state for a single domain (e.g., "coding").
///
/// Updated incrementally as signals arrive. This is the integration layer's
/// per-domain summary — what Craig calls the "meta-representation" of
/// interoceptive state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainState {
    /// Domain name.
    pub domain: String,

    /// Running valence trend: exponentially weighted moving average of
    /// recent signal valences. Range: [-1.0, 1.0].
    pub valence_trend: f64,

    /// Current anomaly level for this domain: 0.0 = normal, >2.0 = high.
    pub anomaly_level: f64,

    /// Recent action success rate in this domain: [0.0, 1.0].
    pub action_success_rate: f64,

    /// Drive alignment score for this domain: [0.0, 1.0].
    pub alignment_score: f64,

    /// Metacognitive confidence for this domain: [0.0, 1.0].
    pub confidence: f64,

    /// Number of signals processed for this domain (all sources).
    pub signal_count: u64,

    /// Last time this domain state was updated.
    pub last_updated: DateTime<Utc>,
}

impl DomainState {
    /// Create a fresh domain state with neutral baselines.
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            valence_trend: 0.0,
            anomaly_level: 0.0,
            action_success_rate: 0.5,
            alignment_score: 0.5,
            confidence: 0.5,
            signal_count: 0,
            last_updated: Utc::now(),
        }
    }

    /// Update this domain state with a new signal, using exponential
    /// weighted moving average (alpha controls recency bias).
    pub fn update(&mut self, signal: &InteroceptiveSignal, alpha: f64) {
        let alpha = alpha.clamp(0.0, 1.0);

        // Always update valence trend (from any source).
        self.valence_trend = alpha * signal.valence + (1.0 - alpha) * self.valence_trend;

        // Source-specific field updates.
        match signal.source {
            SignalSource::Anomaly => {
                self.anomaly_level = alpha * signal.arousal * 3.0
                    + (1.0 - alpha) * self.anomaly_level;
            }
            SignalSource::Feedback => {
                // Map valence [-1,1] back to success rate [0,1].
                let rate = (signal.valence + 1.0) / 2.0;
                self.action_success_rate =
                    alpha * rate + (1.0 - alpha) * self.action_success_rate;
            }
            SignalSource::Alignment => {
                let score = (signal.valence + 1.0) / 2.0;
                self.alignment_score =
                    alpha * score + (1.0 - alpha) * self.alignment_score;
            }
            SignalSource::Confidence => {
                let conf = (signal.valence + 1.0) / 2.0;
                self.confidence = alpha * conf + (1.0 - alpha) * self.confidence;
            }
            SignalSource::Accumulator => {
                // Accumulator directly contributes to valence_trend (already done above).
            }
        }

        self.signal_count += 1;
        self.last_updated = Utc::now();
    }
}

// ── Somatic Marker ────────────────────────────────────────────────────

/// A cached emotional association with a situation (Damasio's somatic marker).
///
/// When the system encounters a situation hash it has seen before,
/// the cached marker provides an instant "gut feeling" before full analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SomaticMarker {
    /// Hash of the situation / context that triggered this marker.
    pub situation_hash: u64,

    /// The average emotional valence associated with this situation.
    pub valence: f64,

    /// How many times this situation has been encountered.
    pub encounter_count: u32,

    /// When this marker was last accessed (for LRU eviction).
    pub last_accessed: DateTime<Utc>,
}

impl SomaticMarker {
    /// Create a new marker from a first encounter.
    pub fn new(situation_hash: u64, valence: f64) -> Self {
        Self {
            situation_hash,
            valence,
            encounter_count: 1,
            last_accessed: Utc::now(),
        }
    }

    /// Update the marker with a new encounter's valence.
    /// Uses incremental mean update.
    pub fn update(&mut self, new_valence: f64) {
        self.encounter_count += 1;
        let n = self.encounter_count as f64;
        self.valence += (new_valence - self.valence) / n;
        self.last_accessed = Utc::now();
    }
}

// ── Interoceptive State (snapshot) ────────────────────────────────────

/// Complete snapshot of the interoceptive system's current state.
///
/// This is what gets injected into the system prompt so the LLM can
/// "feel" its own internal state — Craig's conscious interoceptive awareness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteroceptiveState {
    /// Per-domain aggregated states.
    pub domain_states: HashMap<String, DomainState>,

    /// Global arousal level: weighted average across all domains.
    /// 0.0 = calm, 1.0 = high alert.
    pub global_arousal: f64,

    /// Number of signals currently in the buffer.
    pub buffer_size: usize,

    /// Recent somatic marker lookups (situation_hash → valence).
    /// Only includes markers accessed in this session.
    pub active_markers: Vec<SomaticMarker>,

    /// Snapshot timestamp.
    pub timestamp: DateTime<Utc>,
}

impl InteroceptiveState {
    /// Format this state for injection into a system prompt.
    pub fn to_prompt_section(&self) -> String {
        if self.domain_states.is_empty() {
            return String::from("Internal state: no data yet.");
        }

        let mut lines = vec!["## Internal State (Interoceptive)".to_string()];

        // Sort domains for deterministic output.
        let mut domains: Vec<_> = self.domain_states.values().collect();
        domains.sort_by(|a, b| a.domain.cmp(&b.domain));

        for ds in &domains {
            let sentiment = if ds.valence_trend > 0.3 {
                "positive"
            } else if ds.valence_trend < -0.3 {
                "negative"
            } else {
                "neutral"
            };
            lines.push(format!(
                "- **{}**: {} (valence {:.2}, anomaly {:.1}, confidence {:.0}%, alignment {:.0}%)",
                ds.domain,
                sentiment,
                ds.valence_trend,
                ds.anomaly_level,
                ds.confidence * 100.0,
                ds.alignment_score * 100.0,
            ));
        }

        if self.global_arousal > 0.5 {
            lines.push(format!(
                "- ⚡ Elevated arousal: {:.0}%",
                self.global_arousal * 100.0
            ));
        }

        if !self.active_markers.is_empty() {
            lines.push(format!(
                "- Somatic markers active: {} situation(s) recognized",
                self.active_markers.len()
            ));
        }

        lines.join("\n")
    }
}

// ── Regulation Action ─────────────────────────────────────────────────

/// An action suggested by the regulation layer.
///
/// These are advisory only — the caller (RustClaw hooks) decides whether
/// to act on them. This separation keeps engramai a pure library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RegulationAction {
    /// Suggest updating SOUL.md based on persistent emotional patterns.
    SoulUpdateSuggestion {
        domain: String,
        reason: String,
        valence_trend: f64,
    },

    /// Suggest adjusting memory retrieval parameters.
    RetrievalAdjustment {
        expand_search: bool,
        reason: String,
    },

    /// Suggest changing behavior for a specific action.
    BehaviorShift {
        action: String,
        recommendation: String,
        success_rate: f64,
    },

    /// Alert: something needs attention.
    Alert {
        severity: AlertSeverity,
        message: String,
        domains: Vec<String>,
    },
}

/// Alert severity levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_clamps_values() {
        let sig = InteroceptiveSignal::new(SignalSource::Anomaly, None, 5.0, -2.0);
        assert_eq!(sig.valence, 1.0);
        assert_eq!(sig.arousal, 0.0);

        let sig = InteroceptiveSignal::new(SignalSource::Anomaly, None, -5.0, 3.0);
        assert_eq!(sig.valence, -1.0);
        assert_eq!(sig.arousal, 1.0);
    }

    #[test]
    fn signal_negative_and_urgent() {
        let calm_positive = InteroceptiveSignal::new(
            SignalSource::Accumulator,
            Some("coding".into()),
            0.5,
            0.2,
        );
        assert!(!calm_positive.is_negative());
        assert!(!calm_positive.is_urgent());

        let alarming = InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("trading".into()),
            -0.8,
            0.9,
        );
        assert!(alarming.is_negative());
        assert!(alarming.is_urgent());
    }

    #[test]
    fn signal_with_context() {
        let sig = InteroceptiveSignal::new(SignalSource::Anomaly, None, -0.5, 0.8)
            .with_context(SignalContext::AnomalyDetected {
                metric: "recall_latency".into(),
                z_score: 2.5,
                baseline_mean: 120.0,
            });
        assert!(sig.context.is_some());
    }

    #[test]
    fn domain_state_update_ewma() {
        let mut ds = DomainState::new("coding");
        assert_eq!(ds.valence_trend, 0.0);

        // Send several positive signals.
        let alpha = 0.3;
        for _ in 0..10 {
            let sig = InteroceptiveSignal::new(
                SignalSource::Accumulator,
                Some("coding".into()),
                0.8,
                0.2,
            );
            ds.update(&sig, alpha);
        }
        // After 10 positive signals, trend should be strongly positive.
        assert!(ds.valence_trend > 0.6, "got {}", ds.valence_trend);
        assert_eq!(ds.signal_count, 10);
    }

    #[test]
    fn domain_state_source_specific_updates() {
        let mut ds = DomainState::new("trading");
        let alpha = 0.5;

        // Feedback signal: high success → maps to high action_success_rate.
        let feedback = InteroceptiveSignal::new(
            SignalSource::Feedback,
            Some("trading".into()),
            0.6, // maps to rate = 0.8
            0.3,
        );
        ds.update(&feedback, alpha);
        // Initial 0.5, blended with 0.8 at alpha=0.5 → 0.65
        assert!((ds.action_success_rate - 0.65).abs() < 0.01);

        // Anomaly signal: high arousal → anomaly_level rises.
        let anomaly = InteroceptiveSignal::new(
            SignalSource::Anomaly,
            Some("trading".into()),
            -0.5,
            0.9,
        );
        ds.update(&anomaly, alpha);
        // anomaly_level: 0.5 * 0.9 * 3.0 + 0.5 * 0.0 = 1.35
        assert!((ds.anomaly_level - 1.35).abs() < 0.01);
    }

    #[test]
    fn somatic_marker_incremental_mean() {
        let mut marker = SomaticMarker::new(12345, 0.5);
        assert_eq!(marker.encounter_count, 1);
        assert_eq!(marker.valence, 0.5);

        marker.update(-0.5);
        assert_eq!(marker.encounter_count, 2);
        assert!((marker.valence - 0.0).abs() < f64::EPSILON);

        marker.update(0.5);
        assert_eq!(marker.encounter_count, 3);
        // (0.5 + (-0.5) + 0.5) / 3 ≈ 0.1667
        assert!((marker.valence - 1.0 / 6.0).abs() < 0.01);
    }

    #[test]
    fn interoceptive_state_prompt_output() {
        let mut state = InteroceptiveState {
            domain_states: HashMap::new(),
            global_arousal: 0.3,
            buffer_size: 42,
            active_markers: vec![],
            timestamp: Utc::now(),
        };

        // Empty → short message.
        assert_eq!(state.to_prompt_section(), "Internal state: no data yet.");

        // Add a domain.
        let mut ds = DomainState::new("coding");
        ds.valence_trend = 0.6;
        ds.confidence = 0.85;
        ds.alignment_score = 0.9;
        state.domain_states.insert("coding".into(), ds);

        let prompt = state.to_prompt_section();
        assert!(prompt.contains("coding"));
        assert!(prompt.contains("positive"));
        assert!(!prompt.contains("arousal")); // 0.3 < 0.5 threshold
    }
}
