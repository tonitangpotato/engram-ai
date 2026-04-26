//! L5 Knowledge Topic — structured synthesis row (§4.1 `knowledge_topics`).
//!
//! See design §5 (KnowledgeTopic) and §4.1 (knowledge_topics table).
//! Mirrors a `graph_entities` row of kind `Topic` via shared UUID; the
//! `topic_id` here is the same UUID as the mirrored entity's `id`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::GraphError;

/// L5 Knowledge Topic — structured synthesis row (§4.1 `knowledge_topics`).
/// Mirrors a `graph_entities` row of kind `Topic` via shared UUID; see §4.1 note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeTopic {
    pub topic_id: Uuid, // == entity_id of the mirrored Topic entity
    pub title: String,
    pub summary: String,
    pub embedding: Option<Vec<f32>>,
    pub source_memories: Vec<String>, // MemoryIds the topic synthesizes over
    pub contributing_entities: Vec<Uuid>, // entities that appear across source_memories
    pub cluster_weights: Option<serde_json::Value>, // affect-weighting record (GOAL-3.7 input)
    pub synthesis_run_id: Option<Uuid>, // back-link to graph_pipeline_runs
    pub synthesized_at: f64,
    pub superseded_by: Option<Uuid>,
    pub superseded_at: Option<f64>,
    pub namespace: String,
}

impl KnowledgeTopic {
    /// Construct a fresh, live knowledge topic. All optional/aggregate fields
    /// start empty; callers populate `embedding`, `source_memories`,
    /// `contributing_entities`, and `cluster_weights` as synthesis progresses.
    pub fn new(
        topic_id: Uuid,
        title: String,
        summary: String,
        namespace: String,
        synthesized_at: f64,
    ) -> Self {
        Self {
            topic_id,
            title,
            summary,
            embedding: None,
            source_memories: vec![],
            contributing_entities: vec![],
            cluster_weights: None,
            synthesis_run_id: None,
            synthesized_at,
            superseded_by: None,
            superseded_at: None,
            namespace,
        }
    }

    /// True iff this topic has not been superseded. `superseded_by` is the
    /// canonical signal; `superseded_at` should agree but is informational.
    pub fn is_live(&self) -> bool {
        self.superseded_by.is_none()
    }

    /// One-shot supersede: marks this topic as replaced by `by` at time `at`.
    /// Returns `Err(GraphError::Invariant("topic already superseded"))` if
    /// the topic is already superseded (§4.1 GUARD-3 — no erasure, and
    /// supersede is monotonic).
    pub fn supersede(&mut self, by: Uuid, at: f64) -> Result<(), GraphError> {
        if self.superseded_by.is_some() {
            return Err(GraphError::Invariant("topic already superseded"));
        }
        self.superseded_by = Some(by);
        self.superseded_at = Some(at);
        Ok(())
    }

    /// Validate that `embedding` length matches the expected dim.
    /// Used by storage write path before persisting the blob.
    /// Returns Err(GraphError::Invariant("knowledge topic embedding dim mismatch")) — verbatim message per §4.1.
    pub fn validate_embedding_dim(&self, expected_dim: usize) -> Result<(), GraphError> {
        if let Some(v) = &self.embedding {
            if v.len() != expected_dim {
                return Err(GraphError::Invariant(
                    "knowledge topic embedding dim mismatch",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fresh() -> KnowledgeTopic {
        KnowledgeTopic::new(
            Uuid::new_v4(),
            "Title".to_string(),
            "Summary".to_string(),
            "ns".to_string(),
            1.0,
        )
    }

    #[test]
    fn new_topic_is_live() {
        let t = fresh();
        assert!(t.is_live());
        assert!(t.embedding.is_none());
        assert!(t.cluster_weights.is_none());
        assert!(t.synthesis_run_id.is_none());
        assert!(t.superseded_by.is_none());
        assert!(t.superseded_at.is_none());
        assert!(t.source_memories.is_empty());
        assert!(t.contributing_entities.is_empty());
    }

    #[test]
    fn supersede_sets_pointers() {
        let mut t = fresh();
        let other = Uuid::new_v4();
        let now = 42.0;
        t.supersede(other, now).unwrap();
        assert!(!t.is_live());
        assert_eq!(t.superseded_by, Some(other));
        assert_eq!(t.superseded_at, Some(now));
    }

    #[test]
    fn supersede_twice_errors() {
        let mut t = fresh();
        t.supersede(Uuid::new_v4(), 1.0).unwrap();
        match t.supersede(Uuid::new_v4(), 2.0) {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "topic already superseded"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_embedding_dim_none_ok() {
        let t = fresh();
        t.validate_embedding_dim(384).unwrap();
        t.validate_embedding_dim(0).unwrap();
        t.validate_embedding_dim(1).unwrap();
    }

    #[test]
    fn validate_embedding_dim_match_ok() {
        let mut t = fresh();
        t.embedding = Some(vec![0.0; 384]);
        t.validate_embedding_dim(384).unwrap();
    }

    #[test]
    fn validate_embedding_dim_mismatch_errors() {
        let mut t = fresh();
        t.embedding = Some(vec![0.0; 100]);
        match t.validate_embedding_dim(384) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "knowledge topic embedding dim mismatch")
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn serde_roundtrip_topic() {
        let mut t = fresh();
        t.embedding = Some(vec![0.1, 0.2, 0.3, -0.4]);
        t.source_memories = vec!["mem-1".into(), "mem-2".into()];
        t.contributing_entities = vec![Uuid::new_v4(), Uuid::new_v4()];
        t.cluster_weights = Some(json!({"cluster": "a", "weight": 0.5}));
        t.synthesis_run_id = Some(Uuid::new_v4());

        let s = serde_json::to_string(&t).unwrap();
        let back: KnowledgeTopic = serde_json::from_str(&s).unwrap();

        assert_eq!(t.topic_id, back.topic_id);
        assert_eq!(t.title, back.title);
        assert_eq!(t.summary, back.summary);
        assert_eq!(t.embedding, back.embedding);
        assert_eq!(t.source_memories, back.source_memories);
        assert_eq!(t.contributing_entities, back.contributing_entities);
        assert_eq!(t.cluster_weights, back.cluster_weights);
        assert_eq!(t.synthesis_run_id, back.synthesis_run_id);
        assert_eq!(t.synthesized_at, back.synthesized_at);
        assert_eq!(t.superseded_by, back.superseded_by);
        assert_eq!(t.superseded_at, back.superseded_at);
        assert_eq!(t.namespace, back.namespace);
    }
}
