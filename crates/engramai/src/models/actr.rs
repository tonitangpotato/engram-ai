//! ACT-R activation-based retrieval.
//!
//! The core equation from Anderson's ACT-R theory:
//!     A_i = B_i + Σ(W_j · S_ji) + ε
//!
//! Where:
//!     B_i = base-level activation (frequency × recency)
//!     Σ(W_j · S_ji) = spreading activation from current context
//!     ε = noise (stochastic retrieval, not implemented here)
//!
//! Base-level activation (power law of practice and recency):
//!     B_i = ln(Σ_k  t_k^(-d))
//!
//! Where t_k = time since k-th access, d = decay parameter (~0.5)

use chrono::{DateTime, Utc};

use crate::types::MemoryRecord;

/// ACT-R base-level activation.
///
/// B_i = ln(Σ_k (now - t_k)^(-d))
///
/// Higher when accessed more often and more recently.
/// Returns -inf if no accesses (unretrievable).
pub fn base_level_activation(record: &MemoryRecord, now: DateTime<Utc>, decay: f64) -> f64 {
    if record.access_times.is_empty() {
        return f64::NEG_INFINITY;
    }

    let mut total = 0.0;
    for t_k in &record.access_times {
        let age_secs = (now - *t_k).num_seconds() as f64;
        let age_secs = age_secs.max(0.001); // Avoid division by zero
        total += age_secs.powf(-decay);
    }

    if total <= 0.0 {
        return f64::NEG_INFINITY;
    }

    total.ln()
}

/// Simple spreading activation from current context.
///
/// In full ACT-R, this uses semantic similarity between context elements
/// and memory chunks. Here we use keyword overlap as a proxy.
///
/// Σ(W_j · S_ji) ≈ weight × (overlap / total_keywords)
pub fn spreading_activation(record: &MemoryRecord, context_keywords: &[String], weight: f64) -> f64 {
    if context_keywords.is_empty() {
        return 0.0;
    }

    let content_lower = record.content.to_lowercase();
    let matches = context_keywords
        .iter()
        .filter(|kw| content_lower.contains(&kw.to_lowercase()))
        .count();

    weight * (matches as f64 / context_keywords.len() as f64)
}

/// Full retrieval activation score.
///
/// A_i = B_i + context_match + importance_boost - contradiction_penalty
///
/// Combines ACT-R base-level with context spreading activation
/// and emotional/importance modulation.
pub fn retrieval_activation(
    record: &MemoryRecord,
    context_keywords: &[String],
    now: DateTime<Utc>,
    base_decay: f64,
    context_weight: f64,
    importance_weight: f64,
    contradiction_penalty: f64,
) -> f64 {
    let base = base_level_activation(record, now, base_decay);

    if base == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }

    let context = spreading_activation(record, context_keywords, context_weight);

    // Importance modulation (amygdala analog)
    let importance_boost = record.importance * importance_weight;

    // Contradiction penalty
    let penalty = if record.contradicted_by.is_some() {
        contradiction_penalty
    } else {
        0.0
    };

    base + context + importance_boost - penalty
}

/// Normalize ACT-R activation to \[0, 1\] using a sigmoid function.
///
/// The old linear normalization `(activation + 10) / 20` compressed the useful
/// range into a narrow 0.13–0.40 band, making recency differences almost
/// invisible after weighting. A sigmoid centered near the median activation
/// of typical single-access memories (~1 day old) gives 3× better discrimination
/// between recent and old memories.
///
/// # Parameters
///
/// * `activation` - Raw ACT-R activation score (typically -10 to +3)
/// * `center` - Sigmoid midpoint (default -5.5, ≈ 1 day old single-access memory)
/// * `scale` - Sigmoid steepness (default 1.5; smaller = steeper)
///
/// # Returns
///
/// Value in \[0, 1\]. Recent/frequent memories → closer to 1.0, old/rare → closer to 0.0.
pub fn normalize_activation(activation: f64, center: f64, scale: f64) -> f64 {
    if activation == f64::NEG_INFINITY {
        return 0.0;
    }
    1.0 / (1.0 + (-(activation - center) / scale).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    
    fn make_record(age_secs: i64) -> (MemoryRecord, DateTime<Utc>) {
        let now = Utc::now();
        let access_time = now - Duration::seconds(age_secs);
        let record = MemoryRecord {
            id: "test".to_string(),
            content: "test content".to_string(),
            memory_type: crate::types::MemoryType::Factual,
            importance: 0.5,
            access_times: vec![access_time],
            working_strength: 1.0,
            core_strength: 0.0,
            layer: crate::types::MemoryLayer::Working,
            consolidation_count: 0,
            last_consolidated: None,
            created_at: access_time,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            pinned: false,
            metadata: None,
        };
        (record, now)
    }
    
    #[test]
    fn test_normalize_neg_infinity() {
        assert_eq!(normalize_activation(f64::NEG_INFINITY, -5.5, 1.5), 0.0);
    }
    
    #[test]
    fn test_normalize_center_gives_half() {
        let result = normalize_activation(-5.5, -5.5, 1.5);
        assert!((result - 0.5).abs() < 0.001, "center should map to ~0.5, got {result}");
    }
    
    #[test]
    fn test_normalize_monotonic() {
        // Higher activation → higher normalized score
        let low = normalize_activation(-8.0, -5.5, 1.5);
        let mid = normalize_activation(-5.5, -5.5, 1.5);
        let high = normalize_activation(-2.0, -5.5, 1.5);
        assert!(low < mid, "low {low} should be < mid {mid}");
        assert!(mid < high, "mid {mid} should be < high {high}");
    }
    
    #[test]
    fn test_normalize_bounded() {
        // Output always in [0, 1]
        for x in [-100.0, -10.0, -5.5, 0.0, 5.0, 100.0] {
            let n = normalize_activation(x, -5.5, 1.5);
            assert!(n >= 0.0 && n <= 1.0, "normalize({x}) = {n} out of [0,1]");
        }
    }
    
    #[test]
    fn test_recency_discrimination_improved() {
        // The core fix: sigmoid gives 3x better discrimination than old linear
        let d = 0.5;
        let center = -5.5;
        let scale = 1.5;
        
        // 1 hour old memory vs 7 day old memory (single access)
        let b_1h = (3600.0_f64).powf(-d).ln();   // B_i for 1 hour
        let b_7d = (604800.0_f64).powf(-d).ln();  // B_i for 7 days
        
        let old_delta = ((b_1h + 10.0) / 20.0).clamp(0.0, 1.0)
            - ((b_7d + 10.0) / 20.0).clamp(0.0, 1.0);
        let new_delta = normalize_activation(b_1h, center, scale)
            - normalize_activation(b_7d, center, scale);
        
        // Sigmoid should give at least 2.5x better discrimination
        assert!(
            new_delta > old_delta * 2.5,
            "sigmoid delta ({new_delta:.4}) should be >2.5x old delta ({old_delta:.4})"
        );
    }
    
    #[test]
    fn test_base_level_recency() {
        // More recent memory → higher base-level activation
        let (recent, now) = make_record(60);     // 1 minute ago
        let (old, _) = make_record(604800);       // 7 days ago
        
        let b_recent = base_level_activation(&recent, now, 0.5);
        let b_old = base_level_activation(&old, now, 0.5);
        assert!(b_recent > b_old, "recent ({b_recent}) should have higher activation than old ({b_old})");
    }
    
    #[test]
    fn test_base_level_frequency() {
        // More frequently accessed → higher base-level activation
        let now = Utc::now();
        let mut frequent = make_record(3600).0;
        // Add 4 more accesses
        for i in 1..5 {
            frequent.access_times.push(now - Duration::seconds(3600 * i));
        }
        let (single, _) = make_record(3600);
        
        let b_freq = base_level_activation(&frequent, now, 0.5);
        let b_single = base_level_activation(&single, now, 0.5);
        assert!(b_freq > b_single, "frequent ({b_freq}) should have higher activation than single ({b_single})");
    }
    
    #[test]
    fn test_base_level_no_accesses() {
        let (mut record, now) = make_record(60);
        record.access_times.clear();
        assert_eq!(base_level_activation(&record, now, 0.5), f64::NEG_INFINITY);
    }
    
    #[test]
    fn test_spreading_activation_matches() {
        let (record, _) = make_record(60);
        let context = vec!["test".to_string(), "content".to_string()];
        let spread = spreading_activation(&record, &context, 1.5);
        assert!(spread > 0.0, "should have positive spreading activation");
        assert!((spread - 1.5).abs() < 0.01, "full match should give ~weight, got {spread}");
    }
    
    #[test]
    fn test_spreading_activation_no_match() {
        let (record, _) = make_record(60);
        let context = vec!["zzzznotpresent".to_string()];
        let spread = spreading_activation(&record, &context, 1.5);
        assert_eq!(spread, 0.0);
    }
    
    #[test]
    fn test_normalize_scale_effect() {
        // Smaller scale → steeper transition → bigger gap between close values
        let steep = normalize_activation(-3.0, -5.5, 0.5) - normalize_activation(-8.0, -5.5, 0.5);
        let gentle = normalize_activation(-3.0, -5.5, 3.0) - normalize_activation(-8.0, -5.5, 3.0);
        // Both should be positive (monotonic), but steep should produce more extreme values
        assert!(steep > 0.0);
        assert!(gentle > 0.0);
        // With steep scale, the gap from -3 to -8 should be larger
        assert!(steep > gentle, "steep ({steep}) should discriminate more than gentle ({gentle})");
    }
}
