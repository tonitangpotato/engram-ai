//! T29.4 part-5 — `get_cross_namespace_neighbors` read-switch contract.
//!
//! Per-memory cross-NS neighbour query: "given memory id X, what
//! cross-namespace neighbours does it have?". Legacy reads
//! `hebbian_links` joined to `memories`; unified reads `edges` joined
//! to `nodes`. Both filter on `(source_id = ?1 OR target_id = ?1)` so
//! the caller can be on either side of the ISS-117 canonical row.
//!
//! Acceptance contract:
//!
//!   1. Basic match — both substrates surface the cross-NS neighbour
//!      regardless of which endpoint the caller specifies.
//!   2. Same-NS neighbours are filtered out — only rows whose
//!      `target_ns != source_ns` survive.
//!   3. OR-match works when caller is the high-id endpoint of the
//!      canonical (ns, id) tuple ordering. ISS-117 stored a single
//!      canonical row; the reader MUST find it from either side.
//!   4. Fan-out — multiple cross-NS neighbours all surface on both
//!      substrates.
//!
//! **ISS-118 follow-up**: all cross-NS test pairs in this suite use
//! ids whose lex order *inverts* their namespace lex order
//! (e.g. `hub` in `ns_aaa` vs `apple` in `ns_zzz` — raw id order
//! "hub" > "apple", but ns order "ns_aaa" < "ns_zzz"). This is the
//! coverage gap that allowed ISS-118 to slip past the original
//! ISS-117 tests; pinning it here so future regressions can't hide
//! by accidental id choice.

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
        source: "t29.4-part5-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, namespace).expect("seed memory");
}

#[test]
fn t29_4_get_cross_namespace_neighbors_basic_match() {
    // Cross-axis pair: id-order "hub" > "apple", ns-order ns_aaa <
    // ns_zzz. ISS-117 writer stamps source = ("ns_aaa", "hub")
    // (lower (ns,id) tuple wins) → single canonical row in both
    // substrates.
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
        // Query from the lower-tuple side (the canonical source).
        let neighbours = storage.get_cross_namespace_neighbors("hub").unwrap();
        assert_eq!(neighbours.len(), 1, "{label}: hub finds 1 cross-NS neighbour");
        let link = &neighbours[0];
        assert_eq!(link.source_id, "hub", "{label} source_id is caller");
        assert_eq!(link.source_ns, "ns_aaa", "{label} source_ns");
        assert_eq!(link.target_id, "apple", "{label} target_id");
        assert_eq!(link.target_ns, "ns_zzz", "{label} target_ns");
    }
}

#[test]
fn t29_4_get_cross_namespace_neighbors_or_match_high_tuple_caller() {
    // Caller is the HIGH-tuple endpoint ("apple" in "ns_zzz" is the
    // canonical TARGET, not source). Without OR-match the legacy
    // reader would miss this. ISS-118 root fix also depends on the
    // canonical row surviving reopen — this test trips both failure
    // modes if either regresses.
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
        // Query from the high-tuple side. ISS-117 made apple the
        // canonical target — reader's OR-match must find it.
        let neighbours = storage.get_cross_namespace_neighbors("apple").unwrap();
        assert_eq!(
            neighbours.len(),
            1,
            "{label}: OR-match must find canonical row from high-tuple side"
        );
        let link = &neighbours[0];
        assert_eq!(link.source_id, "apple", "{label} caller is source_id");
        assert_eq!(link.source_ns, "ns_zzz", "{label} caller's ns");
        assert_eq!(link.target_id, "hub", "{label} other endpoint projected");
        assert_eq!(link.target_ns, "ns_aaa", "{label} other ns");
    }
}

#[test]
fn t29_4_get_cross_namespace_neighbors_excludes_same_ns() {
    // Mixed bag: hub@ns_hub has one cross-NS neighbour (apple@ns_other)
    // AND one same-NS neighbour (sibling@ns_hub). Reader must return
    // ONLY the cross-NS one.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "hub", "ns_hub");
        seed_memory_in_ns(&mut storage, "sibling", "ns_hub");
        seed_memory_in_ns(&mut storage, "apple", "ns_other");

        // Same-NS link (within ns_hub).
        for _ in 0..3 {
            storage.record_coactivation_ns("hub", "sibling", 3, "ns_hub").unwrap();
        }
        // Cross-NS link.
        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_hub", "apple", "ns_other", 3)
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
        let neighbours = storage.get_cross_namespace_neighbors("hub").unwrap();
        let ids: HashSet<&str> = neighbours.iter().map(|l| l.target_id.as_str()).collect();
        assert_eq!(
            ids,
            ["apple"].into_iter().collect::<HashSet<_>>(),
            "{label}: same-NS neighbour 'sibling' must NOT appear in cross-NS reader"
        );
    }
}

#[test]
fn t29_4_get_cross_namespace_neighbors_multi_neighbours() {
    // Fan-out: hub@ns_hub has 3 cross-NS neighbours, all in ns_other,
    // with ids that all sort LOWER than "hub" (cross-axis). ISS-117
    // canonical row writer stamps source = "hub" for all three
    // (because ns_hub < ns_other); reader's OR-match finds them via
    // source_id = ?1. ISS-118 would have wiped all 3 on reopen.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_in_ns(&mut storage, "hub", "ns_hub");
        seed_memory_in_ns(&mut storage, "a", "ns_other");
        seed_memory_in_ns(&mut storage, "b", "ns_other");
        seed_memory_in_ns(&mut storage, "c", "ns_other");
        for neighbour in &["a", "b", "c"] {
            for _ in 0..3 {
                storage
                    .record_cross_namespace_coactivation(
                        "hub", "ns_hub", neighbour, "ns_other", 3,
                    )
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
        let neighbours = storage.get_cross_namespace_neighbors("hub").unwrap();
        let ids: HashSet<&str> = neighbours.iter().map(|l| l.target_id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "b", "c"].into_iter().collect::<HashSet<_>>(),
            "{label}: all 3 cross-NS neighbours must surface"
        );

        for link in &neighbours {
            assert_eq!(link.source_id, "hub", "{label} caller projection");
            assert_eq!(link.source_ns, "ns_hub", "{label} caller ns");
            assert_eq!(link.target_ns, "ns_other", "{label} target ns");
        }
    }
}
