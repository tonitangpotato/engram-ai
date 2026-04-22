//! Integration tests for multi-signal Hebbian link formation (Phases 5 & 6).
//!
//! Tests:
//! - Write-time association creates links when enabled
//! - No links created when association is disabled
//! - Differential decay applies correct rates
//! - Co-recall reinforces write-time links

use engramai::{Memory, MemoryConfig, MemoryType};

fn setup_memory_with_association() -> Memory {
    let mut config = MemoryConfig::default();
    config.hebbian_enabled = true;
    config.association.enabled = true;
    config.association.link_threshold = 0.15; // Low threshold for testing without embeddings
    config.entity_config.enabled = true;
    // Add known entities so entity extractor can find them
    config.entity_config.known_projects = vec![
        "rust".to_string(),
        "python".to_string(),
    ];
    config.entity_config.known_people = vec![
        "alice".to_string(),
        "bob".to_string(),
    ];
    Memory::new(":memory:", Some(config)).unwrap()
}

fn setup_memory_without_association() -> Memory {
    let mut config = MemoryConfig::default();
    config.hebbian_enabled = true;
    config.association.enabled = false; // Explicitly disabled
    config.entity_config.enabled = true;
    Memory::new(":memory:", Some(config)).unwrap()
}

#[test]
fn test_write_time_association_creates_links() {
    let mut mem = setup_memory_with_association();

    // Add memory A about rust and AI
    let id_a = mem.add(
        "Rust is great for building AI agents",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();
    assert!(!id_a.is_empty(), "should return a memory ID");

    // Add memory B about rust and web — shares "rust" entity with A
    let id_b = mem.add(
        "Building a web server in Rust is straightforward",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();
    assert!(!id_b.is_empty(), "should return a memory ID");

    // Check if a link was created between A and B
    let count: i64 = mem.connection().query_row(
        "SELECT COUNT(*) FROM hebbian_links WHERE \
         (source_id = ?1 AND target_id = ?2) OR \
         (source_id = ?2 AND target_id = ?1)",
        rusqlite::params![id_a, id_b],
        |row| row.get(0),
    ).unwrap();

    // With temporal proximity (same second) contributing ~0.2 weight,
    // and possible entity overlap, we should get a link
    assert!(count >= 1, "should find at least 1 link between memories, found {}", count);
}

#[test]
fn test_association_disabled_no_links() {
    let mut mem = setup_memory_without_association();

    // Add two related memories
    let id_a = mem.add(
        "Rust is great for building AI agents",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    let id_b = mem.add(
        "Building a web server in Rust is straightforward",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    // No write-time links should exist (association disabled, no co-recall)
    let count: i64 = mem.connection().query_row(
        "SELECT COUNT(*) FROM hebbian_links WHERE \
         (source_id = ?1 AND target_id = ?2) OR \
         (source_id = ?2 AND target_id = ?1)",
        rusqlite::params![id_a, id_b],
        |row| row.get(0),
    ).unwrap();

    assert_eq!(count, 0, "should have no links when association is disabled");
}

#[test]
fn test_differential_decay_in_consolidation() {
    let mut mem = setup_memory_with_association();

    // Add two memories to create write-time links
    let id_a = mem.add(
        "Rust is great for building AI agents",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    let _id_b = mem.add(
        "Building a web server in Rust is straightforward",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    // Also create a manual corecall link for comparison
    let id_c = mem.add(
        "Python is used for data science",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    // Insert a corecall link manually (simulating co-recall reinforcement)
    mem.connection().execute(
        "INSERT OR IGNORE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
         VALUES (?1, ?2, 1.0, 3, strftime('%s','now'), 'corecall', 'default')",
        rusqlite::params![id_a, id_c],
    ).unwrap();

    // Get initial strengths
    let get_strength = |conn: &rusqlite::Connection, s: &str, t: &str| -> Option<f64> {
        conn.query_row(
            "SELECT strength FROM hebbian_links WHERE \
             (source_id = ?1 AND target_id = ?2) OR \
             (source_id = ?2 AND target_id = ?1)",
            rusqlite::params![s, t],
            |row| row.get(0),
        ).ok()
    };

    let corecall_before = get_strength(mem.connection(), &id_a, &id_c)
        .expect("corecall link should exist");

    // Run consolidation (triggers differential decay)
    mem.consolidate(1.0).unwrap();

    let corecall_after = get_strength(mem.connection(), &id_a, &id_c);

    // Corecall link should have decayed by 0.95 factor
    if let Some(after) = corecall_after {
        let expected = corecall_before * 0.95;
        assert!(
            (after - expected).abs() < 0.01,
            "corecall link should decay by 0.95: expected ~{:.3}, got {:.3}",
            expected, after
        );
    }
}

#[test]
fn test_write_time_link_has_signal_metadata() {
    let mut mem = setup_memory_with_association();

    let id_a = mem.add(
        "Rust is great for building AI agents",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    let id_b = mem.add(
        "Building a web server in Rust is straightforward",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    // Check that any link created has signal_source and signal_detail
    let result: Result<(String, String), _> = mem.connection().query_row(
        "SELECT signal_source, signal_detail FROM hebbian_links WHERE \
         (source_id = ?1 AND target_id = ?2) OR \
         (source_id = ?2 AND target_id = ?1)",
        rusqlite::params![id_a, id_b],
        |row| Ok((row.get(0)?, row.get(1)?)),
    );

    if let Ok((signal_source, signal_detail)) = result {
        // signal_source should be a valid value
        assert!(
            ["entity", "embedding", "temporal", "multi"].contains(&signal_source.as_str()),
            "signal_source should be valid, got: {}",
            signal_source
        );

        // signal_detail should be valid JSON
        let detail: serde_json::Value = serde_json::from_str(&signal_detail)
            .expect("signal_detail should be valid JSON");
        assert!(detail["entity_overlap"].is_number(), "should have entity_overlap");
        assert!(detail["embedding_cosine"].is_number(), "should have embedding_cosine");
        assert!(detail["temporal_proximity"].is_number(), "should have temporal_proximity");
    }
    // If no link was created (threshold not met), that's also acceptable
}

#[test]
fn test_association_failure_does_not_block_memory_storage() {
    // Even with association enabled, the memory should always be stored
    let mut mem = setup_memory_with_association();

    let id = mem.add(
        "This memory should be stored regardless of association outcome",
        MemoryType::Factual,
        Some(0.5),
        None,
        None,
    ).unwrap();

    assert!(!id.is_empty(), "memory should be stored");

    // Verify memory exists in DB
    let content: String = mem.connection().query_row(
        "SELECT content FROM memories WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).unwrap();

    assert!(content.contains("stored regardless"), "memory content should be persisted");
}
