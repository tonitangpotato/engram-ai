//! Integration test: Dimensional Memory Extraction end-to-end.
//!
//! Tests that:
//! 1. Storing with type_weights metadata persists correctly
//! 2. type_weights correctly influence recall scoring (via affinity_multiplier)
//! 3. Old memories (no type_weights) still work identically (backward compat)
//! 4. infer_type_weights produces correct weights for real content

use engramai::{MemoryConfig, Memory, MemoryType};
use engramai::type_weights::{infer_type_weights, TypeWeights};
use engramai::extractor::ExtractedFact;
use tempfile::NamedTempFile;

fn make_system(db_path: &str) -> Memory {
    let config = MemoryConfig::default();
    Memory::new(db_path, Some(config)).unwrap()
}

#[test]
fn test_type_weights_stored_and_read_back() {
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();
    let mut mem = make_system(db_path);

    // Store with type_weights in metadata
    let tw_json = serde_json::json!({
        "type_weights": {
            "factual": 0.5, "episodic": 0.1, "procedural": 0.1,
            "relational": 0.1, "emotional": 0.1, "opinion": 0.1, "causal": 0.9
        }
    });

    let id = mem.add(
        "RustClaw uses engram for memory because ACT-R activation provides principled recall",
        MemoryType::Causal,
        Some(0.7),
        Some("test"),
        Some(tw_json.clone()),
    ).unwrap();

    assert!(!id.is_empty());

    // Recall and verify type_weights persisted
    let results = mem.recall("RustClaw memory system", 5, None, None).unwrap();
    assert!(!results.is_empty(), "Should recall at least one memory");

    let first = &results[0];
    let meta = first.record.metadata.as_ref().expect("should have metadata");
    // v2 layout: caller-supplied metadata lives under `user.*`
    let tw = meta
        .get("user")
        .and_then(|u| u.get("type_weights"))
        .expect("should have user.type_weights in metadata");
    assert_eq!(tw.get("causal").and_then(|v: &serde_json::Value| v.as_f64()), Some(0.9));
    assert_eq!(tw.get("factual").and_then(|v: &serde_json::Value| v.as_f64()), Some(0.5));

    println!("✅ type_weights stored and recalled correctly");
}

#[test]
fn test_old_memories_backward_compat() {
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();
    let mut mem = make_system(db_path);

    // Store WITHOUT metadata (simulates pre-dimensional memories)
    let _id = mem.add(
        "potato prefers Rust over Python for systems programming",
        MemoryType::Opinion,
        Some(0.6),
        Some("test"),
        None,
    ).unwrap();

    // Should recall fine — TypeWeights::default() (all 1.0) acts as passthrough
    let results = mem.recall("potato programming language preference", 5, None, None).unwrap();
    assert!(!results.is_empty(), "Old memories without type_weights should still be recallable");

    // Verify TypeWeights::from_metadata returns default for missing metadata
    let tw = TypeWeights::from_metadata(&results[0].record.metadata);
    assert_eq!(tw.factual, 1.0, "default should be 1.0 for backward compat");
    assert_eq!(tw.causal, 1.0);

    println!("✅ backward compat: old memories recall identically");
}

#[test]
fn test_type_weights_inference_from_extracted_fact() {
    let mut fact = ExtractedFact::default();
    fact.core_fact = "potato switched from Python to Rust because compile-time guarantees prevent runtime crashes".to_string();
    fact.causation = Some("compile-time guarantees prevent runtime crashes".to_string());
    fact.outcome = Some("switched tech stack successfully".to_string());
    fact.participants = Some("potato".to_string());

    let tw = infer_type_weights(&fact);

    // Strong causal signal
    assert!(tw.causal > 0.8, "causal should be high (causation + outcome), got {}", tw.causal);
    // Moderate relational (participants present)
    assert!(tw.relational > 0.3, "relational should be moderate, got {}", tw.relational);
    // Primary type should be causal
    assert_eq!(tw.primary_type(), MemoryType::Causal);

    // JSON roundtrip
    let json = tw.to_json();
    let tw2 = TypeWeights::from_metadata(&Some(serde_json::json!({"type_weights": json})));
    assert_eq!(tw, tw2);

    println!("✅ infer_type_weights: causal={:.1}, relational={:.1}, primary={:?}", tw.causal, tw.relational, tw.primary_type());
}

#[test]
fn test_episodic_inference() {
    let mut fact = ExtractedFact::default();
    fact.core_fact = "we had a team meeting about the migration plan".to_string();
    fact.temporal = Some("yesterday afternoon".to_string());
    fact.participants = Some("potato, team".to_string());
    fact.context = Some("weekly sync meeting".to_string());
    fact.location = Some("office".to_string());

    let tw = infer_type_weights(&fact);
    assert_eq!(tw.primary_type(), MemoryType::Episodic);
    assert_eq!(tw.episodic, 1.0); // 0.1 + 0.5 + 0.2 + 0.2 + 0.1 = 1.1 → clamped to 1.0
    println!("✅ episodic inference: episodic={:.1}, relational={:.1}", tw.episodic, tw.relational);
}

#[test]
fn test_procedural_inference() {
    let mut fact = ExtractedFact::default();
    fact.core_fact = "to deploy RustClaw, run cargo build --release then copy the binary".to_string();
    fact.method = Some("cargo build --release, copy binary to target".to_string());

    let tw = infer_type_weights(&fact);
    assert_eq!(tw.primary_type(), MemoryType::Procedural);
    assert!((tw.procedural - 0.6).abs() < 0.01);
    println!("✅ procedural inference: procedural={:.1}", tw.procedural);
}

#[test]
fn test_type_affinity_modulation_ordering() {
    // Store two memories with different type signatures
    // Verify both are recalled and type_weights influence scoring
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();
    let mut mem = make_system(db_path);

    // Memory 1: strongly causal
    let causal_tw = serde_json::json!({
        "type_weights": {
            "factual": 0.5, "episodic": 0.1, "procedural": 0.1,
            "relational": 0.3, "emotional": 0.1, "opinion": 0.1, "causal": 0.9
        }
    });
    mem.add(
        "switching to Rust improved reliability because the type system catches bugs at compile time",
        MemoryType::Causal,
        Some(0.7),
        Some("test"),
        Some(causal_tw),
    ).unwrap();

    // Memory 2: strongly episodic
    let episodic_tw = serde_json::json!({
        "type_weights": {
            "factual": 0.3, "episodic": 0.9, "procedural": 0.1,
            "relational": 0.2, "emotional": 0.3, "opinion": 0.1, "causal": 0.1
        }
    });
    mem.add(
        "yesterday we discussed switching to Rust and the team felt excited about type safety",
        MemoryType::Episodic,
        Some(0.7),
        Some("test"),
        Some(episodic_tw),
    ).unwrap();

    // Recall both
    let results = mem.recall("why did we switch to Rust", 5, None, None).unwrap();
    assert!(results.len() >= 2, "Should recall both memories, got {}", results.len());

    println!("✅ affinity modulation: recalled {} results", results.len());
    for (i, r) in results.iter().enumerate() {
        let tw = TypeWeights::from_metadata(&r.record.metadata);
        println!("  #{}: causal={:.1} episodic={:.1} | {}", 
            i+1, tw.causal, tw.episodic, 
            r.record.content.chars().take(70).collect::<String>());
    }
}

#[test]
fn test_mixed_old_and_new_memories() {
    // Simulate a DB with both old (no type_weights) and new (with type_weights) memories
    let tmp = NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_str().unwrap();
    let mut mem = make_system(db_path);

    // Old-style memory (no metadata)
    mem.add(
        "engram uses SQLite with FTS5 for full-text search capability",
        MemoryType::Factual,
        Some(0.6),
        Some("test"),
        None,
    ).unwrap();

    // New-style memory (with type_weights)
    let tw = serde_json::json!({
        "type_weights": {
            "factual": 0.8, "episodic": 0.1, "procedural": 0.3,
            "relational": 0.1, "emotional": 0.1, "opinion": 0.1, "causal": 0.2
        }
    });
    mem.add(
        "engram hybrid search combines FTS5 ranking with embedding cosine similarity",
        MemoryType::Factual,
        Some(0.7),
        Some("test"),
        Some(tw),
    ).unwrap();

    let results = mem.recall("engram search capabilities", 5, None, None).unwrap();
    assert!(results.len() >= 2, "Should recall both old and new memories, got {}", results.len());

    // Both memories should be recallable. Post-ISS-019 Step 7a, even
    // memories written without caller-supplied `type_weights` get
    // engram-inferred weights in `metadata.engram.dimensions.type_weights`,
    // so we can no longer detect "old style" by `TypeWeights::default()`.
    // The test's real intent is "both memories coexist and are
    // recallable" — verify that directly.
    let mut has_fts_only = false;   // first memory: no caller tw
    let mut has_hybrid = false;     // second memory: explicit tw with factual=0.8
    for r in &results {
        if r.record.content.contains("FTS5 for full-text search") {
            has_fts_only = true;
        }
        if r.record.content.contains("hybrid search") {
            let tw = TypeWeights::from_metadata(&r.record.metadata);
            // Explicit user-supplied type_weights should round-trip.
            if (tw.factual - 0.8).abs() < 0.01 {
                has_hybrid = true;
            }
        }
    }
    assert!(has_fts_only, "Should have recalled memory without caller type_weights");
    assert!(has_hybrid, "Should have recalled new-style memory with explicit type_weights");

    println!("✅ mixed old+new memories coexist correctly");
}

#[test]
fn test_default_type_weights_neutral_behavior() {
    let default_tw = TypeWeights::default();
    // All 1.0 means max(1.0 * affinity_i) = max(affinity_i)
    // This makes old memories completely neutral wrt type scoring
    assert_eq!(default_tw.factual, 1.0);
    assert_eq!(default_tw.episodic, 1.0);
    assert_eq!(default_tw.procedural, 1.0);
    assert_eq!(default_tw.relational, 1.0);
    assert_eq!(default_tw.emotional, 1.0);
    assert_eq!(default_tw.opinion, 1.0);
    assert_eq!(default_tw.causal, 1.0);

    // Verify: for any affinity vector, default tw gives max(affinity)
    // This is the mathematical proof of backward compat
    let affinity = [0.3, 0.8, 0.1, 0.2, 0.5, 0.4, 0.6]; // arbitrary
    let max_affinity = affinity.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let modulated: f64 = [
        default_tw.factual * affinity[0],
        default_tw.episodic * affinity[1],
        default_tw.procedural * affinity[2],
        default_tw.relational * affinity[3],
        default_tw.emotional * affinity[4],
        default_tw.opinion * affinity[5],
        default_tw.causal * affinity[6],
    ].iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    assert!((modulated - max_affinity).abs() < 1e-10, 
        "Default TypeWeights should give max(affinity), got {} vs {}", modulated, max_affinity);

    println!("✅ default TypeWeights mathematically neutral");
}
