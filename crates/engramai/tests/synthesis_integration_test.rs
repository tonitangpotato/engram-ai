//! Integration tests for the knowledge synthesis pipeline.
//!
//! Tests the full flow: Memory API → synthesis engine → storage,
//! exercising cluster discovery, gate check, dry-run, sleep cycle,
//! insight listing, and reversal. All without LLM (graceful degradation).

use engramai::{Memory, MemoryConfig, MemoryType, SynthesisSettings};

/// Create a Memory instance with synthesis enabled.
fn setup() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("synth_test.db");
    let config = MemoryConfig::default();
    let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();

    let mut settings = SynthesisSettings::default();
    settings.enabled = true;
    // Lower thresholds to make clustering easier in tests
    settings.cluster_discovery.min_cluster_size = 2;
    settings.cluster_discovery.cluster_threshold = 0.1;
    mem.set_synthesis_settings(settings);

    (mem, dir)
}

/// Helper: add a memory and return its ID.
fn add_memory(mem: &mut Memory, content: &str, importance: f64) -> String {
    mem.add(content, MemoryType::Factual, Some(importance), None, None).unwrap()
}

/// Helper: create Hebbian links between memories via direct SQL.
fn coactivate(mem: &mut Memory, id_a: &str, id_b: &str) {
    let conn = mem.connection();
    let now = chrono::Utc::now().timestamp() as f64;
    conn.execute(
        "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id_a, id_b, 3.0, 5, now],
    ).unwrap();
    conn.execute(
        "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id_b, id_a, 3.0, 5, now],
    ).unwrap();
}

#[test]
fn test_synthesize_empty_db() {
    let (mut mem, _dir) = setup();
    let report = mem.synthesize().unwrap();
    assert_eq!(report.clusters_found, 0);
    assert_eq!(report.clusters_synthesized, 0);
    assert!(report.insights_created.is_empty());
    assert!(report.errors.is_empty());
}

#[test]
fn test_synthesize_dry_run() {
    let (mut mem, _dir) = setup();

    // Add related memories with Hebbian links
    let id1 = add_memory(&mut mem, "Rust borrow checker prevents data races at compile time", 0.7);
    let id2 = add_memory(&mut mem, "Rust ownership model eliminates use-after-free bugs", 0.7);
    let id3 = add_memory(&mut mem, "Rust lifetimes ensure references are always valid", 0.6);

    coactivate(&mut mem, &id1, &id2);
    coactivate(&mut mem, &id2, &id3);
    coactivate(&mut mem, &id1, &id3);

    let report = mem.synthesize_dry_run().unwrap();

    // Should find at least one cluster but create no insights (dry run)
    assert!(report.clusters_found > 0, "Expected clusters from Hebbian links");
    assert_eq!(report.clusters_synthesized, 0, "Dry run should not synthesize");
    assert!(report.insights_created.is_empty(), "Dry run should not create insights");
    assert!(report.sources_demoted.is_empty(), "Dry run should not demote sources");
    assert!(!report.gate_results.is_empty(), "Should have gate results");
}

#[test]
fn test_synthesize_no_llm_graceful_degradation() {
    let (mut mem, _dir) = setup();

    // Add cluster of related memories
    let id1 = add_memory(&mut mem, "Financial independence requires multiple income streams", 0.8);
    let id2 = add_memory(&mut mem, "SaaS products provide recurring revenue for financial freedom", 0.8);
    let id3 = add_memory(&mut mem, "Building tools that others pay for is a path to financial independence", 0.7);

    coactivate(&mut mem, &id1, &id2);
    coactivate(&mut mem, &id2, &id3);
    coactivate(&mut mem, &id1, &id3);

    // Synthesize without LLM provider — should discover clusters, run gate, but skip insight generation
    let report = mem.synthesize().unwrap();

    assert!(report.clusters_found > 0, "Should discover clusters");
    // Without LLM, insights can't be generated
    assert_eq!(report.insights_created.len(), 0, "No LLM → no insights");
    assert!(report.sources_demoted.is_empty(), "No insights → no demotions");
}

#[test]
fn test_list_insights_empty() {
    let (mem, _dir) = setup();
    let insights = mem.list_insights(None).unwrap();
    assert!(insights.is_empty());
}

#[test]
fn test_sleep_cycle() {
    let (mut mem, _dir) = setup();

    // Add some memories
    add_memory(&mut mem, "Sleep consolidates short-term memories into long-term storage", 0.6);
    add_memory(&mut mem, "The hippocampus replays experiences during slow-wave sleep", 0.6);

    let report = mem.sleep_cycle(1.0, None).unwrap();
    assert!(report.consolidation_ok);
    // Synthesis may or may not find clusters depending on links
}

#[test]
fn test_sleep_cycle_synthesis_disabled() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("nosync.db");
    let config = MemoryConfig::default();
    let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();
    // Don't enable synthesis settings

    add_memory(&mut mem, "This memory should only consolidate, not synthesize", 0.5);

    let report = mem.sleep_cycle(1.0, None).unwrap();
    assert!(report.consolidation_ok);
    assert!(report.synthesis.is_none(), "Synthesis should be None when not enabled");
}

#[test]
fn test_insight_sources_nonexistent() {
    let (mem, _dir) = setup();
    let sources = mem.insight_sources("nonexistent-id").unwrap();
    assert!(sources.is_empty());
}

#[test]
fn test_insights_for_memory_nonexistent() {
    let (mem, _dir) = setup();
    let provenance = mem.insights_for_memory("nonexistent-id").unwrap();
    assert!(provenance.is_empty());
}

#[test]
fn test_get_provenance_nonexistent() {
    let (mem, _dir) = setup();
    let chain = mem.get_provenance("nonexistent-id", 5).unwrap();
    assert_eq!(chain.root_id, "nonexistent-id");
    assert!(chain.layers.is_empty());
}

#[test]
fn test_is_insight_helper() {
    use engramai::is_insight;
    use engramai::MemoryRecord;
    use engramai::MemoryLayer;
    use chrono::Utc;

    // Regular memory — not an insight
    let regular = MemoryRecord {
        id: "test-1".into(),
        content: "regular memory".into(),
        memory_type: MemoryType::Factual,
        importance: 0.5,
        created_at: Utc::now(),
        access_times: vec![],
        working_strength: 1.0,
        core_strength: 0.0,
        layer: MemoryLayer::Working,
        pinned: false,
        metadata: None,
        source: String::new(),
        consolidation_count: 0,
        last_consolidated: None,
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
    };
    assert!(!is_insight(&regular));

    // Insight memory — has is_synthesis: true
    let mut insight = regular.clone();
    insight.metadata = Some(serde_json::json!({"is_synthesis": true}));
    assert!(is_insight(&insight));

    // Memory with is_synthesis: false
    let mut not_insight = regular.clone();
    not_insight.metadata = Some(serde_json::json!({"is_synthesis": false}));
    assert!(!is_insight(&not_insight));
}

#[test]
fn test_synthesize_settings_passthrough() {
    // Verify that custom settings are respected
    let (mut mem, _dir) = setup();

    let mut custom = SynthesisSettings::default();
    custom.enabled = true;
    custom.max_insights_per_consolidation = 1;
    custom.max_llm_calls_per_run = 0; // No LLM budget at all

    let report = mem.synthesize_with(&custom).unwrap();
    // Even if clusters found, no LLM budget means no insights
    assert_eq!(report.insights_created.len(), 0);
}

#[test]
fn test_sleep_cycle_with_linked_memories() {
    let (mut mem, _dir) = setup();

    // Add related memories with Hebbian links
    let id1 = add_memory(&mut mem, "ACT-R activation model calculates memory retrieval probability", 0.7);
    let id2 = add_memory(&mut mem, "Base-level activation depends on frequency and recency of access", 0.7);
    let id3 = add_memory(&mut mem, "Spreading activation follows Hebbian link strengths", 0.6);

    coactivate(&mut mem, &id1, &id2);
    coactivate(&mut mem, &id2, &id3);
    coactivate(&mut mem, &id1, &id3);

    let report = mem.sleep_cycle(1.0, None).unwrap();
    assert!(report.consolidation_ok);

    // Synthesis should have been attempted
    let synth = report.synthesis.expect("Synthesis should have run");
    assert!(synth.clusters_found > 0, "Should find clusters from Hebbian links");
    // No LLM → no insights, but gate results should exist
    assert!(!synth.gate_results.is_empty());
}
