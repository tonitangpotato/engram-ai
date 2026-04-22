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
    // ── Engram-internal sources ──────────────────────────────────────
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

    // ── Runtime-sourced signals (from host agent) ────────────────────
    /// Token budget consumption and rate pressure.
    OperationalLoad,
    /// Loop depth, retries, tool failure patterns.
    ExecutionStress,
    /// Task completion rate, response latency, session coherence.
    CognitiveFlow,
    /// Memory utilization, disk I/O, queue depth.
    ResourcePressure,
    /// Voice/audio emotion detected from speech (wav2vec2 SER).
    VoiceEmotion,
}

impl std::fmt::Display for SignalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anomaly => write!(f, "anomaly"),
            Self::Accumulator => write!(f, "accumulator"),
            Self::Feedback => write!(f, "feedback"),
            Self::Confidence => write!(f, "confidence"),
            Self::Alignment => write!(f, "alignment"),
            Self::OperationalLoad => write!(f, "operational_load"),
            Self::ExecutionStress => write!(f, "execution_stress"),
            Self::CognitiveFlow => write!(f, "cognitive_flow"),
            Self::ResourcePressure => write!(f, "resource_pressure"),
            Self::VoiceEmotion => write!(f, "voice_emotion"),
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
    /// Accumulator: direct empathy valence.
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

    // ── Runtime signal contexts ──────────────────────────────────────

    /// Token budget pressure from host agent.
    TokenPressure {
        budget_used_pct: f64,
        tokens_per_second: f64,
        budget_runway_secs: f64,
    },

    /// Execution stress from agentic loop.
    LoopStress {
        loop_depth: u32,
        retry_count: u32,
        tool_failure_rate: f64,
        consecutive_failures: u32,
    },

    /// Cognitive flow from task execution.
    TaskFlow {
        task_completion_rate: f64,
        response_latency_ms: u64,
        session_duration_secs: u64,
    },

    /// Resource pressure from system metrics.
    SystemPressure {
        disk_free_gb: f64,
        queue_depth: u32,
    },

    /// Voice emotion detected from audio (speech emotion recognition).
    VoiceEmotion {
        /// Detected primary emotion label (e.g., "angry", "happy", "sad").
        primary_emotion: String,
        /// Confidence score for the primary emotion [0.0, 1.0].
        confidence: f64,
        /// All emotion scores (label → probability).
        all_scores: HashMap<String, f64>,
        /// Speaker identifier (e.g., Telegram user ID).
        speaker_id: Option<String>,
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
            // Runtime signal sources: all contribute to valence_trend (already done above).
            // Their specific metrics are carried in SignalContext and affect arousal via the
            // hub's global_arousal computation. No separate per-domain field needed — the
            // valence and arousal on the signal itself encode the semantic meaning.
            SignalSource::OperationalLoad
            | SignalSource::ExecutionStress
            | SignalSource::CognitiveFlow
            | SignalSource::ResourcePressure
            | SignalSource::VoiceEmotion => {
                // Valence already updated above. These runtime sources also contribute
                // to anomaly_level when arousal is high (indicates abnormal operating state).
                if signal.arousal > 0.5 {
                    self.anomaly_level =
                        alpha * signal.arousal * 2.0 + (1.0 - alpha) * self.anomaly_level;
                }
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

    /// The average empathy valence associated with this situation.
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

    /// Suggest updating IDENTITY.md based on stable behavioral patterns.
    /// Unlike SoulUpdateSuggestion (triggered by negative trends),
    /// this is triggered by stable positive or distinctive patterns
    /// that reflect the agent's evolved capabilities/traits.
    IdentityEvolutionSuggestion {
        /// The aspect of identity that evolved (e.g., "capability", "trait", "pattern")
        aspect: IdentityAspect,
        /// Description of the observed evolution
        observation: String,
        /// Domains where the pattern was observed
        domains: Vec<String>,
        /// Confidence in this observation (0.0-1.0)
        confidence: f64,
        /// Suggested text for IDENTITY.md
        suggestion: String,
    },

    /// Suggest adjusting heartbeat frequency based on system state.
    ///
    /// When anomalies/stress are elevated across domains → increase frequency
    /// (shorter intervals) for closer monitoring. When system is stable →
    /// decrease frequency (longer intervals) to save resources.
    HeartbeatFrequencyAdjustment {
        /// Direction of the adjustment.
        direction: HeartbeatAdjustDirection,
        /// Multiplier for current interval (e.g., 0.5 = halve interval, 2.0 = double).
        interval_multiplier: f64,
        /// Human-readable reason for the adjustment.
        reason: String,
        /// Which domains contributed to this decision.
        domains: Vec<String>,
    },
}

/// Direction of heartbeat frequency adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeartbeatAdjustDirection {
    /// Increase frequency (shorter interval) — more anomalies need closer monitoring.
    Increase,
    /// Decrease frequency (longer interval) — system is stable, save resources.
    Decrease,
}

/// What aspect of identity evolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityAspect {
    /// A new capability or skill emerged (e.g., "strong at Rust debugging")
    Capability,
    /// A behavioral pattern stabilized (e.g., "prefers sub-agents for small tasks")
    BehavioralPattern,
    /// A personality trait shifted (e.g., "more direct communicator")
    PersonalityTrait,
    /// A domain specialization appeared (e.g., "coding domain expert")
    DomainSpecialization,
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

// ── Adaptive Baseline ─────────────────────────────────────────────────

/// Rolling statistics for a single metric, using Welford's online algorithm.
///
/// Tracks mean and variance incrementally (O(1) per update, no stored history).
/// Used by the regulation layer to compute σ-deviations adaptively rather
/// than relying on hardcoded thresholds.
///
/// Cold-start behavior: `is_calibrated()` returns false until `min_samples`
/// observations have been recorded, signaling the caller to use conservative
/// fallback thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveBaseline {
    /// Running count of observations.
    pub count: u64,
    /// Running mean (Welford's M).
    pub mean: f64,
    /// Running sum of squared deviations from mean (Welford's S).
    /// Variance = m2 / (count - 1).
    m2: f64,
    /// Minimum observations before the baseline is considered calibrated.
    pub min_samples: u64,
    /// Exponential decay factor for gradual drift adaptation.
    /// 0.0 = pure Welford (no decay), >0 = recent samples weighted more.
    /// Recommended: 0.0 for short histories, 0.01-0.05 for long-running.
    pub decay: f64,
}

impl AdaptiveBaseline {
    /// Create a new baseline requiring `min_samples` before calibration.
    pub fn new(min_samples: u64) -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min_samples,
            decay: 0.0,
        }
    }

    /// Create with exponential decay for adapting to drift.
    pub fn with_decay(min_samples: u64, decay: f64) -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min_samples,
            decay: decay.clamp(0.0, 0.5),
        }
    }

    /// Record a new observation.
    pub fn observe(&mut self, value: f64) {
        self.count += 1;

        if self.decay > 0.0 && self.count > 1 {
            // Exponentially-weighted Welford: blend in new observation
            // with a decay that reduces influence of old observations.
            let alpha = self.decay;
            let delta = value - self.mean;
            self.mean += alpha * delta;
            let delta2 = value - self.mean;
            self.m2 = (1.0 - alpha) * (self.m2 + alpha * delta * delta2);
        } else {
            // Standard Welford's online algorithm.
            let delta = value - self.mean;
            self.mean += delta / self.count as f64;
            let delta2 = value - self.mean;
            self.m2 += delta * delta2;
        }
    }

    /// Whether enough samples have been collected for reliable statistics.
    pub fn is_calibrated(&self) -> bool {
        self.count >= self.min_samples
    }

    /// Population variance (biased, but stable with small N).
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            return 0.0;
        }
        if self.decay > 0.0 {
            self.m2.max(0.0)
        } else {
            (self.m2 / self.count as f64).max(0.0)
        }
    }

    /// Standard deviation.
    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Compute how many σ the given value deviates from the mean.
    ///
    /// Returns `None` if not yet calibrated or stddev is effectively zero
    /// (all observations identical → any deviation is infinitely surprising,
    /// but we can't meaningfully quantify it).
    pub fn sigma_deviation(&self, value: f64) -> Option<f64> {
        if !self.is_calibrated() {
            return None;
        }
        let sd = self.stddev();
        if sd < 1e-10 {
            // All observations nearly identical — if value matches, 0σ; otherwise, flag it.
            if (value - self.mean).abs() < 1e-10 {
                return Some(0.0);
            }
            // Large deviation from zero-variance baseline → cap at 5σ to avoid infinity.
            return Some(5.0);
        }
        Some((value - self.mean).abs() / sd)
    }

    /// Classify the deviation of a value into a severity level.
    pub fn deviation_level(&self, value: f64) -> DeviationLevel {
        match self.sigma_deviation(value) {
            None => DeviationLevel::Uncalibrated,
            Some(sigma) if sigma < 1.5 => DeviationLevel::Normal,
            Some(sigma) if sigma < 2.5 => DeviationLevel::Elevated,
            Some(sigma) if sigma < 3.5 => DeviationLevel::High,
            Some(_) => DeviationLevel::Extreme,
        }
    }
}

/// How far a signal deviates from its adaptive baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviationLevel {
    /// Not enough data yet — use conservative fallback.
    Uncalibrated,
    /// Within 1.5σ — normal operating range.
    Normal,
    /// 1.5–2.5σ — worth noting, no action needed.
    Elevated,
    /// 2.5–3.5σ — should trigger a response.
    High,
    /// >3.5σ — definitely abnormal, immediate action.
    Extreme,
}

impl DeviationLevel {
    /// Whether this level warrants a regulation action.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::High | Self::Extreme)
    }

    /// Whether this level is at least elevated.
    pub fn is_elevated(&self) -> bool {
        matches!(self, Self::Elevated | Self::High | Self::Extreme)
    }
}

impl std::fmt::Display for DeviationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncalibrated => write!(f, "uncalibrated"),
            Self::Normal => write!(f, "normal"),
            Self::Elevated => write!(f, "elevated"),
            Self::High => write!(f, "high"),
            Self::Extreme => write!(f, "extreme"),
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

    // ── AdaptiveBaseline tests ────────────────────────────────────────

    #[test]
    fn baseline_uncalibrated_before_min_samples() {
        let mut bl = AdaptiveBaseline::new(5);
        bl.observe(1.0);
        bl.observe(2.0);
        bl.observe(3.0);
        assert!(!bl.is_calibrated());
        assert_eq!(bl.sigma_deviation(5.0), None);
        assert_eq!(bl.deviation_level(5.0), DeviationLevel::Uncalibrated);
    }

    #[test]
    fn baseline_calibrates_at_min_samples() {
        let mut bl = AdaptiveBaseline::new(5);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            bl.observe(v);
        }
        assert!(bl.is_calibrated());
        assert_eq!(bl.count, 5);
        assert!((bl.mean - 3.0).abs() < 0.01);
        // stddev of [1,2,3,4,5] population = sqrt(2) ≈ 1.414
        assert!((bl.stddev() - std::f64::consts::SQRT_2).abs() < 0.01);
    }

    #[test]
    fn baseline_sigma_deviation_correct() {
        let mut bl = AdaptiveBaseline::new(5);
        for v in [10.0, 10.0, 10.0, 10.0, 10.0, 12.0, 8.0, 10.0, 10.0, 10.0] {
            bl.observe(v);
        }
        // Mean ≈ 10.0, low stddev
        let _sd = bl.stddev();
        let dev = bl.sigma_deviation(10.0).unwrap();
        assert!(dev < 0.5, "at-mean deviation should be near 0, got {}", dev);

        // A value far from mean should have high σ
        let dev_far = bl.sigma_deviation(15.0).unwrap();
        assert!(dev_far > 2.0, "5 units from mean should be >2σ, got {}", dev_far);
    }

    #[test]
    fn baseline_deviation_levels() {
        let mut bl = AdaptiveBaseline::new(3);
        // Create baseline with mean=0, stddev=1 (approximately)
        for v in [-1.0, 0.0, 1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 0.0] {
            bl.observe(v);
        }
        let sd = bl.stddev();

        // Within 1.5σ → Normal
        assert_eq!(bl.deviation_level(bl.mean), DeviationLevel::Normal);

        // 2σ → Elevated
        assert_eq!(bl.deviation_level(bl.mean + 2.0 * sd), DeviationLevel::Elevated);

        // 3σ → High
        assert_eq!(bl.deviation_level(bl.mean + 3.0 * sd), DeviationLevel::High);

        // 4σ → Extreme
        assert_eq!(bl.deviation_level(bl.mean + 4.0 * sd), DeviationLevel::Extreme);
    }

    #[test]
    fn baseline_zero_variance_handling() {
        let mut bl = AdaptiveBaseline::new(3);
        for _ in 0..5 {
            bl.observe(42.0);
        }
        // All identical → stddev ≈ 0
        assert!(bl.stddev() < 1e-10);
        // Same value → 0σ
        assert_eq!(bl.sigma_deviation(42.0), Some(0.0));
        // Different value → capped at 5σ
        assert_eq!(bl.sigma_deviation(43.0), Some(5.0));
    }

    #[test]
    fn baseline_with_decay_adapts() {
        let mut bl = AdaptiveBaseline::with_decay(3, 0.1);
        // Establish baseline around 10.0
        for _ in 0..10 {
            bl.observe(10.0);
        }
        let mean_before = bl.mean;

        // Shift to 20.0 — with decay, mean should drift faster
        for _ in 0..20 {
            bl.observe(20.0);
        }
        // Mean should have moved significantly toward 20
        assert!(bl.mean > mean_before + 3.0,
            "mean should drift toward 20 with decay, got {}", bl.mean);
    }

    #[test]
    fn deviation_level_actionable() {
        assert!(!DeviationLevel::Uncalibrated.is_actionable());
        assert!(!DeviationLevel::Normal.is_actionable());
        assert!(!DeviationLevel::Elevated.is_actionable());
        assert!(DeviationLevel::High.is_actionable());
        assert!(DeviationLevel::Extreme.is_actionable());
    }
}
