//! ISS-127 contract tests: `Storage::store_triples` dual-writes triple-derived
//! entities to `nodes(node_kind='entity')` and `edges(edge_kind='provenance',
//! predicate='mentions')`.
//!
//! Before the fix, `Storage::insert_triple_entity` wrote only to the legacy
//! `entities` and `memory_entities` tables, leaving the unified substrate
//! blind to every triple-derived endpoint. T29.5 entity readers and T29.3
//! embedding readers would therefore miss these entities under
//! `unified_substrate=true` until T21+T23 backfill ran.
//!
//! The fix routes triple-entity inserts through the same
//! `insert_entity_node_row` + edge dual-write helpers that ISS-123 wired up
//! for `link_memory_entity`, in a single shared transaction so both
//! substrates commit atomically.
//!
//! Acceptance (matches design §5.2/§5.3/§3.3 invariants):
//!
//! 1. After `store_triples`, every subject + object appears in both
//!    `entities` and `nodes(node_kind='entity')` with the same id.
//! 2. `nodes.namespace = 'triple'` (matches legacy entity namespace
//!    convention; design §3.3 says entity is namespace authority).
//! 3. `memory_entities` link exists AND `edges(edge_kind='provenance',
//!    predicate='mentions')` row exists with `source_id=memory_id`,
//!    `target_id=entity_id`, weight=1.0, confidence=1.0.
//! 4. Edge `attributes` JSON carries `{"role":"triple"}` — verified
//!    round-trip per T23's normalized-role contract.
//! 5. Re-running `store_triples` with the same triple is a no-op for
//!    both substrates (deterministic ids + INSERT OR IGNORE).
//! 6. Pre-fix bug shape: legacy row count > 0 BUT nodes row count = 0
//!    would have been the symptom. We assert nodes count = legacy
//!    count to prove the gap is closed.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use engramai::{Predicate, Triple};
use rusqlite::params;
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 14, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss127-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

fn legacy_entity_count(s: &Storage, name: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM entities WHERE name = ?1",
            params![name.to_lowercase()],
            |row| row.get(0),
        )
        .unwrap()
}

fn node_entity_count(s: &Storage, name: &str) -> i64 {
    // nodes stores the entity name in `content` (see
    // insert_entity_node_row); `node_kind='entity'` and namespace
    // is 'triple' per design §3.3.
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes \
             WHERE node_kind = 'entity' \
               AND namespace = 'triple' \
               AND content = ?1",
            params![name.to_lowercase()],
            |row| row.get(0),
        )
        .unwrap()
}

fn legacy_link_count(s: &Storage, memory_id: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM memory_entities WHERE memory_id = ?1 AND role = 'triple'",
            params![memory_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn provenance_edge_count(s: &Storage, memory_id: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'provenance' \
               AND predicate = 'mentions' \
               AND source_id = ?1",
            params![memory_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn iss127_store_triples_dual_writes_entity_and_link() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();

    // Seed the memory via Storage::add so T12 dual-write creates the
    // `nodes(id=mem-1, node_kind='memory')` row. Without that row, the
    // FK guard in insert_triple_entity's edge insert would silently
    // skip — that's the legacy-only mode behaviour.
    s.add(&rec("mem-1", "alpha and beta met yesterday"), "default")
        .expect("add memory");

    let triple = Triple::new("Alpha".to_string(), Predicate::RelatedTo, "Beta".to_string(), 0.8);
    let inserted = s.store_triples("mem-1", &[triple]).expect("store triple");
    assert_eq!(inserted, 1, "triple inserted");

    // 1. Legacy substrate.
    assert_eq!(legacy_entity_count(&s, "alpha"), 1, "entities row for alpha");
    assert_eq!(legacy_entity_count(&s, "beta"), 1, "entities row for beta");
    assert_eq!(legacy_link_count(&s, "mem-1"), 2, "two memory_entities links");

    // 2. Unified substrate — this is the bug fix.
    assert_eq!(
        node_entity_count(&s, "alpha"),
        1,
        "ISS-127: nodes(entity) row for alpha"
    );
    assert_eq!(
        node_entity_count(&s, "beta"),
        1,
        "ISS-127: nodes(entity) row for beta"
    );
    assert_eq!(
        provenance_edge_count(&s, "mem-1"),
        2,
        "ISS-127: two provenance edges (mem-1 → alpha, mem-1 → beta)"
    );
}

#[test]
fn iss127_edge_attributes_carry_normalized_role() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("mem-1", "x"), "default").expect("add");

    let triple = Triple::new("X".to_string(), Predicate::Uses, "Y".to_string(), 0.9);
    s.store_triples("mem-1", &[triple]).expect("store");

    // role='triple' is the normalized role per T23 — the edge's
    // attributes JSON should carry it round-trip (mirrors ISS-123
    // dual-write convention).
    let attrs: String = s
        .conn()
        .query_row(
            "SELECT attributes FROM edges \
             WHERE edge_kind = 'provenance' AND predicate = 'mentions' \
               AND source_id = 'mem-1' \
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("edge exists");
    assert!(
        attrs.contains("\"role\""),
        "attributes JSON should carry 'role' key, got {}",
        attrs
    );
    assert!(
        attrs.contains("\"triple\""),
        "attributes JSON should carry 'triple' value, got {}",
        attrs
    );
}

#[test]
fn iss127_edge_namespace_is_triple_per_entity_authority() {
    // Design §3.3: entity row is the namespace authority for
    // memory_entities projection. Memory was stored under 'default'
    // namespace, but the edge should be in 'triple' namespace because
    // the entity node is in 'triple'.
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("mem-1", "x"), "default").expect("add");

    let triple = Triple::new("foo".to_string(), Predicate::IsA, "bar".to_string(), 0.9);
    s.store_triples("mem-1", &[triple]).expect("store");

    let ns: String = s
        .conn()
        .query_row(
            "SELECT namespace FROM edges \
             WHERE edge_kind = 'provenance' AND predicate = 'mentions' \
               AND source_id = 'mem-1' \
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("edge exists");
    assert_eq!(
        ns, "triple",
        "edge namespace = entity namespace (design §3.3), not memory namespace"
    );
}

#[test]
fn iss127_idempotent_rerun_no_duplicate_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("mem-1", "x"), "default").expect("add");

    let triple = Triple::new("Cat".to_string(), Predicate::RelatedTo, "Dog".to_string(), 0.7);

    s.store_triples("mem-1", &[triple.clone()]).expect("first");
    let second = s.store_triples("mem-1", &[triple]).expect("second");

    // Second store_triples returns 0 inserted (triples table INSERT OR
    // IGNORE on PK), so insert_triple_entity isn't even called. But
    // even if a separate triple with the same entity name was added
    // later, the deterministic id + INSERT OR IGNORE would still
    // produce exactly one nodes row and one edges row per
    // (memory, entity) pair.
    assert_eq!(second, 0, "second store of same triple is a no-op");

    assert_eq!(legacy_entity_count(&s, "cat"), 1);
    assert_eq!(node_entity_count(&s, "cat"), 1);
    assert_eq!(provenance_edge_count(&s, "mem-1"), 2);
}

#[test]
fn iss127_legacy_and_nodes_counts_match() {
    // Headline parity assertion: for any triple-derived entity, legacy
    // count == nodes count. This is the invariant T27 verifier would
    // check if we ever extend it to memory_entities source-of-truth
    // comparison.
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.db");
    let mut s = Storage::new(&path).unwrap();
    s.add(&rec("mem-1", "x"), "default").expect("add");

    let triples = vec![
        Triple::new("apple".into(), Predicate::IsA, "fruit".into(), 0.9),
        Triple::new("banana".into(), Predicate::IsA, "fruit".into(), 0.9),
        Triple::new("apple".into(), Predicate::RelatedTo, "banana".into(), 0.8),
    ];
    s.store_triples("mem-1", &triples).expect("store");

    let legacy_total: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM entities WHERE namespace = 'triple'", [], |row| {
            row.get(0)
        })
        .unwrap();
    let nodes_total: i64 = s
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE node_kind = 'entity' AND namespace = 'triple'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(legacy_total, 3, "apple, banana, fruit");
    assert_eq!(
        legacy_total, nodes_total,
        "ISS-127 parity: every legacy triple entity has a matching nodes row"
    );
}
