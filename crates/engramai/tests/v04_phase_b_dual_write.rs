//! T12 — Phase B dual-write: `Storage::add` writes to both
//! `memories` and `nodes` in the same transaction.
//!
//! Acceptance per design.md §5.2:
//!
//! > Every new memory produces 1 legacy row + 1 nodes row,
//! > byte-equal content/timestamps.
//!
//! This test seeds a single memory via `Storage::add` and checks:
//!
//! 1. A row exists in `nodes` with `node_kind='memory'` and the same id.
//! 2. Scalar fields are byte-equal across `memories` and `nodes`:
//!    content, layer, memory_type, namespace, importance,
//!    working_strength, core_strength, pinned, created_at,
//!    consolidation_count, source.
//! 3. `occurred_at` round-trips (NULL or matching epoch).
//! 4. `nodes_fts` contains the content (FTS trigger fired) — `MATCH`
//!    on a token from the content returns the new row's fts_rowid.
//! 5. Re-adding the same record id is a no-op for `nodes` (INSERT OR
//!    IGNORE), so we never get duplicate node rows or duplicate fts
//!    rows. This is the idempotency guarantee referenced in §5.2.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

/// Build a single, fully-populated memory record for the test.
fn sample_record(id: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 13, 4, 30, 0).unwrap();
    let occurred = Utc.with_ymd_and_hms(2025, 1, 15, 12, 0, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: "potato likes pickles and ferments cabbage every winter".into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: Some(occurred),
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: true,
        consolidation_count: 2,
        last_consolidated: Some(created),
        source: "t12-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

#[test]
fn t12_add_memory_dual_writes_to_nodes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("dual.db");

    let mut storage = Storage::new(&path).unwrap();
    let rec = sample_record("mem-t12-1");
    storage.add(&rec, "default").expect("add memory");

    // Scope the first read pass so we can re-borrow `storage` mutably
    // for the duplicate-add idempotency check below.
    let (fts_rowid_first, m_imp, n_imp): (i64, f64, f64) = {
        let conn = storage.conn();

        // (1) Node row exists with kind=memory and matching id.
        let (kind, content, layer, memory_type, namespace): (
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT node_kind, content, layer, memory_type, namespace
                 FROM nodes WHERE id = ?1",
                params![rec.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("nodes row missing for new memory");
        assert_eq!(kind, "memory");
        assert_eq!(content, rec.content);
        assert_eq!(layer.as_deref(), Some("core"));
        assert_eq!(memory_type.as_deref(), Some("factual"));
        assert_eq!(namespace, "default");

        // (2) Scalars byte-equal between legacy and unified rows.
        let (m_imp, m_ws, m_cs, m_pin, m_cc, m_src): (f64, f64, f64, i64, i64, String) = conn
            .query_row(
                "SELECT importance, working_strength, core_strength, pinned,
                        consolidation_count, source FROM memories WHERE id = ?1",
                params![rec.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        let (n_imp, n_ws, n_cs, n_pin, n_cc, n_src): (f64, f64, f64, i64, i64, String) = conn
            .query_row(
                "SELECT importance, working_strength, core_strength, pinned,
                        consolidation_count, source FROM nodes WHERE id = ?1",
                params![rec.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert!((m_imp - n_imp).abs() < 1e-12, "importance drift");
        assert!((m_ws - n_ws).abs() < 1e-12, "working_strength drift");
        assert!((m_cs - n_cs).abs() < 1e-12, "core_strength drift");
        assert_eq!(m_pin, n_pin);
        assert_eq!(m_cc, n_cc);
        assert_eq!(m_src, n_src);

        // (3) occurred_at round-trips.
        let (m_occ, n_occ): (Option<f64>, Option<f64>) = conn
            .query_row(
                "SELECT m.occurred_at, n.occurred_at
                   FROM memories m JOIN nodes n ON m.id = n.id
                  WHERE m.id = ?1",
                params![rec.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        match (m_occ, n_occ) {
            (Some(a), Some(b)) => assert!((a - b).abs() < 1e-9, "occurred_at drift"),
            (None, None) => panic!("test record set occurred_at, but both NULL"),
            _ => panic!("occurred_at mismatch (one NULL, one not)"),
        }

        // (4) FTS5 trigger fired — content searchable via nodes_fts.
        let fts_rowid: i64 = conn
            .query_row(
                "SELECT fts_rowid FROM nodes WHERE id = ?1",
                params![rec.id],
                |r| r.get(0),
            )
            .unwrap();
        let hit_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH 'pickles'",
                [],
                |r| r.get(0),
            )
            .expect("FTS lookup for inserted node");
        assert_eq!(hit_rowid, fts_rowid);

        (fts_rowid, m_imp, n_imp)
    };
    // Silence unused-binding warnings (the asserts above already used them).
    let _ = (fts_rowid_first, m_imp, n_imp);

    // (5) Idempotency — re-`add` of the same id must not duplicate the
    // node row or the FTS row. Legacy `memories` will of course error
    // on the PK, so we wrap the second add in an expectation that
    // *the legacy insert* fails BEFORE any further dual-write would
    // run. The single-transaction guarantee in `add()` means a
    // partial dual-write is impossible.
    let dup = storage.add(&rec, "default");
    assert!(
        dup.is_err(),
        "second add of identical id should fail on legacy PK; got Ok(())"
    );

    let conn = storage.conn();
    let n_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = ?1",
            params![rec.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_rows, 1, "nodes table got a duplicate after failed second add");
    let fts_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'pickles'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_rows, 1, "nodes_fts got a duplicate row after failed second add");
}

// ===========================================================================
// T13 — ResolutionPipeline entity + edge dual-write
// ===========================================================================

use engramai::graph::{
    CanonicalPredicate, Edge, EdgeEnd, Entity, EntityKind, Predicate, ResolutionMethod,
};
use engramai::graph::store::SqliteGraphStore;
use engramai::graph::storage_graph::init_graph_tables;
use rusqlite::Connection;
use uuid::Uuid;

/// Build a minimal Entity for the test.
fn sample_entity(name: &str) -> Entity {
    let now = Utc::now();
    Entity {
        id: Uuid::new_v4(),
        canonical_name: name.into(),
        kind: EntityKind::Other("test".into()),
        summary: String::new(),
        attributes: serde_json::json!({}),
        history: vec![],
        merged_into: None,
        first_seen: now,
        last_seen: now,
        created_at: now,
        updated_at: now,
        episode_mentions: vec![],
        memory_mentions: vec![],
        activation: 0.0,
        importance: 0.5,
        identity_confidence: 0.8,
        agent_affect: None,
        arousal: 0.0,
        somatic_fingerprint: None,
        embedding: None,
    }
}

#[test]
fn t13_insert_entity_dual_writes_to_nodes_kind_entity() {
    let mut conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    init_graph_tables(&conn).expect("init graph tables");

    let e = sample_entity("Alice");
    let eid = e.id;
    {
        let mut store = SqliteGraphStore::new(&mut conn);
        engramai::graph::store::GraphWrite::insert_entity(&mut store, &e)
            .expect("insert_entity");
    }

    // Legacy row landed.
    let (legacy_name, legacy_ns): (String, String) = conn
        .query_row(
            "SELECT canonical_name, namespace FROM graph_entities WHERE id = ?1",
            params![eid.as_bytes().to_vec()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("legacy graph_entities row");
    assert_eq!(legacy_name, "Alice");
    assert_eq!(legacy_ns, "default");

    // Unified row landed with kind=entity and same content.
    let (kind, content, ns): (String, String, String) = conn
        .query_row(
            "SELECT node_kind, content, namespace FROM nodes WHERE id = ?1",
            params![eid.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("unified nodes row");
    assert_eq!(kind, "entity");
    assert_eq!(content, "Alice");
    assert_eq!(ns, "default");

    // FTS picked up the canonical_name.
    let fts_rowid: i64 = conn
        .query_row(
            "SELECT fts_rowid FROM nodes WHERE id = ?1",
            params![eid.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    let hit_rowid: i64 = conn
        .query_row(
            "SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH 'Alice'",
            [],
            |r| r.get(0),
        )
        .expect("FTS hit");
    assert_eq!(hit_rowid, fts_rowid);
}

#[test]
fn t13_insert_edge_dual_writes_to_unified_edges() {
    let mut conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    init_graph_tables(&conn).expect("init graph tables");

    // graph_edges.memory_id FK → memories(id). The table must exist
    // for SQLite to even parse the FK clause on insert, even when the
    // inserted memory_id is NULL. init_graph_tables does NOT create
    // `memories` (that's owned by Storage::open in the legacy
    // bootstrap), so we create a minimal stub here. No rows needed.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (id TEXT PRIMARY KEY, content TEXT);",
    )
    .unwrap();

    let subj_e = sample_entity("Bob");
    let obj_e = sample_entity("Acme");
    let subj = subj_e.id;
    let obj = obj_e.id;

    let now = Utc::now();
    let mut edge = Edge::new(
        subj,
        Predicate::Canonical(CanonicalPredicate::WorksAt),
        EdgeEnd::Entity { id: obj },
        Some(now),
        now,
    );
    edge.summary = "Bob works at Acme".into();
    // memory_id intentionally NOT set — keeps this test focused on the
    // entity-edge dual-write path. The Phase B behavior for
    // source_memory_id (always NULL in unified edges) is asserted
    // below.
    edge.resolution_method = ResolutionMethod::LlmTieBreaker;
    edge.confidence = 0.9;
    let eid = edge.id;

    {
        let mut store = SqliteGraphStore::new(&mut conn);
        use engramai::graph::store::GraphWrite;
        store.insert_entity(&subj_e).expect("insert subj");
        store.insert_entity(&obj_e).expect("insert obj");
        store.insert_edge(&edge).expect("insert_edge");
    }

    // Legacy graph_edges row landed (sanity).
    let legacy_summary: String = conn
        .query_row(
            "SELECT summary FROM graph_edges WHERE id = ?1",
            params![eid.as_bytes().to_vec()],
            |r| r.get(0),
        )
        .expect("legacy graph_edges row");
    assert_eq!(legacy_summary, "Bob works at Acme");

    // Unified edges row landed with edge_kind='assertion'.
    let (edge_kind, source_id, target_id, predicate, summary): (
        String,
        String,
        Option<String>,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT edge_kind, source_id, target_id, predicate, summary
             FROM edges WHERE id = ?1",
            params![eid.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("unified edges row");
    assert_eq!(edge_kind, "assertion");
    assert_eq!(source_id, subj.to_string());
    assert_eq!(target_id.as_deref(), Some(obj.to_string().as_str()));
    assert_eq!(predicate, "works_at");
    assert_eq!(summary, "Bob works at Acme");

    // source_memory_id is intentionally NULL in Phase B (T13 docstring
    // explains why): even when edge.memory_id is set, the unified
    // `edges.source_memory_id` FK targets `nodes(id)` (specifically a
    // memory node), and memories that were not written via
    // `Storage::add` (T12 dual-write) don't have a corresponding
    // unified node yet. Phase C backfill (T19) closes the gap. Until
    // then we always write NULL here so the dual-write succeeds atomically.
    let sm_id: Option<String> = conn
        .query_row(
            "SELECT source_memory_id FROM edges WHERE id = ?1",
            params![eid.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        sm_id.is_none(),
        "source_memory_id should be NULL until Phase C backfill"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// T14 — Hebbian dual-write: `Storage::record_association` writes to both
// `hebbian_links` (legacy) and `edges` (unified, edge_kind='associative').
//
// Acceptance per design.md §4.3 + §8.10 T14:
//
//   * Every record_association call produces 1 hebbian_links row +
//     1 edges row in the same transaction.
//   * Repeated calls with the same (src, tgt, signal_source) collapse
//     to one edges row with accumulating weight and incrementing
//     coactivation_count.
//   * Different signal_source between the same pair gets its own row
//     (signal_source is part of the identity via the partial unique
//     index `idx_edges_assoc_unique`).
//   * (src, tgt) is canonicalized — calling B→A after A→B updates the
//     same row, not a new one.
//   * Legacy hebbian_links remains the system of record for now;
//     dual-write is additive (§5.2 Phase B contract).
// ─────────────────────────────────────────────────────────────────────────────

/// Seed two memory rows so dual-write's FK constraint to `nodes(id)` is
/// satisfied. Returns their ids.
fn seed_two_memories(storage: &mut Storage) -> (String, String) {
    let now = Utc.with_ymd_and_hms(2026, 5, 13, 8, 0, 0).unwrap();
    let mut rec_a = sample_record("t14-a");
    rec_a.created_at = now;
    rec_a.content = "alpha memory about pickles".into();
    let mut rec_b = sample_record("t14-b");
    rec_b.created_at = now;
    rec_b.content = "beta memory about cabbage".into();
    storage.add(&rec_a, "default").unwrap();
    storage.add(&rec_b, "default").unwrap();
    ("t14-a".to_string(), "t14-b".to_string())
}

#[test]
fn t14_record_association_dual_writes_to_edges() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t14a.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    storage
        .record_association(&a, &b, 0.5, "entity", r#"{"entity_overlap":0.4}"#, "default")
        .unwrap();

    let conn = storage.conn();

    // Legacy row exists.
    let legacy_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hebbian_links \
             WHERE (source_id = ?1 AND target_id = ?2) \
                OR (source_id = ?2 AND target_id = ?1)",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 1, "hebbian_links must have one row");

    // Unified edges row exists with expected typing + payload.
    let (edge_kind, predicate, weight, sig_source, sig_detail, coact): (
        String, String, f64, String, String, i64,
    ) = conn
        .query_row(
            "SELECT edge_kind, predicate, weight, \
                    json_extract(attributes, '$.signal_source'), \
                    json_extract(attributes, '$.signal_detail'), \
                    json_extract(attributes, '$.coactivation_count') \
             FROM edges \
             WHERE (source_id = ?1 AND target_id = ?2) \
                OR (source_id = ?2 AND target_id = ?1)",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();

    assert_eq!(edge_kind, "associative", "edge_kind must be 'associative'");
    assert_eq!(predicate, "co_activated", "predicate must be 'co_activated'");
    assert!((weight - 0.5).abs() < 1e-9, "first-write weight = delta_weight");
    assert_eq!(sig_source, "entity", "signal_source round-trips");
    assert_eq!(
        sig_detail, r#"{"entity_overlap":0.4}"#,
        "signal_detail round-trips verbatim"
    );
    assert_eq!(coact, 1, "first write seeds coactivation_count = 1");
}

#[test]
fn t14_record_association_accumulates_on_same_signal_source() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t14b.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Three writes, same (src, tgt, signal_source) — must collapse to
    // one row with summed weight and incremented coactivation_count.
    storage
        .record_association(&a, &b, 0.3, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association(&a, &b, 0.2, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association(&a, &b, 0.1, "entity", "{}", "default")
        .unwrap();

    let conn = storage.conn();

    let row_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1, "same signal_source must collapse to one row");

    let (weight, coact): (f64, i64) = conn
        .query_row(
            "SELECT weight, \
                    json_extract(attributes, '$.coactivation_count') \
             FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert!(
        (weight - 0.6).abs() < 1e-9,
        "weight must sum-accumulate: 0.3+0.2+0.1 = 0.6, got {weight}"
    );
    assert_eq!(
        coact, 3,
        "coactivation_count increments by 1 per write (1 + 1 + 1 = 3)"
    );
}

#[test]
fn t14_distinct_signal_source_creates_separate_row() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t14c.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Same (src, tgt), different signal_source → 2 rows.
    storage
        .record_association(&a, &b, 0.5, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association(&a, &b, 0.4, "temporal", "{}", "default")
        .unwrap();

    let row_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        row_count, 2,
        "distinct signal_source must produce 2 rows (identity includes signal_source)"
    );

    // Each signal_source has its own weight.
    let entity_w: f64 = storage
        .conn()
        .query_row(
            "SELECT weight FROM edges \
             WHERE edge_kind = 'associative' \
               AND json_extract(attributes, '$.signal_source') = 'entity' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    let temporal_w: f64 = storage
        .conn()
        .query_row(
            "SELECT weight FROM edges \
             WHERE edge_kind = 'associative' \
               AND json_extract(attributes, '$.signal_source') = 'temporal' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert!((entity_w - 0.5).abs() < 1e-9, "entity weight unaffected by temporal write");
    assert!((temporal_w - 0.4).abs() < 1e-9, "temporal weight unaffected by entity write");
}

#[test]
fn t14_reverse_direction_collapses_to_same_row() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t14d.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Write A→B then B→A with same signal_source — canonical (min, max)
    // ordering inside the helper must collapse these to one row.
    storage
        .record_association(&a, &b, 0.4, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association(&b, &a, 0.3, "entity", "{}", "default")
        .unwrap();

    let row_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        row_count, 1,
        "reverse-direction call must collapse to one row (canonical src<tgt ordering)"
    );

    let (weight, coact): (f64, i64) = storage
        .conn()
        .query_row(
            "SELECT weight, \
                    json_extract(attributes, '$.coactivation_count') \
             FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert!(
        (weight - 0.7).abs() < 1e-9,
        "weight sums across both directions: 0.4 + 0.3 = 0.7, got {weight}"
    );
    assert_eq!(coact, 2, "coactivation_count counts both directional writes");

    // The stored row uses canonical (min, max) ordering — verify.
    let (stored_src, stored_tgt): (String, String) = storage
        .conn()
        .query_row(
            "SELECT source_id, target_id FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (expected_lo, expected_hi) = if a < b { (&a, &b) } else { (&b, &a) };
    assert_eq!(
        &stored_src, expected_lo,
        "source_id must be lexicographic min of pair"
    );
    assert_eq!(
        &stored_tgt, expected_hi,
        "target_id must be lexicographic max of pair"
    );
}
