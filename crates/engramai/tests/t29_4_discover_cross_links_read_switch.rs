//! T29.4 part-4 — `discover_cross_links` read-switch contract.
//!
//! Cross-namespace Hebbian discovery. Phase B
//! `record_cross_namespace_coactivation` writes a single canonical
//! row to `hebbian_links` and dual-writes to
//! `edges(edge_kind='associative')` with namespace = `"ns1:ns2"`.
//! This reader filters both substrates on either direction marker
//! (`"a:b"` or `"b:a"`) and joins memories/nodes for source and
//! target namespace columns.
//!
//! Acceptance contract:
//!
//!   1. Same-namespace pairs are excluded (different code path —
//!      record_cross_namespace_coactivation routes to ns when
//!      ns1 == ns2).
//!   2. Cross-NS pairs are returned with correct source_ns /
//!      target_ns on both substrates.
//!   3. Query order is symmetric: discover_cross_links("a", "b")
//!      and discover_cross_links("b", "a") return the same set.
//!   4. Multiple cross-NS pairs all surface.
//!   5. Same-NS pairs in the same DB do not pollute the cross-NS
//!      reader.

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
        source: "t29.4-cross-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, namespace).expect("seed memory");
}

#[test]
fn t29_4_discover_cross_links_single_pair_matches_on_both_substrates() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "a", "ns_a");
        seed_memory_in_ns(&mut storage, "b", "ns_b");
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("a", "ns_a", "b", "ns_b", 3)
                .unwrap();
        }
    }

    for (label, storage) in [
        ("legacy", Storage::new(&db_path).unwrap()),
        (
            "unified",
            Storage::with_unified_substrate(&db_path, true).unwrap(),
        ),
    ] {
        let links = storage.discover_cross_links("ns_a", "ns_b").unwrap();
        assert_eq!(links.len(), 1, "{label} should find exactly 1 cross link");

        let link = &links[0];
        // Pair canonicalised by ISS-117 — endpoints are {a, b} as a
        // set, namespaces correspond.
        let endpoints: HashSet<&str> = [link.source_id.as_str(), link.target_id.as_str()]
            .into_iter()
            .collect();
        assert_eq!(
            endpoints,
            ["a", "b"].into_iter().collect::<HashSet<_>>(),
            "{label} endpoints"
        );

        let namespaces: HashSet<Option<String>> =
            [link.source_ns.clone(), link.target_ns.clone()]
                .into_iter()
                .collect();
        assert_eq!(
            namespaces,
            [Some("ns_a".to_string()), Some("ns_b".to_string())]
                .into_iter()
                .collect::<HashSet<_>>(),
            "{label} namespaces"
        );
    }
}

#[test]
fn t29_4_discover_cross_links_excludes_same_namespace_pairs() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        // Same-NS pair (must NOT appear in cross-NS reader).
        seed_memory_in_ns(&mut storage, "x", "shared");
        seed_memory_in_ns(&mut storage, "y", "shared");
        for _ in 0..3 {
            // ns1 == ns2 → routes to record_coactivation_ns under the
            // hood, namespace stamped as "shared" not "shared:shared".
            storage
                .record_cross_namespace_coactivation("x", "shared", "y", "shared", 3)
                .unwrap();
        }
        // Cross-NS pair.
        seed_memory_in_ns(&mut storage, "p", "left");
        seed_memory_in_ns(&mut storage, "q", "right");
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("p", "left", "q", "right", 3)
                .unwrap();
        }
    }

    for (label, storage) in [
        ("legacy", Storage::new(&db_path).unwrap()),
        (
            "unified",
            Storage::with_unified_substrate(&db_path, true).unwrap(),
        ),
    ] {
        // Search for the actual cross-NS pair: returns only p-q.
        let links = storage.discover_cross_links("left", "right").unwrap();
        assert_eq!(links.len(), 1, "{label} cross-NS only");
        let endpoints: HashSet<&str> =
            [links[0].source_id.as_str(), links[0].target_id.as_str()]
                .into_iter()
                .collect();
        assert_eq!(
            endpoints,
            ["p", "q"].into_iter().collect::<HashSet<_>>(),
            "{label} endpoints"
        );

        // Same-NS query (shared:shared marker doesn't exist).
        let none = storage.discover_cross_links("shared", "shared").unwrap();
        assert!(
            none.is_empty(),
            "{label} same-NS query must not match cross-NS reader"
        );
    }
}

#[test]
fn t29_4_discover_cross_links_symmetric_query_order() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "a", "ns_a");
        seed_memory_in_ns(&mut storage, "b", "ns_b");
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("a", "ns_a", "b", "ns_b", 3)
                .unwrap();
        }
    }

    for unified in [false, true] {
        let storage = if unified {
            Storage::with_unified_substrate(&db_path, true).unwrap()
        } else {
            Storage::new(&db_path).unwrap()
        };
        let ab = storage.discover_cross_links("ns_a", "ns_b").unwrap();
        let ba = storage.discover_cross_links("ns_b", "ns_a").unwrap();
        assert_eq!(
            ab.len(),
            ba.len(),
            "discover should be symmetric (unified={unified})"
        );
        assert_eq!(ab.len(), 1, "exactly one pair (unified={unified})");
    }
}

#[test]
fn t29_4_discover_cross_links_multiple_pairs() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        for id in ["a1", "a2", "a3"] {
            seed_memory_in_ns(&mut storage, id, "left");
        }
        for id in ["b1", "b2", "b3"] {
            seed_memory_in_ns(&mut storage, id, "right");
        }
        for (l, r) in [("a1", "b1"), ("a2", "b2"), ("a3", "b3")] {
            for _ in 0..3 {
                storage
                    .record_cross_namespace_coactivation(l, "left", r, "right", 3)
                    .unwrap();
            }
        }
    }

    for (label, storage) in [
        ("legacy", Storage::new(&db_path).unwrap()),
        (
            "unified",
            Storage::with_unified_substrate(&db_path, true).unwrap(),
        ),
    ] {
        let links = storage.discover_cross_links("left", "right").unwrap();
        assert_eq!(links.len(), 3, "{label} three cross-NS pairs");

        let pair_set: HashSet<(String, String)> = links
            .iter()
            .map(|l| {
                let mut ids = [l.source_id.clone(), l.target_id.clone()];
                ids.sort();
                (ids[0].clone(), ids[1].clone())
            })
            .collect();
        let expected: HashSet<(String, String)> = [
            ("a1".to_string(), "b1".to_string()),
            ("a2".to_string(), "b2".to_string()),
            ("a3".to_string(), "b3".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(pair_set, expected, "{label} pair set");
    }
}
