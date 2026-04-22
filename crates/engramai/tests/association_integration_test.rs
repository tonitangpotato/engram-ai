//! Integration test for multi-signal Hebbian association discovery.
//!
//! Tests the full pipeline: store memories → associations form automatically.
//!
//! KEY FINDING: Entity extraction only matches known technologies (Rust, Python, etc.)
//! and structural patterns (file paths, issue IDs). For general text, embedding cosine
//! and temporal proximity are the dominant signals. Entity overlap is 0 for most
//! natural language content that doesn't mention specific tech names.

use engramai::{Memory, MemoryConfig, MemoryType};
use engramai::config::AssociationConfig;

fn config_with_association() -> MemoryConfig {
    let mut config = MemoryConfig::default();
    config.association = AssociationConfig {
        enabled: true,
        w_entity: 0.3,
        w_embedding: 0.5,
        w_temporal: 0.2,
        link_threshold: 0.15,
        max_links_per_memory: 5,
        candidate_limit: 50,
        temporal_window_days: 7,
        initial_strength: 0.5,
        decay_corecall: 0.95,
        decay_multi: 0.90,
        decay_single: 0.85,
    };
    config.entity_config.enabled = true;
    config
}

fn config_disabled() -> MemoryConfig {
    let mut config = MemoryConfig::default();
    config.association.enabled = false;
    config.entity_config.enabled = true;
    config
}

fn new_memory(config: MemoryConfig) -> Memory {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_assoc.db");
    let db_str = db_path.to_str().unwrap().to_string();
    std::mem::forget(dir);
    Memory::new(&db_str, Some(config)).expect("create memory")
}

fn get_link_count(mem: &Memory) -> i64 {
    mem.connection()
        .query_row(
            "SELECT COUNT(*) FROM hebbian_links",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .unwrap()
}

fn get_signal_links(mem: &Memory) -> Vec<(String, String, f64, String, String)> {
    let conn = mem.connection();
    let mut stmt = conn
        .prepare(
            "SELECT source_id, target_id, strength, \
             COALESCE(signal_source, 'co_recall') as sig_src, \
             COALESCE(signal_detail, '') as sig_det \
             FROM hebbian_links ORDER BY source_id, target_id",
        )
        .unwrap();
    stmt.query_map([], |row: &rusqlite::Row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

/// Test 1: When association is disabled, no links form on write.
#[test]
fn test_assoc_disabled_no_links() {
    let config = config_disabled();
    let mut mem = new_memory(config);

    mem.add("Rust programming language", MemoryType::Factual, Some(0.5), None, None)
        .expect("store 1");
    mem.add(
        "Rust is a systems programming language",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )
    .expect("store 2");

    let count = get_link_count(&mem);
    assert_eq!(count, 0, "no links should form when association is disabled");
}

/// Test 2: With association enabled, related memories form links via embedding + temporal.
#[test]
fn test_assoc_creates_links_on_write() {
    let config = config_with_association();
    let mut mem = new_memory(config);

    // Store first memory — no candidates exist yet, so no links
    mem.add(
        "RustClaw is an AI agent framework built in Rust",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )
    .expect("store 1");

    let count_after_first = get_link_count(&mem);
    assert_eq!(count_after_first, 0, "first memory has no candidates to link with");

    // Store second memory with semantic overlap
    mem.add(
        "RustClaw uses Rust for high-performance AI agent execution",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )
    .expect("store 2");

    let count_after_second = get_link_count(&mem);
    println!("Links after second memory: {}", count_after_second);

    assert!(
        count_after_second >= 1,
        "second memory should form links with first, got {}",
        count_after_second
    );

    // Verify link metadata structure
    let links = get_signal_links(&mem);
    for (src, tgt, strength, sig_source, sig_detail) in &links {
        println!(
            "  Link: {} → {} | strength={:.3} | source={} | detail={}",
            &src[..8], &tgt[..8], strength, sig_source, sig_detail
        );

        assert!(
            ["entity", "embedding", "temporal", "multi"].contains(&sig_source.as_str()),
            "invalid signal_source: {}",
            sig_source
        );

        if !sig_detail.is_empty() {
            let detail: serde_json::Value =
                serde_json::from_str(sig_detail).expect("signal_detail should be valid JSON");
            assert!(detail["entity_overlap"].is_number(), "should have entity_overlap");
            assert!(detail["embedding_cosine"].is_number(), "should have embedding_cosine");
            assert!(detail["temporal_proximity"].is_number(), "should have temporal_proximity");
        }
    }
}

/// Test 3: Three related memories form a connected cluster.
#[test]
fn test_assoc_cluster_formation() {
    let config = config_with_association();
    let mut mem = new_memory(config);

    mem.add(
        "Hebbian learning is a neural network learning rule",
        MemoryType::Factual,
        Some(0.6),
        None,
        None,
    )
    .expect("store 1");

    mem.add(
        "Neurons that fire together wire together - Hebbian principle",
        MemoryType::Factual,
        Some(0.6),
        None,
        None,
    )
    .expect("store 2");

    mem.add(
        "Hebbian learning strengthens synaptic connections between neurons",
        MemoryType::Factual,
        Some(0.6),
        None,
        None,
    )
    .expect("store 3");

    let links = get_signal_links(&mem);
    println!("Links in cluster:");
    for (src, tgt, strength, sig_source, _sig_detail) in &links {
        println!("  {} → {} | strength={:.3} | source={}", &src[..8], &tgt[..8], strength, sig_source);
    }

    assert!(
        links.len() >= 2,
        "3 related memories should form at least 2 links, got {}",
        links.len()
    );
}

/// Test 4: Entity overlap boosts links when tech names are present.
/// Memories with known technology names (Rust, Python) should have
/// entity overlap > 0, while generic text has entity overlap = 0.
#[test]
fn test_assoc_entity_overlap_with_tech_names() {
    let mut config = config_with_association();
    config.association.link_threshold = 0.01; // Very low to see all signals
    let mut mem = new_memory(config);

    // Pair with tech entities: "Rust" should be extracted
    mem.add(
        "Rust provides memory safety without garbage collection",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )
    .expect("store 1");

    mem.add(
        "The Rust borrow checker ensures safe concurrent access",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    )
    .expect("store 2");

    let links = get_signal_links(&mem);
    assert!(!links.is_empty(), "should have links for Rust-related content");

    for (_src, _tgt, _str, _sig_src, sig_detail) in &links {
        let detail: serde_json::Value = serde_json::from_str(sig_detail).unwrap();
        let entity_overlap = detail["entity_overlap"].as_f64().unwrap();
        println!(
            "Entity overlap for Rust-related pair: {:.3} (embedding: {:.3})",
            entity_overlap,
            detail["embedding_cosine"].as_f64().unwrap()
        );

        // Both mention "Rust" → entity overlap should be > 0
        assert!(
            entity_overlap > 0.0,
            "memories mentioning 'Rust' should have entity overlap > 0, got {:.3}",
            entity_overlap
        );
    }
}

/// Test 5: Max links per memory is respected.
#[test]
fn test_assoc_max_links_budget() {
    let mut config = config_with_association();
    config.association.max_links_per_memory = 2;
    config.association.link_threshold = 0.1;
    let mut mem = new_memory(config);

    for i in 0..6 {
        mem.add(
            &format!("Rust programming concept number {} about memory safety", i),
            MemoryType::Factual,
            Some(0.5),
            None,
            None,
        )
        .expect("store");
    }

    let conn = mem.connection();
    let max_outbound: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(cnt), 0) FROM \
             (SELECT source_id, COUNT(*) as cnt FROM hebbian_links GROUP BY source_id)",
            [],
            |row: &rusqlite::Row| row.get(0),
        )
        .unwrap();

    println!("Max outbound links per memory: {}", max_outbound);
    assert!(
        max_outbound <= 2,
        "no memory should have more than max_links_per_memory outbound links, got {}",
        max_outbound
    );
}

/// Test 6: Signal metadata quality — correct JSON structure and plausible values.
#[test]
fn test_assoc_signal_metadata_quality() {
    let config = config_with_association();
    let mut mem = new_memory(config);

    mem.add(
        "Engram is a cognitive memory system for AI agents using Rust",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .expect("store 1");

    mem.add(
        "Engram implements ACT-R activation decay for AI agent memory in Rust",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .expect("store 2");

    let links = get_signal_links(&mem);
    assert!(!links.is_empty(), "should have at least one link");

    for (src, tgt, strength, sig_source, sig_detail) in &links {
        // Initial strength should be 0.5
        assert!(
            (*strength - 0.5).abs() < 1e-6,
            "initial strength should be 0.5, got {}",
            strength
        );

        let detail: serde_json::Value =
            serde_json::from_str(sig_detail).expect("signal_detail should be valid JSON");

        let entity_overlap = detail["entity_overlap"].as_f64().unwrap();
        let embedding_cosine = detail["embedding_cosine"].as_f64().unwrap();
        let temporal_prox = detail["temporal_proximity"].as_f64().unwrap();

        println!(
            "Link {}→{}: entity={:.3}, embedding={:.3}, temporal={:.3}, source={}",
            &src[..8], &tgt[..8], entity_overlap, embedding_cosine, temporal_prox, sig_source
        );

        // Both mention "Rust" → entity overlap should be > 0
        assert!(
            entity_overlap > 0.0,
            "both mention 'Rust', entity overlap should be > 0"
        );

        // Embedding cosine should be high for semantically similar content
        assert!(
            embedding_cosine > 0.5,
            "semantically similar memories should have embedding cosine > 0.5, got {:.3}",
            embedding_cosine
        );

        // Temporal proximity should be ~1.0 (stored in same test run)
        assert!(
            temporal_prox > 0.9,
            "temporal proximity should be high for near-simultaneous storage, got {:.3}",
            temporal_prox
        );
    }
}

/// Test 7: All write-time links have signal_source and signal_detail.
#[test]
fn test_assoc_write_time_links_have_metadata() {
    let config = config_with_association();
    let mut mem = new_memory(config);

    mem.add("GID is a graph-indexed development tool", MemoryType::Factual, Some(0.7), None, None)
        .expect("store 1");
    mem.add(
        "GID uses Infomap clustering for code graph analysis",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .expect("store 2");

    let total_links = get_link_count(&mem);
    if total_links > 0 {
        let conn = mem.connection();
        let with_metadata: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM hebbian_links WHERE signal_source IS NOT NULL AND signal_detail IS NOT NULL",
                [],
                |row: &rusqlite::Row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            with_metadata, total_links,
            "all write-time links should have signal metadata, got {}/{}",
            with_metadata, total_links
        );
    }
}

/// Test 8: End-to-end — store, recall, verify association enrichment.
#[test]
fn test_assoc_e2e_store_and_recall() {
    let config = config_with_association();
    let mut mem = new_memory(config);

    mem.add(
        "GID is a graph-indexed development tool for code intelligence",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .expect("store 1");

    mem.add(
        "GID uses Infomap clustering for community detection in code graphs",
        MemoryType::Factual,
        Some(0.7),
        None,
        None,
    )
    .expect("store 2");

    mem.add(
        "Code intelligence tools help AI agents understand repository structure",
        MemoryType::Factual,
        Some(0.6),
        None,
        None,
    )
    .expect("store 3");

    let total_links = get_link_count(&mem);
    println!("Total links after 3 memories: {}", total_links);

    let links = get_signal_links(&mem);
    for (src, tgt, strength, sig_source, sig_detail) in &links {
        println!(
            "  {}→{} | str={:.3} | src={} | det={}",
            &src[..8], &tgt[..8], strength, sig_source,
            &sig_detail[..sig_detail.len().min(80)]
        );
    }

    // At least 2 links should form among 3 related memories
    assert!(
        total_links >= 2,
        "3 related memories should form at least 2 links, got {}",
        total_links
    );

    // Recall should work
    let results = mem
        .recall("GID code graph", 5, None, None)
        .expect("recall should succeed");
    println!("\nRecall results for 'GID code graph':");
    for r in &results {
        println!("  - [{}] {:.80}", &r.record.id[..8], r.record.content);
    }

    assert!(!results.is_empty(), "recall should return results");
    println!("\n✅ End-to-end test passed");
}

/// Test 9: Embedding cosine dominates for semantically similar but entity-poor content.
/// This verifies the multi-signal fusion works correctly when only embedding fires.
#[test]
fn test_assoc_embedding_dominant_signal() {
    let mut config = config_with_association();
    config.association.link_threshold = 0.01; // Very low to capture all
    let mut mem = new_memory(config);

    // No tech entities, but semantically very similar
    mem.add(
        "The cat sat on the warm sunny windowsill",
        MemoryType::Episodic,
        Some(0.3),
        None,
        None,
    )
    .expect("store 1");

    mem.add(
        "A kitten was resting by the window in the sunlight",
        MemoryType::Episodic,
        Some(0.3),
        None,
        None,
    )
    .expect("store 2");

    let links = get_signal_links(&mem);
    assert!(!links.is_empty(), "semantically similar content should form links");

    for (_src, _tgt, _str, sig_source, sig_detail) in &links {
        let detail: serde_json::Value = serde_json::from_str(sig_detail).unwrap();
        let entity_overlap = detail["entity_overlap"].as_f64().unwrap();
        let embedding_cosine = detail["embedding_cosine"].as_f64().unwrap();

        println!("Embedding-dominant link: entity={:.3}, cosine={:.3}, source={}", 
            entity_overlap, embedding_cosine, sig_source);

        // No tech entities → entity overlap should be 0
        assert!(
            entity_overlap < 0.01,
            "no tech entities → entity overlap should be ~0"
        );
        // But embedding should show similarity
        assert!(
            embedding_cosine > 0.3,
            "semantically similar content should have cosine > 0.3, got {:.3}",
            embedding_cosine
        );
    }
}
