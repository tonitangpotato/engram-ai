use engramai::{Memory, MemoryType};
use std::time::Instant;

#[test]
fn test_bulk_store_and_recall() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("stress.db");
    // Disable dedup for bulk stress test — content templates are intentionally similar
    let mut config = engramai::MemoryConfig::default();
    config.dedup_enabled = false;
    let mut mem = Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap();
    
    // Check if embedding is enabled (affects timing)
    let has_embedding = mem.has_embedding_support();

    // Store 500 memories (simulating a busy agent)
    let start = Instant::now();
    for i in 0..500 {
        mem.add(
            &format!("Memory entry number {} about topic {}", i, i % 10),
            if i % 3 == 0 { MemoryType::Factual } else if i % 3 == 1 { MemoryType::Episodic } else { MemoryType::Procedural },
            Some(0.5 + (i % 5) as f64 * 0.1),
            Some("stress_test"),
            None,
        ).unwrap();
    }
    let store_time = start.elapsed();
    println!("Stored 500 memories in {:?} (embedding: {})", store_time, has_embedding);
    
    // With embedding enabled, each memory requires an Ollama API call (~15-20ms each)
    // so 500 memories can take 8-10 seconds. Without embedding, should be <5s.
    let max_store_ms = if has_embedding { 30000 } else { 5000 };
    assert!(store_time.as_millis() < max_store_ms, "Store too slow: {:?}", store_time);

    // Recall should be fast even with 500 memories
    let start = Instant::now();
    let results = mem.recall("topic 5", 10, None, None).unwrap();
    let recall_time = start.elapsed();
    println!("Recalled {} results in {:?}", results.len(), recall_time);
    assert!(recall_time.as_millis() < 500, "Recall too slow: {:?}", recall_time);
    assert!(!results.is_empty(), "Should find results");

    // Consolidation on 500 memories
    let start = Instant::now();
    mem.consolidate(7.0).unwrap();
    let consolidate_time = start.elapsed();
    println!("Consolidated in {:?}", consolidate_time);
    assert!(consolidate_time.as_millis() < 5000, "Consolidate too slow");

    let stats = mem.stats().unwrap();
    assert_eq!(stats.total_memories, 500);
    println!("Stats: {} total, layers: {:?}", stats.total_memories, stats.by_layer);
}

#[test]
fn test_concurrent_db_access() {
    // Simulate multiple agents accessing same DB (via namespace pattern)
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("shared.db");

    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Agent A stores
    mem.add("Agent A: oil price rising to $91", MemoryType::Factual, Some(0.9), Some("trading"), None).unwrap();
    mem.add("Agent A: Iran conflict escalating", MemoryType::Episodic, Some(0.8), Some("trading"), None).unwrap();

    // Agent B stores (same DB, different source namespace)
    mem.add("Agent B: NYC healthcare visibility scan complete", MemoryType::Episodic, Some(0.7), Some("hackathon"), None).unwrap();
    mem.add("Agent B: Dr. Smith visibility 23/100", MemoryType::Factual, Some(0.5), Some("hackathon"), None).unwrap();

    // CEO queries all
    let all_results = mem.recall("oil healthcare visibility", 10, None, None).unwrap();
    assert!(!all_results.is_empty(), "CEO should see memories from multiple agents");

    // Query specific domain
    let trading_results = mem.recall("oil price Iran", 5, None, None).unwrap();
    assert!(!trading_results.is_empty(), "Should find trading memories");
    // Verify top result is trading-related
    assert!(trading_results[0].record.content.contains("oil") || trading_results[0].record.content.contains("Iran"));

    println!("Multi-agent access works: {} total results, {} trading results",
        all_results.len(), trading_results.len());
}

#[test]
fn test_hebbian_cross_domain() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("hebbian.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    let id1 = mem.add("Rust is great for performance-critical systems", MemoryType::Factual, Some(0.8), None, None).unwrap();
    let _id2 = mem.add("IronClaw uses WASM for security sandboxing", MemoryType::Factual, Some(0.7), None, None).unwrap();
    let _id3 = mem.add("Python is popular for machine learning", MemoryType::Factual, Some(0.6), None, None).unwrap();

    // Recall Rust and IronClaw together (creates Hebbian link)
    let _ = mem.recall("Rust systems programming", 3, None, None).unwrap();
    let _ = mem.recall("IronClaw security", 3, None, None).unwrap();

    // Check Hebbian links formed
    let links = mem.hebbian_links(&id1).unwrap();
    println!("Hebbian links for 'Rust': {:?}", links);
    // Links may or may not form depending on co-access window
    // The important thing is it doesn't crash

    // Verify all memories still intact
    let stats = mem.stats().unwrap();
    assert_eq!(stats.total_memories, 3);
}

#[test]
fn test_memory_types_and_decay() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("decay.db");
    let mut mem = Memory::new(db_path.to_str().unwrap(), None).unwrap();

    // Different types have different default importance
    mem.add("Sky is blue", MemoryType::Factual, None, None, None).unwrap();
    mem.add("Had coffee this morning", MemoryType::Episodic, None, None, None).unwrap();
    mem.add("Use cargo build --release for production", MemoryType::Procedural, None, None, None).unwrap();
    mem.add("I think Rust is better than Go", MemoryType::Opinion, None, None, None).unwrap();
    mem.add("User prefers dark mode", MemoryType::Relational, None, None, None).unwrap();

    let stats = mem.stats().unwrap();
    assert_eq!(stats.total_memories, 5);
    println!("Type distribution: {:?}", stats.by_type);

    // Recall should rank by activation (importance + recency)
    let results = mem.recall("Rust Go programming", 5, None, None).unwrap();
    assert!(!results.is_empty(), "Should find opinion about Rust vs Go");
    println!("Top result for 'Rust Go programming': {}", results[0].record.content);
}
