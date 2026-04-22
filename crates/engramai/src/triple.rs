//! Triple types for LLM-based knowledge graph extraction.
//!
//! Triples represent subject-predicate-object relationships extracted from
//! memory content, used to enrich Hebbian link quality.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Predicate types for knowledge triples.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    IsA,
    PartOf,
    Uses,
    DependsOn,
    CausedBy,
    LeadsTo,
    Implements,
    Contradicts,
    RelatedTo,
}

impl Predicate {
    /// Parse a string into a Predicate, normalizing common variations.
    /// Falls back to `RelatedTo` for unknown predicates.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().replace(['-', ' '], "_").as_str() {
            "is_a" | "isa" | "is" | "type_of" => Predicate::IsA,
            "part_of" | "partof" | "belongs_to" => Predicate::PartOf,
            "uses" | "use" | "utilizes" => Predicate::Uses,
            "depends_on" | "dependson" | "requires" => Predicate::DependsOn,
            "caused_by" | "causedby" | "due_to" => Predicate::CausedBy,
            "leads_to" | "leadsto" | "results_in" => Predicate::LeadsTo,
            "implements" | "implement" | "realizes" => Predicate::Implements,
            "contradicts" | "contradict" | "conflicts_with" => Predicate::Contradicts,
            "related_to" | "relatedto" | "associated_with" => Predicate::RelatedTo,
            _ => Predicate::RelatedTo,
        }
    }

    /// Return the canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Predicate::IsA => "is_a",
            Predicate::PartOf => "part_of",
            Predicate::Uses => "uses",
            Predicate::DependsOn => "depends_on",
            Predicate::CausedBy => "caused_by",
            Predicate::LeadsTo => "leads_to",
            Predicate::Implements => "implements",
            Predicate::Contradicts => "contradicts",
            Predicate::RelatedTo => "related_to",
        }
    }
}

impl fmt::Display for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Source of a triple extraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripleSource {
    Llm,
    Rule,
    Manual,
}

/// A subject-predicate-object triple extracted from memory content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triple {
    pub subject: String,
    pub predicate: Predicate,
    pub object: String,
    pub confidence: f64,
    pub source: TripleSource,
}

impl Triple {
    /// Create a new Triple with source defaulting to `Llm`.
    /// Confidence is clamped to [0.0, 1.0].
    pub fn new(subject: String, predicate: Predicate, object: String, confidence: f64) -> Self {
        Self {
            subject,
            predicate,
            object,
            confidence: confidence.clamp(0.0, 1.0),
            source: TripleSource::Llm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predicate_round_trip() {
        let variants = vec![
            Predicate::IsA,
            Predicate::PartOf,
            Predicate::Uses,
            Predicate::DependsOn,
            Predicate::CausedBy,
            Predicate::LeadsTo,
            Predicate::Implements,
            Predicate::Contradicts,
            Predicate::RelatedTo,
        ];
        for v in variants {
            let s = v.as_str();
            let parsed = Predicate::from_str_lossy(s);
            assert_eq!(v, parsed, "Round-trip failed for {}", s);
        }
    }

    #[test]
    fn test_unknown_predicate_falls_back_to_related_to() {
        assert_eq!(Predicate::from_str_lossy("foobar"), Predicate::RelatedTo);
        assert_eq!(Predicate::from_str_lossy(""), Predicate::RelatedTo);
        assert_eq!(Predicate::from_str_lossy("UNKNOWN_THING"), Predicate::RelatedTo);
    }

    #[test]
    fn test_triple_new_clamps_confidence() {
        let t1 = Triple::new("A".into(), Predicate::IsA, "B".into(), 1.5);
        assert!((t1.confidence - 1.0).abs() < f64::EPSILON);

        let t2 = Triple::new("A".into(), Predicate::IsA, "B".into(), -0.3);
        assert!((t2.confidence - 0.0).abs() < f64::EPSILON);

        let t3 = Triple::new("A".into(), Predicate::IsA, "B".into(), 0.7);
        assert!((t3.confidence - 0.7).abs() < f64::EPSILON);

        // Source defaults to Llm
        assert_eq!(t1.source, TripleSource::Llm);
    }

    #[test]
    fn test_triple_source_serde() {
        let sources = vec![TripleSource::Llm, TripleSource::Rule, TripleSource::Manual];
        for src in sources {
            let json = serde_json::to_string(&src).unwrap();
            let parsed: TripleSource = serde_json::from_str(&json).unwrap();
            assert_eq!(src, parsed);
        }
    }

    #[test]
    fn test_predicate_display() {
        assert_eq!(format!("{}", Predicate::IsA), "is_a");
        assert_eq!(format!("{}", Predicate::DependsOn), "depends_on");
        assert_eq!(format!("{}", Predicate::RelatedTo), "related_to");
    }
}
