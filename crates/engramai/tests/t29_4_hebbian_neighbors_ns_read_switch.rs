//! T29.4 part-3 — `get_hebbian_neighbors_ns` read-switch contract.
//!
//! Namespaced variant of part-1. Also pins the ISS-117 OR-match fix
//! for the legacy path — the ns variant was previously
//! single-direction `WHERE source_id = ?` and silently hid
//! neighbours when the caller passed the high-id endpoint of a
//! formed link.
//!
//! Acceptance contract:
//!
//!   1. With `Some(ns)` filter, returns only neighbours whose
//!      edge/link row carries `namespace = ns`.
//!   2. With `Some("*")` or `None`, delegates to non-ns reader (all
//!      namespaces).
//!   3. Both substrates return the same neighbour set for a
//!      ns-scoped query against the same data.
//!   4. OR-match on legacy: passing high-id endpoint still finds the
//!      neighbour.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use std::collections::HashSet;
use tempfile::tempdir;

fn seed_memory_in_ns(storage: &mut Storage, id: &str, namespace: &str) {
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
        source: "t29.4-ns-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, namespace).expect("seed memory");
}

#[test]
fn t29_4_neighbors_ns_filters_by_namespace_on_both_substrates() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        // Memories in ns=tenant1.
        seed_memory_in_ns(&mut storage, "a", "tenant1");
        seed_memory_in_ns(&mut storage, "b", "tenant1");
        // Memory in ns=tenant2 — sharing a does not exist, so build
        // a separate pair to make sure ns filter excludes it.
        seed_memory_in_ns(&mut storage, "a2", "tenant2");
        seed_memory_in_ns(&mut storage, "b2", "tenant2");

        for _ in 0..3 {
            storage
                .record_coactivation_ns("a", "b", 3, "tenant1")
                .unwrap();
            storage
                .record_coactivation_ns("a2", "b2", 3, "tenant2")
                .unwrap();
        }
    }

    let legacy = Storage::new(&db_path).unwrap();
    let unified = Storage::with_unified_substrate(&db_path, true).unwrap();

    // ns=tenant1 query for "a" should return only "b" — even though
    // "a2 -> b2" exists in tenant2.
    for (label, storage) in [("legacy", &legacy), ("unified", &unified)] {
        let ns: HashSet<String> = storage
            .get_hebbian_neighbors_ns("a", Some("tenant1"))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            ns,
            ["b".to_string()].into_iter().collect::<HashSet<_>>(),
            "{label} tenant1 should only see b"
        );

        // Wrong namespace returns empty even though the pair exists.
        let wrong: HashSet<String> = storage
            .get_hebbian_neighbors_ns("a", Some("tenant2"))
            .unwrap()
            .into_iter()
            .collect();
        assert!(wrong.is_empty(), "{label} tenant2 should not see ab pair");
    }
}

#[test]
fn t29_4_neighbors_ns_wildcard_delegates_to_unfiltered_reader() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "a", "t1");
        seed_memory_in_ns(&mut storage, "b", "t1");
        for _ in 0..3 {
            storage.record_coactivation_ns("a", "b", 3, "t1").unwrap();
        }
    }

    for unified_flag in [false, true] {
        let storage = if unified_flag {
            Storage::with_unified_substrate(&db_path, true).unwrap()
        } else {
            Storage::new(&db_path).unwrap()
        };

        let with_star: HashSet<String> = storage
            .get_hebbian_neighbors_ns("a", Some("*"))
            .unwrap()
            .into_iter()
            .collect();
        let with_none: HashSet<String> = storage
            .get_hebbian_neighbors_ns("a", None)
            .unwrap()
            .into_iter()
            .collect();
        let direct: HashSet<String> = storage
            .get_hebbian_neighbors("a")
            .unwrap()
            .into_iter()
            .collect();

        assert_eq!(
            with_star, direct,
            "* must match direct (unified={unified_flag})"
        );
        assert_eq!(
            with_none, direct,
            "None must match direct (unified={unified_flag})"
        );
    }
}

#[test]
fn t29_4_neighbors_ns_legacy_or_match_finds_high_id_endpoint() {
    // Pre-ISS-117 the ns reader was single-direction. This pins the
    // fix so that passing the high-id endpoint of a formed link
    // still returns the other endpoint on the legacy path.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        // Pair has canonical (aaa_low, zzz_high) after ISS-117 — caller
        // passes high first.
        seed_memory_in_ns(&mut storage, "aaa_low", "tns");
        seed_memory_in_ns(&mut storage, "zzz_high", "tns");
        for _ in 0..3 {
            storage
                .record_coactivation_ns("zzz_high", "aaa_low", 3, "tns")
                .unwrap();
        }
    }

    let legacy = Storage::new(&db_path).unwrap();
    let result: HashSet<String> = legacy
        .get_hebbian_neighbors_ns("zzz_high", Some("tns"))
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        result,
        ["aaa_low".to_string()].into_iter().collect::<HashSet<_>>(),
        "OR-match: high-id caller must see low-id neighbour"
    );
}
