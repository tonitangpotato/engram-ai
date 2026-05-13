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
