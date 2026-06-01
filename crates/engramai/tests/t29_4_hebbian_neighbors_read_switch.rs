//! T29.4 part-1 — `get_hebbian_neighbors` read-switch contract.
//!
//! Per design §5.4, the `unified_substrate` flag flips retrieval
//! adapters from legacy tables to `nodes`/`edges`. This file pins
//! that `get_hebbian_neighbors` returns the same **neighbour set**
//! under both substrates after enough co-activations to cross the
//! formed-link threshold.
//!
//! **Why set-equality, not byte-equality** (design §4.3, §5.4):
//! Phase B dual-write (`dual_write_hebbian_to_edges`, ISS-116) is
//! unconditional — every `record_coactivation*` call adds
//! `delta_weight=0.1` to the unified `edges` row, including
//! tracking-phase calls where the legacy `hebbian_links` row stays
//! at `strength=0.0`. Both readers filter `> 0`, so:
//!
//!   * Legacy `WHERE strength > 0` hides tracking-phase pairs.
//!   * Unified `WHERE weight > 0` surfaces tracking-phase pairs.
//!
//! Below threshold, the two paths return different sets **by design**.
//! Once the threshold is crossed, both paths agree on the set of
//! formed-link neighbours, which is what this contract tests.
//!
//! Acceptance contract:
//!
//!   1. After enough `record_coactivation` calls to cross threshold,
//!      `unified_substrate=true` and `=false` return the same
//!      neighbour set for the same memory.
//!   2. Ordering is not guaranteed (HashSet equality, not Vec).
//!   3. Multi-neighbour case: a memory with three formed neighbours
//!      returns all three on both substrates.
//!   4. The flag only changes reads — both substrates contain the
//!      same data via the writer dual-write.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use std::collections::HashSet;
use tempfile::tempdir;

fn seed_memory(storage: &mut Storage, id: &str) {
    let rec = MemoryRecord {
        id: id.into(),
        content: format!("content-{id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "t29.4-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, "default").expect("seed memory");
}

#[test]
fn t29_4_get_hebbian_neighbors_single_formed_link_matches() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
        // Threshold-cross: 3 calls to record_coactivation with threshold=3.
        for _ in 0..3 {
            storage.record_coactivation("a", "b", 3).unwrap();
        }
    }

    // Legacy reader.
    let legacy = {
        let storage = Storage::new(&db_path).unwrap();
        storage.get_hebbian_neighbors("a").unwrap()
    };

    // Unified reader.
    let unified = {
        let storage = Storage::with_unified_substrate(&db_path, true).unwrap();
        storage.get_hebbian_neighbors("a").unwrap()
    };

    let legacy_set: HashSet<String> = legacy.into_iter().collect();
    let unified_set: HashSet<String> = unified.into_iter().collect();

    assert_eq!(
        legacy_set, unified_set,
        "neighbour sets must match across substrates after formed link"
    );
    assert_eq!(legacy_set, ["b".to_string()].into_iter().collect());
}

#[test]
fn t29_4_get_hebbian_neighbors_multi_neighbour_matches() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        for id in ["a", "b", "c", "d"] {
            seed_memory(&mut storage, id);
        }
        for _ in 0..3 {
            storage.record_coactivation("a", "b", 3).unwrap();
            storage.record_coactivation("a", "c", 3).unwrap();
            storage.record_coactivation("a", "d", 3).unwrap();
        }
    }

    let legacy: HashSet<String> = Storage::new(&db_path)
        .unwrap()
        .get_hebbian_neighbors("a")
        .unwrap()
        .into_iter()
        .collect();

    let unified: HashSet<String> = Storage::with_unified_substrate(&db_path, true)
        .unwrap()
        .get_hebbian_neighbors("a")
        .unwrap()
        .into_iter()
        .collect();

    assert_eq!(legacy, unified, "multi-neighbour set must match");
    let expected: HashSet<String> = ["b", "c", "d"].into_iter().map(String::from).collect();
    assert_eq!(legacy, expected);
}

#[test]
fn t29_4_get_hebbian_neighbors_or_match_both_endpoints() {
    // Both `get_hebbian_neighbors(a)` and `get_hebbian_neighbors(b)`
    // must return the other endpoint, on both substrates. This
    // exercises the OR-match SQL on both legacy and unified paths.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
        for _ in 0..3 {
            storage.record_coactivation("a", "b", 3).unwrap();
        }
    }

    let legacy = Storage::new(&db_path).unwrap();
    let unified = Storage::with_unified_substrate(&db_path, true).unwrap();

    for memory_id in ["a", "b"] {
        let other = if memory_id == "a" { "b" } else { "a" };
        let legacy_set: HashSet<String> = legacy
            .get_hebbian_neighbors(memory_id)
            .unwrap()
            .into_iter()
            .collect();
        let unified_set: HashSet<String> = unified
            .get_hebbian_neighbors(memory_id)
            .unwrap()
            .into_iter()
            .collect();
        let expected: HashSet<String> = [other.to_string()].into_iter().collect();
        assert_eq!(legacy_set, expected, "legacy for {memory_id}");
        assert_eq!(unified_set, expected, "unified for {memory_id}");
    }
}

#[test]
fn t29_4_get_hebbian_neighbors_unified_surfaces_tracking_phase() {
    // Below threshold, legacy hides the pair (strength stays 0.0)
    // but unified surfaces it (weight accumulated from dual-write).
    // This pins the documented Phase D divergence — design §5.4
    // "unified semantics is the destination".
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
        // Only 1 coactivation — well below threshold=3. Legacy
        // strength stays 0, but unified weight = 0.1.
        storage.record_coactivation("a", "b", 3).unwrap();
    }

    let legacy: HashSet<String> = Storage::new(&db_path)
        .unwrap()
        .get_hebbian_neighbors("a")
        .unwrap()
        .into_iter()
        .collect();

    let unified: HashSet<String> = Storage::with_unified_substrate(&db_path, true)
        .unwrap()
        .get_hebbian_neighbors("a")
        .unwrap()
        .into_iter()
        .collect();

    assert!(
        legacy.is_empty(),
        "legacy must hide sub-threshold pairs (strength = 0); got {legacy:?}"
    );
    assert_eq!(
        unified,
        ["b".to_string()].into_iter().collect::<HashSet<_>>(),
        "unified must surface tracking-phase pair (weight = 0.1)"
    );
}
