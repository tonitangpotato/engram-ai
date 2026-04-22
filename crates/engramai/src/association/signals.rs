//! Signal computation for multi-signal Hebbian link formation.
//!
//! Computes three association signals between memory pairs:
//! - Entity overlap (Jaccard similarity)
//! - Embedding cosine similarity
//! - Temporal proximity (exponential decay)

use std::collections::HashSet;

use crate::config::AssociationConfig;
use crate::embeddings::EmbeddingProvider;

/// Computed signal scores for a pair of memories.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignalScores {
    /// Jaccard similarity of entity sets [0, 1]
    pub entity_overlap: f64,
    /// Cosine similarity of embeddings [0, 1] (0 if no embeddings)
    pub embedding_cosine: f64,
    /// Exponential decay based on temporal distance [0, 1]
    pub temporal_proximity: f64,
}

impl SignalScores {
    /// Weighted combination using config weights.
    ///
    /// Weights are used as-is (not normalized) to match the config values directly.
    pub fn combined(&self, config: &AssociationConfig) -> f64 {
        config.w_entity * self.entity_overlap
            + config.w_embedding * self.embedding_cosine
            + config.w_temporal * self.temporal_proximity
    }

    /// Dominant signal name for signal_source field.
    ///
    /// Returns the name of the signal with the highest value.
    pub fn dominant_signal(&self) -> &'static str {
        if self.entity_overlap >= self.embedding_cosine
            && self.entity_overlap >= self.temporal_proximity
        {
            "entity"
        } else if self.embedding_cosine >= self.temporal_proximity {
            "embedding"
        } else {
            "temporal"
        }
    }

    /// Signal source classification (single signal vs multi).
    ///
    /// Counts how many signals are above `threshold`:
    /// - 0 or 1 active signals → returns the dominant single signal name
    /// - 2+ active signals → returns "multi"
    pub fn signal_source(&self, threshold: f64) -> &'static str {
        let count = [
            self.entity_overlap > threshold,
            self.embedding_cosine > threshold,
            self.temporal_proximity > threshold,
        ]
        .iter()
        .filter(|&&x| x)
        .count();

        if count >= 2 {
            "multi"
        } else {
            self.dominant_signal()
        }
    }

    /// JSON representation for signal_detail column.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Stateless signal computation utilities.
pub struct SignalComputer;

impl SignalComputer {
    /// Compute entity overlap via Jaccard similarity.
    ///
    /// Jaccard index = |A ∩ B| / |A ∪ B|.
    /// Returns 0.0 if both sets are empty.
    pub fn entity_jaccard(entities_a: &[String], entities_b: &[String]) -> f64 {
        if entities_a.is_empty() && entities_b.is_empty() {
            return 0.0;
        }

        let set_a: HashSet<&String> = entities_a.iter().collect();
        let set_b: HashSet<&String> = entities_b.iter().collect();

        let intersection = set_a.intersection(&set_b).count();
        let union = set_a.union(&set_b).count();

        if union == 0 {
            0.0
        } else {
            intersection as f64 / union as f64
        }
    }

    /// Compute embedding cosine similarity.
    ///
    /// Returns 0.0 if either embedding is None.
    /// Delegates to `EmbeddingProvider::cosine_similarity` for the actual math.
    pub fn embedding_cosine(emb_a: Option<&[f32]>, emb_b: Option<&[f32]>) -> f64 {
        match (emb_a, emb_b) {
            (Some(a), Some(b)) => EmbeddingProvider::cosine_similarity(a, b) as f64,
            _ => 0.0,
        }
    }

    /// Compute temporal proximity with exponential decay.
    ///
    /// Formula: exp(-|t_a - t_b| / (half_life_days * 86400))
    /// Uses 3 days as the half-life constant.
    pub fn temporal_proximity(timestamp_a: f64, timestamp_b: f64, half_life_days: f64) -> f64 {
        let delta_secs = (timestamp_a - timestamp_b).abs();
        let decay_constant = half_life_days * 86400.0;
        (-delta_secs / decay_constant).exp()
    }

    /// Compute all signals at once.
    ///
    /// Uses a default half-life of 3 days for temporal proximity.
    pub fn compute_all(
        entities_a: &[String],
        entities_b: &[String],
        emb_a: Option<&[f32]>,
        emb_b: Option<&[f32]>,
        time_a: f64,
        time_b: f64,
    ) -> SignalScores {
        SignalScores {
            entity_overlap: Self::entity_jaccard(entities_a, entities_b),
            embedding_cosine: Self::embedding_cosine(emb_a, emb_b),
            temporal_proximity: Self::temporal_proximity(time_a, time_b, 3.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Entity Jaccard tests ---

    #[test]
    fn test_entity_jaccard_no_overlap() {
        let a = vec!["cat".to_string(), "dog".to_string()];
        let b = vec!["fish".to_string(), "bird".to_string()];
        let score = SignalComputer::entity_jaccard(&a, &b);
        assert!((score - 0.0).abs() < f64::EPSILON, "disjoint sets should be 0.0, got {}", score);
    }

    #[test]
    fn test_entity_jaccard_full_overlap() {
        let a = vec!["cat".to_string(), "dog".to_string()];
        let b = vec!["dog".to_string(), "cat".to_string()];
        let score = SignalComputer::entity_jaccard(&a, &b);
        assert!((score - 1.0).abs() < f64::EPSILON, "identical sets should be 1.0, got {}", score);
    }

    #[test]
    fn test_entity_jaccard_partial() {
        let a = vec!["cat".to_string(), "dog".to_string(), "fish".to_string()];
        let b = vec!["dog".to_string(), "bird".to_string()];
        // intersection = {dog} = 1, union = {cat, dog, fish, bird} = 4
        let score = SignalComputer::entity_jaccard(&a, &b);
        assert!((score - 0.25).abs() < f64::EPSILON, "partial overlap should be 0.25, got {}", score);
    }

    #[test]
    fn test_entity_jaccard_both_empty() {
        let a: Vec<String> = vec![];
        let b: Vec<String> = vec![];
        let score = SignalComputer::entity_jaccard(&a, &b);
        assert!((score - 0.0).abs() < f64::EPSILON, "both empty should be 0.0, got {}", score);
    }

    // --- Embedding cosine tests ---

    #[test]
    fn test_embedding_cosine_identical() {
        let v = vec![1.0f32, 2.0, 3.0];
        let score = SignalComputer::embedding_cosine(Some(&v), Some(&v));
        assert!((score - 1.0).abs() < 1e-6, "identical vectors should be ~1.0, got {}", score);
    }

    #[test]
    fn test_embedding_cosine_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let score = SignalComputer::embedding_cosine(Some(&a), Some(&b));
        assert!(score.abs() < 1e-6, "orthogonal vectors should be ~0.0, got {}", score);
    }

    #[test]
    fn test_embedding_cosine_none() {
        let v = vec![1.0f32, 2.0, 3.0];
        assert!((SignalComputer::embedding_cosine(None, Some(&v)) - 0.0).abs() < f64::EPSILON);
        assert!((SignalComputer::embedding_cosine(Some(&v), None) - 0.0).abs() < f64::EPSILON);
        assert!((SignalComputer::embedding_cosine(None, None) - 0.0).abs() < f64::EPSILON);
    }

    // --- Temporal proximity tests ---

    #[test]
    fn test_temporal_same_time() {
        let t = 1700000000.0;
        let score = SignalComputer::temporal_proximity(t, t, 3.0);
        assert!((score - 1.0).abs() < f64::EPSILON, "same time should be 1.0, got {}", score);
    }

    #[test]
    fn test_temporal_distant() {
        let t1 = 1700000000.0;
        let t2 = t1 + 30.0 * 86400.0; // 30 days apart
        let score = SignalComputer::temporal_proximity(t1, t2, 3.0);
        // exp(-30*86400 / (3*86400)) = exp(-10) ≈ 0.0000454
        assert!(score < 0.001, "30 days apart should be near 0, got {}", score);
        assert!(score > 0.0, "should still be positive");
    }

    // --- Signal source tests ---

    #[test]
    fn test_signal_source_multi() {
        let scores = SignalScores {
            entity_overlap: 0.5,
            embedding_cosine: 0.6,
            temporal_proximity: 0.1,
        };
        // threshold 0.2: entity (0.5 > 0.2) and embedding (0.6 > 0.2) are active = 2 signals
        assert_eq!(scores.signal_source(0.2), "multi");
    }

    #[test]
    fn test_signal_source_single() {
        let scores = SignalScores {
            entity_overlap: 0.5,
            embedding_cosine: 0.1,
            temporal_proximity: 0.1,
        };
        // threshold 0.2: only entity (0.5 > 0.2) is active = 1 signal
        assert_eq!(scores.signal_source(0.2), "entity");
    }

    #[test]
    fn test_signal_source_single_embedding() {
        let scores = SignalScores {
            entity_overlap: 0.0,
            embedding_cosine: 0.8,
            temporal_proximity: 0.0,
        };
        assert_eq!(scores.signal_source(0.2), "embedding");
    }

    #[test]
    fn test_signal_source_single_temporal() {
        let scores = SignalScores {
            entity_overlap: 0.0,
            embedding_cosine: 0.0,
            temporal_proximity: 0.9,
        };
        assert_eq!(scores.signal_source(0.2), "temporal");
    }

    // --- Combined score test ---

    #[test]
    fn test_combined_score() {
        let scores = SignalScores {
            entity_overlap: 0.5,
            embedding_cosine: 0.8,
            temporal_proximity: 0.3,
        };
        let config = AssociationConfig::default();
        // w_entity=0.3, w_embedding=0.5, w_temporal=0.2
        // combined = 0.3*0.5 + 0.5*0.8 + 0.2*0.3 = 0.15 + 0.40 + 0.06 = 0.61
        let combined = scores.combined(&config);
        assert!(
            (combined - 0.61).abs() < 1e-10,
            "combined should be 0.61, got {}",
            combined
        );
    }

    // --- compute_all test ---

    #[test]
    fn test_compute_all() {
        let entities_a = vec!["cat".to_string(), "dog".to_string()];
        let entities_b = vec!["dog".to_string(), "bird".to_string()];
        let emb_a = vec![1.0f32, 0.0, 0.0];
        let emb_b = vec![0.0f32, 1.0, 0.0];
        let t = 1700000000.0;

        let scores = SignalComputer::compute_all(
            &entities_a,
            &entities_b,
            Some(&emb_a),
            Some(&emb_b),
            t,
            t, // same time
        );

        // Jaccard: intersection={dog}=1, union={cat,dog,bird}=3 → 1/3
        assert!((scores.entity_overlap - 1.0 / 3.0).abs() < 1e-10);
        // Cosine: orthogonal → 0
        assert!(scores.embedding_cosine.abs() < 1e-6);
        // Temporal: same time → 1.0
        assert!((scores.temporal_proximity - 1.0).abs() < f64::EPSILON);
    }

    // --- to_json test ---

    #[test]
    fn test_to_json() {
        let scores = SignalScores {
            entity_overlap: 0.5,
            embedding_cosine: 0.8,
            temporal_proximity: 0.3,
        };
        let json = scores.to_json();
        assert!(json.contains("entity_overlap"));
        assert!(json.contains("embedding_cosine"));
        assert!(json.contains("temporal_proximity"));
        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!((parsed["entity_overlap"].as_f64().unwrap() - 0.5).abs() < 1e-10);
    }

    // --- dominant_signal tests ---

    #[test]
    fn test_dominant_signal_entity() {
        let scores = SignalScores {
            entity_overlap: 0.9,
            embedding_cosine: 0.2,
            temporal_proximity: 0.1,
        };
        assert_eq!(scores.dominant_signal(), "entity");
    }

    #[test]
    fn test_dominant_signal_embedding() {
        let scores = SignalScores {
            entity_overlap: 0.1,
            embedding_cosine: 0.9,
            temporal_proximity: 0.2,
        };
        assert_eq!(scores.dominant_signal(), "embedding");
    }

    #[test]
    fn test_dominant_signal_temporal() {
        let scores = SignalScores {
            entity_overlap: 0.1,
            embedding_cosine: 0.2,
            temporal_proximity: 0.9,
        };
        assert_eq!(scores.dominant_signal(), "temporal");
    }
}
