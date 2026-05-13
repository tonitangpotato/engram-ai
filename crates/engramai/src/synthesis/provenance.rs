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
            occurred_at: None,
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
            superseded_by: None,
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

    // ---------------------------------------------------------------
    // T29.2 Phase D — synthesis_provenance read-switch contract tests
    //
    // Each test exercises both legacy (`Storage::new`) and unified
    // (`Storage::with_unified_substrate(_, true)`) read paths against
    // the same dual-written data. T16's dual-write guarantees both
    // `synthesis_provenance` rows AND `edges WHERE
    // edge_kind='provenance' AND predicate='derived_from'` rows exist
    // after `record_provenance`, so two storage handles to the same
    // DB file see equivalent records regardless of flag.
    // ---------------------------------------------------------------

    fn open_pair(dir: &std::path::Path) -> (Storage, Storage) {
        let db_path = dir.join("t29_2.db");
        let legacy = Storage::new(&db_path).expect("legacy storage");
        let unified = Storage::with_unified_substrate(&db_path, true)
            .expect("unified storage");
        (legacy, unified)
    }

    /// Sort records by id so equality checks are order-independent.
    fn sort_records(mut v: Vec<ProvenanceRecord>) -> Vec<ProvenanceRecord> {
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    #[test]
    fn t29_2_unified_matches_legacy_get_insight_sources() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        // Seed memories + provenance via legacy (dual-write lands in
        // both tables).
        for (id, content) in [
            ("src1", "Source 1"),
            ("src2", "Source 2"),
            ("ins1", "Insight 1"),
        ] {
            legacy.add(&make_memory(id, content, 0.5), "default").unwrap();
        }
        legacy.record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.5))).unwrap();
        legacy.record_provenance(&make_provenance("p2", "ins1", "src2", "c1", Some(0.6))).unwrap();

        let legacy_rows = sort_records(legacy.get_insight_sources("ins1").unwrap());
        let unified_rows = sort_records(unified.get_insight_sources("ins1").unwrap());

        assert_eq!(legacy_rows.len(), 2);
        assert_eq!(unified_rows.len(), legacy_rows.len(), "row count parity");
        for (l, u) in legacy_rows.iter().zip(unified_rows.iter()) {
            assert_eq!(l.id, u.id, "id mismatch");
            assert_eq!(l.insight_id, u.insight_id, "insight_id mismatch");
            assert_eq!(l.source_id, u.source_id, "source_id mismatch");
            assert_eq!(l.cluster_id, u.cluster_id, "cluster_id mismatch");
            assert_eq!(l.gate_decision, u.gate_decision, "gate_decision mismatch");
            assert!((l.confidence - u.confidence).abs() < 1e-9, "confidence mismatch");
            assert_eq!(
                l.source_original_importance, u.source_original_importance,
                "source_original_importance mismatch"
            );
            // synthesis_timestamp: legacy stores RFC3339, T25 re-emits
            // verbatim → must be exact equality to the nanosecond.
            assert_eq!(
                l.synthesis_timestamp.to_rfc3339(),
                u.synthesis_timestamp.to_rfc3339(),
                "synthesis_timestamp mismatch"
            );
        }
    }

    #[test]
    fn t29_2_unified_matches_legacy_get_memory_insights() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        for (id, content) in [
            ("src1", "Source 1"),
            ("ins1", "Insight 1"),
            ("ins2", "Insight 2"),
        ] {
            legacy.add(&make_memory(id, content, 0.5), "default").unwrap();
        }
        // Two insights both derived from same source — exercises
        // target_id keying on unified path.
        legacy.record_provenance(&make_provenance("p1", "ins1", "src1", "c1", Some(0.5))).unwrap();
        legacy.record_provenance(&make_provenance("p2", "ins2", "src1", "c2", Some(0.5))).unwrap();

        let legacy_rows = sort_records(legacy.get_memory_insights("src1").unwrap());
        let unified_rows = sort_records(unified.get_memory_insights("src1").unwrap());

        assert_eq!(legacy_rows.len(), 2);
        assert_eq!(unified_rows.len(), legacy_rows.len());
        for (l, u) in legacy_rows.iter().zip(unified_rows.iter()) {
            assert_eq!(l.id, u.id);
            assert_eq!(l.insight_id, u.insight_id);
            assert_eq!(l.source_id, u.source_id);
            assert_eq!(l.cluster_id, u.cluster_id);
        }
    }

    #[test]
    fn t29_2_unified_matches_legacy_check_coverage() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        for (id, content) in [
            ("src1", "Source 1"),
            ("src2", "Source 2"),
            ("src3", "Source 3"),
            ("ins1", "Insight 1"),
        ] {
            legacy.add(&make_memory(id, content, 0.5), "default").unwrap();
        }
        // src1 + src2 covered, src3 not covered.
        legacy.record_provenance(&make_provenance("p1", "ins1", "src1", "c1", None)).unwrap();
        legacy.record_provenance(&make_provenance("p2", "ins1", "src2", "c1", None)).unwrap();

        let members = vec![
            "src1".to_string(),
            "src2".to_string(),
            "src3".to_string(),
        ];

        let legacy_cov = legacy.check_coverage(&members).unwrap();
        let unified_cov = unified.check_coverage(&members).unwrap();
        assert!((legacy_cov - 2.0 / 3.0).abs() < 1e-9, "legacy coverage 2/3");
        assert!((legacy_cov - unified_cov).abs() < 1e-9, "parity");

        // Empty input — both paths return 0.0 without touching DB.
        assert_eq!(legacy.check_coverage(&[]).unwrap(), 0.0);
        assert_eq!(unified.check_coverage(&[]).unwrap(), 0.0);
    }

    #[test]
    fn t29_2_unified_path_ignores_other_edge_kinds() {
        // Pin the (edge_kind='provenance', predicate='derived_from')
        // filter: a hypothetical associative edge between the same
        // node pair must NOT be returned by provenance readers.
        use rusqlite::params;

        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        for (id, content) in [("src1", "Source 1"), ("ins1", "Insight 1")] {
            legacy.add(&make_memory(id, content, 0.5), "default").unwrap();
        }
        legacy.record_provenance(&make_provenance("p1", "ins1", "src1", "c1", None)).unwrap();

        // Inject a contrived associative edge between the same pair.
        // Use the legacy handle's connection so it lives in the same
        // DB file as the unified reader.
        let now_epoch: f64 = chrono::Utc::now().timestamp() as f64;
        legacy.connection().execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, \
                                weight, confidence, namespace, attributes, \
                                recorded_at, created_at, updated_at) \
             VALUES (?1, 'ins1', 'src1', 'associative', 'co_activated', \
                     0.5, 1.0, 'default', '{}', ?2, ?2, ?2)",
            params!["assoc-noise-1", now_epoch],
        ).unwrap();

        // Unified reader still returns ONLY the provenance edge.
        let unified_sources = unified.get_insight_sources("ins1").unwrap();
        assert_eq!(unified_sources.len(), 1);
        assert_eq!(unified_sources[0].id, "p1");

        let unified_insights = unified.get_memory_insights("src1").unwrap();
        assert_eq!(unified_insights.len(), 1);
        assert_eq!(unified_insights[0].id, "p1");

        // check_coverage on src1: associative noise is NOT counted.
        let cov = unified.check_coverage(&["src1".to_string()]).unwrap();
        assert!((cov - 1.0).abs() < 1e-9, "src1 covered by provenance only");
    }

    #[test]
    fn t29_2_unified_path_empty_result_matches_legacy() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        legacy.add(&make_memory("orphan", "no provenance", 0.5), "default").unwrap();

        // Both branches: unknown insight_id, unknown source_id.
        assert!(legacy.get_insight_sources("nonexistent").unwrap().is_empty());
        assert!(unified.get_insight_sources("nonexistent").unwrap().is_empty());
        assert!(legacy.get_memory_insights("orphan").unwrap().is_empty());
        assert!(unified.get_memory_insights("orphan").unwrap().is_empty());
    }

    #[test]
    fn t29_2_unified_gate_scores_roundtrip() {
        // Exercise the nested-JSON attribute reconstruction in
        // row_to_provenance_from_edge: gate_scores must come back as
        // Some(GateScores { … }), not as None (would mean we forgot
        // is_object) and not as a stringly value.
        use crate::synthesis::types::GateScores;

        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut legacy, unified) = open_pair(tmp.path());

        for (id, content) in [("src1", "Source 1"), ("ins1", "Insight 1")] {
            legacy.add(&make_memory(id, content, 0.5), "default").unwrap();
        }

        let scores = GateScores {
            quality: 0.81,
            type_diversity: 4,
            estimated_cost: 0.03,
            member_count: 7,
        };
        let prov = ProvenanceRecord {
            id: "p1".to_string(),
            insight_id: "ins1".to_string(),
            source_id: "src1".to_string(),
            cluster_id: "cluster_x".to_string(),
            synthesis_timestamp: Utc::now(),
            gate_decision: "Synthesize".to_string(),
            gate_scores: Some(scores),
            confidence: 0.91,
            source_original_importance: Some(0.42),
        };
        legacy.record_provenance(&prov).unwrap();

        let rows = unified.get_insight_sources("ins1").unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        let gs = r.gate_scores.as_ref().expect("gate_scores present on unified path");
        assert!((gs.quality - 0.81).abs() < 1e-9);
        assert_eq!(gs.type_diversity, 4);
        assert!((gs.estimated_cost - 0.03).abs() < 1e-9);
        assert_eq!(gs.member_count, 7);
        assert_eq!(r.cluster_id, "cluster_x");
        assert_eq!(r.source_original_importance, Some(0.42));
    }

}
