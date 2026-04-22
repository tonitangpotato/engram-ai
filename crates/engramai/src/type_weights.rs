//! Type weight inference for dimensional memory extraction.
//!
//! Converts dimensional `ExtractedFact` fields into continuous type weights
//! (0.0–1.0) for each of the 7 memory types. Replaces the discrete
//! `MemoryType` classification with a multi-label probability vector.

use serde::{Deserialize, Serialize};

use crate::extractor::ExtractedFact;
use crate::types::MemoryType;

/// Continuous weights for 7 memory types (0.0–1.0 each).
///
/// Used during recall to compute `type_boost = max(weight_i × affinity_i)`.
/// Old memories without type_weights use `Default` (all 1.0) for backward compat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeWeights {
    pub factual: f64,
    pub episodic: f64,
    pub procedural: f64,
    pub relational: f64,
    pub emotional: f64,
    pub opinion: f64,
    pub causal: f64,
}

impl Default for TypeWeights {
    /// All 1.0 — old memories get neutral weights.
    ///
    /// `type_boost = max(1.0 × affinity_i) = max(affinity_i)`,
    /// which is equivalent to the old discrete-match behavior for neutral queries
    /// (affinity all 1.0 → type_boost = 1.0).
    fn default() -> Self {
        Self {
            factual: 1.0,
            episodic: 1.0,
            procedural: 1.0,
            relational: 1.0,
            emotional: 1.0,
            opinion: 1.0,
            causal: 1.0,
        }
    }
}

impl TypeWeights {
    /// Return the MemoryType with the highest weight (for DB column compat).
    pub fn primary_type(&self) -> MemoryType {
        let weights = [
            (self.factual, MemoryType::Factual),
            (self.episodic, MemoryType::Episodic),
            (self.procedural, MemoryType::Procedural),
            (self.relational, MemoryType::Relational),
            (self.emotional, MemoryType::Emotional),
            (self.opinion, MemoryType::Opinion),
            (self.causal, MemoryType::Causal),
        ];
        weights
            .iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, t)| *t)
            .unwrap_or(MemoryType::Factual)
    }

    /// Deserialize from metadata JSON. Returns `Default` if missing or malformed.
    ///
    /// Dual-path (ISS-019 Step 7a) — priority order preserves the
    /// v1 contract "caller-supplied overrides engram-inferred":
    /// 1. v2 user-supplied: `metadata.user.type_weights` (caller passed
    ///    `{"type_weights": ...}` to `Memory::add` — wins over inferred)
    /// 2. v1 flat layout: `metadata.type_weights` (older DB rows where
    ///    the caller's top-level key is the only source)
    /// 3. v2 engram-internal: `metadata.engram.dimensions.type_weights`
    ///    (auto-inferred fallback when caller did not supply one)
    pub fn from_metadata(metadata: &Option<serde_json::Value>) -> Self {
        let Some(m) = metadata.as_ref() else {
            return Self::default();
        };
        // 1. v2 user-supplied (caller explicitly set type_weights)
        if let Some(tw) = m
            .get("user")
            .and_then(|u| u.get("type_weights"))
            .and_then(|tw| serde_json::from_value::<TypeWeights>(tw.clone()).ok())
        {
            return tw;
        }
        // 2. v1 flat fallback (older DB rows)
        if let Some(tw) = m
            .get("type_weights")
            .and_then(|tw| serde_json::from_value::<TypeWeights>(tw.clone()).ok())
        {
            return tw;
        }
        // 3. v2 engram-inferred fallback
        m.get("engram")
            .and_then(|e| e.get("dimensions"))
            .and_then(|d| d.get("type_weights"))
            .and_then(|tw| serde_json::from_value::<TypeWeights>(tw.clone()).ok())
            .unwrap_or_default()
    }

    /// Serialize to a JSON value suitable for embedding in metadata.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// Infer type weights from an ExtractedFact's dimensional fields.
///
/// Baseline: all types start at 0.1.
/// Each non-empty dimension adds weight to relevant types.
/// Result is clamped to [0.0, 1.0].
pub fn infer_type_weights(fact: &ExtractedFact) -> TypeWeights {
    let mut w = TypeWeights {
        factual: 0.1,
        episodic: 0.1,
        procedural: 0.1,
        relational: 0.1,
        emotional: 0.1,
        opinion: 0.1,
        causal: 0.1,
    };

    // core_fact is always non-empty (required field)
    w.factual += 0.4;

    if fact.temporal.is_some() {
        w.episodic += 0.5;
    }
    if fact.participants.is_some() {
        w.relational += 0.4;
        w.episodic += 0.2;
    }
    if fact.causation.is_some() {
        w.causal += 0.5;
    }
    if fact.outcome.is_some() {
        w.causal += 0.3;
    }
    if fact.method.is_some() {
        w.procedural += 0.5;
    }
    if fact.context.is_some() {
        w.episodic += 0.2;
    }
    if fact.location.is_some() {
        w.episodic += 0.1;
    }
    if fact.relations.is_some() {
        w.relational += 0.3;
    }
    if fact.sentiment.is_some() {
        w.emotional += 0.5;
    }
    if fact.stance.is_some() {
        w.opinion += 0.5;
    }

    // Clamp all to [0.0, 1.0]
    w.factual = w.factual.clamp(0.0, 1.0);
    w.episodic = w.episodic.clamp(0.0, 1.0);
    w.procedural = w.procedural.clamp(0.0, 1.0);
    w.relational = w.relational.clamp(0.0, 1.0);
    w.emotional = w.emotional.clamp(0.0, 1.0);
    w.opinion = w.opinion.clamp(0.0, 1.0);
    w.causal = w.causal.clamp(0.0, 1.0);

    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fact(fields: &[&str]) -> ExtractedFact {
        let mut fact = ExtractedFact::default();
        fact.core_fact = "some fact".to_string();
        for &f in fields {
            match f {
                "temporal" => fact.temporal = Some("yesterday".to_string()),
                "participants" => fact.participants = Some("potato".to_string()),
                "causation" => fact.causation = Some("because X".to_string()),
                "outcome" => fact.outcome = Some("resulted in Y".to_string()),
                "method" => fact.method = Some("by doing Z".to_string()),
                "context" => fact.context = Some("during meeting".to_string()),
                "location" => fact.location = Some("office".to_string()),
                "relations" => fact.relations = Some("related to project A".to_string()),
                "sentiment" => fact.sentiment = Some("frustrated".to_string()),
                "stance" => fact.stance = Some("prefers Rust".to_string()),
                _ => {}
            }
        }
        fact
    }

    #[test]
    fn test_baseline_only_core_fact() {
        let w = infer_type_weights(&make_fact(&[]));
        assert_eq!(w.factual, 0.5); // 0.1 + 0.4
        assert_eq!(w.episodic, 0.1);
        assert_eq!(w.procedural, 0.1);
        assert_eq!(w.relational, 0.1);
        assert_eq!(w.emotional, 0.1);
        assert_eq!(w.opinion, 0.1);
        assert_eq!(w.causal, 0.1);
    }

    #[test]
    fn test_causal_memory() {
        let w = infer_type_weights(&make_fact(&["causation", "outcome", "participants"]));
        assert!((w.causal - 0.9).abs() < 1e-10); // 0.1 + 0.5 + 0.3
        assert!((w.relational - 0.5).abs() < 1e-10); // 0.1 + 0.4
        assert!((w.episodic - 0.3).abs() < 1e-10); // 0.1 + 0.2 (participants)
    }

    #[test]
    fn test_episodic_memory() {
        let w = infer_type_weights(&make_fact(&["temporal", "participants", "context", "location"]));
        assert_eq!(w.episodic, 1.0); // 0.1 + 0.5 + 0.2 + 0.2 + 0.1 = 1.1 → clamped to 1.0
    }

    #[test]
    fn test_procedural_memory() {
        let w = infer_type_weights(&make_fact(&["method"]));
        assert!((w.procedural - 0.6).abs() < 1e-10); // 0.1 + 0.5
    }

    #[test]
    fn test_emotional_opinion() {
        let w = infer_type_weights(&make_fact(&["sentiment", "stance"]));
        assert!((w.emotional - 0.6).abs() < 1e-10); // 0.1 + 0.5
        assert!((w.opinion - 0.6).abs() < 1e-10); // 0.1 + 0.5
    }

    #[test]
    fn test_primary_type() {
        let w = infer_type_weights(&make_fact(&["causation", "outcome"]));
        assert_eq!(w.primary_type(), MemoryType::Causal);

        let w = infer_type_weights(&make_fact(&["temporal", "participants", "context"]));
        assert_eq!(w.primary_type(), MemoryType::Episodic);
    }

    #[test]
    fn test_default_is_all_ones() {
        let w = TypeWeights::default();
        assert_eq!(w.factual, 1.0);
        assert_eq!(w.causal, 1.0);
    }

    #[test]
    fn test_from_metadata_missing() {
        let w = TypeWeights::from_metadata(&None);
        assert_eq!(w, TypeWeights::default());
    }

    #[test]
    fn test_from_metadata_present() {
        let meta = serde_json::json!({
            "type_weights": {
                "factual": 0.5, "episodic": 0.8, "procedural": 0.1,
                "relational": 0.4, "emotional": 0.1, "opinion": 0.1, "causal": 0.9
            }
        });
        let w = TypeWeights::from_metadata(&Some(meta));
        assert_eq!(w.factual, 0.5);
        assert_eq!(w.causal, 0.9);
    }

    #[test]
    fn test_roundtrip_json() {
        let w = infer_type_weights(&make_fact(&["causation", "temporal"]));
        let json = w.to_json();
        let w2: TypeWeights = serde_json::from_value(json).unwrap();
        assert_eq!(w, w2);
    }
}
