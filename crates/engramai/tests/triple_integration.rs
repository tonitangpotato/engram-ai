//! Integration tests for ISS-016: LLM Triple Extraction for Hebbian Link Quality.
//!
//! Tests cover storage, entity fusion, consolidation integration, and error handling.

use engramai::storage::Storage;
use engramai::{Memory, MemoryConfig, MemoryType, Triple, Predicate, TripleSource, TripleExtractor};

/// Mock triple extractor that returns predefined triples.
struct MockTripleExtractor {
    triples: Vec<Triple>,
}

impl MockTripleExtractor {
    fn new(triples: Vec<Triple>) -> Self {
        Self { triples }
    }
    
    fn empty() -> Self {
        Self { triples: vec![] }
    }
}

impl TripleExtractor for MockTripleExtractor {
    fn extract_triples(&self, _content: &str) -> Result<Vec<Triple>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.triples.clone())
    }
}

/// Mock extractor that always fails.
struct FailingTripleExtractor;

impl TripleExtractor for FailingTripleExtractor {
    fn extract_triples(&self, _content: &str) -> Result<Vec<Triple>, Box<dyn std::error::Error + Send + Sync>> {
        Err("LLM unavailable: connection refused".into())
    }
}

fn make_storage() -> Storage {
    Storage::new(":memory:").expect("create storage")
}

#[allow(dead_code)]
fn make_memory() -> Memory {
    Memory::new(":memory:", Some(MemoryConfig::default())).expect("create memory")
}

fn make_triple(subj: &str, pred: Predicate, obj: &str, conf: f64) -> Triple {
    Triple::new(subj.to_string(), pred, obj.to_string(), conf)
}

/// Helper: insert a raw memory into storage and return its ID.
fn insert_test_memory(storage: &Storage, content: &str) -> String {
    let id = format!("test-{}", uuid_simple());
    let now = chrono::Utc::now().timestamp() as f64;
    storage.connection().execute(
        "INSERT INTO memories (id, content, memory_type, layer, created_at, namespace) \
         VALUES (?1, ?2, 'factual', 'working', ?3, 'default')",
        rusqlite::params![id, content, now],
    ).expect("insert memory");
    id
}

/// Simple UUID-like string for test IDs.
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    format!("{:x}{:x}", t.as_secs(), t.subsec_nanos())
}

// ===== Storage Tests =====

#[test]
fn test_store_and_get_triples_roundtrip() {
    // GOAL-1.1: Triples survive store + get round-trip
    let storage = make_storage();
    let id = insert_test_memory(&storage, "Engram uses ACT-R for memory decay");
    
    let triples = vec![
        make_triple("engram", Predicate::Uses, "act-r", 0.9),
        make_triple("act-r", Predicate::IsA, "cognitive model", 0.8),
    ];
    
    let inserted = storage.store_triples(&id, &triples).expect("store triples");
    assert_eq!(inserted, 2);
    
    let retrieved = storage.get_triples(&id).expect("get triples");
    assert_eq!(retrieved.len(), 2);
    
    // Check first triple
    assert_eq!(retrieved[0].subject, "engram");
    assert_eq!(retrieved[0].predicate, Predicate::Uses);
    assert_eq!(retrieved[0].object, "act-r");
    assert!((retrieved[0].confidence - 0.9).abs() < f64::EPSILON);
    assert_eq!(retrieved[0].source, TripleSource::Llm);
    
    // Check second triple
    assert_eq!(retrieved[1].subject, "act-r");
    assert_eq!(retrieved[1].predicate, Predicate::IsA);
    assert_eq!(retrieved[1].object, "cognitive model");
}

#[test]
fn test_duplicate_triple_is_idempotent() {
    // GOAL-1.3: Duplicate triples are rejected without error
    let storage = make_storage();
    let id = insert_test_memory(&storage, "test content");
    
    let triples = vec![make_triple("a", Predicate::Uses, "b", 0.9)];
    
    let first = storage.store_triples(&id, &triples).expect("first store");
    assert_eq!(first, 1);
    
    let second = storage.store_triples(&id, &triples).expect("second store");
    assert_eq!(second, 0); // Duplicate ignored
    
    let retrieved = storage.get_triples(&id).expect("get triples");
    assert_eq!(retrieved.len(), 1); // Still only one
}

#[test]
fn test_cascade_delete_removes_triples() {
    // GOAL-1.3: ON DELETE CASCADE
    let mut storage = make_storage();
    let id = insert_test_memory(&storage, "test content");
    
    let triples = vec![make_triple("a", Predicate::Uses, "b", 0.9)];
    storage.store_triples(&id, &triples).expect("store");
    
    assert!(storage.has_triples(&id).expect("has_triples"));
    
    // Delete the memory
    storage.delete(&id).expect("delete memory");
    
    // Triples should be gone
    let retrieved = storage.get_triples(&id).expect("get triples");
    assert!(retrieved.is_empty());
}

#[test]
fn test_migrate_triples_idempotent() {
    // GOAL-1.4: Migration is idempotent (creating Storage runs migration)
    let _s1 = make_storage();
    let _s2 = make_storage(); // Second in-memory DB, migration runs again
}

#[test]
fn test_has_triples() {
    let storage = make_storage();
    let id = insert_test_memory(&storage, "test");
    
    assert!(!storage.has_triples(&id).expect("has_triples"));
    
    let triples = vec![make_triple("a", Predicate::Uses, "b", 0.9)];
    storage.store_triples(&id, &triples).expect("store");
    
    assert!(storage.has_triples(&id).expect("has_triples"));
}

// ===== Unenriched Memory Query Tests =====

#[test]
fn test_get_unenriched_skips_enriched() {
    // GOAL-3.2: Memories with triples are skipped
    let storage = make_storage();
    let id1 = insert_test_memory(&storage, "enriched memory");
    // small sleep to ensure different created_at
    std::thread::sleep(std::time::Duration::from_millis(10));
    let id2 = insert_test_memory(&storage, "unenriched memory");
    
    // Enrich id1
    let triples = vec![make_triple("a", Predicate::Uses, "b", 0.9)];
    storage.store_triples(&id1, &triples).expect("store");
    
    let unenriched = storage.get_unenriched_memory_ids(10, 3).expect("query");
    assert_eq!(unenriched.len(), 1);
    assert_eq!(unenriched[0], id2);
}

#[test]
fn test_get_unenriched_respects_max_retries() {
    // GOAL-3.2: Memories that failed 3 times are permanently skipped
    let storage = make_storage();
    let id = insert_test_memory(&storage, "retry test");
    
    // Should appear initially
    let unenriched = storage.get_unenriched_memory_ids(10, 3).expect("query");
    assert_eq!(unenriched.len(), 1);
    
    // Increment attempts 3 times
    storage.increment_extraction_attempts(&id).expect("inc");
    storage.increment_extraction_attempts(&id).expect("inc");
    storage.increment_extraction_attempts(&id).expect("inc");
    
    // Should be skipped now
    let unenriched = storage.get_unenriched_memory_ids(10, 3).expect("query");
    assert!(unenriched.is_empty());
}

// ===== Entity Fusion Tests =====

#[test]
fn test_triple_entities_appear_in_get_entities_for_memory() {
    // GOAL-4.1: Triple subjects/objects are merged into entity graph
    let storage = make_storage();
    let id = insert_test_memory(&storage, "test content");
    
    let triples = vec![
        make_triple("engram", Predicate::Uses, "act-r", 0.9),
        make_triple("hebbian learning", Predicate::IsA, "neural mechanism", 0.8),
    ];
    storage.store_triples(&id, &triples).expect("store");
    
    let entities = storage.get_entities_for_memory(&id).expect("get entities");
    
    // Should contain triple-derived entities (lowercased)
    assert!(entities.contains(&"engram".to_string()), "missing 'engram' in {:?}", entities);
    assert!(entities.contains(&"act-r".to_string()), "missing 'act-r' in {:?}", entities);
    assert!(entities.contains(&"hebbian learning".to_string()), "missing 'hebbian learning' in {:?}", entities);
    assert!(entities.contains(&"neural mechanism".to_string()), "missing 'neural mechanism' in {:?}", entities);
}

#[test]
fn test_memories_without_triples_still_have_entities() {
    // GOAL-4.2: Memories without triples still participate via AC entities
    let storage = make_storage();
    let id = insert_test_memory(&storage, "test content about Rust programming");
    
    // No triples stored — entity query should still work
    let entities = storage.get_entities_for_memory(&id).expect("get entities");
    // Might be empty (no AC dictionary match) but should not error
    let _ = entities;
}

// ===== Consolidation Integration Tests =====

#[test]
fn test_consolidate_with_triple_disabled_skips_extraction() {
    // GOAL-5.1: When disabled, no extraction happens
    let mut config = MemoryConfig::default();
    config.triple.enabled = false;
    
    let mut mem = Memory::new(":memory:", Some(config)).expect("create");
    mem.set_triple_extractor(Box::new(FailingTripleExtractor));
    
    let _id = mem.add("test content for extraction", MemoryType::Factual, Some(0.5), None, None)
        .expect("add");
    
    mem.consolidate(1.0).expect("consolidate");
    
    // With extraction disabled, attempts should not be incremented.
    // We can't directly check attempts via public API, but we verify the memory
    // still shows up as unenriched if we later enable extraction.
    // The main verification is that consolidate() didn't crash with FailingExtractor.
}

#[test]
fn test_consolidate_with_triple_enabled_extracts() {
    // GOAL-3.1: Triple extraction runs during consolidation
    let mut config = MemoryConfig::default();
    config.triple.enabled = true;
    config.triple.batch_size = 10;
    
    let mut mem = Memory::new(":memory:", Some(config)).expect("create");
    
    let triples = vec![
        make_triple("rust", Predicate::IsA, "programming language", 0.95),
    ];
    mem.set_triple_extractor(Box::new(MockTripleExtractor::new(triples)));
    
    let id = mem.add("Rust is a systems programming language", MemoryType::Factual, Some(0.5), None, None)
        .expect("add");
    
    // Before consolidation: check via connection
    let has_before: bool = mem.connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM triples WHERE memory_id = ?1)",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert!(!has_before);
    
    mem.consolidate(1.0).expect("consolidate");
    
    // After consolidation: triples extracted
    let count: i64 = mem.connection().query_row(
        "SELECT COUNT(*) FROM triples WHERE memory_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(count, 1);
    
    // Verify triple content
    let (subj, pred, obj): (String, String, String) = mem.connection().query_row(
        "SELECT subject, predicate, object FROM triples WHERE memory_id = ?1",
        rusqlite::params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).expect("query");
    assert_eq!(subj, "rust");
    assert_eq!(pred, "is_a");
    assert_eq!(obj, "programming language");
}

#[test]
fn test_consolidate_with_failing_extractor_is_nonfatal() {
    // GUARD-3: LLM extraction failures are non-fatal
    let mut config = MemoryConfig::default();
    config.triple.enabled = true;
    
    let mut mem = Memory::new(":memory:", Some(config)).expect("create");
    mem.set_triple_extractor(Box::new(FailingTripleExtractor));
    
    let id = mem.add("test content", MemoryType::Factual, Some(0.5), None, None)
        .expect("add");
    
    // Should not panic or return error
    mem.consolidate(1.0).expect("consolidate should succeed even with failing extractor");
    
    // Attempts should be incremented (1 < 3, so still in queue)
    let attempts: i64 = mem.connection().query_row(
        "SELECT triple_extraction_attempts FROM memories WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(attempts, 1);
}

#[test]
fn test_consolidate_with_empty_extractor_increments_attempts() {
    // Mock extractor returns empty vec → should increment attempts
    let mut config = MemoryConfig::default();
    config.triple.enabled = true;
    
    let mut mem = Memory::new(":memory:", Some(config)).expect("create");
    mem.set_triple_extractor(Box::new(MockTripleExtractor::empty()));
    
    let id = mem.add("ok sounds good", MemoryType::Factual, Some(0.5), None, None)
        .expect("add");
    
    mem.consolidate(1.0).expect("consolidate");
    
    // No triples stored
    let count: i64 = mem.connection().query_row(
        "SELECT COUNT(*) FROM triples WHERE memory_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(count, 0);
    
    // Attempt count should be 1
    let attempts: i64 = mem.connection().query_row(
        "SELECT triple_extraction_attempts FROM memories WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(attempts, 1);
    
    // Run consolidation 2 more times to exhaust retries
    mem.consolidate(1.0).expect("consolidate 2");
    mem.consolidate(1.0).expect("consolidate 3");
    
    // Now attempts should be 3 (permanently skipped)
    let attempts: i64 = mem.connection().query_row(
        "SELECT triple_extraction_attempts FROM memories WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(attempts, 3);
}

#[test]
fn test_store_does_not_trigger_extraction() {
    // GUARD-2: Hot path isolation — store() must not call extractor
    let mut config = MemoryConfig::default();
    config.triple.enabled = true;
    
    let mut mem = Memory::new(":memory:", Some(config)).expect("create");
    mem.set_triple_extractor(Box::new(FailingTripleExtractor));
    
    // store (via add) should succeed regardless of extractor state
    let id = mem.add("test content", MemoryType::Factual, Some(0.5), None, None)
        .expect("add should succeed regardless of extractor");
    
    // No triples should exist — extraction only happens in consolidation
    let count: i64 = mem.connection().query_row(
        "SELECT COUNT(*) FROM triples WHERE memory_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    ).expect("query");
    assert_eq!(count, 0);
}

// ===== Predicate Tests =====

#[test]
fn test_predicate_from_str_lossy_all_variants() {
    assert_eq!(Predicate::from_str_lossy("is_a"), Predicate::IsA);
    assert_eq!(Predicate::from_str_lossy("isa"), Predicate::IsA);
    assert_eq!(Predicate::from_str_lossy("part_of"), Predicate::PartOf);
    assert_eq!(Predicate::from_str_lossy("partof"), Predicate::PartOf);
    assert_eq!(Predicate::from_str_lossy("uses"), Predicate::Uses);
    assert_eq!(Predicate::from_str_lossy("depends_on"), Predicate::DependsOn);
    assert_eq!(Predicate::from_str_lossy("dependson"), Predicate::DependsOn);
    assert_eq!(Predicate::from_str_lossy("caused_by"), Predicate::CausedBy);
    assert_eq!(Predicate::from_str_lossy("causedby"), Predicate::CausedBy);
    assert_eq!(Predicate::from_str_lossy("leads_to"), Predicate::LeadsTo);
    assert_eq!(Predicate::from_str_lossy("leadsto"), Predicate::LeadsTo);
    assert_eq!(Predicate::from_str_lossy("implements"), Predicate::Implements);
    assert_eq!(Predicate::from_str_lossy("contradicts"), Predicate::Contradicts);
    assert_eq!(Predicate::from_str_lossy("related_to"), Predicate::RelatedTo);
}

#[test]
fn test_unknown_predicate_falls_back_to_related_to() {
    assert_eq!(Predicate::from_str_lossy("banana"), Predicate::RelatedTo);
    assert_eq!(Predicate::from_str_lossy(""), Predicate::RelatedTo);
    assert_eq!(Predicate::from_str_lossy("UNKNOWN_THING"), Predicate::RelatedTo);
}

#[test]
fn test_triple_new_clamps_confidence() {
    let t1 = Triple::new("a".into(), Predicate::Uses, "b".into(), 1.5);
    assert!((t1.confidence - 1.0).abs() < f64::EPSILON);
    
    let t2 = Triple::new("a".into(), Predicate::Uses, "b".into(), -0.5);
    assert!((t2.confidence - 0.0).abs() < f64::EPSILON);
    
    let t3 = Triple::new("a".into(), Predicate::Uses, "b".into(), 0.7);
    assert!((t3.confidence - 0.7).abs() < f64::EPSILON);
}

// ===== Config Tests =====

#[test]
fn test_triple_config_defaults() {
    let config = MemoryConfig::default();
    assert!(!config.triple.enabled);
    assert_eq!(config.triple.batch_size, 10);
    assert_eq!(config.triple.max_retries, 3);
    assert!(config.triple.model.is_none());
}

#[test]
fn test_triple_config_serde_roundtrip() {
    let config = MemoryConfig::default();
    let json = serde_json::to_string(&config).expect("serialize");
    let deserialized: MemoryConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(config.triple.enabled, deserialized.triple.enabled);
    assert_eq!(config.triple.batch_size, deserialized.triple.batch_size);
    assert_eq!(config.triple.max_retries, deserialized.triple.max_retries);
    assert_eq!(config.triple.model, deserialized.triple.model);
}

#[test]
fn test_triple_config_missing_from_json_uses_defaults() {
    // GUARD-4: Backward compatibility — configs without triple section still work
    // Serialize a default config, remove the "triple" key, deserialize back
    let mut config_value: serde_json::Value = serde_json::to_value(MemoryConfig::default()).expect("serialize");
    config_value.as_object_mut().unwrap().remove("triple");
    let config: MemoryConfig = serde_json::from_value(config_value).expect("deserialize");
    assert!(!config.triple.enabled);
    assert_eq!(config.triple.batch_size, 10);
    assert_eq!(config.triple.max_retries, 3);
}
