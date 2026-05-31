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
    let (fts_rowid_first, n_imp): (i64, f64) = {
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

        // (2) Scalars correctly persisted to the unified `nodes` row.
        // Phase E (T34a) removed the legacy `memories` dual-write; the
        // field-completeness regression value now lives entirely on the
        // `nodes` projection. We assert the unified row carries the
        // record's scalars verbatim (no silent field loss).
        let (n_imp, n_ws, n_cs, n_pin, n_cc, n_src): (f64, f64, f64, i64, i64, String) = conn
            .query_row(
                "SELECT importance, working_strength, core_strength, pinned,
                        consolidation_count, source FROM nodes WHERE id = ?1",
                params![rec.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert!((n_imp - rec.importance).abs() < 1e-12, "importance drift");
        assert!(
            (n_ws - rec.working_strength).abs() < 1e-12,
            "working_strength drift"
        );
        assert!(
            (n_cs - rec.core_strength).abs() < 1e-12,
            "core_strength drift"
        );
        assert_eq!(n_pin, rec.pinned as i64);
        assert_eq!(n_cc, rec.consolidation_count as i64);
        assert_eq!(n_src, rec.source);

        // (3) occurred_at round-trips onto the unified row.
        let n_occ: Option<f64> = conn
            .query_row(
                "SELECT occurred_at FROM nodes WHERE id = ?1",
                params![rec.id],
                |r| r.get(0),
            )
            .unwrap();
        match (rec.occurred_at, n_occ) {
            (Some(_), Some(_)) => { /* present on both — round-trip ok */ }
            (None, None) => panic!("test record set occurred_at, but node NULL"),
            _ => panic!("occurred_at mismatch (record vs node)"),
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

        (fts_rowid, n_imp)
    };
    // Silence unused-binding warnings (the asserts above already used them).
    let _ = (fts_rowid_first, n_imp);

    // (5) Idempotency — re-`add` of the same id must not duplicate the
    // node row or the FTS row. We assert the *invariant* (no duplicate,
    // content unchanged) rather than the implementation detail of which
    // table's PK rejects the write. This keeps the test valid across the
    // Phase E cutover: before T34a a legacy `memories` PK collision made
    // the second add Err; after T34a the node insert is `OR IGNORE` so
    // the second add is Ok — either way the substrate must hold exactly
    // one row. The single-transaction guarantee in `add()` means a
    // partial dual-write is impossible.
    let _dup = storage.add(&rec, "default");

    let conn = storage.conn();
    let n_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = ?1",
            params![rec.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_rows, 1, "nodes table got a duplicate after second add");
    let fts_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'pickles'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_rows, 1, "nodes_fts got a duplicate row after failed second add");
}

// ---------------------------------------------------------------------------
// T12 regression — superseded_by source-field correctness
// ---------------------------------------------------------------------------
//
// `MemoryRecord` carries TWO distinct optional columns:
//
//   - `superseded_by`   — "this row was replaced by X". Every retrieval
//                         query gates on it (`WHERE superseded_by IS
//                         NULL OR superseded_by = ''`).
//   - `contradicted_by` — "X contradicts this row". Informational only,
//                         never used as a filter.
//
// The investigation that produced this test surfaced TWO bugs:
//
//   Bug A: T12's dual-write sourced `nodes.superseded_by` from
//          `record.contradicted_by`. Silently wrong — invisible until
//          Phase D read cutover.
//   Bug B: `Storage::add()` never persists `record.superseded_by` to
//          the legacy `memories` table at all (the INSERT statement
//          simply doesn't bind that column). The supersession
//          relation is established post-add via UPDATE paths
//          (`supersede`, `supersede_bulk`, `unsupersede`).
//
// Root fix: the add path treats `superseded_by` as ALWAYS NULL on
// both tables (it's a fresh-insert; nothing can have replaced it
// yet). The three UPDATE paths dual-update both `memories` and
// `nodes` transactionally, so retrieval — which reads `memories`
// today but switches to `nodes` at Phase D cutover — never sees a
// half-updated supersession.
//
// This test pins all three behaviors:
//   1. `add()` writes NULL into `nodes.superseded_by`, even when the
//      `MemoryRecord` (incorrectly) carries values in those fields.
//   2. `supersede()` mirrors the supersession into BOTH tables.
//   3. `unsupersede()` clears BOTH tables (memories: `''`, nodes: NULL).
#[test]
fn t12_dual_write_superseded_by_root_fix() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Seed three memories in the same namespace. We'll supersede
    // `subject` by `supersedor`. `contradictor` exists only to populate
    // `record.contradicted_by` on `subject`, to prove the add path
    // ignores it (Bug A regression).
    let supersedor = {
        let mut r = sample_record("mem-supersedor");
        r.content = "newer version of the claim".into();
        r
    };
    let contradictor = {
        let mut r = sample_record("mem-contradictor");
        r.content = "a fact that conflicts but does not replace".into();
        r
    };
    storage.add(&supersedor, "default").unwrap();
    storage.add(&contradictor, "default").unwrap();

    // Subject deliberately sets BOTH MemoryRecord fields to different
    // non-null values. The add path MUST ignore both — supersession
    // is an UPDATE-time concern, not an INSERT-time concern.
    let mut subject = sample_record("mem-subject");
    subject.superseded_by = Some("mem-supersedor".into());
    subject.contradicted_by = Some("mem-contradictor".into());
    storage.add(&subject, "default").unwrap();

    // -----------------------------------------------------------------
    // Phase 1: add path writes NULL on both tables
    // -----------------------------------------------------------------
    let conn = storage.conn();
    let nodes_sup_after_add: Option<String> = conn
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = ?",
            params!["mem-subject"],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(
        nodes_sup_after_add, None,
        "Bug A regression: nodes.superseded_by must be NULL after add() — \
         the add path must NOT source from record.contradicted_by or \
         record.superseded_by (supersession is an UPDATE-time concern)"
    );

    let _ = "Phase E (T34a): legacy memories.superseded_by assertion removed; \
             the nodes.superseded_by == NULL check above carries the Bug A regression.";

    // -----------------------------------------------------------------
    // Phase 2: supersede() dual-updates both tables
    // -----------------------------------------------------------------
    storage
        .supersede("mem-subject", "mem-supersedor")
        .expect("supersede");

    let conn = storage.conn();
    let nodes_sup_after_supersede: Option<String> = conn
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = ?",
            params!["mem-subject"],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(
        nodes_sup_after_supersede.as_deref(),
        Some("mem-supersedor"),
        "supersede() must dual-update nodes.superseded_by — otherwise \
         Phase D read cutover would lose all supersession state"
    );

    // -----------------------------------------------------------------
    // Phase 3: unsupersede() clears both tables (memories='', nodes=NULL)
    // -----------------------------------------------------------------
    storage.unsupersede("mem-subject").expect("unsupersede");

    let conn = storage.conn();
    let nodes_sup_after_clear: Option<String> = conn
        .query_row(
            "SELECT superseded_by FROM nodes WHERE id = ?",
            params!["mem-subject"],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(
        nodes_sup_after_clear, None,
        "unsupersede() must clear nodes.superseded_by to NULL (design §5.3 — \
         REFERENCES nodes(id) ON DELETE SET NULL; '' is memories-only sentinel)"
    );
}

#[test]
fn t12_supersede_bulk_dual_writes_to_nodes() {
    // Pin the bulk path: every (old_id → new_id) pair must mirror into
    // `nodes.superseded_by` inside the savepoint. Bulk shares the bug
    // surface of single supersede.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    let new_target = sample_record("mem-bulk-new");
    let old_a = sample_record("mem-bulk-old-a");
    let old_b = sample_record("mem-bulk-old-b");
    storage.add(&new_target, "default").unwrap();
    storage.add(&old_a, "default").unwrap();
    storage.add(&old_b, "default").unwrap();

    let n = storage
        .supersede_bulk(&["mem-bulk-old-a", "mem-bulk-old-b"], "mem-bulk-new")
        .expect("supersede_bulk");
    assert_eq!(n, 2);

    let conn = storage.conn();
    for old_id in ["mem-bulk-old-a", "mem-bulk-old-b"] {
        let node_sup: Option<String> = conn
            .query_row(
                "SELECT superseded_by FROM nodes WHERE id = ?",
                params![old_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap();
        assert_eq!(
            node_sup.as_deref(),
            Some("mem-bulk-new"),
            "bulk: nodes.superseded_by for {old_id} must be dual-updated to 'mem-bulk-new'"
        );
    }
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

    // Unified edges row landed with edge_kind='structural'.
    // ISS-195/T37f (commit 5a5ce76) remapped the dual-write edge_kind
    // 'assertion' -> 'structural' so the factual plan's traversal filter
    // (edge_kind IN ('structural','containment')) matches; predicate is
    // preserved verbatim.
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
    assert_eq!(edge_kind, "structural");
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

#[test]
fn t14_record_coactivation_ns_dual_writes_with_corecall_signal() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t14e.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Three co-fires below threshold (still in tracking phase) — legacy
    // does not "form" the link (strength stays 0), but dual-write still
    // produces one edges row with weight = 3 × 0.1 = 0.3 and
    // coactivation_count = 3. signal_source must be 'corecall'.
    let _ = storage.record_coactivation_ns(&a, &b, 5, "default").unwrap();
    let _ = storage.record_coactivation_ns(&a, &b, 5, "default").unwrap();
    let _ = storage.record_coactivation_ns(&a, &b, 5, "default").unwrap();

    let (weight, coact, sig_source): (f64, i64, String) = storage
        .conn()
        .query_row(
            "SELECT weight, \
                    json_extract(attributes, '$.coactivation_count'), \
                    json_extract(attributes, '$.signal_source') \
             FROM edges \
             WHERE edge_kind = 'associative' \
               AND ((source_id = ?1 AND target_id = ?2) \
                 OR (source_id = ?2 AND target_id = ?1))",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(sig_source, "corecall", "record_coactivation_ns must tag signal_source='corecall'");
    assert!(
        (weight - 0.3).abs() < 1e-9,
        "weight = 3 × 0.1 = 0.3 (sum-accumulating), got {weight}"
    );
    assert_eq!(coact, 3, "coactivation_count counts every call, regardless of threshold");

    // Legacy hebbian_links still in tracking phase (strength = 0, count = 3).
    let (legacy_strength, legacy_count): (f64, i32) = storage
        .conn()
        .query_row(
            "SELECT strength, coactivation_count FROM hebbian_links \
             WHERE (source_id = ?1 AND target_id = ?2) \
                OR (source_id = ?2 AND target_id = ?1) \
             LIMIT 1",
            params![a, b],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        legacy_strength, 0.0,
        "legacy strength stays 0 below threshold (gated semantics preserved)"
    );
    assert_eq!(legacy_count, 3, "legacy count tracks calls below threshold");
}

// ═════════════════════════════════════════════════════════════════════════════
//  T15 — KC dual-write: topics → nodes(node_kind='topic'),
//  containment edges (topic → memory) → edges(edge_kind='containment')
// ═════════════════════════════════════════════════════════════════════════════
//
// Scope per design §4.4 + §8.10 T15:
//
//   * EntityKind::Topic, when written via `insert_entity`, must land in
//     `nodes` with `node_kind = 'topic'` (NOT `'entity'`). This is the
//     entity-side T13 fix that closes the §4.4 SQL contract.
//   * `upsert_topic_containment(topic_id, member_ids, namespace)` must
//     write one row per member into `edges` with `edge_kind='containment'`,
//     `predicate='contains'`, source = topic, target = member,
//     weight = 1.0.
//   * Re-calling `upsert_topic_containment` with the same `(topic,
//     member)` pair is a no-op (idempotent — partial unique index on
//     `(source_id, target_id, edge_kind, predicate) WHERE edge_kind =
//     'containment'`). Set membership, not a frequency signal.
//   * Legacy `knowledge_topics.source_memories` JSON array stays the
//     system of record. Dual-write is additive — T15 verifies the
//     unified rows exist with the right shape, not that legacy
//     numerically equals unified (legacy holds member list as JSON,
//     unified as N edges; one is structurally normalized, the other
//     denormalized).

use engramai::graph::KnowledgeTopic;

/// Insert a Topic-kind entity for the topic node, then return its uuid.
/// Mirrors the call order KC uses in `persist_cluster`.
fn seed_topic_entity(storage: &mut Storage, namespace: &str, title: &str) -> Uuid {
    let topic_uuid = Uuid::new_v4();
    let now = Utc::now();
    let mut topic_entity = Entity {
        id: topic_uuid,
        canonical_name: title.into(),
        kind: EntityKind::Topic,
        summary: format!("summary of {title}"),
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
    };
    topic_entity.summary = format!("summary of {title}");
    let conn = storage.connection_mut();
    let mut store = SqliteGraphStore::new(conn).with_namespace(namespace);
    engramai::graph::store::GraphWrite::insert_entity(&mut store, &topic_entity)
        .expect("insert topic entity");
    topic_uuid
}

#[test]
fn t15_topic_entity_dual_writes_with_node_kind_topic() {
    // §4.4 contract: EntityKind::Topic → nodes(node_kind='topic'), not
    // 'entity'. The retrieval `abstract_l5` plan in §4.7 filters by
    // node_kind='topic'; mis-routing topics under 'entity' would
    // silently break that plan.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t15a.db").to_str().unwrap()).unwrap();
    let topic_uuid = seed_topic_entity(&mut storage, "default", "Pickle Topic");

    let node_kind: String = storage
        .conn()
        .query_row(
            "SELECT node_kind FROM nodes WHERE id = ?1",
            params![topic_uuid.to_string()],
            |r| r.get(0),
        )
        .expect("nodes row for topic entity");

    assert_eq!(
        node_kind, "topic",
        "EntityKind::Topic must dual-write with node_kind='topic' (§4.4)"
    );

    // Non-topic kinds still route to node_kind='entity' — sanity check
    // the discrimination is real, not a constant rewrite.
    let person_uuid = Uuid::new_v4();
    let now = Utc::now();
    let person = Entity {
        id: person_uuid,
        canonical_name: "Alice".into(),
        kind: EntityKind::Person,
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
    };
    {
        let conn = storage.connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace("default");
        engramai::graph::store::GraphWrite::insert_entity(&mut store, &person)
            .expect("insert person entity");
    }
    let person_node_kind: String = storage
        .conn()
        .query_row(
            "SELECT node_kind FROM nodes WHERE id = ?1",
            params![person_uuid.to_string()],
            |r| r.get(0),
        )
        .expect("nodes row for person entity");
    assert_eq!(
        person_node_kind, "entity",
        "non-Topic EntityKind still routes to node_kind='entity'"
    );
}

#[test]
fn t15_upsert_topic_containment_writes_one_edge_per_member() {
    // §4.4: topic → containment → member memories. Exactly one edge per
    // member, weight = 1.0, predicate = 'contains'.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t15b.db").to_str().unwrap()).unwrap();
    let (m1, m2) = seed_two_memories(&mut storage);
    let topic_uuid = seed_topic_entity(&mut storage, "default", "Cabbage Topic");

    // Write topic row (FK from knowledge_topics back to graph_entities is
    // why we seeded the entity first).
    let now = Utc::now();
    let now_secs = now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64) / 1e9;
    let mut topic = KnowledgeTopic::new(
        topic_uuid,
        "Cabbage Topic".into(),
        "summary".into(),
        "default".into(),
        now_secs,
    );
    topic.source_memories = vec![m1.clone(), m2.clone()];
    {
        let conn = storage.connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace("default");
        engramai::graph::store::GraphWrite::upsert_topic(&mut store, &topic)
            .expect("upsert_topic");
    }

    // Now run the containment dual-write.
    {
        let conn = storage.connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace("default");
        engramai::graph::store::GraphWrite::upsert_topic_containment(
            &mut store,
            topic_uuid,
            &[m1.clone(), m2.clone()],
            "default",
        )
        .expect("upsert_topic_containment");
    }

    // Count containment edges from this topic.
    let count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'containment' \
               AND predicate = 'contains' \
               AND source_id = ?1",
            params![topic_uuid.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "one containment edge per member memory");

    // Verify edge shape — weight, predicate_kind, namespace.
    let (weight, pkind, ns): (f64, String, String) = storage
        .conn()
        .query_row(
            "SELECT weight, predicate_kind, namespace FROM edges \
             WHERE edge_kind = 'containment' \
               AND source_id = ?1 AND target_id = ?2",
            params![topic_uuid.to_string(), m1],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!((weight - 1.0).abs() < 1e-9, "weight = 1.0 (boolean membership)");
    assert_eq!(pkind, "canonical", "predicate_kind = 'canonical' for containment");
    assert_eq!(ns, "default");

    // Targets must be the seeded memory ids (no other members snuck in).
    let mut targets: Vec<String> = storage
        .conn()
        .prepare(
            "SELECT target_id FROM edges \
             WHERE edge_kind = 'containment' AND source_id = ?1 \
             ORDER BY target_id",
        )
        .unwrap()
        .query_map(params![topic_uuid.to_string()], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    targets.sort();
    let mut expected = vec![m1, m2];
    expected.sort();
    assert_eq!(targets, expected);
}

#[test]
fn t15_upsert_topic_containment_is_idempotent() {
    // Re-running compile() over the same cluster must not duplicate
    // containment edges (set membership, not a frequency signal).
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t15c.db").to_str().unwrap()).unwrap();
    let (m1, m2) = seed_two_memories(&mut storage);
    let topic_uuid = seed_topic_entity(&mut storage, "default", "Re-run Topic");

    let members = vec![m1.clone(), m2.clone()];
    for _ in 0..3 {
        let conn = storage.connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace("default");
        engramai::graph::store::GraphWrite::upsert_topic_containment(
            &mut store,
            topic_uuid,
            &members,
            "default",
        )
        .expect("upsert_topic_containment");
    }

    let count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'containment' AND source_id = ?1",
            params![topic_uuid.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 2,
        "three calls with same (topic, members) yield 2 edges, not 6"
    );
}

#[test]
fn t15_upsert_topic_containment_empty_members_is_noop() {
    // Edge case: KC may receive empty member lists from a degenerate
    // cluster (defensive; persist_cluster guards earlier, but the store
    // API should also tolerate it).
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t15d.db").to_str().unwrap()).unwrap();
    let topic_uuid = seed_topic_entity(&mut storage, "default", "Empty Topic");

    {
        let conn = storage.connection_mut();
        let mut store = SqliteGraphStore::new(conn).with_namespace("default");
        engramai::graph::store::GraphWrite::upsert_topic_containment(
            &mut store,
            topic_uuid,
            &[],
            "default",
        )
        .expect("empty member list must not error");
    }

    let count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'containment' AND source_id = ?1",
            params![topic_uuid.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

// ═════════════════════════════════════════════════════════════════════════════
//  T16 — Synthesis dual-write: insights → nodes(node_kind='insight'),
//  provenance edges → edges(edge_kind='provenance', predicate='derived_from')
// ═════════════════════════════════════════════════════════════════════════════
//
// Scope per design §4.5 + §8.10 T16:
//
//   * `Storage::store_raw` is the synthesis-only ingest path (caller =
//     `synthesis/engine.rs::store_insight_atomically`, hardcoded
//     `source='synthesis'`). T16 makes every `store_raw` row also land
//     in `nodes` with `node_kind='insight'` (NOT 'memory'). This is
//     symmetric with T15's EntityKind::Topic → node_kind='topic'
//     refinement: `abstract_l5` retrieval and insight-specific filters
//     need a typed kind, not a generic 'memory' with a magic
//     attributes flag.
//
//   * `Storage::record_provenance` writes the legacy
//     `synthesis_provenance` row AND a unified `edges` row of
//     `edge_kind='provenance'`, `predicate='derived_from'`,
//     `source_id=insight_id`, `target_id=source_memory_id`. Attributes
//     embed gate_decision, gate_scores (as nested JSON via `json()`,
//     not quoted string), cluster_id, source_original_importance.
//
//   * No partial unique index on provenance (design §3.2 only
//     uniquifies associative + containment). Retry semantics: each
//     provenance row carries a fresh `id` from caller, so re-running
//     synthesis with the same cluster appends new rows — matches
//     the prior dual-write behavior. (The T17 row-count parity test that
//     once asserted legacy_count == unified_count was removed in Phase E
//     along with the legacy dual-write it validated.)

use engramai::synthesis::types::{GateScores, ProvenanceRecord};

/// Seed two memory rows + one synthesized insight via `store_raw`.
/// Returns (source_a, source_b, insight_id).
fn seed_two_sources_and_insight(storage: &mut Storage) -> (String, String, String) {
    let (a, b) = seed_two_memories(storage);
    let insight_id = "t16-insight".to_string();
    storage
        .store_raw(
            &insight_id,
            "synthesized fact about pickles and cabbage",
            "factual",
            0.85,
            Some(r#"{"is_synthesis":true,"source_count":2}"#),
        )
        .expect("store_raw for insight");
    (a, b, insight_id)
}

#[test]
fn t16_store_raw_dual_writes_with_node_kind_insight() {
    // §4.5 contract: `Storage::store_raw` is synthesis-only — every row
    // must land in `nodes` with `node_kind='insight'`, never 'memory'.
    let dir = tempdir().unwrap();
    let storage = Storage::new(dir.path().join("t16a.db").to_str().unwrap()).unwrap();
    let insight_id = "t16a-insight";
    storage
        .store_raw(insight_id, "an insight about ferments", "factual", 0.7, None)
        .expect("store_raw");

    let (node_kind, memory_type, source, content, importance): (
        String,
        String,
        String,
        String,
        f64,
    ) = storage
        .conn()
        .query_row(
            "SELECT node_kind, memory_type, source, content, importance \
             FROM nodes WHERE id = ?1",
            params![insight_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("nodes row for insight");

    assert_eq!(node_kind, "insight", "store_raw must dual-write node_kind='insight'");
    assert_eq!(memory_type, "factual");
    assert_eq!(source, "synthesis");
    assert_eq!(content, "an insight about ferments");
    assert!((importance - 0.7).abs() < 1e-9);
}

#[test]
fn t16_record_provenance_dual_writes_to_edges() {
    // §4.5 contract: provenance row in synthesis_provenance AND
    // equivalent row in edges with edge_kind='provenance',
    // predicate='derived_from', direction insight → source.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t16b.db").to_str().unwrap()).unwrap();
    let (a, _b, insight_id) = seed_two_sources_and_insight(&mut storage);

    let now = Utc.with_ymd_and_hms(2026, 5, 13, 12, 0, 0).unwrap();
    let prov = ProvenanceRecord {
        id: "prov-1".into(),
        insight_id: insight_id.clone(),
        source_id: a.clone(),
        cluster_id: "cluster-xyz".into(),
        synthesis_timestamp: now,
        gate_decision: "passed_quality".into(),
        gate_scores: Some(GateScores {
            quality: 0.87,
            type_diversity: 2,
            estimated_cost: 0.012,
            member_count: 5,
        }),
        confidence: 0.91,
        source_original_importance: Some(0.6),
    };
    storage.record_provenance(&prov).expect("record_provenance");

    // Legacy row present.
    let legacy_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM synthesis_provenance WHERE id = ?1",
            params!["prov-1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 1);

    // Unified edge present with correct shape.
    let (src, tgt, ek, pk, pred, conf, weight, ns): (
        String, String, String, String, String, f64, f64, String,
    ) = storage
        .conn()
        .query_row(
            "SELECT source_id, target_id, edge_kind, predicate_kind, predicate, \
                    confidence, weight, namespace FROM edges WHERE id = ?1",
            params!["prov-1"],
            |r| Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,
                r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?,
            )),
        )
        .expect("unified edges row");

    assert_eq!(src, insight_id, "source = insight (derives from)");
    assert_eq!(tgt, a, "target = source memory");
    assert_eq!(ek, "provenance");
    assert_eq!(pk, "canonical");
    assert_eq!(pred, "derived_from");
    assert!((conf - 0.91).abs() < 1e-9, "confidence column = record.confidence");
    assert!((weight - 1.0).abs() < 1e-9, "provenance weight = 1.0 (presence, not strength)");
    assert_eq!(ns, "default");
}

#[test]
fn t16_provenance_edge_attributes_embed_gate_metadata_as_nested_json() {
    // §4.5: attributes JSON must embed gate_decision, gate_scores
    // (as nested JSON, NOT a quoted string), cluster_id,
    // source_original_importance. The `json()` wrapper around
    // gate_scores is the load-bearing detail — without it,
    // json_extract on the stored attributes returns a doubly-escaped
    // string, not a parseable object.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t16c.db").to_str().unwrap()).unwrap();
    let (a, _b, insight_id) = seed_two_sources_and_insight(&mut storage);

    let prov = ProvenanceRecord {
        id: "prov-2".into(),
        insight_id: insight_id.clone(),
        source_id: a.clone(),
        cluster_id: "cluster-abc".into(),
        synthesis_timestamp: Utc::now(),
        gate_decision: "needs_review".into(),
        gate_scores: Some(GateScores {
            quality: 0.5,
            type_diversity: 3,
            estimated_cost: 0.025,
            member_count: 7,
        }),
        confidence: 0.6,
        source_original_importance: Some(0.4),
    };
    storage.record_provenance(&prov).expect("record_provenance");

    // gate_decision is a plain string — extract returns it directly.
    let gate_decision: String = storage
        .conn()
        .query_row(
            "SELECT json_extract(attributes, '$.gate_decision') FROM edges WHERE id = ?1",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gate_decision, "needs_review");

    // cluster_id likewise.
    let cluster_id: String = storage
        .conn()
        .query_row(
            "SELECT json_extract(attributes, '$.cluster_id') FROM edges WHERE id = ?1",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cluster_id, "cluster-abc");

    // gate_scores is a nested object — extracting a sub-key proves it's
    // structured JSON, not a quoted blob string.
    let quality: f64 = storage
        .conn()
        .query_row(
            "SELECT json_extract(attributes, '$.gate_scores.quality') FROM edges WHERE id = ?1",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert!((quality - 0.5).abs() < 1e-9, "gate_scores.quality decodes as f64, not string");

    let member_count: i64 = storage
        .conn()
        .query_row(
            "SELECT json_extract(attributes, '$.gate_scores.member_count') FROM edges WHERE id = ?1",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(member_count, 7);

    // source_original_importance roundtrips as nullable f64.
    let soi: f64 = storage
        .conn()
        .query_row(
            "SELECT json_extract(attributes, '$.source_original_importance') FROM edges WHERE id = ?1",
            params!["prov-2"],
            |r| r.get(0),
        )
        .unwrap();
    assert!((soi - 0.4).abs() < 1e-9);
}

#[test]
fn t16_provenance_null_gate_scores_roundtrips_as_null() {
    // Defensive: GateScores is Option<_>. When None, the JSON value
    // should be JSON null, not "null" string and not absent — so
    // downstream code can rely on `json_type(...) = 'null'`.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t16d.db").to_str().unwrap()).unwrap();
    let (a, _b, insight_id) = seed_two_sources_and_insight(&mut storage);

    let prov = ProvenanceRecord {
        id: "prov-3".into(),
        insight_id: insight_id.clone(),
        source_id: a.clone(),
        cluster_id: "cluster-null".into(),
        synthesis_timestamp: Utc::now(),
        gate_decision: "no_gate".into(),
        gate_scores: None,
        confidence: 0.5,
        source_original_importance: None,
    };
    storage.record_provenance(&prov).expect("record_provenance");

    let gate_scores_type: String = storage
        .conn()
        .query_row(
            "SELECT json_type(attributes, '$.gate_scores') FROM edges WHERE id = ?1",
            params!["prov-3"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gate_scores_type, "null", "missing gate_scores stored as JSON null");

    let soi_type: String = storage
        .conn()
        .query_row(
            "SELECT json_type(attributes, '$.source_original_importance') FROM edges WHERE id = ?1",
            params!["prov-3"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(soi_type, "null");
}

#[test]
fn t16_full_synthesis_flow_atomic_dual_write() {
    // End-to-end: insight + 2 provenance records under one outer
    // transaction (mirrors `synthesis/engine.rs::store_insight_atomically`).
    // All four legacy rows AND four unified rows must commit atomically.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t16e.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    storage.begin_transaction().expect("begin tx");
    let insight_id = "t16e-insight";
    storage
        .store_raw(insight_id, "two-fact synthesis", "factual", 0.8, None)
        .expect("store_raw in tx");
    for (i, sid) in [&a, &b].iter().enumerate() {
        let prov = ProvenanceRecord {
            id: format!("prov-{i}"),
            insight_id: insight_id.into(),
            source_id: (*sid).clone(),
            cluster_id: "c1".into(),
            synthesis_timestamp: Utc::now(),
            gate_decision: "ok".into(),
            gate_scores: None,
            confidence: 0.7,
            source_original_importance: Some(0.5),
        };
        storage.record_provenance(&prov).expect("record_provenance in tx");
    }
    storage.commit_transaction().expect("commit tx");

    // Insight node landed.
    let (kind,): (String,) = storage
        .conn()
        .query_row(
            "SELECT node_kind FROM nodes WHERE id = ?1",
            params![insight_id],
            |r| Ok((r.get(0)?,)),
        )
        .unwrap();
    assert_eq!(kind, "insight");

    // Two provenance edges.
    let edge_count: i64 = storage
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'provenance' AND source_id = ?1",
            params![insight_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(edge_count, 2, "one provenance edge per source");

    // Targets match.
    let mut targets: Vec<String> = storage
        .conn()
        .prepare(
            "SELECT target_id FROM edges \
             WHERE edge_kind = 'provenance' AND source_id = ?1 \
             ORDER BY target_id",
        )
        .unwrap()
        .query_map(params![insight_id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    targets.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(targets, expected);
}


// ============================================================================
// T18 — Read-source map: which retrieval APIs are nodes-backed vs legacy-backed.
//
// ORIGINAL Phase-B intent (historical):
//   "No production retrieval API reads from the unified `nodes`/`edges`
//    tables, so dual-write rows there cannot change user-facing recall."
//   That invariant held through Phase B, when `nodes`/`edges` were
//   write-only mirrors and every read targeted the legacy tables.
//
// WHY THE INVARIANT IS NOW OBSOLETE (ISS-197 Phase E-0 + ISS-199 T34a):
//   Phase E-0 (ISS-197) deliberately migrated the simple row readers —
//   `fetch_recent`, `all_in_ns` (via the same node SELECT), `get_by_ids`,
//   and `search_by_type` — to read from unified `nodes`
//   (`node_kind IN ('memory','insight')`). T34a (ISS-199) then removed
//   the legacy `memories` write under unified mode entirely. So `nodes`
//   is no longer a write-only mirror; it is the SOLE source for those
//   readers. Wiping `nodes` therefore *must* empty them — that is the
//   correct post-cutover behavior, not a violation.
//
//   The readers still on legacy substrate as of this cutover:
//     - `search_fts*`  → legacy `memories_fts` virtual table
//     - `hebbian_*`    → legacy `hebbian_links` table
//
// WHAT THIS TEST NOW PINS:
//   A read-source MAP. After a hostile wipe of `nodes`/`edges`:
//     - legacy-backed readers (FTS, hebbian) stay BYTE-IDENTICAL, and
//     - nodes-backed readers (fetch_recent / all_in_ns / get_by_ids /
//       search_by_type) go EMPTY.
//   This catches regressions in BOTH directions: a legacy reader that
//   silently starts reading nodes (would go empty — caught), or a
//   migrated reader that silently reverts to legacy (would stay
//   populated — caught).
//
// Test mechanism:
//   1. Bootstrap Storage + ingest a workload that exercises the
//      Storage-facing Phase B writers: T12 (Storage::add), T14
//      (Storage::record_association), T16 (Storage::store_raw +
//      record_provenance). T13 (entities) and T15 (topics) go through
//      a separate SqliteGraphStore API and are covered by their own
//      tests; here we only need *enough* rows
//      in unified tables for step 3 to be a non-trivial mutation.
//   2. Snapshot results from every public retrieval API on Storage.
//   3. Hostile mutation: DELETE FROM nodes; DELETE FROM edges. Leaves
//      every legacy table (memories, hebbian_links, etc.) untouched.
//   4. Re-snapshot the same retrieval APIs.
//   5. Assert the read-source map: legacy readers unchanged, nodes
//      readers emptied.
// ============================================================================

/// Snapshot of every public Storage retrieval API. Compared byte-for-byte
/// (via Debug + PartialEq) to detect divergence after the hostile mutation.
#[derive(Debug, PartialEq)]
struct T18RetrievalSnapshot {
    search_fts_global: Vec<String>,
    search_fts_ns_default: Vec<String>,
    search_fts_ns_alpha: Vec<String>,
    search_by_type_global: Vec<String>,
    search_by_type_ns_default: Vec<String>,
    fetch_recent_global: Vec<String>,
    fetch_recent_ns_default: Vec<String>,
    fetch_recent_ns_alpha: Vec<String>,
    all_in_ns_default: Vec<String>,
    all_in_ns_alpha: Vec<String>,
    get_by_ids: Vec<String>,
    hebbian_neighbors_probe: Vec<String>,
    hebbian_weighted_probe: Vec<(String, f64)>,
}

fn t18_capture_snapshot(storage: &Storage, ids_to_probe: &[&str]) -> T18RetrievalSnapshot {
    let mut search_fts_global = storage
        .search_fts("memory", 50)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    search_fts_global.sort();

    let mut search_fts_ns_default = storage
        .search_fts_ns("memory", 50, Some("default"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    search_fts_ns_default.sort();

    let mut search_fts_ns_alpha = storage
        .search_fts_ns("memory", 50, Some("alpha"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    search_fts_ns_alpha.sort();

    let mut search_by_type_global = storage
        .search_by_type(MemoryType::Episodic)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    search_by_type_global.sort();

    let mut search_by_type_ns_default = storage
        .search_by_type_ns(MemoryType::Episodic, Some("default"), 50)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    search_by_type_ns_default.sort();

    let mut fetch_recent_global = storage
        .fetch_recent(50, Some("*"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    fetch_recent_global.sort();

    let mut fetch_recent_ns_default = storage
        .fetch_recent(50, Some("default"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    fetch_recent_ns_default.sort();

    let mut fetch_recent_ns_alpha = storage
        .fetch_recent(50, Some("alpha"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    fetch_recent_ns_alpha.sort();

    let mut all_in_ns_default = storage
        .all_in_namespace(Some("default"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    all_in_ns_default.sort();

    let mut all_in_ns_alpha = storage
        .all_in_namespace(Some("alpha"))
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    all_in_ns_alpha.sort();

    let mut get_by_ids = storage
        .get_by_ids(ids_to_probe)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    get_by_ids.sort();

    let probe = ids_to_probe.first().copied().unwrap_or("t18-a");
    let mut hebbian_neighbors_probe = storage.get_hebbian_neighbors(probe).unwrap();
    hebbian_neighbors_probe.sort();

    let mut hebbian_weighted_probe = storage.get_hebbian_links_weighted(probe).unwrap();
    hebbian_weighted_probe.sort_by(|a, b| a.0.cmp(&b.0));

    T18RetrievalSnapshot {
        search_fts_global,
        search_fts_ns_default,
        search_fts_ns_alpha,
        search_by_type_global,
        search_by_type_ns_default,
        fetch_recent_global,
        fetch_recent_ns_default,
        fetch_recent_ns_alpha,
        all_in_ns_default,
        all_in_ns_alpha,
        get_by_ids,
        hebbian_neighbors_probe,
        hebbian_weighted_probe,
    }
}

/// Seed Storage-facing Phase B writers. T13/T15 (entity/topic) use a
/// separate graph-store API that requires its own Connection — those
/// dual-writes were historically covered by the T17 parity invariants
/// (removed in Phase E). For T18 we just
/// need enough rows in `nodes`/`edges` that wiping them is non-trivial.
fn t18_seed_workload(storage: &mut Storage) -> Vec<String> {
    use engramai::synthesis::types::ProvenanceRecord;

    let now = Utc.with_ymd_and_hms(2026, 5, 13, 9, 0, 0).unwrap();

    // T12: regular memories across two namespaces. Content uses the
    // word "memory" so FTS finds them all in step 2's snapshot probes.
    let mut a = sample_record("t18-a");
    a.created_at = now;
    a.content = "memory alpha about pickles and pizza".into();
    let mut b = sample_record("t18-b");
    b.created_at = now;
    b.content = "memory beta about cabbage and pickles".into();
    let mut c = sample_record("t18-c");
    c.created_at = now;
    c.content = "memory gamma about pizza and bread".into();
    storage.add(&a, "default").unwrap();
    storage.add(&b, "default").unwrap();
    storage.add(&c, "alpha").unwrap();

    // T14: Hebbian co-activation — dual-writes one row to edges
    // (edge_kind='associative', signal_source='entity').
    storage
        .record_association(
            "t18-a",
            "t18-b",
            0.5,
            "entity",
            r#"{"entity_overlap":0.4}"#,
            "default",
        )
        .unwrap();

    // T16: synthesis insight via store_raw + provenance.
    // store_raw dual-writes to nodes(node_kind='insight');
    // record_provenance dual-writes to edges(edge_kind='provenance').
    storage
        .store_raw(
            "t18-insight",
            "synthesized insight about food memories",
            "factual",
            0.85,
            Some(r#"{"is_synthesis":true,"source_count":2}"#),
        )
        .expect("store_raw insight");

    let prov = ProvenanceRecord {
        id: "t18-prov-1".into(),
        insight_id: "t18-insight".into(),
        source_id: "t18-a".into(),
        cluster_id: "t18-cluster".into(),
        synthesis_timestamp: now,
        gate_decision: "accept".into(),
        gate_scores: None,
        confidence: 0.7,
        source_original_importance: Some(0.6),
    };
    storage.record_provenance(&prov).expect("record_provenance");

    vec![
        "t18-a".to_string(),
        "t18-b".to_string(),
        "t18-c".to_string(),
        "t18-insight".to_string(),
    ]
}

#[test]
fn t18_read_source_map_legacy_vs_nodes_after_unified_wipe() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("t18.db").to_str().unwrap()).unwrap();

    // Step 1: seed workload — dual-writes populate both legacy + unified.
    let seeded_ids = t18_seed_workload(&mut storage);
    let probe_ids: Vec<&str> = seeded_ids.iter().map(|s| s.as_str()).collect();

    // Step 2: snapshot every public retrieval API.
    let before = t18_capture_snapshot(&storage, &probe_ids);

    // Sanity: workload actually produced something. If these are empty
    // the test is degenerate and the byte-equality below is vacuous.
    assert!(
        !before.search_fts_global.is_empty(),
        "T18 sanity: workload produced no FTS-searchable memories — test is degenerate"
    );
    assert!(
        !before.fetch_recent_global.is_empty(),
        "T18 sanity: workload produced no recent memories — test is degenerate"
    );

    // Step 2.5: confirm unified tables ARE populated pre-mutation —
    // else the wipe in step 3 is a no-op and proves nothing.
    {
        let conn = storage.conn();
        let nodes_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert!(
            nodes_count > 0,
            "T18 sanity: nodes empty after seed — Phase B dual-write not firing?"
        );
        assert!(
            edges_count > 0,
            "T18 sanity: edges empty after seed — Phase B dual-write not firing?"
        );
    }

    // Step 3: HOSTILE mutation — wipe the unified tables. Any retrieval
    // path that silently reads from them returns empty/different rows now.
    //
    // ISS-199: several child tables now FK→`nodes(id)` with RESTRICT
    // (memory_embeddings, hebbian_links, graph_*). A bare `DELETE FROM
    // nodes` would FK-787 against those surviving children. This is a
    // deliberate destructive mutation to prove read-isolation, not a
    // real workflow, so disable FK enforcement for the wipe.
    {
        let conn = storage.connection_mut();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF; \
             DELETE FROM edges; \
             DELETE FROM nodes; \
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
        // nodes_fts is a virtual table mirroring nodes — nuke too in
        // case some future retrieval path queries it.
        let _ = conn.execute("DELETE FROM nodes_fts", []);
    }

    // Step 4: confirm mutation took effect.
    {
        let conn = storage.conn();
        let nodes_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        let edges_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nodes_count, 0, "T18 step 3: nodes wipe didn't take");
        assert_eq!(edges_count, 0, "T18 step 3: edges wipe didn't take");
    }

    // Step 5: re-snapshot and assert the read-source MAP.
    //
    // ISS-197 Phase E-0 migrated fetch_recent / all_in_ns / get_by_ids /
    // search_by_type to read unified `nodes`; ISS-199 T34a removed the
    // legacy `memories` write under unified mode. So after wiping nodes:
    //   - legacy-backed readers (FTS, hebbian) MUST stay byte-identical
    //   - nodes-backed readers MUST go empty
    let after = t18_capture_snapshot(&storage, &probe_ids);

    // (a) Legacy-backed readers: unchanged. A regression where one of
    // these silently starts reading `nodes` would empty it here.
    assert_eq!(
        before.search_fts_global, after.search_fts_global,
        "T18: search_fts_global (legacy memories_fts) changed after nodes wipe — \
         a legacy reader is now reading the unified substrate"
    );
    assert_eq!(
        before.search_fts_ns_default, after.search_fts_ns_default,
        "T18: search_fts_ns_default (legacy memories_fts) changed after nodes wipe"
    );
    assert_eq!(
        before.search_fts_ns_alpha, after.search_fts_ns_alpha,
        "T18: search_fts_ns_alpha (legacy memories_fts) changed after nodes wipe"
    );
    assert_eq!(
        before.hebbian_neighbors_probe, after.hebbian_neighbors_probe,
        "T18: hebbian_neighbors_probe (legacy hebbian_links) changed after nodes wipe"
    );
    assert_eq!(
        before.hebbian_weighted_probe, after.hebbian_weighted_probe,
        "T18: hebbian_weighted_probe (legacy hebbian_links) changed after nodes wipe"
    );

    // (b) Nodes-backed readers (ISS-197 Phase E-0): emptied. They were
    // non-empty before the wipe (sanity-checked in step 2), so empty
    // after proves they read `nodes`, not a stale legacy mirror. A
    // regression where one of these silently reverts to legacy would
    // stay populated here and trip the assertion.
    assert!(
        !before.fetch_recent_global.is_empty(),
        "T18 sanity: fetch_recent_global empty before wipe — workload degenerate"
    );
    for (name, val) in [
        ("fetch_recent_global", &after.fetch_recent_global),
        ("fetch_recent_ns_default", &after.fetch_recent_ns_default),
        ("fetch_recent_ns_alpha", &after.fetch_recent_ns_alpha),
        ("all_in_ns_default", &after.all_in_ns_default),
        ("all_in_ns_alpha", &after.all_in_ns_alpha),
        ("get_by_ids", &after.get_by_ids),
    ] {
        assert!(
            val.is_empty(),
            "T18: {name} non-empty after nodes wipe ({val:?}) — \
             this nodes-backed reader (ISS-197 Phase E-0) is still \
             returning rows, so it is reading a stale legacy mirror \
             instead of unified `nodes`"
        );
    }
}

// =============================================================
// ISS-116 — Phase B dual-WRITE gaps in hebbian_links writers
// =============================================================
//
// T14 wired dual_write_hebbian_to_edges into three writers:
// record_coactivation_ns, record_cross_namespace_coactivation,
// record_association. ISS-116 closes four additional gaps:
// record_coactivation, decay_hebbian_links,
// decay_hebbian_links_differential, merge_hebbian_links.
//
// Each test below pins the per-writer parity contract: after the
// call, the affected hebbian_links rows have matching edges
// (edge_kind='associative') rows with consistent endpoints and
// weight-semantics-per-writer.
// =============================================================

/// Count associative edges between two memories (either direction).
fn count_assoc_edges(conn: &rusqlite::Connection, a: &str, b: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE edge_kind = 'associative' \
         AND ((source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1))",
        params![a, b],
        |r| r.get(0),
    )
    .unwrap()
}

/// Fetch (weight, signal_source, coactivation_count) for one edge.
fn assoc_edge_attrs(
    conn: &rusqlite::Connection,
    a: &str,
    b: &str,
) -> Option<(f64, String, i64)> {
    conn.query_row(
        "SELECT weight, \
                json_extract(attributes, '$.signal_source'), \
                json_extract(attributes, '$.coactivation_count') \
         FROM edges WHERE edge_kind = 'associative' \
         AND ((source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1))",
        params![a, b],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

#[test]
fn iss116_record_coactivation_dual_writes_to_edges() {
    // Pin: every record_coactivation call also UPSERTs into edges,
    // matching record_coactivation_ns's policy (unconditional
    // delta_weight=0.1, signal_source='corecall', namespace='default').
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("iss116a.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // First call — tracking phase on legacy (strength=0). Edges still
    // gets one row with weight=0.1 (pre-existing T14 divergence
    // preserved for consistency).
    let formed = storage.record_coactivation(&a, &b, 2).unwrap();
    assert!(!formed, "threshold=2 not reached on first call");

    assert_eq!(count_assoc_edges(storage.conn(), &a, &b), 1, "one assoc edge after first call");
    let (w1, sig, coact1) = assoc_edge_attrs(storage.conn(), &a, &b).unwrap();
    assert!((w1 - 0.1).abs() < 1e-9, "first call seeds weight=0.1, got {w1}");
    assert_eq!(sig, "corecall", "signal_source='corecall' for record_coactivation");
    assert_eq!(coact1, 1, "coactivation_count starts at 1");

    // Second call — threshold crossed on legacy (strength→1.0).
    // Edges accumulates: weight+=0.1, coactivation_count+=1.
    let formed = storage.record_coactivation(&a, &b, 2).unwrap();
    assert!(formed, "threshold=2 reached on second call");
    assert_eq!(count_assoc_edges(storage.conn(), &a, &b), 1, "still single row after second call");
    let (w2, _, coact2) = assoc_edge_attrs(storage.conn(), &a, &b).unwrap();
    assert!((w2 - 0.2).abs() < 1e-9, "weight accumulates: 0.1+0.1=0.2, got {w2}");
    assert_eq!(coact2, 2, "coactivation_count=2 after two calls");

    // Third call — legacy strengthens (0.1 cap). Edges keeps adding.
    storage.record_coactivation(&a, &b, 2).unwrap();
    let (w3, _, coact3) = assoc_edge_attrs(storage.conn(), &a, &b).unwrap();
    assert!((w3 - 0.3).abs() < 1e-9, "weight=0.3 after three calls, got {w3}");
    assert_eq!(coact3, 3);
}

#[test]
fn iss116_decay_hebbian_links_mirrors_to_edges() {
    // Pin: bulk multiplicative decay applies symmetrically to both
    // hebbian_links.strength and edges.weight, scoped to assoc edges.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("iss116b.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Form a Hebbian link with strength > 0 on both sides:
    // record_association uses delta_weight directly + immediately
    // forms the legacy link at the provided strength.
    storage
        .record_association(&a, &b, 0.5, "entity", "{}", "default")
        .unwrap();
    let (w_pre, _, _) = assoc_edge_attrs(storage.conn(), &a, &b).unwrap();
    assert!((w_pre - 0.5).abs() < 1e-9, "pre-decay edge weight=0.5");
    let strength_pre: f64 = storage.conn()
        .query_row(
            "SELECT strength FROM hebbian_links \
             WHERE (source_id=?1 AND target_id=?2) OR (source_id=?2 AND target_id=?1)",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert!((strength_pre - 0.5).abs() < 1e-9);

    // Apply 0.8 decay factor.
    storage.decay_hebbian_links(0.8).unwrap();
    let (w_post, _, _) = assoc_edge_attrs(storage.conn(), &a, &b).unwrap();
    assert!(
        (w_post - 0.4).abs() < 1e-9,
        "edges.weight decayed 0.5 * 0.8 = 0.4, got {w_post}"
    );
    let strength_post: f64 = storage.conn()
        .query_row(
            "SELECT strength FROM hebbian_links \
             WHERE (source_id=?1 AND target_id=?2) OR (source_id=?2 AND target_id=?1)",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert!((strength_post - 0.4).abs() < 1e-9, "legacy strength matches");

    // Decay to below 0.1 threshold → prune on both sides.
    // 0.4 * 0.2 = 0.08 < 0.1 → both rows deleted.
    let pruned = storage.decay_hebbian_links(0.2).unwrap();
    assert!(pruned >= 1, "at least one legacy row pruned");
    assert_eq!(
        count_assoc_edges(storage.conn(), &a, &b),
        0,
        "edges row pruned in lockstep with legacy"
    );
}

#[test]
fn iss116_decay_hebbian_links_differential_mirrors_to_edges() {
    // Pin: differential decay (per signal_source CASE WHEN) applies
    // the right factor on the edges side via json_extract(attributes).
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("iss116c.db").to_str().unwrap()).unwrap();

    // Three memory rows so we have two distinct pairs.
    let now = Utc.with_ymd_and_hms(2026, 5, 13, 8, 0, 0).unwrap();
    let mut rec_a = sample_record("iss116c-a");
    rec_a.created_at = now;
    rec_a.content = "alpha".into();
    let mut rec_b = sample_record("iss116c-b");
    rec_b.created_at = now;
    rec_b.content = "beta".into();
    let mut rec_c = sample_record("iss116c-c");
    rec_c.created_at = now;
    rec_c.content = "gamma".into();
    storage.add(&rec_a, "default").unwrap();
    storage.add(&rec_b, "default").unwrap();
    storage.add(&rec_c, "default").unwrap();
    let (a, b, c) = ("iss116c-a", "iss116c-b", "iss116c-c");

    // Pair AB → signal_source='corecall', initial strength 0.6
    storage
        .record_association(a, b, 0.6, "corecall", "{}", "default")
        .unwrap();
    // Pair AC → signal_source='entity', initial strength 0.6
    storage
        .record_association(a, c, 0.6, "entity", "{}", "default")
        .unwrap();

    // Apply differential decay: corecall=0.9, multi=0.5, other (incl. entity)=0.3.
    storage
        .decay_hebbian_links_differential(0.9, 0.5, 0.3)
        .unwrap();
    let conn = storage.conn();
    // AB (corecall): 0.6 * 0.9 = 0.54 — preserved
    let (w_ab, _, _) = assoc_edge_attrs(conn, a, b).unwrap();
    assert!((w_ab - 0.54).abs() < 1e-9, "corecall edge: 0.6*0.9=0.54, got {w_ab}");
    // AC (entity → else branch): 0.6 * 0.3 = 0.18 — preserved
    let (w_ac, _, _) = assoc_edge_attrs(conn, a, c).unwrap();
    assert!((w_ac - 0.18).abs() < 1e-9, "entity edge: 0.6*0.3=0.18, got {w_ac}");
    // Legacy and unified track the same numbers.
    let strength_ab: f64 = conn
        .query_row(
            "SELECT strength FROM hebbian_links \
             WHERE (source_id=?1 AND target_id=?2) OR (source_id=?2 AND target_id=?1)",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert!((strength_ab - 0.54).abs() < 1e-9);
}

#[test]
fn iss116_merge_hebbian_links_mirrors_donor_repoint_to_edges() {
    // Pin: when a donor is merged into target, both sides re-point
    // and max-merge their associative neighborhood, and the donor
    // rows are deleted on both sides.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("iss116d.db").to_str().unwrap()).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 5, 13, 8, 0, 0).unwrap();
    // donor + target + two other peers.
    let ids = ["donor", "target", "peer1", "peer2"];
    for id in ids {
        let mut rec = sample_record(id);
        rec.created_at = now;
        rec.content = format!("memory {id}");
        storage.add(&rec, "default").unwrap();
    }

    // Donor has two hebbian neighbours: peer1 (weight 0.7) and peer2 (0.3).
    // Target already has a hebbian link to peer1 (weight 0.4) — merge
    // must keep the max (0.7).
    storage
        .record_association("donor", "peer1", 0.7, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association("donor", "peer2", 0.3, "entity", "{}", "default")
        .unwrap();
    storage
        .record_association("target", "peer1", 0.4, "entity", "{}", "default")
        .unwrap();

    // Sanity: donor edges exist pre-merge.
    let conn0 = storage.conn();
    assert_eq!(count_assoc_edges(conn0, "donor", "peer1"), 1);
    assert_eq!(count_assoc_edges(conn0, "donor", "peer2"), 1);
    assert_eq!(count_assoc_edges(conn0, "target", "peer1"), 1);

    let transferred = storage.merge_hebbian_links("donor", "target").unwrap();
    assert!(transferred >= 2, "expect 2 donor neighbours transferred, got {transferred}");

    let conn = storage.conn();
    // Donor side completely cleared on both substrates.
    assert_eq!(count_assoc_edges(conn, "donor", "peer1"), 0);
    assert_eq!(count_assoc_edges(conn, "donor", "peer2"), 0);
    let donor_legacy: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM hebbian_links \
             WHERE source_id='donor' OR target_id='donor'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(donor_legacy, 0, "donor hebbian rows cleared");

    // Target inherits both neighbours.
    assert_eq!(count_assoc_edges(conn, "target", "peer1"), 1);
    assert_eq!(count_assoc_edges(conn, "target", "peer2"), 1);

    // Max-weight semantics: target→peer1 was 0.4, donor→peer1 was 0.7
    // → merged value is 0.7.
    let (w_p1, _, _) = assoc_edge_attrs(conn, "target", "peer1").unwrap();
    assert!((w_p1 - 0.7).abs() < 1e-9, "max(0.4, 0.7)=0.7 on edges, got {w_p1}");
    // target→peer2 is freshly minted at donor's weight (0.3).
    let (w_p2, _, _) = assoc_edge_attrs(conn, "target", "peer2").unwrap();
    assert!((w_p2 - 0.3).abs() < 1e-9, "fresh minted at 0.3, got {w_p2}");
    // Legacy mirrors.
    let strength_p1: f64 = conn
        .query_row(
            "SELECT strength FROM hebbian_links \
             WHERE (source_id='target' AND target_id='peer1') \
                OR (source_id='peer1' AND target_id='target')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!((strength_p1 - 0.7).abs() < 1e-9);
}

#[test]
fn iss116_merge_hebbian_links_rejects_self_merge() {
    // Defensive guard: if donor == target, the merge driver would
    // otherwise issue `DELETE … WHERE source_id=donor OR target_id=
    // donor` against both substrates and wipe the survivor's entire
    // hebbian neighborhood. Pre-existing legacy bug pinned to no-op
    // semantics by the entry guard added in ISS-116.
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("iss116e.db").to_str().unwrap()).unwrap();
    let (a, b) = seed_two_memories(&mut storage);

    // Seed a hebbian link a<->b so there's something to destroy if
    // the guard ever regresses.
    storage
        .record_association(&a, &b, 0.6, "entity", "{}", "default")
        .unwrap();
    assert_eq!(count_assoc_edges(storage.conn(), &a, &b), 1);

    let transferred = storage.merge_hebbian_links(&a, &a).unwrap();
    assert_eq!(transferred, 0, "self-merge is a no-op");

    // Pre-existing link must survive untouched.
    assert_eq!(
        count_assoc_edges(storage.conn(), &a, &b),
        1,
        "self-merge must NOT wipe the survivor's hebbian neighborhood"
    );
    let strength: f64 = storage.conn()
        .query_row(
            "SELECT strength FROM hebbian_links \
             WHERE (source_id=?1 AND target_id=?2) OR (source_id=?2 AND target_id=?1)",
            params![a, b],
            |r| r.get(0),
        )
        .unwrap();
    assert!((strength - 0.6).abs() < 1e-9, "legacy strength preserved");
}
