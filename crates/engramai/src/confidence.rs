//! Confidence Calibration — Two-dimensional metacognitive monitoring.
//!
//! Combines content reliability (how trustworthy is the information) with
//! retrieval salience (how strongly was it retrieved) to produce calibrated
//! confidence scores.
//!
//! Based on metacognition research: humans monitor both what they know
//! and how well they know it.

use crate::types::{MemoryRecord, MemoryType};
use serde::{Deserialize, Serialize};

/// Default reliability scores by memory type.
///
/// Based on cognitive science: emotional memories are vivid but may be
/// reconstructed, factual memories are reliable but decay, opinions
/// are subjective by nature.
pub mod default_reliability {
    pub const FACTUAL: f64 = 0.85;
    pub const EPISODIC: f64 = 0.90;
    pub const RELATIONAL: f64 = 0.75;
    pub const EMOTIONAL: f64 = 0.95;
    pub const PROCEDURAL: f64 = 0.90;
    pub const OPINION: f64 = 0.60;
    pub const CAUSAL: f64 = 0.80;
}

/// Confidence labels with thresholds.
pub mod confidence_thresholds {
    pub const CERTAIN: f64 = 0.8;
    pub const LIKELY: f64 = 0.6;
    pub const UNCERTAIN: f64 = 0.4;
    // Below UNCERTAIN = "vague"
}

/// Detailed confidence breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceDetail {
    /// Content reliability score (0.0-1.0)
    pub reliability: f64,
    /// Retrieval salience score (0.0-1.0)
    pub salience: f64,
    /// Combined confidence score (0.0-1.0)
    pub combined: f64,
    /// Human-readable confidence label
    pub label: String,
    /// Explanation of the confidence assessment
    pub description: String,
}

/// Get the base reliability score for a memory type.
pub fn type_reliability(memory_type: MemoryType) -> f64 {
    match memory_type {
        MemoryType::Factual => default_reliability::FACTUAL,
        MemoryType::Episodic => default_reliability::EPISODIC,
        MemoryType::Relational => default_reliability::RELATIONAL,
        MemoryType::Emotional => default_reliability::EMOTIONAL,
        MemoryType::Procedural => default_reliability::PROCEDURAL,
        MemoryType::Opinion => default_reliability::OPINION,
        MemoryType::Causal => default_reliability::CAUSAL,
    }
}

/// Calculate content reliability for a memory record.
///
/// Factors:
/// - Base reliability from memory type
/// - Reduction if contradicted by other memories
/// - Boost if pinned (manually verified)
/// - Small boost from importance (salient memories are better encoded)
pub fn content_reliability(record: &MemoryRecord) -> f64 {
    let mut reliability = type_reliability(record.memory_type);
    
    // Reduce reliability if contradicted
    if record.contradicted_by.is_some() {
        reliability *= 0.7; // 30% reduction
    }
    
    // Boost if pinned (manual verification implies reliability)
    if record.pinned {
        reliability = (reliability * 1.1).min(1.0);
    }
    
    // Small boost from importance (salient memories are better encoded)
    reliability = reliability * 0.95 + record.importance * 0.05;
    
    reliability.clamp(0.0, 1.0)
}

/// Calculate retrieval salience for a memory record.
///
/// Normalized effective strength relative to maximum possible or observed.
/// If all_records is provided, normalizes against the maximum observed.
/// Otherwise uses a sigmoid fallback.
pub fn retrieval_salience(record: &MemoryRecord, all_records: Option<&[MemoryRecord]>) -> f64 {
    let effective = effective_strength(record);
    
    if let Some(records) = all_records {
        if records.is_empty() {
            return sigmoid_salience(effective);
        }
        
        let max_strength = records.iter()
            .map(effective_strength)
            .fold(f64::NEG_INFINITY, f64::max);
        
        if max_strength <= 0.0 {
            return sigmoid_salience(effective);
        }
        
        (effective / max_strength).clamp(0.0, 1.0)
    } else {
        sigmoid_salience(effective)
    }
}

/// Calculate effective memory strength (working + core).
fn effective_strength(record: &MemoryRecord) -> f64 {
    record.working_strength + record.core_strength
}

/// Sigmoid-based salience when no normalization reference is available.
///
/// Maps effective strength to 0-1 range with reasonable curve.
fn sigmoid_salience(strength: f64) -> f64 {
    // Sigmoid centered around strength=1.0
    1.0 / (1.0 + (-2.0 * (strength - 0.5)).exp())
}

/// Calculate combined confidence score.
///
/// Weights reliability more heavily (70%) than salience (30%)
/// because "what you know" matters more than "how well you retrieved it".
pub fn confidence_score(record: &MemoryRecord, all_records: Option<&[MemoryRecord]>) -> f64 {
    let reliability = content_reliability(record);
    let salience = retrieval_salience(record, all_records);
    
    // 70% reliability, 30% salience
    (0.7 * reliability + 0.3 * salience).clamp(0.0, 1.0)
}

/// Get human-readable confidence label.
pub fn confidence_label(score: f64) -> &'static str {
    if score >= confidence_thresholds::CERTAIN {
        "certain"
    } else if score >= confidence_thresholds::LIKELY {
        "likely"
    } else if score >= confidence_thresholds::UNCERTAIN {
        "uncertain"
    } else {
        "vague"
    }
}

/// Generate detailed confidence assessment.
pub fn confidence_detail(
    record: &MemoryRecord,
    all_records: Option<&[MemoryRecord]>,
) -> ConfidenceDetail {
    let reliability = content_reliability(record);
    let salience = retrieval_salience(record, all_records);
    let combined = 0.7 * reliability + 0.3 * salience;
    let label = confidence_label(combined).to_string();
    
    // Generate description based on factors
    let mut factors = Vec::new();
    
    // Type-based description
    let type_desc = match record.memory_type {
        MemoryType::Factual => "factual knowledge",
        MemoryType::Episodic => "specific episode",
        MemoryType::Relational => "relationship knowledge",
        MemoryType::Emotional => "emotional memory",
        MemoryType::Procedural => "procedural knowledge",
        MemoryType::Opinion => "opinion/subjective",
        MemoryType::Causal => "causal relationship",
    };
    factors.push(format!("Based on {}", type_desc));
    
    // Contradiction warning
    if record.contradicted_by.is_some() {
        factors.push("possibly contradicted".to_string());
    }
    
    // Pinned indicator
    if record.pinned {
        factors.push("manually verified".to_string());
    }
    
    // Strength indicator
    let strength = effective_strength(record);
    if strength > 1.5 {
        factors.push("strongly retained".to_string());
    } else if strength < 0.3 {
        factors.push("weakly retained".to_string());
    }
    
    let description = factors.join("; ");
    
    ConfidenceDetail {
        reliability,
        salience,
        combined,
        label,
        description,
    }
}

/// Batch calculate confidence for multiple records.
pub fn batch_confidence(records: &[MemoryRecord]) -> Vec<(String, f64, &'static str)> {
    records.iter()
        .map(|r| {
            let score = confidence_score(r, Some(records));
            let label = confidence_label(score);
            (r.id.clone(), score, label)
        })
        .collect()
}

/// Confidence calibration for recall results.
///
/// Given activation scores and records, produces calibrated confidence.
/// This is used in recall to convert raw activation to meaningful confidence.
pub fn calibrate_confidence(
    record: &MemoryRecord,
    activation: f64,
    all_records: &[MemoryRecord],
) -> f64 {
    // Normalize activation to 0-1 range (rough heuristic)
    let normalized_activation = ((activation + 10.0) / 20.0).clamp(0.0, 1.0);
    
    // Get reliability
    let reliability = content_reliability(record);
    
    // Get salience from record strength
    let salience = retrieval_salience(record, Some(all_records));
    
    // Combine: activation weight, reliability weight, salience weight
    // Activation: 40%, Reliability: 40%, Salience: 20%
    (0.4 * normalized_activation + 0.4 * reliability + 0.2 * salience).clamp(0.0, 1.0)
}

/// Convert a confidence score into an [`InteroceptiveSignal`].
///
/// - `valence`: confidence mapped to [-1, 1] (0.5 → 0, 1.0 → 1.0, 0.0 → -1.0).
/// - `arousal`: elevated when confidence is very low (uncertainty = alerting).
pub fn confidence_to_signal(
    record: &MemoryRecord,
    all_records: Option<&[MemoryRecord]>,
    query: Option<&str>,
) -> crate::interoceptive::InteroceptiveSignal {
    use crate::interoceptive::{InteroceptiveSignal, SignalContext, SignalSource};

    let score = confidence_score(record, all_records);
    let valence = score * 2.0 - 1.0; // [0,1] → [-1,1]
    let arousal = if score < 0.4 {
        (1.0 - score) * 0.6 // low confidence → elevated arousal
    } else {
        0.1
    };

    let mut sig = InteroceptiveSignal::new(SignalSource::Confidence, None, valence, arousal);

    if let Some(q) = query {
        sig = sig.with_context(SignalContext::RecallConfidence {
            query: q.to_string(),
            score,
        });
    }

    sig
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::types::MemoryLayer;
    
    fn make_record(
        memory_type: MemoryType,
        importance: f64,
        pinned: bool,
        contradicted: bool,
        working: f64,
        core: f64,
    ) -> MemoryRecord {
        MemoryRecord {
            id: "test".to_string(),
            content: "Test memory".to_string(),
            memory_type,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: working,
            core_strength: core,
            importance,
            pinned,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: if contradicted { Some("other".to_string()) } else { None },
            metadata: None,
        }
    }
    
    #[test]
    fn test_type_reliability() {
        assert!((type_reliability(MemoryType::Factual) - 0.85).abs() < 0.01);
        assert!((type_reliability(MemoryType::Opinion) - 0.60).abs() < 0.01);
        assert!(type_reliability(MemoryType::Emotional) > type_reliability(MemoryType::Opinion));
    }
    
    #[test]
    fn test_content_reliability_basic() {
        let record = make_record(MemoryType::Factual, 0.5, false, false, 1.0, 0.0);
        let reliability = content_reliability(&record);
        
        // Should be close to base reliability
        assert!(reliability > 0.8);
        assert!(reliability < 0.9);
    }
    
    #[test]
    fn test_content_reliability_contradicted() {
        let normal = make_record(MemoryType::Factual, 0.5, false, false, 1.0, 0.0);
        let contradicted = make_record(MemoryType::Factual, 0.5, false, true, 1.0, 0.0);
        
        let rel_normal = content_reliability(&normal);
        let rel_contradicted = content_reliability(&contradicted);
        
        // Contradicted should have lower reliability
        assert!(rel_contradicted < rel_normal);
    }
    
    #[test]
    fn test_content_reliability_pinned() {
        let normal = make_record(MemoryType::Factual, 0.5, false, false, 1.0, 0.0);
        let pinned = make_record(MemoryType::Factual, 0.5, true, false, 1.0, 0.0);
        
        let rel_normal = content_reliability(&normal);
        let rel_pinned = content_reliability(&pinned);
        
        // Pinned should have higher reliability
        assert!(rel_pinned > rel_normal);
    }
    
    #[test]
    fn test_retrieval_salience() {
        let strong = make_record(MemoryType::Factual, 0.5, false, false, 1.5, 0.5);
        let weak = make_record(MemoryType::Factual, 0.5, false, false, 0.2, 0.0);
        
        let records = vec![strong.clone(), weak.clone()];
        
        let sal_strong = retrieval_salience(&strong, Some(&records));
        let sal_weak = retrieval_salience(&weak, Some(&records));
        
        // Strong should have higher salience
        assert!(sal_strong > sal_weak);
        // Strong should be normalized to 1.0
        assert!((sal_strong - 1.0).abs() < 0.01);
    }
    
    #[test]
    fn test_confidence_score() {
        let record = make_record(MemoryType::Factual, 0.8, true, false, 1.0, 0.5);
        let score = confidence_score(&record, None);
        
        // High reliability factual, pinned, good strength -> high confidence
        assert!(score > 0.7);
    }
    
    #[test]
    fn test_confidence_labels() {
        assert_eq!(confidence_label(0.9), "certain");
        assert_eq!(confidence_label(0.8), "certain");
        assert_eq!(confidence_label(0.7), "likely");
        assert_eq!(confidence_label(0.5), "uncertain");
        assert_eq!(confidence_label(0.2), "vague");
    }
    
    #[test]
    fn test_confidence_detail() {
        let record = make_record(MemoryType::Emotional, 0.9, true, false, 1.0, 0.0);
        let detail = confidence_detail(&record, None);
        
        assert!(detail.reliability > 0.9); // Emotional + pinned
        assert!(detail.combined > 0.7);
        assert!(detail.description.contains("emotional"));
        assert!(detail.description.contains("verified"));
    }
    
    #[test]
    fn test_batch_confidence() {
        let records = vec![
            make_record(MemoryType::Factual, 0.5, false, false, 1.0, 0.0),
            make_record(MemoryType::Opinion, 0.3, false, false, 0.5, 0.0),
        ];
        
        let confidences = batch_confidence(&records);
        
        assert_eq!(confidences.len(), 2);
        // Factual should have higher confidence than Opinion
        assert!(confidences[0].1 > confidences[1].1);
    }
    
    #[test]
    fn test_calibrate_confidence() {
        let record = make_record(MemoryType::Factual, 0.8, false, false, 1.0, 0.0);
        let records = vec![record.clone()];
        
        // High activation should give high confidence
        let high_conf = calibrate_confidence(&record, 5.0, &records);
        let low_conf = calibrate_confidence(&record, -5.0, &records);
        
        assert!(high_conf > low_conf);
        assert!(high_conf <= 1.0);
        assert!(low_conf >= 0.0);
    }

    #[test]
    fn test_confidence_to_signal_high_confidence() {
        let record = make_record(MemoryType::Factual, 0.9, true, false, 1.0, 0.5);
        let sig = confidence_to_signal(&record, None, Some("test query"));

        assert!(matches!(sig.source, crate::interoceptive::SignalSource::Confidence));
        assert!(sig.domain.is_none());
        // High confidence → positive valence
        assert!(sig.valence > 0.3, "valence was {}", sig.valence);
        // High confidence → low arousal (no alarm)
        assert!(sig.arousal < 0.2, "arousal was {}", sig.arousal);
        assert!(matches!(
            sig.context,
            Some(crate::interoceptive::SignalContext::RecallConfidence { .. })
        ));
    }

    #[test]
    fn test_confidence_to_signal_low_confidence() {
        // Contradicted, opinion, low importance → low confidence
        let record = make_record(MemoryType::Opinion, 0.2, false, true, 0.1, 0.0);
        let sig = confidence_to_signal(&record, None, None);

        // Low confidence → negative valence
        assert!(sig.valence < 0.0, "valence was {}", sig.valence);
        // Low confidence → elevated arousal (uncertainty = alerting)
        assert!(sig.arousal > 0.3, "arousal was {}", sig.arousal);
        // No query → no context
        assert!(sig.context.is_none());
    }
}
