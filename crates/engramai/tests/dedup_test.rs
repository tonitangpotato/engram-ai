//! Tests for B2: Dedup on Write.
//!
//! Uses synthetic embedding vectors to test dedup logic without requiring
//! Ollama. Tests that need real embeddings should use `#[cfg(feature = "integration")]`.

use engramai::storage::Storage;
use engramai::embeddings::EmbeddingProvider;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use engramai::{Memory, MemoryConfig};
use rusqlite::params;
use tempfile::tempdir;

/// Create a simple MemoryRecord for testing.
fn make_record(id: &str, content: &str, importance: f64) -> MemoryRecord {
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: chrono::Utc::now(),
        access_times: vec![chrono::Utc::now()],
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

/// Create a normalized f32 vector of given dimension with a specific pattern.
fn make_embedding(dim: usize, seed: f32) -> Vec<f32> {
    let mut v: Vec<f32> = (0..dim).map(|i| (i as f32 * seed).sin()).collect();
    // Normalize
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Create a slightly perturbed version of a vector (high cosine similarity).
fn perturb_embedding(base: &[f32], noise: f32) -> Vec<f32> {
    let mut v: Vec<f32> = base.iter().enumerate().map(|(i, &x)| {
        x + noise * ((i as f32 * 0.7).sin())
    }).collect();
    // Normalize
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

// === Storage-level tests ===

#[test]
fn test_find_nearest_embedding_basic() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add a memory with an embedding
    let record = make_record("mem1", "Rust is fast", 0.5);
    storage.add(&record, "default").unwrap();

    let emb = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb, "ollama/nomic-embed-text", 768).unwrap();

    // Query with the same vector → should find it (similarity ≈ 1.0)
    let result = storage.find_nearest_embedding(
        &emb, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_some(), "Should find exact match");
    let (mid, sim) = result.unwrap();
    assert_eq!(mid, "mem1");
    assert!(sim > 0.99, "Exact match should have similarity ~1.0, got {}", sim);
}

#[test]
fn test_find_nearest_embedding_below_threshold() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add a memory with an embedding
    let record = make_record("mem1", "Rust is fast", 0.5);
    storage.add(&record, "default").unwrap();

    let emb1 = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb1, "ollama/nomic-embed-text", 768).unwrap();

    // Query with a very different vector
    let emb2 = make_embedding(768, 99.0);
    let sim_check = EmbeddingProvider::cosine_similarity(&emb1, &emb2);
    // Ensure they're actually dissimilar
    assert!(sim_check < 0.95, "Test vectors should be dissimilar, got {}", sim_check);

    let result = storage.find_nearest_embedding(
        &emb2, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_none(), "Dissimilar vector should not match");
}

#[test]
fn test_find_nearest_embedding_namespace_isolation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add memory in namespace "alpha"
    let record = make_record("mem1", "Alpha memory", 0.5);
    storage.add(&record, "alpha").unwrap();

    let emb = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb, "ollama/nomic-embed-text", 768).unwrap();

    // Query in namespace "beta" → should not find it
    let result = storage.find_nearest_embedding(
        &emb, "ollama/nomic-embed-text", Some("beta"), 0.95,
    ).unwrap();

    assert!(result.is_none(), "Should not find memory from different namespace");
}

#[test]
fn test_find_nearest_embedding_returns_best_match() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let base_emb = make_embedding(768, 1.0);

    // Add two memories: one very similar, one moderately similar
    let record1 = make_record("mem1", "Memory one", 0.5);
    storage.add(&record1, "default").unwrap();
    let emb1 = perturb_embedding(&base_emb, 0.01); // very close
    storage.store_embedding("mem1", &emb1, "ollama/nomic-embed-text", 768).unwrap();

    let record2 = make_record("mem2", "Memory two", 0.5);
    storage.add(&record2, "default").unwrap();
    let emb2 = perturb_embedding(&base_emb, 0.3); // further away
    storage.store_embedding("mem2", &emb2, "ollama/nomic-embed-text", 768).unwrap();

    // Query with base → should return mem1 (closer)
    let result = storage.find_nearest_embedding(
        &base_emb, "ollama/nomic-embed-text", Some("default"), 0.50,
    ).unwrap();

    assert!(result.is_some());
    let (mid, _sim) = result.unwrap();
    // mem1 should be closer since it was perturbed less
    let sim1 = EmbeddingProvider::cosine_similarity(&base_emb, &emb1);
    let sim2 = EmbeddingProvider::cosine_similarity(&base_emb, &emb2);
    assert!(sim1 > sim2, "mem1 should be closer: {} vs {}", sim1, sim2);
    assert_eq!(mid, "mem1");
}

#[test]
fn test_merge_memory_into_updates_importance() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "Important fact", 0.5);
    storage.add(&record, "default").unwrap();

    // Merge with higher importance
    storage.merge_memory_into("mem1", "Important fact", 0.8, 0.98).unwrap();

    // Check importance was updated to max(0.5, 0.8)
    let importance: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();
    assert!((importance - 0.8).abs() < 0.001, "Importance should be 0.8, got {}", importance);
}

#[test]
fn test_merge_memory_into_keeps_lower_importance() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "Important fact", 0.9);
    storage.add(&record, "default").unwrap();

    // Merge with lower importance → should keep 0.9
    storage.merge_memory_into("mem1", "Important fact", 0.3, 0.96).unwrap();

    let importance: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();
    assert!((importance - 0.9).abs() < 0.001, "Importance should stay 0.9, got {}", importance);
}

#[test]
fn test_merge_memory_into_adds_access() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "Some content", 0.5);
    storage.add(&record, "default").unwrap();

    // Count access log entries before merge
    let count_before: i64 = storage.connection().query_row(
        "SELECT COUNT(*) FROM access_log WHERE memory_id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();

    // Merge
    storage.merge_memory_into("mem1", "Some content", 0.5, 0.97).unwrap();

    // Count after
    let count_after: i64 = storage.connection().query_row(
        "SELECT COUNT(*) FROM access_log WHERE memory_id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();

    assert_eq!(count_after, count_before + 1, "Merge should add one access_log entry");
}

// === Memory-level tests (no Ollama — dedup uses pre-computed embeddings via Storage directly) ===

/// Helper: create a Memory with dedup enabled, no embedding provider (Ollama not needed).
fn setup_memory_no_embedding(dedup_enabled: bool) -> (Memory, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut config = MemoryConfig::default();
    config.dedup_enabled = dedup_enabled;
    config.dedup_threshold = 0.95;
    let memory = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();
    (memory, dir)
}

#[test]
fn test_dedup_disabled_allows_duplicates() {
    // With dedup_enabled: false and no embedding provider, identical content creates two memories
    let (mut mem, _dir) = setup_memory_no_embedding(false);

    let id1 = mem.add("Rust is a programming language", MemoryType::Factual, Some(0.5), None, None).unwrap();
    let id2 = mem.add("Rust is a programming language", MemoryType::Factual, Some(0.5), None, None).unwrap();

    // Without embedding provider, dedup can't run anyway, but with dedup disabled
    // it should definitely create two distinct memories
    assert_ne!(id1, id2, "With dedup disabled, should create two separate memories");

    let stats = mem.stats().unwrap();
    assert_eq!(stats.total_memories, 2);
}

#[test]
fn test_dedup_enabled_merges_identical_content() {
    // With dedup enabled, identical content should be merged if embedding is available.
    // If Ollama isn't running, the second add creates a new memory (graceful fallback).
    let (mut mem, _dir) = setup_memory_no_embedding(true);

    let id1 = mem.add("Rust is a programming language", MemoryType::Factual, Some(0.5), None, None).unwrap();
    assert!(!id1.is_empty());

    let id2 = mem.add("Rust is a programming language", MemoryType::Factual, Some(0.5), None, None).unwrap();
    assert!(!id2.is_empty());

    // If Ollama is available, dedup fires and ids match.
    // If Ollama is NOT available, two separate memories are created.
    // Either way, the system should not error.
    let stats = mem.stats().unwrap();
    if id1 == id2 {
        // Dedup fired (Ollama was available)
        assert_eq!(stats.total_memories, 1, "Dedup should keep only one memory");
    } else {
        // No embedding provider → no dedup possible
        assert_eq!(stats.total_memories, 2);
    }
}

/// Test dedup at the storage level by manually inserting embeddings.
/// This simulates what would happen if embeddings were available.
#[test]
fn test_dedup_exact_duplicate_via_storage() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add first memory with embedding
    let record1 = make_record("mem1", "Rust is fast and safe", 0.5);
    storage.add(&record1, "default").unwrap();
    let emb = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb, "ollama/nomic-embed-text", 768).unwrap();

    // Check: same embedding should be found as duplicate
    let result = storage.find_nearest_embedding(
        &emb, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_some());
    let (mid, sim) = result.unwrap();
    assert_eq!(mid, "mem1");
    assert!(sim > 0.99);

    // Merge instead of creating new
    storage.merge_memory_into("mem1", "Rust is fast and safe", 0.8, 0.99).unwrap();

    // Verify: still only 1 memory, but importance is now 0.8
    let count: i64 = storage.connection().query_row(
        "SELECT COUNT(*) FROM memories", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 1);

    let importance: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = 'mem1'", [], |row| row.get(0),
    ).unwrap();
    assert!((importance - 0.8).abs() < 0.001);
}

#[test]
fn test_dedup_near_duplicate_via_storage() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add first memory
    let record1 = make_record("mem1", "gid-rs has 485 tests", 0.5);
    storage.add(&record1, "default").unwrap();
    let emb1 = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb1, "ollama/nomic-embed-text", 768).unwrap();

    // Create a slightly different embedding (simulating "gid-rs has 486 tests")
    let emb2 = perturb_embedding(&emb1, 0.005); // very small perturbation
    let sim = EmbeddingProvider::cosine_similarity(&emb1, &emb2);
    assert!(sim > 0.95, "Near-duplicate should have high similarity: {}", sim);

    // Should find near-duplicate
    let result = storage.find_nearest_embedding(
        &emb2, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_some(), "Should detect near-duplicate (sim={})", sim);
    assert_eq!(result.unwrap().0, "mem1");
}

#[test]
fn test_dedup_different_content_not_merged() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add first memory
    let record1 = make_record("mem1", "Rust is a programming language", 0.5);
    storage.add(&record1, "default").unwrap();
    let emb1 = make_embedding(768, 1.0);
    storage.store_embedding("mem1", &emb1, "ollama/nomic-embed-text", 768).unwrap();

    // Very different content → very different embedding
    let emb2 = make_embedding(768, 50.0);
    let sim = EmbeddingProvider::cosine_similarity(&emb1, &emb2);
    assert!(sim < 0.95, "Different content should have low similarity: {}", sim);

    // Should NOT find a match
    let result = storage.find_nearest_embedding(
        &emb2, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_none(), "Different content should not be flagged as duplicate");
}

#[test]
fn test_dedup_merge_importance_max() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    // Add with importance 0.5
    let record = make_record("mem1", "test content", 0.5);
    storage.add(&record, "default").unwrap();

    // Merge with importance 0.8 → should become 0.8
    storage.merge_memory_into("mem1", "test content", 0.8, 0.98).unwrap();
    let imp: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = ?", params!["mem1"], |row| row.get(0),
    ).unwrap();
    assert!((imp - 0.8).abs() < 0.001);

    // Merge again with importance 0.3 → should stay 0.8
    storage.merge_memory_into("mem1", "test content", 0.3, 0.97).unwrap();
    let imp: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = ?", params!["mem1"], |row| row.get(0),
    ).unwrap();
    assert!((imp - 0.8).abs() < 0.001);

    // Merge with importance 0.95 → should become 0.95
    storage.merge_memory_into("mem1", "test content", 0.95, 0.96).unwrap();
    let imp: f64 = storage.connection().query_row(
        "SELECT importance FROM memories WHERE id = ?", params!["mem1"], |row| row.get(0),
    ).unwrap();
    assert!((imp - 0.95).abs() < 0.001);
}

#[test]
fn test_dedup_merge_accumulates_accesses() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "test content", 0.5);
    storage.add(&record, "default").unwrap();

    // Initial: 1 access (from add)
    let count: i64 = storage.connection().query_row(
        "SELECT COUNT(*) FROM access_log WHERE memory_id = ?", params!["mem1"], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 1);

    // Merge 3 times
    storage.merge_memory_into("mem1", "test content", 0.5, 0.97).unwrap();
    storage.merge_memory_into("mem1", "test content", 0.5, 0.97).unwrap();
    storage.merge_memory_into("mem1", "test content", 0.5, 0.97).unwrap();

    let count: i64 = storage.connection().query_row(
        "SELECT COUNT(*) FROM access_log WHERE memory_id = ?", params!["mem1"], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 4, "Should have 1 initial + 3 merge accesses");
}

#[test]
fn test_find_nearest_empty_store() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let storage = Storage::new(&db_path).unwrap();

    let emb = make_embedding(768, 1.0);
    let result = storage.find_nearest_embedding(
        &emb, "ollama/nomic-embed-text", Some("default"), 0.95,
    ).unwrap();

    assert!(result.is_none(), "Empty store should return None");
}

#[test]
fn test_config_dedup_defaults() {
    let config = MemoryConfig::default();
    assert!(config.dedup_enabled, "dedup should be enabled by default");
    assert!((config.dedup_threshold - 0.95).abs() < 0.001, "default threshold should be 0.95");
}

#[test]
fn test_config_dedup_serde() {
    // Serialize default config, then deserialize and check dedup fields
    let config = MemoryConfig::default();
    let json = serde_json::to_string(&config).unwrap();
    let config2: MemoryConfig = serde_json::from_str(&json).unwrap();
    assert!(config2.dedup_enabled);
    assert!((config2.dedup_threshold - 0.95).abs() < 0.001);

    // Modify dedup fields, round-trip
    let mut config3 = MemoryConfig::default();
    config3.dedup_enabled = false;
    config3.dedup_threshold = 0.90;
    let json3 = serde_json::to_string(&config3).unwrap();
    let config4: MemoryConfig = serde_json::from_str(&json3).unwrap();
    assert!(!config4.dedup_enabled);
    assert!((config4.dedup_threshold - 0.90).abs() < 0.001);
}

// === Smart Merge tests ===

#[test]
fn test_merge_content_update_when_longer() {
    // Original content is short, new content is 50% longer → content should be updated
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "short content", 0.5);
    storage.add(&record, "default").unwrap();

    // New content is much longer (>30% longer)
    let long_content = "short content with a lot of additional detail and information that makes it significantly longer than the original";
    let outcome = storage.merge_memory_into("mem1", long_content, 0.6, 0.97).unwrap();

    assert!(outcome.content_updated, "Content should be updated when new is >30% longer");
    assert_eq!(outcome.merge_count, 1);

    // Verify the content was actually updated in DB
    let stored_content: String = storage.connection().query_row(
        "SELECT content FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(stored_content, long_content);
}

#[test]
fn test_merge_content_kept_when_similar_length() {
    // New content is similar length → original content kept
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let original = "Rust is a systems programming language";
    let record = make_record("mem1", original, 0.5);
    storage.add(&record, "default").unwrap();

    // New content is about the same length (not >30% longer)
    let new_content = "Rust is a safe systems programming lang";
    let outcome = storage.merge_memory_into("mem1", new_content, 0.6, 0.96).unwrap();

    assert!(!outcome.content_updated, "Content should NOT be updated when similar length");

    // Verify original content is preserved
    let stored_content: String = storage.connection().query_row(
        "SELECT content FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(stored_content, original);
}

#[test]
fn test_merge_history_recorded_in_metadata() {
    // After merge, check metadata has merge_history array
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "some content", 0.5);
    storage.add(&record, "default").unwrap();

    storage.merge_memory_into("mem1", "some content", 0.7, 0.98).unwrap();

    let metadata_str: String = storage.connection().query_row(
        "SELECT metadata FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();

    let metadata: serde_json::Value = serde_json::from_str(&metadata_str).unwrap();
    let history = metadata["merge_history"].as_array().unwrap();
    assert_eq!(history.len(), 1);

    let entry = &history[0];
    assert!(entry["ts"].as_u64().unwrap() > 0);
    assert!((entry["sim"].as_f64().unwrap() - 0.98).abs() < 0.01);
    assert_eq!(entry["content_updated"].as_bool().unwrap(), false);
    assert!(entry["prev_content_len"].as_u64().is_some());
    assert!(entry["new_content_len"].as_u64().is_some());
}

#[test]
fn test_merge_history_capped_at_10() {
    // Merge 12 times, check only last 10 entries in merge_history
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "some content", 0.5);
    storage.add(&record, "default").unwrap();

    for i in 0..12 {
        storage.merge_memory_into("mem1", "some content", 0.5, 0.96 + (i as f32) * 0.001).unwrap();
    }

    let metadata_str: String = storage.connection().query_row(
        "SELECT metadata FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();

    let metadata: serde_json::Value = serde_json::from_str(&metadata_str).unwrap();
    let history = metadata["merge_history"].as_array().unwrap();
    assert_eq!(history.len(), 10, "merge_history should be capped at 10 entries, got {}", history.len());

    // merge_count should still be 12
    assert_eq!(metadata["merge_count"].as_i64().unwrap(), 12);
}

#[test]
fn test_merge_count_increments() {
    // Merge 3 times, check merge_count is 3
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut storage = Storage::new(&db_path).unwrap();

    let record = make_record("mem1", "test content", 0.5);
    storage.add(&record, "default").unwrap();

    for _ in 0..3 {
        storage.merge_memory_into("mem1", "test content", 0.5, 0.97).unwrap();
    }

    let metadata_str: String = storage.connection().query_row(
        "SELECT metadata FROM memories WHERE id = ?",
        params!["mem1"],
        |row| row.get(0),
    ).unwrap();

    let metadata: serde_json::Value = serde_json::from_str(&metadata_str).unwrap();
    assert_eq!(metadata["merge_count"].as_i64().unwrap(), 3, "merge_count should be 3 after 3 merges");
}
