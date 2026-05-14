//! T29.4 part-6 — `get_all_cross_links` read-switch contract.
//!
//! Unbounded list of every cross-NS hebbian pair in the DB, sorted
//! by strength DESC. Legacy reads `hebbian_links` JOIN `memories`;
//! unified reads `edges` JOIN `nodes`. Both filter `n1.namespace !=
//! n2.namespace` in SQL so the cross-NS marker namespace on the
//! edge row isn't load-bearing.
//!
//! Acceptance contract:
//!
//!   1. Empty DB → empty list on both substrates.
//!   2. Single cross-NS pair → one CrossLink with correct
//!      source_ns/target_ns; same-NS pairs filtered out.
//!   3. Multiple cross-NS pairs → all surface; same-NS pairs in the
//!      same DB do not pollute the result.
//!   4. Ordering — results sorted by strength/weight DESC.
//!
//! **ISS-118 follow-up**: all cross-NS pairs use cross-axis ids
//! (id-lex-order inverts ns-lex-order) so any future regression
//! in either the migration or OR-match retrofit fails loudly.

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
        source: "t29.4-part6-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, namespace).expect("seed memory");
}

#[test]
fn t29_4_get_all_cross_links_empty_db() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    // Touch DB so unified-substrate open doesn't trip on missing tables.
    {
        let _ = Storage::new(&db_path).unwrap();
    }

    for (label, storage) in [
        ("legacy", Storage::new(&db_path).unwrap()),
        (
            "unified",
            Storage::with_unified_substrate(&db_path, true).unwrap(),
        ),
    ] {
        let links = storage.get_all_cross_links().unwrap();
        assert!(links.is_empty(), "{label}: empty DB should return no links");
    }
}

#[test]
fn t29_4_get_all_cross_links_single_pair() {
    // Cross-axis pair: id "hub" > "apple", ns "ns_aaa" < "ns_zzz".
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "hub", "ns_aaa");
        seed_memory_in_ns(&mut storage, "apple", "ns_zzz");
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_aaa", "apple", "ns_zzz", 3)
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
        let links = storage.get_all_cross_links().unwrap();
        assert_eq!(links.len(), 1, "{label}: expect 1 cross-NS pair");
        let link = &links[0];

        // Canonical row direction is (ns_aaa, hub) → (ns_zzz, apple)
        // because ns_aaa < ns_zzz.
        let endpoints: HashSet<&str> = [link.source_id.as_str(), link.target_id.as_str()]
            .into_iter()
            .collect();
        assert_eq!(
            endpoints,
            ["hub", "apple"].into_iter().collect::<HashSet<_>>(),
            "{label} endpoints"
        );

        let namespaces: HashSet<&str> = [link.source_ns.as_str(), link.target_ns.as_str()]
            .into_iter()
            .collect();
        assert_eq!(
            namespaces,
            ["ns_aaa", "ns_zzz"].into_iter().collect::<HashSet<_>>(),
            "{label} namespaces"
        );
        assert!(link.strength > 0.0, "{label} positive strength");
    }
}

#[test]
fn t29_4_get_all_cross_links_filters_same_ns() {
    // Three same-NS pairs + one cross-NS pair. Reader must return
    // only the cross-NS one.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        // Same-NS cluster in ns_alpha.
        seed_memory_in_ns(&mut storage, "x1", "ns_alpha");
        seed_memory_in_ns(&mut storage, "x2", "ns_alpha");
        seed_memory_in_ns(&mut storage, "x3", "ns_alpha");
        for _ in 0..3 {
            storage.record_coactivation_ns("x1", "x2", 3, "ns_alpha").unwrap();
            storage.record_coactivation_ns("x2", "x3", 3, "ns_alpha").unwrap();
            storage.record_coactivation_ns("x1", "x3", 3, "ns_alpha").unwrap();
        }
        // Cross-NS pair with cross-axis ids.
        seed_memory_in_ns(&mut storage, "hub", "ns_aaa");
        seed_memory_in_ns(&mut storage, "apple", "ns_zzz");
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_aaa", "apple", "ns_zzz", 3)
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
        let links = storage.get_all_cross_links().unwrap();
        assert_eq!(
            links.len(),
            1,
            "{label}: same-NS pairs must be filtered out"
        );
        let endpoints: HashSet<&str> = [
            links[0].source_id.as_str(),
            links[0].target_id.as_str(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            endpoints,
            ["hub", "apple"].into_iter().collect::<HashSet<_>>(),
            "{label}: only cross-NS pair survives"
        );
    }
}

#[test]
fn t29_4_get_all_cross_links_multi_pairs_sorted_by_strength() {
    // Three cross-NS pairs with different strengths. Verify all
    // surface AND that ordering is strength DESC on both substrates.
    //
    // ISS-117 writer increments strength by ~0.1 per coactivation
    // after threshold-form. We give pairs 5 / 10 / 15 coactivations
    // → roughly distinct strengths, ordered descending (note: legacy
    // caps at 1.0, unified accumulates uncapped — see T29.4 design
    // note on tracking-phase / weight-cap divergence). The relative
    // ordering must hold on both substrates.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        // Hub in lower-ns has higher id than all neighbours (cross-axis).
        seed_memory_in_ns(&mut storage, "hub", "ns_hub");
        seed_memory_in_ns(&mut storage, "a", "ns_other");
        seed_memory_in_ns(&mut storage, "b", "ns_other");
        seed_memory_in_ns(&mut storage, "c", "ns_other");

        // c gets fewest coactivations → weakest link.
        for _ in 0..5 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_hub", "c", "ns_other", 3)
                .unwrap();
        }
        for _ in 0..10 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_hub", "b", "ns_other", 3)
                .unwrap();
        }
        for _ in 0..15 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_hub", "a", "ns_other", 3)
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
        let links = storage.get_all_cross_links().unwrap();
        let ids: HashSet<&str> = links.iter().map(|l| {
            // Either side could be "hub" — pick the non-hub endpoint.
            if l.source_id == "hub" {
                l.target_id.as_str()
            } else {
                l.source_id.as_str()
            }
        }).collect();
        assert_eq!(
            ids,
            ["a", "b", "c"].into_iter().collect::<HashSet<_>>(),
            "{label}: all 3 cross-NS pairs must surface"
        );

        // Verify ORDER BY strength DESC: strengths are monotonically
        // non-increasing across the result list.
        for window in links.windows(2) {
            assert!(
                window[0].strength >= window[1].strength,
                "{label}: results must be sorted by strength DESC \
                 (got {} before {})",
                window[0].strength,
                window[1].strength,
            );
        }
    }
}
