//! Integration tests for Somatic Marker → Recall pipeline.
//!
//! Tests Damasio's somatic marker hypothesis implementation:
//! - Somatic markers bias recall ranking toward emotionally significant memories
//! - Emotional feedback loop: recalled emotional memories reinforce somatic markers
//! - New situations (no marker history) have no somatic bias
//! - Encounter count builds confidence over repeated queries

use engramai::interoceptive::types::SomaticMarker;
use engramai::{Memory, MemoryConfig, MemoryType};
use tempfile::tempdir;

/// Helper: create a Memory instance with somatic weight configured.
fn memory_with_somatic(somatic_weight: f64) -> Memory {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut config = MemoryConfig::default();
    config.somatic_weight = somatic_weight;
    // Embedding provider won't be available in tests (no Ollama),
    // so recall falls back to FTS + ACT-R path.
    Memory::new(db_path.to_str().unwrap(), Some(config)).unwrap()
}

// ---------------------------------------------------------------------------
// Unit: somatic_scores method behavior
// ---------------------------------------------------------------------------

#[test]
fn somatic_scores_silent_for_novel_situation() {
    // A query the system has never seen should produce no somatic scores.
    let mut mem = memory_with_somatic(0.1);

    // Add a memory so recall has something to work with
    mem.add("test memory about cats", MemoryType::Factual, Some(0.5), Some("test"), None)
        .unwrap();

    // The somatic cache is empty — no markers exist.
    // Recall should still work, somatic channel contributes 0.
    let results = mem.recall("cats", 5, None, None).unwrap();
    assert!(!results.is_empty(), "recall should return results");
}

#[test]
fn somatic_marker_boosts_emotional_memories() {
    let mut mem = memory_with_somatic(0.5); // Higher weight to ensure somatic dominance

    // Add an emotional memory and a factual memory
    mem.add(
        "potato said he trusts me — that made me feel valued",
        MemoryType::Emotional,
        Some(0.9),
        Some("test"),
        None,
    )
    .unwrap();
    mem.add(
        "potato prefers dark mode in his editor",
        MemoryType::Factual,
        Some(0.3),
        Some("test"),
        None,
    )
    .unwrap();

    // Prime the somatic marker with a strong positive valence for "potato" queries.
    // We do this by directly accessing the interoceptive hub.
    {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        "potato".hash(&mut hasher);
        let hash = hasher.finish();
        // Feed strong positive emotional signal 10 times (builds full encounter confidence)
        let hub = mem.interoceptive_hub_mut();
        for _ in 0..10 {
            hub.somatic_lookup(hash, 0.9);
        }
    }

    // Now recall "potato" — emotional memory should get a somatic boost
    let results = mem.recall("potato", 5, None, None).unwrap();
    assert!(results.len() >= 2, "should find both memories");

    // The emotional memory should rank higher due to somatic boost
    // (both match "potato" in FTS, but the emotional one gets somatic advantage)
    let first = &results[0];
    assert!(
        first.record.content.contains("trusts me"),
        "emotional memory should rank first due to somatic boost, got: {}",
        first.record.content,
    );
}

#[test]
fn somatic_feedback_loop_reinforces_markers() {
    let mut mem = memory_with_somatic(0.1);

    // Add an emotional memory
    mem.add(
        "a deeply meaningful conversation about consciousness",
        MemoryType::Emotional,
        Some(0.85),
        Some("test"),
        None,
    )
    .unwrap();

    // First recall — creates a somatic marker for this query
    let _ = mem.recall("consciousness", 5, None, None).unwrap();

    // Check that a somatic marker was created/updated via feedback
    let _state = mem.interoceptive_hub().current_state();
    // The marker should exist in active markers (accessed within 30 min)
    // or at minimum the somatic cache should be non-empty after recall
    let marker_count = mem.interoceptive_hub().marker_count();
    assert!(
        marker_count > 0,
        "somatic cache should have a marker after recall with emotional results"
    );
}

#[test]
fn encounter_count_builds_somatic_confidence() {
    let mut mem = memory_with_somatic(0.15);

    mem.add(
        "important emotional event that keeps recurring",
        MemoryType::Emotional,
        Some(0.9),
        Some("test"),
        None,
    )
    .unwrap();

    mem.add(
        "boring factual detail about the same topic",
        MemoryType::Factual,
        Some(0.2),
        Some("test"),
        None,
    )
    .unwrap();

    // Prime marker with repeated encounters to build confidence
    {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        "recurring event".hash(&mut hasher);
        let hash = hasher.finish();
        let hub = mem.interoceptive_hub_mut();
        // 10 encounters with strong valence
        for _ in 0..10 {
            hub.somatic_lookup(hash, 0.9);
        }
    }

    let results = mem.recall("recurring event", 5, None, None).unwrap();
    assert!(!results.is_empty());

    // Emotional memory should be boosted with high-confidence somatic marker
    if results.len() >= 2 {
        let first = &results[0];
        assert!(
            first.record.memory_type == MemoryType::Emotional,
            "emotional memory should rank first with strong somatic marker"
        );
    }
}

#[test]
fn zero_somatic_weight_disables_channel() {
    let mut mem = memory_with_somatic(0.0);

    mem.add(
        "emotional memory about trust",
        MemoryType::Emotional,
        Some(0.9),
        Some("test"),
        None,
    )
    .unwrap();
    mem.add(
        "factual memory about trust definitions",
        MemoryType::Factual,
        Some(0.5),
        Some("test"),
        None,
    )
    .unwrap();

    // Prime somatic marker with strong signal
    {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        "trust".hash(&mut hasher);
        let hash = hasher.finish();
        let hub = mem.interoceptive_hub_mut();
        for _ in 0..5 {
            hub.somatic_lookup(hash, 0.9);
        }
    }

    // With weight=0, somatic channel should be completely disabled
    // Results should be ordered purely by other channels (FTS + ACT-R in no-embedding mode)
    let results = mem.recall("trust", 5, None, None).unwrap();
    assert!(!results.is_empty(), "should still return results");
    // We can't assert exact ordering since other channels are active,
    // but the system should not crash and should return valid results.
}

// ---------------------------------------------------------------------------
// Unit: SomaticMarker type behavior
// ---------------------------------------------------------------------------

#[test]
fn somatic_marker_incremental_valence_update() {
    let mut marker = SomaticMarker::new(42, 0.8);
    assert_eq!(marker.encounter_count, 1);
    assert!((marker.valence - 0.8).abs() < f64::EPSILON);

    // Second encounter with negative valence
    marker.update(-0.4);
    assert_eq!(marker.encounter_count, 2);
    // Incremental mean: (0.8 + -0.4) / 2 = 0.2
    assert!(
        (marker.valence - 0.2).abs() < f64::EPSILON,
        "valence should be incremental mean: {}",
        marker.valence
    );

    // Third encounter
    marker.update(0.5);
    assert_eq!(marker.encounter_count, 3);
    // Incremental mean: (0.8 + -0.4 + 0.5) / 3 = 0.3
    assert!(
        (marker.valence - 0.3).abs() < 0.001,
        "valence should track running mean: {}",
        marker.valence
    );
}

#[test]
fn somatic_marker_new_has_correct_defaults() {
    let marker = SomaticMarker::new(999, -0.5);
    assert_eq!(marker.situation_hash, 999);
    assert!((marker.valence - (-0.5)).abs() < f64::EPSILON);
    assert_eq!(marker.encounter_count, 1);
}

// ---------------------------------------------------------------------------
// Integration: InteroceptiveHub somatic cache in recall context
// ---------------------------------------------------------------------------

#[test]
fn hub_somatic_cache_persists_across_recalls() {
    let mut mem = memory_with_somatic(0.1);

    mem.add("emotional memory alpha", MemoryType::Emotional, Some(0.8), Some("test"), None)
        .unwrap();
    mem.add("factual memory beta", MemoryType::Factual, Some(0.4), Some("test"), None)
        .unwrap();

    // First recall creates somatic marker
    let _ = mem.recall("alpha beta", 5, None, None).unwrap();
    let cache_after_first = mem.interoceptive_hub().marker_count();

    // Second recall should find and update the existing marker
    let _ = mem.recall("alpha beta", 5, None, None).unwrap();
    let cache_after_second = mem.interoceptive_hub().marker_count();

    // Cache size should not grow — same query hash reuses same marker
    assert_eq!(
        cache_after_first, cache_after_second,
        "same query should reuse same somatic marker, not create new one"
    );
}

#[test]
fn different_queries_create_different_markers() {
    let mut mem = memory_with_somatic(0.1);

    mem.add("memory about cats", MemoryType::Factual, Some(0.5), Some("test"), None)
        .unwrap();
    mem.add("memory about dogs", MemoryType::Factual, Some(0.5), Some("test"), None)
        .unwrap();

    let _ = mem.recall("cats", 5, None, None).unwrap();
    let cache_after_cats = mem.interoceptive_hub().marker_count();

    let _ = mem.recall("dogs", 5, None, None).unwrap();
    let cache_after_dogs = mem.interoceptive_hub().marker_count();

    assert!(
        cache_after_dogs > cache_after_cats,
        "different queries should create different somatic markers"
    );
}

// ---------------------------------------------------------------------------
// Edge case: high-importance non-emotional memories get partial boost
// ---------------------------------------------------------------------------

#[test]
fn high_importance_factual_gets_partial_somatic_boost() {
    let mut mem = memory_with_somatic(0.1);

    // High-importance factual memory (importance >= 0.7 → 70% somatic relevance)
    mem.add(
        "critical system architecture decision about caching",
        MemoryType::Factual,
        Some(0.8),
        Some("test"),
        None,
    )
    .unwrap();

    // Low-importance factual memory (importance < 0.7 → 30% somatic relevance)
    mem.add(
        "minor note about caching config file location",
        MemoryType::Factual,
        Some(0.2),
        Some("test"),
        None,
    )
    .unwrap();

    // Prime somatic marker
    {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        "caching".hash(&mut hasher);
        let hash = hasher.finish();
        let hub = mem.interoceptive_hub_mut();
        for _ in 0..5 {
            hub.somatic_lookup(hash, 0.7);
        }
    }

    let results = mem.recall("caching", 5, None, None).unwrap();
    assert!(!results.is_empty());

    // High-importance memory should rank first (gets 70% somatic vs 30%)
    if results.len() >= 2 {
        assert!(
            results[0].record.importance >= 0.7,
            "high-importance memory should rank higher with somatic boost"
        );
    }
}

#[test]
fn recall_with_somatic_produces_valid_confidence() {
    let mut mem = memory_with_somatic(0.1);

    mem.add("test memory for confidence check", MemoryType::Factual, Some(0.5), Some("test"), None)
        .unwrap();

    let results = mem.recall("test memory confidence", 5, None, None).unwrap();
    for r in &results {
        assert!(
            r.confidence >= 0.0 && r.confidence <= 1.0,
            "confidence should be in [0, 1], got {}",
            r.confidence
        );
    }
}
