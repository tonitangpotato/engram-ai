//! Provenance tracking and undo for synthesized insights.
//!
//! Tracks the relationship between synthesized insights and their source memories.
//! Supports chain traversal (tracing an insight back through multiple synthesis layers)
//! and undo (restoring source importances and removing the insight).

use crate::storage::Storage;
use crate::synthesis::types::{ProvenanceChain, RestoredSource, UndoSynthesis};

/// Get the full provenance chain for a memory, traversing up to `max_depth` levels.
///
/// - Level 0: direct sources of the memory (if it's an insight)
/// - Level 1: sources of those sources' insights
/// - etc.
///
/// Returns an empty chain (no layers) if the memory has no provenance records.
pub fn get_provenance_chain(
    storage: &Storage,
    memory_id: &str,
    max_depth: usize,
) -> Result<ProvenanceChain, Box<dyn std::error::Error>> {
    let mut chain = ProvenanceChain {
        root_id: memory_id.to_string(),
        layers: Vec::new(),
    };

    let mut current_ids = vec![memory_id.to_string()];

    for _depth in 0..max_depth {
        let mut layer = Vec::new();
        for id in &current_ids {
            let sources = storage.get_insight_sources(id)?;
            layer.extend(sources);
        }
        if layer.is_empty() {
            break;
        }
        current_ids = layer.iter().map(|r| r.source_id.clone()).collect();
        chain.layers.push(layer);
    }

    Ok(chain)
}

/// Undo a synthesis: delete the insight, restore source importances, remove provenance.
///
/// Steps:
/// 1. Get provenance records for the insight
/// 2. Restore each source memory's original importance (if recorded)
/// 3. Delete the insight memory
/// 4. Delete provenance records
///
/// Uses a transaction to ensure atomicity.
pub fn undo_synthesis(
    storage: &mut Storage,
    insight_id: &str,
) -> Result<UndoSynthesis, Box<dyn std::error::Error>> {
    // 1. Get provenance records
    let records = storage.get_insight_sources(insight_id)?;
    if records.is_empty() {
        return Err(format!("No provenance records found for insight {insight_id}").into());
    }

    // Use transaction for atomicity
    storage.begin_transaction()?;

    let result = (|| -> Result<UndoSynthesis, Box<dyn std::error::Error>> {
        // 2. Restore each source's importance
        let mut restored_sources = Vec::new();
        for record in &records {
            if let Some(original_importance) = record.source_original_importance {
                storage.update_importance(&record.source_id, original_importance)?;
                restored_sources.push(RestoredSource {
                    memory_id: record.source_id.clone(),
                    original_importance,
                    restored: true,
                });
            } else {
                restored_sources.push(RestoredSource {
                    memory_id: record.source_id.clone(),
                    original_importance: 0.0,
                    restored: false,
                });
            }
        }

        // 3. Delete provenance records (before insight, due to FK constraint)
        storage.delete_provenance(insight_id)?;

        // 4. Delete the insight memory record
        storage.delete(insight_id)?;

        Ok(UndoSynthesis {
            insight_id: insight_id.to_string(),
            restored_sources,
        })
    })();

    match &result {
        Ok(_) => storage.commit_transaction()?,
        Err(_) => {
            let _ = storage.rollback_transaction();
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthesis::types::ProvenanceRecord;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;

    /// Create a minimal MemoryRecord for testing.
    fn make_memory(id: &str, content: &str, importance: f64) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.0,
            importance,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".to_string(),
            contradicts: None,
            contradicted_by: None,
            metadata: None,
        }
    }

    /// Create a ProvenanceRecord for testing.
    fn make_provenance(
        id: &str,
        insight_id: &str,
        source_id: &str,
        cluster_id: &str,
        original_importance: Option<f64>,
    ) -> ProvenanceRecord {
        ProvenanceRecord {
            id: id.to_string(),
            insight_id: insight_id.to_string(),
            source_id: source_id.to_string(),
            cluster_id: cluster_id.to_string(),
            synthesis_timestamp: Utc::now(),
            gate_decision: "Synthesize".to_string(),
            gate_scores: None,
            confidence: 0.85,
            source_original_importance: original_importance,
        }
    }

    #[test]
    fn test_record_and_query_provenance() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        // Insert source memories
        let mem1 = make_memory("src1", "Source memory 1", 0.8);
        let mem2 = make_memory("src2", "Source memory 2", 0.7);
        let insight = make_memory("ins1", "Synthesized insight", 0.9);

        storage.add(&mem1, "default").unwrap();
        storage.add(&mem2, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        // Record provenance
        let prov1 = make_provenance("p1", "ins1", "src1", "cluster_a", Some(0.8));
        let prov2 = make_provenance("p2", "ins1", "src2", "cluster_a", Some(0.7));

        storage.record_provenance(&prov1).unwrap();
        storage.record_provenance(&prov2).unwrap();

        // Query sources for insight
        let sources = storage.get_insight_sources("ins1").unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources.iter().any(|r| r.source_id == "src1"));
        assert!(sources.iter().any(|r| r.source_id == "src2"));

        // Query insights for a source
        let insights = storage.get_memory_insights("src1").unwrap();
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].insight_id, "ins1");
    }

    #[test]
    fn test_delete_provenance() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem1 = make_memory("src1", "Source 1", 0.5);
        let insight = make_memory("ins1", "Insight 1", 0.9);
        storage.add(&mem1, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        let prov = make_provenance("p1", "ins1", "src1", "cluster_a", Some(0.5));
        storage.record_provenance(&prov).unwrap();

        let count = storage.delete_provenance("ins1").unwrap();
        assert_eq!(count, 1);

        let sources = storage.get_insight_sources("ins1").unwrap();
        assert!(sources.is_empty());
    }

    #[test]
    fn test_check_coverage() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem1 = make_memory("src1", "Source 1", 0.5);
        let mem2 = make_memory("src2", "Source 2", 0.5);
        let mem3 = make_memory("src3", "Source 3", 0.5);
        let insight = make_memory("ins1", "Insight 1", 0.9);

        storage.add(&mem1, "default").unwrap();
        storage.add(&mem2, "default").unwrap();
        storage.add(&mem3, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        // Only src1 and src2 have provenance
        storage
            .record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.5)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p2", "ins1", "src2", "c1", Some(0.5)))
            .unwrap();

        let member_ids = vec![
            "src1".to_string(),
            "src2".to_string(),
            "src3".to_string(),
        ];
        let coverage = storage.check_coverage(&member_ids).unwrap();
        assert!((coverage - 2.0 / 3.0).abs() < 0.001);

        // Empty list
        let coverage = storage.check_coverage(&[]).unwrap();
        assert_eq!(coverage, 0.0);
    }

    #[test]
    fn test_provenance_chain_single_level() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem1 = make_memory("src1", "Source 1", 0.5);
        let mem2 = make_memory("src2", "Source 2", 0.6);
        let insight = make_memory("ins1", "Insight from src1+src2", 0.9);

        storage.add(&mem1, "default").unwrap();
        storage.add(&mem2, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        storage
            .record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.5)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p2", "ins1", "src2", "c1", Some(0.6)))
            .unwrap();

        let chain = get_provenance_chain(&storage, "ins1", 3).unwrap();
        assert_eq!(chain.root_id, "ins1");
        assert_eq!(chain.layers.len(), 1);
        assert_eq!(chain.layers[0].len(), 2);
    }

    #[test]
    fn test_provenance_chain_multi_level() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        // Level 0: raw memories
        let mem_a = make_memory("a", "Raw memory A", 0.5);
        let mem_b = make_memory("b", "Raw memory B", 0.6);
        // Level 1: first insight from a+b
        let ins1 = make_memory("ins1", "Insight from a+b", 0.8);
        // Another raw memory
        let mem_c = make_memory("c", "Raw memory C", 0.4);
        // Level 2: second insight from ins1+c
        let ins2 = make_memory("ins2", "Meta-insight from ins1+c", 0.9);

        for mem in &[&mem_a, &mem_b, &ins1, &mem_c, &ins2] {
            storage.add(mem, "default").unwrap();
        }

        // ins1 ← a, b
        storage
            .record_provenance(&make_provenance("p1", "ins1", "a", "c1", Some(0.5)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p2", "ins1", "b", "c1", Some(0.6)))
            .unwrap();

        // ins2 ← ins1, c
        storage
            .record_provenance(&make_provenance("p3", "ins2", "ins1", "c2", Some(0.8)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p4", "ins2", "c", "c2", Some(0.4)))
            .unwrap();

        let chain = get_provenance_chain(&storage, "ins2", 5).unwrap();
        assert_eq!(chain.root_id, "ins2");
        // Layer 0: ins2's direct sources (ins1, c)
        assert_eq!(chain.layers.len(), 2);
        assert_eq!(chain.layers[0].len(), 2);
        // Layer 1: ins1's sources (a, b) — c has no sources so only ins1 contributes
        assert_eq!(chain.layers[1].len(), 2);
    }

    #[test]
    fn test_provenance_chain_max_depth() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem_a = make_memory("a", "Raw A", 0.5);
        let ins1 = make_memory("ins1", "Insight 1", 0.8);
        let ins2 = make_memory("ins2", "Insight 2", 0.9);

        storage.add(&mem_a, "default").unwrap();
        storage.add(&ins1, "default").unwrap();
        storage.add(&ins2, "default").unwrap();

        storage
            .record_provenance(&make_provenance("p1", "ins1", "a", "c1", Some(0.5)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p2", "ins2", "ins1", "c2", Some(0.8)))
            .unwrap();

        // max_depth=1 should only get one layer
        let chain = get_provenance_chain(&storage, "ins2", 1).unwrap();
        assert_eq!(chain.layers.len(), 1);
        assert_eq!(chain.layers[0].len(), 1);
        assert_eq!(chain.layers[0][0].source_id, "ins1");
    }

    #[test]
    fn test_provenance_chain_no_sources() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem = make_memory("raw1", "Just a raw memory", 0.5);
        storage.add(&mem, "default").unwrap();

        let chain = get_provenance_chain(&storage, "raw1", 5).unwrap();
        assert_eq!(chain.root_id, "raw1");
        assert!(chain.layers.is_empty());
    }

    #[test]
    fn test_undo_synthesis() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        // Source memories with known importances
        let mem1 = make_memory("src1", "Source 1", 0.8);
        let mem2 = make_memory("src2", "Source 2", 0.7);
        let insight = make_memory("ins1", "Synthesized insight", 0.9);

        storage.add(&mem1, "default").unwrap();
        storage.add(&mem2, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        // Simulate demotion: lower source importances
        storage.update_importance("src1", 0.4).unwrap();
        storage.update_importance("src2", 0.35).unwrap();

        // Record provenance with original importances
        storage
            .record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.8)))
            .unwrap();
        storage
            .record_provenance(&make_provenance("p2", "ins1", "src2", "c1", Some(0.7)))
            .unwrap();

        // Undo
        let result = undo_synthesis(&mut storage, "ins1").unwrap();
        assert_eq!(result.insight_id, "ins1");
        assert_eq!(result.restored_sources.len(), 2);
        assert!(result.restored_sources.iter().all(|r| r.restored));

        // Verify insight is deleted
        assert!(storage.get("ins1").unwrap().is_none());

        // Verify provenance is deleted
        let sources = storage.get_insight_sources("ins1").unwrap();
        assert!(sources.is_empty());

        // Verify importances are restored
        let src1 = storage.get("src1").unwrap().expect("src1 should exist");
        assert!((src1.importance - 0.8).abs() < 0.001);
        let src2 = storage.get("src2").unwrap().expect("src2 should exist");
        assert!((src2.importance - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_undo_synthesis_no_provenance() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let result = undo_synthesis(&mut storage, "nonexistent");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No provenance records found"));
    }

    #[test]
    fn test_undo_synthesis_partial_restore() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem1 = make_memory("src1", "Source 1", 0.8);
        let insight = make_memory("ins1", "Insight", 0.9);
        storage.add(&mem1, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        // One provenance record has original_importance, one doesn't
        storage
            .record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.8)))
            .unwrap();
        storage
            .record_provenance(&ProvenanceRecord {
                id: "p2".to_string(),
                insight_id: "ins1".to_string(),
                source_id: "src1".to_string(),
                cluster_id: "c1".to_string(),
                synthesis_timestamp: Utc::now(),
                gate_decision: "Synthesize".to_string(),
                gate_scores: None,
                confidence: 0.85,
                source_original_importance: None, // no original importance recorded
            })
            .unwrap();

        let result = undo_synthesis(&mut storage, "ins1").unwrap();
        // One restored, one not
        let restored_count = result.restored_sources.iter().filter(|r| r.restored).count();
        let not_restored_count = result.restored_sources.iter().filter(|r| !r.restored).count();
        assert_eq!(restored_count, 1);
        assert_eq!(not_restored_count, 1);
    }

    #[test]
    fn test_update_importance() {
        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem = make_memory("m1", "Test memory", 0.5);
        storage.add(&mem, "default").unwrap();

        storage.update_importance("m1", 0.9).unwrap();

        let updated = storage.get("m1").unwrap().expect("should exist");
        assert!((updated.importance - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_provenance_with_gate_scores() {
        use crate::synthesis::types::GateScores;

        let mut storage = Storage::new(":memory:").expect("in-memory db");

        let mem1 = make_memory("src1", "Source 1", 0.5);
        let insight = make_memory("ins1", "Insight", 0.9);
        storage.add(&mem1, "default").unwrap();
        storage.add(&insight, "default").unwrap();

        let scores = GateScores {
            quality: 0.75,
            type_diversity: 3,
            estimated_cost: 0.02,
            member_count: 5,
        };

        let prov = ProvenanceRecord {
            id: "p1".to_string(),
            insight_id: "ins1".to_string(),
            source_id: "src1".to_string(),
            cluster_id: "c1".to_string(),
            synthesis_timestamp: Utc::now(),
            gate_decision: "Synthesize".to_string(),
            gate_scores: Some(scores),
            confidence: 0.85,
            source_original_importance: Some(0.5),
        };

        storage.record_provenance(&prov).unwrap();

        let sources = storage.get_insight_sources("ins1").unwrap();
        assert_eq!(sources.len(), 1);
        let retrieved = &sources[0];
        assert!(retrieved.gate_scores.is_some());
        let gs = retrieved.gate_scores.as_ref().unwrap();
        assert!((gs.quality - 0.75).abs() < 0.001);
        assert_eq!(gs.type_diversity, 3);
        assert_eq!(gs.member_count, 5);
    }
}
