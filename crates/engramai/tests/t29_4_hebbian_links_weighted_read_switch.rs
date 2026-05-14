//! T29.4 part-2 — `get_hebbian_links_weighted` read-switch contract.
//!
//! Same flag-gated switch as part-1, but returns `(neighbour, weight)`
//! tuples. Per design §4.3 the **weight values diverge** between
//! substrates (legacy `strength` caps at 1.0, unified `weight`
//! accumulates without cap), so this contract asserts:
//!
//!   * Neighbour **identity sets** match across substrates after a
//!     formed link.
//!   * Both substrates return a weight `> 0` for each formed neighbour.
//!   * Unified weight ≥ legacy strength for the same pair (Phase D
//!     "destination" property — unified accumulates more).
//!
//! It does NOT assert numeric equality on the weight column.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use std::collections::{HashMap, HashSet};
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
fn t29_4_links_weighted_neighbour_set_matches() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        for id in ["a", "b", "c"] {
            seed_memory(&mut storage, id);
        }
        for _ in 0..3 {
            storage.record_coactivation("a", "b", 3).unwrap();
            storage.record_coactivation("a", "c", 3).unwrap();
        }
    }

    let legacy = Storage::new(&db_path).unwrap();
    let unified = Storage::with_unified_substrate(&db_path, true).unwrap();

    let legacy_neighbours: HashSet<String> = legacy
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let unified_neighbours: HashSet<String> = unified
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();

    let expected: HashSet<String> = ["b", "c"].into_iter().map(String::from).collect();
    assert_eq!(legacy_neighbours, expected, "legacy neighbour set");
    assert_eq!(unified_neighbours, expected, "unified neighbour set");
}

#[test]
fn t29_4_links_weighted_both_positive_for_formed_links() {
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

    let legacy_weight = Storage::new(&db_path)
        .unwrap()
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "b")
        .map(|(_, w)| w)
        .expect("legacy returns b");
    let unified_weight = Storage::with_unified_substrate(&db_path, true)
        .unwrap()
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "b")
        .map(|(_, w)| w)
        .expect("unified returns b");

    assert!(legacy_weight > 0.0, "legacy weight must be > 0");
    assert!(unified_weight > 0.0, "unified weight must be > 0");
}

#[test]
fn t29_4_links_weighted_unified_accumulates_without_cap() {
    // Design §4.3: legacy strength caps at 1.0 via .min(1.0); unified
    // weight accumulates without cap (delta_weight=0.1 per call,
    // unconditional). After many coactivations, unified > legacy.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
        // 20 coactivations — well past the formed threshold and the
        // legacy 1.0 cap. Unified accumulates 20 * 0.1 = 2.0 (plus
        // any tracking-phase increment from before the threshold).
        for _ in 0..20 {
            storage.record_coactivation("a", "b", 3).unwrap();
        }
    }

    let legacy_weight = Storage::new(&db_path)
        .unwrap()
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "b")
        .map(|(_, w)| w)
        .expect("legacy returns b");
    let unified_weight = Storage::with_unified_substrate(&db_path, true)
        .unwrap()
        .get_hebbian_links_weighted("a")
        .unwrap()
        .into_iter()
        .find(|(n, _)| n == "b")
        .map(|(_, w)| w)
        .expect("unified returns b");

    assert!(legacy_weight <= 1.0, "legacy strength caps at 1.0 (got {legacy_weight})");
    assert!(
        unified_weight > 1.0,
        "unified weight should exceed cap after 20 coactivations (got {unified_weight})"
    );
}

#[test]
fn t29_4_links_weighted_multi_neighbour_weights_all_positive() {
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

    for unified_flag in [false, true] {
        let storage = if unified_flag {
            Storage::with_unified_substrate(&db_path, true).unwrap()
        } else {
            Storage::new(&db_path).unwrap()
        };
        let by_neighbour: HashMap<String, f64> = storage
            .get_hebbian_links_weighted("a")
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(by_neighbour.len(), 3, "unified={unified_flag}");
        for n in ["b", "c", "d"] {
            let w = by_neighbour
                .get(n)
                .copied()
                .unwrap_or_else(|| panic!("missing {n} on unified={unified_flag}"));
            assert!(w > 0.0, "weight for {n} must be > 0 (unified={unified_flag}, got {w})");
        }
    }
}
