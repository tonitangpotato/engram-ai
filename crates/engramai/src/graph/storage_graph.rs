//! # Graph-layer SQLite schema (DDL migrations)
//!
//! This module owns the **table-creation** half of the v0.3 graph layer.
//! It is paired with [`crate::graph::store`] which owns the CRUD methods.
//!
//! ## Why a separate module
//!
//! Keeping DDL textually separate from CRUD lets every Phase-2/3 method ship
//! with a roundtrip integration test against a real in-memory SQLite database
//! — not just a compile check. Schema↔code mismatches surface immediately
//! instead of leaking into Phase 4 transactional methods.
//!
//! ## Idempotence
//!
//! Every `CREATE TABLE` and `CREATE INDEX` statement uses `IF NOT EXISTS`,
//! and the `ALTER TABLE memories ADD COLUMN` statements are guarded against
//! "duplicate column" errors. [`init_graph_tables`] is therefore safe to call
//! on every `Storage::new` without a version-tracking gate. Calling it on a
//! fresh DB creates the schema; calling it on an upgraded DB is a no-op.
//!
//! ## Source of truth
//!
//! The DDL here matches `.gid/features/v03-graph-layer/design.md` §4.1
//! verbatim (modulo whitespace + the SQLite-flavor `IF NOT EXISTS` hardening).
//! If the design changes, update both — the design doc is the contract; this
//! module is the executable mirror.

use rusqlite::Connection;

use crate::graph::error::GraphError;

/// Initialize all graph-layer tables and indexes on `conn`.
///
/// Called from `Storage::new` after the v0.2 schema is created. Safe to call
/// repeatedly: every statement is idempotent (`IF NOT EXISTS` for tables/
/// indexes, "duplicate column" tolerance for `ALTER TABLE`).
///
/// **Foreign keys:** the caller must have already run `PRAGMA foreign_keys=ON`.
/// `Storage::new` does this in its first `execute_batch`.
///
/// **Returns** `Ok(())` on success, `GraphError::Storage` if any statement
/// fails for a reason other than "duplicate column".
pub fn init_graph_tables(conn: &Connection) -> Result<(), GraphError> {
    // Step 1: create graph tables + indexes (idempotent via IF NOT EXISTS).
    conn.execute_batch(GRAPH_DDL)?;

    // Step 2: additive ALTERs on the existing `memories` table.
    // SQLite has no "ADD COLUMN IF NOT EXISTS" — guard each ALTER by tolerating
    // the "duplicate column" error from a prior run.
    for (col, ddl) in MEMORIES_ALTERS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {
                // Column already added on a prior run; expected idempotent path.
            }
            Err(e) => {
                return Err(GraphError::Migration(format!(
                    "ALTER TABLE memories ADD COLUMN {col}: {e}"
                )));
            }
        }
    }

    // Step 3: additive ALTERs on `graph_entities` for columns added after
    // the initial v0.3 schema landed. Same idempotent pattern as memories.
    // These columns are also present in `GRAPH_DDL` above so fresh DBs get
    // them on table creation; the ALTERs upgrade existing DBs in place.
    for (col, ddl) in GRAPH_ENTITIES_ALTERS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => {
                return Err(GraphError::Migration(format!(
                    "ALTER TABLE graph_entities ADD COLUMN {col}: {e}"
                )));
            }
        }
    }

    // Step 4: indexes that depend on columns added in Step 3 must be created
    // *after* the ALTERs (otherwise legacy DBs that don't yet have the column
    // would fail when GRAPH_DDL ran in Step 1). On fresh DBs this is a harmless
    // re-issue — `IF NOT EXISTS` makes it a no-op.
    conn.execute_batch(GRAPH_POST_ALTER_INDEXES)?;

    Ok(())
}

/// Additive columns on the existing v0.2 `memories` table (GOAL-1.11).
/// Tuple is `(human-name, DDL)`; the human name is used in error messages.
const MEMORIES_ALTERS: &[(&str, &str)] = &[
    (
        "episode_id",
        "ALTER TABLE memories ADD COLUMN episode_id BLOB",
    ),
    (
        "entity_ids",
        "ALTER TABLE memories ADD COLUMN entity_ids TEXT",
    ),
    (
        "edge_ids",
        "ALTER TABLE memories ADD COLUMN edge_ids TEXT",
    ),
];

/// Additive columns on `graph_entities` added after the initial v0.3 schema.
///
/// These are typed first-class fields on `Entity` (see entity.rs §3.1):
/// - `history` — append-only audit of canonical_name changes (`Vec<HistoryEntry>`,
///   stored as JSON text). Promoted from `attributes.history` per design-r1 #51-52
///   so the merge path can never have its audit trail clobbered by caller writes.
/// - `merged_into` — set on a merge loser to the winner's id. Promoted from
///   `attributes.merged_into` for the same reason; `get_entity` follows this
///   redirect transparently (§8 reader semantics).
///
/// Stored outside `attributes` JSON because `validate_attributes` rejects these
/// reserved keys in caller writes; only the merge path mutates them, via
/// dedicated GraphStore methods.
const GRAPH_ENTITIES_ALTERS: &[(&str, &str)] = &[
    (
        "history",
        "ALTER TABLE graph_entities ADD COLUMN history TEXT NOT NULL DEFAULT '[]'",
    ),
    (
        "merged_into",
        "ALTER TABLE graph_entities ADD COLUMN merged_into BLOB",
    ),
    // ISS-033: semantic embedding for §3.4.1 candidate retrieval. NULL until
    // the first resolution pass writes it; recomputed on canonical_name /
    // summary change. No SQL CHECK on length — the dim is a runtime config
    // value, validated application-side at write and on decode (see
    // `entity_embedding_to_blob` / `entity_embedding_from_blob` in
    // `graph::store`). Same blob convention as `knowledge_topics.embedding`
    // (system-wide single dim — design §4.1 blob format note).
    (
        "embedding",
        "ALTER TABLE graph_entities ADD COLUMN embedding BLOB",
    ),
];

/// Full DDL batch for graph-layer tables. Mirrors design §4.1.
///
/// Statements are ordered so foreign-key targets exist before referencing
/// tables. SQLite enforces FK at write time (with `PRAGMA foreign_keys=ON`),
/// not at table-creation time, so the order is for readability and correctness
/// of any direct `sqlite3 < dump.sql` use.
const GRAPH_DDL: &str = r#"
-- ====================================================================
-- v0.3 graph layer schema (.gid/features/v03-graph-layer/design.md §4.1)
-- ====================================================================

-- Canonical graph entities (§3.1).
CREATE TABLE IF NOT EXISTS graph_entities (
    id                  BLOB PRIMARY KEY,                      -- 16-byte UUID
    canonical_name      TEXT NOT NULL,
    kind                TEXT NOT NULL,                         -- serde tag of EntityKind
    summary             TEXT NOT NULL DEFAULT '',
    attributes          TEXT NOT NULL DEFAULT '{}',            -- JSON
    first_seen          REAL NOT NULL,                         -- unix seconds
    last_seen           REAL NOT NULL,
    created_at          REAL NOT NULL,
    updated_at          REAL NOT NULL,
    activation          REAL NOT NULL DEFAULT 0.0,
    agent_affect        TEXT,                                  -- JSON or NULL
    arousal             REAL NOT NULL DEFAULT 0.0,
    importance          REAL NOT NULL DEFAULT 0.3,
    identity_confidence REAL NOT NULL DEFAULT 0.5,
    somatic_fingerprint BLOB,                                  -- 8 * f32 LE, or NULL
    embedding           BLOB,                                  -- ISS-033: f32 array, system-wide embedding dim, or NULL; validated app-side
    namespace           TEXT NOT NULL DEFAULT 'default',
    history             TEXT NOT NULL DEFAULT '[]',            -- JSON: Vec<HistoryEntry>; typed field, see entity.rs §3.1
    merged_into         BLOB REFERENCES graph_entities(id) ON DELETE RESTRICT, -- merge-loser redirect; typed field
    CHECK (activation          BETWEEN 0.0 AND 1.0),
    CHECK (arousal             BETWEEN 0.0 AND 1.0),
    CHECK (importance          BETWEEN 0.0 AND 1.0),
    CHECK (identity_confidence BETWEEN 0.0 AND 1.0),
    CHECK (first_seen <= last_seen),
    CHECK (somatic_fingerprint IS NULL OR length(somatic_fingerprint) = 32)
    -- NOTE: no CHECK on `embedding` length. Dim is runtime config (see
    -- §4.1 blob format note for `embedding`), so we validate in the writer
    -- (`entity_embedding_to_blob`) and on decode (`entity_embedding_from_blob`).
);
CREATE INDEX IF NOT EXISTS idx_graph_entities_namespace ON graph_entities(namespace);
CREATE INDEX IF NOT EXISTS idx_graph_entities_kind      ON graph_entities(kind);
CREATE INDEX IF NOT EXISTS idx_graph_entities_last_seen ON graph_entities(last_seen);
-- Note: idx_graph_entities_merged_into is created in GRAPH_POST_ALTER_INDEXES
-- because it references the merged_into column, which is added via ALTER for
-- legacy DBs upgraded from a pre-promotion build.

-- Aliases (§3.4). Composite PK allows many aliases per entity.
CREATE TABLE IF NOT EXISTS graph_entity_aliases (
    normalized          TEXT NOT NULL,
    canonical_id        BLOB NOT NULL REFERENCES graph_entities(id) ON DELETE CASCADE,
    alias               TEXT NOT NULL,
    former_canonical_id BLOB,                                  -- set on merge
    first_seen          REAL NOT NULL,
    source_episode      BLOB,
    namespace           TEXT NOT NULL DEFAULT 'default',
    PRIMARY KEY (namespace, normalized, canonical_id)
);
CREATE INDEX IF NOT EXISTS idx_graph_aliases_canonical ON graph_entity_aliases(canonical_id);

-- Bi-temporal typed edges (§3.2).
CREATE TABLE IF NOT EXISTS graph_edges (
    id                  BLOB PRIMARY KEY,
    subject_id          BLOB NOT NULL REFERENCES graph_entities(id) ON DELETE RESTRICT,
    predicate_kind      TEXT NOT NULL,                         -- 'canonical' | 'proposed'
    predicate_label     TEXT NOT NULL,
    object_kind         TEXT NOT NULL,                         -- 'entity' | 'literal'
    object_entity_id    BLOB    REFERENCES graph_entities(id) ON DELETE RESTRICT,
    object_literal      TEXT,                                  -- JSON; NULL iff object_kind='entity'
    summary             TEXT NOT NULL DEFAULT '',
    valid_from          REAL,
    valid_to            REAL,
    recorded_at         REAL NOT NULL,
    invalidated_at      REAL,
    invalidated_by      BLOB REFERENCES graph_edges(id),
    supersedes          BLOB REFERENCES graph_edges(id),
    episode_id          BLOB,
    memory_id           TEXT REFERENCES memories(id) ON DELETE RESTRICT,
    resolution_method   TEXT NOT NULL,
    activation          REAL NOT NULL DEFAULT 0.0,
    confidence          REAL NOT NULL DEFAULT 0.5,
    agent_affect        TEXT,
    created_at          REAL NOT NULL,
    namespace           TEXT NOT NULL DEFAULT 'default',
    CHECK (activation BETWEEN 0.0 AND 1.0),
    CHECK (confidence BETWEEN 0.0 AND 1.0),
    CHECK (
        (object_kind = 'entity'  AND object_entity_id IS NOT NULL AND object_literal IS NULL) OR
        (object_kind = 'literal' AND object_literal   IS NOT NULL AND object_entity_id IS NULL)
    ),
    CHECK (valid_from IS NULL OR valid_to IS NULL OR valid_from <= valid_to),
    CHECK (predicate_kind IN ('canonical', 'proposed'))
);
CREATE INDEX IF NOT EXISTS idx_graph_edges_subject        ON graph_edges(subject_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_object_entity  ON graph_edges(object_entity_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_predicate      ON graph_edges(predicate_label);
CREATE INDEX IF NOT EXISTS idx_graph_edges_namespace      ON graph_edges(namespace);
CREATE INDEX IF NOT EXISTS idx_graph_edges_recorded_at    ON graph_edges(recorded_at);
CREATE INDEX IF NOT EXISTS idx_graph_edges_invalidated_at ON graph_edges(invalidated_at);
CREATE INDEX IF NOT EXISTS idx_graph_edges_live
    ON graph_edges(subject_id, predicate_label) WHERE invalidated_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_graph_edges_subject_pred_recorded
    ON graph_edges(subject_id, predicate_label, recorded_at DESC);

-- Predicate registry (§3.3).
CREATE TABLE IF NOT EXISTS graph_predicates (
    kind            TEXT NOT NULL,                             -- 'canonical' | 'proposed'
    label           TEXT NOT NULL,
    raw_first_seen  TEXT NOT NULL,
    usage_count     INTEGER NOT NULL DEFAULT 0,
    first_seen      REAL NOT NULL,
    last_seen       REAL NOT NULL,
    PRIMARY KEY (kind, label)
);
CREATE INDEX IF NOT EXISTS idx_graph_predicates_usage ON graph_predicates(usage_count DESC);

-- Extraction failure surface (GOAL-1.12 / GUARD-1 / GUARD-2).
CREATE TABLE IF NOT EXISTS graph_extraction_failures (
    id              BLOB PRIMARY KEY,
    episode_id      BLOB NOT NULL,
    stage           TEXT NOT NULL,                             -- 'extraction' | 'entity_resolution' | 'edge_resolution'
    error_category  TEXT NOT NULL,                             -- 'timeout' | 'rate_limit' | 'provider_error' | 'parse_error' | 'other'
    error_detail    TEXT,
    occurred_at     REAL NOT NULL,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    resolved_at     REAL,
    namespace       TEXT NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_extraction_failures_episode    ON graph_extraction_failures(episode_id);
CREATE INDEX IF NOT EXISTS idx_extraction_failures_unresolved
    ON graph_extraction_failures(occurred_at) WHERE resolved_at IS NULL;

-- Memory ↔ Entity provenance join (GOAL-1.3, GOAL-1.7).
CREATE TABLE IF NOT EXISTS graph_memory_entity_mentions (
    memory_id       TEXT   NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id       BLOB   NOT NULL REFERENCES graph_entities(id) ON DELETE RESTRICT,
    mention_span    TEXT,
    confidence      REAL   NOT NULL DEFAULT 1.0 CHECK (confidence BETWEEN 0.0 AND 1.0),
    recorded_at     REAL   NOT NULL,
    namespace       TEXT   NOT NULL DEFAULT 'default',
    PRIMARY KEY (memory_id, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_by_memory ON graph_memory_entity_mentions(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_by_entity ON graph_memory_entity_mentions(entity_id);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_ns        ON graph_memory_entity_mentions(namespace);

-- L5 Knowledge Topics (GOAL-1.15 structural hook).
CREATE TABLE IF NOT EXISTS knowledge_topics (
    topic_id                BLOB   PRIMARY KEY REFERENCES graph_entities(id) ON DELETE RESTRICT,
    title                   TEXT   NOT NULL,
    summary                 TEXT   NOT NULL,
    embedding               BLOB,
    source_memories         TEXT   NOT NULL DEFAULT '[]',
    contributing_entities   TEXT   NOT NULL DEFAULT '[]',
    cluster_weights         TEXT,
    synthesis_run_id        BLOB,
    synthesized_at          REAL   NOT NULL,
    superseded_by           BLOB   REFERENCES knowledge_topics(topic_id),
    superseded_at           REAL,
    namespace               TEXT   NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_ns   ON knowledge_topics(namespace);
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_live
    ON knowledge_topics(namespace, synthesized_at DESC) WHERE superseded_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_run  ON knowledge_topics(synthesis_run_id);

-- Pipeline-run ledger (resolution / reextract / knowledge_compile).
CREATE TABLE IF NOT EXISTS graph_pipeline_runs (
    run_id          BLOB PRIMARY KEY,
    kind            TEXT NOT NULL,
    started_at      REAL NOT NULL,
    finished_at     REAL,
    status          TEXT NOT NULL,
    input_summary   TEXT,
    output_summary  TEXT,
    error_detail    TEXT,
    namespace       TEXT NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_graph_pipeline_runs_kind   ON graph_pipeline_runs(kind, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_graph_pipeline_runs_status ON graph_pipeline_runs(status) WHERE status != 'succeeded';

-- Per-decision resolution traces (GOAL-1.7 provenance).
CREATE TABLE IF NOT EXISTS graph_resolution_traces (
    trace_id        BLOB PRIMARY KEY,
    run_id          BLOB NOT NULL REFERENCES graph_pipeline_runs(run_id) ON DELETE CASCADE,
    edge_id         BLOB          REFERENCES graph_edges(id) ON DELETE CASCADE,
    entity_id       BLOB          REFERENCES graph_entities(id) ON DELETE CASCADE,
    stage           TEXT NOT NULL,                             -- 'entity_extract' | 'edge_extract' | 'dedup' | 'persist'
    decision        TEXT NOT NULL,                             -- 'new' | 'matched_existing' | 'superseded' | 'merged' | 'rejected'
    reason          TEXT,
    candidates      TEXT,
    recorded_at     REAL NOT NULL,
    CHECK (edge_id IS NOT NULL OR entity_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_run  ON graph_resolution_traces(run_id);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_edge ON graph_resolution_traces(edge_id);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_ent  ON graph_resolution_traces(entity_id);

-- Idempotence ledger for apply_graph_delta (§5bis).
CREATE TABLE IF NOT EXISTS graph_applied_deltas (
    memory_id       TEXT NOT NULL,
    delta_hash      BLOB NOT NULL,                             -- BLAKE3 of canonical JSON
    schema_version  INTEGER NOT NULL,
    applied_at      REAL NOT NULL,
    report          TEXT NOT NULL,
    PRIMARY KEY (memory_id, delta_hash, schema_version)
);
CREATE INDEX IF NOT EXISTS idx_applied_deltas_memory ON graph_applied_deltas(memory_id);
"#;

/// Indexes whose definitions reference columns added in `GRAPH_ENTITIES_ALTERS`.
///
/// Must be applied **after** the ALTER step in `init_graph_tables`. On a fresh
/// DB this is a harmless re-issue (the table was created with the column in
/// `GRAPH_DDL`); on a legacy DB this runs once the ALTER has added the column.
/// All statements use `IF NOT EXISTS` for idempotence.
const GRAPH_POST_ALTER_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_graph_entities_merged_into
    ON graph_entities(merged_into) WHERE merged_into IS NOT NULL;
-- ISS-033: bound the §3.4.1 candidate-retrieval scan to entities that
-- actually carry an embedding. Partial index keyed on (namespace, last_seen DESC)
-- because both are filters in `search_candidates` (namespace is hard, recency
-- is the dominant ordering for the bounded scan window).
CREATE INDEX IF NOT EXISTS idx_graph_entities_embed_scan
    ON graph_entities(namespace, last_seen DESC) WHERE embedding IS NOT NULL;
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::GRAPH_TABLES;
    use rusqlite::Connection;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory");
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .expect("enable FK");
        // Create the v0.2 `memories` table that some graph tables reference.
        conn.execute_batch(
            r#"
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL
            );
            "#,
        )
        .expect("create memories");
        conn
    }

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    #[test]
    fn init_creates_all_graph_tables() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init ok");
        let names = table_names(&conn);
        for t in GRAPH_TABLES {
            assert!(
                names.iter().any(|n| n == t),
                "expected table {t} to exist after init; have: {names:?}"
            );
        }
    }

    #[test]
    fn init_is_idempotent() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("first init");
        init_graph_tables(&conn).expect("second init (no-op)");
        init_graph_tables(&conn).expect("third init (no-op)");
        // Tables still present, single copy each.
        let names = table_names(&conn);
        for t in GRAPH_TABLES {
            let count = names.iter().filter(|n| n.as_str() == *t).count();
            assert_eq!(count, 1, "table {t} should appear exactly once");
        }
    }

    #[test]
    fn init_adds_memory_columns() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init ok");
        // PRAGMA table_info enumerates columns.
        let mut stmt = conn.prepare("PRAGMA table_info(memories)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for c in ["episode_id", "entity_ids", "edge_ids"] {
            assert!(cols.contains(&c.to_string()), "missing column: {c}");
        }
    }

    #[test]
    fn init_memory_alters_idempotent() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("first init");
        // Second call should not error on duplicate column.
        init_graph_tables(&conn).expect("second init: ALTER tolerates duplicate column");
    }

    #[test]
    fn init_upgrades_legacy_graph_entities_table() {
        // Simulate a DB created by an earlier v0.3 build that lacked the
        // history/merged_into columns. init_graph_tables must add them via
        // ALTER without erroring on the rest of the schema being already there.
        let conn = fresh_conn();
        // Pre-create graph_entities without the new columns. Use the original
        // schema shape that shipped before history/merged_into were promoted.
        conn.execute_batch(
            r#"
            CREATE TABLE graph_entities (
                id                  BLOB PRIMARY KEY,
                canonical_name      TEXT NOT NULL,
                kind                TEXT NOT NULL,
                summary             TEXT NOT NULL DEFAULT '',
                attributes          TEXT NOT NULL DEFAULT '{}',
                first_seen          REAL NOT NULL,
                last_seen           REAL NOT NULL,
                created_at          REAL NOT NULL,
                updated_at          REAL NOT NULL,
                activation          REAL NOT NULL DEFAULT 0.0,
                agent_affect        TEXT,
                arousal             REAL NOT NULL DEFAULT 0.0,
                importance          REAL NOT NULL DEFAULT 0.3,
                identity_confidence REAL NOT NULL DEFAULT 0.5,
                somatic_fingerprint BLOB,
                namespace           TEXT NOT NULL DEFAULT 'default'
            );
            "#,
        )
        .expect("create legacy graph_entities");

        // Insert a row with the legacy shape so the migration must preserve data.
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at
            ) VALUES (X'AA112233445566778899AABBCCDDEEFF', 'legacy', 'Person',
                      1.0, 1.0, 1.0, 1.0)",
            [],
        )
        .expect("insert legacy row");

        // Run init: must succeed and add history + merged_into columns.
        init_graph_tables(&conn).expect("init upgrades legacy schema");

        // New columns now exist.
        let mut stmt = conn.prepare("PRAGMA table_info(graph_entities)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"history".to_string()), "history not added");
        assert!(
            cols.contains(&"merged_into".to_string()),
            "merged_into not added"
        );

        // Legacy row survives, with history defaulted to '[]' and merged_into NULL.
        let (history, merged_into): (String, Option<Vec<u8>>) = conn
            .query_row(
                "SELECT history, merged_into FROM graph_entities
                 WHERE id = X'AA112233445566778899AABBCCDDEEFF'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
            )
            .expect("legacy row still readable");
        assert_eq!(history, "[]", "history must default to empty array");
        assert!(merged_into.is_none(), "merged_into must default to NULL");

        // Second call is a no-op (idempotent).
        init_graph_tables(&conn).expect("second init no-op");
    }

    #[test]
    fn init_adds_graph_entities_typed_audit_columns() {
        // history and merged_into are typed first-class fields on Entity (§3.1)
        // promoted out of attributes JSON per design-r1 #51-52. They must appear
        // as columns on graph_entities so the typed Entity round-trips faithfully.
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init ok");
        let mut stmt = conn.prepare("PRAGMA table_info(graph_entities)").unwrap();
        let cols: Vec<(String, String, i64, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,                  // name
                    row.get::<_, String>(2)?,                  // type
                    row.get::<_, i64>(3)?,                     // notnull
                    row.get::<_, Option<String>>(4)?,          // dflt_value
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let history = cols
            .iter()
            .find(|c| c.0 == "history")
            .expect("history column missing");
        assert_eq!(history.1, "TEXT", "history must be TEXT (JSON)");
        assert_eq!(history.2, 1, "history must be NOT NULL");
        assert_eq!(
            history.3.as_deref(),
            Some("'[]'"),
            "history must default to empty JSON array"
        );

        let merged = cols
            .iter()
            .find(|c| c.0 == "merged_into")
            .expect("merged_into column missing");
        assert_eq!(merged.1, "BLOB", "merged_into must be BLOB (UUID)");
        assert_eq!(merged.2, 0, "merged_into must be nullable (None on non-losers)");
    }

    #[test]
    fn graph_entities_typed_audit_columns_round_trip() {
        // Functional check: defaults work, and we can insert a merge-loser row
        // with merged_into pointing at the winner.
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init");

        // Winner row — accepts default '[]' for history and NULL for merged_into.
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at
            ) VALUES (X'AA112233445566778899AABBCCDDEEFF', 'winner', 'Person',
                      1.0, 1.0, 1.0, 1.0)",
            [],
        )
        .expect("insert winner with defaults");

        // Loser row — explicit history JSON and merged_into FK to winner.
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at, history, merged_into
            ) VALUES (X'BB112233445566778899AABBCCDDEEFF', 'loser', 'Person',
                      1.0, 1.0, 1.0, 1.0,
                      '[{\"at\":1.0,\"from\":\"loser\",\"to\":\"winner\"}]',
                      X'AA112233445566778899AABBCCDDEEFF')",
            [],
        )
        .expect("insert loser pointing at winner");

        // Verify the redirect FK is enforced: bogus merged_into must fail.
        let bad = conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at, merged_into
            ) VALUES (X'CC112233445566778899AABBCCDDEEFF', 'orphan', 'Person',
                      1.0, 1.0, 1.0, 1.0,
                      X'00000000000000000000000000000000')",
            [],
        );
        assert!(
            bad.is_err(),
            "merged_into pointing at non-existent entity must violate FK"
        );
    }

    #[test]
    fn graph_entities_check_constraints_enforced() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init");
        // Out-of-range activation should fail the CHECK.
        let res = conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at, activation
            ) VALUES (X'00112233445566778899AABBCCDDEEFF', 'x', 'Person',
                      1.0, 1.0, 1.0, 1.0, 1.5)",
            [],
        );
        assert!(
            res.is_err(),
            "activation=1.5 should violate CHECK constraint, got: {res:?}"
        );
    }

    #[test]
    fn graph_edges_object_xor_check_enforced() {
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init");
        // Insert a subject entity first to satisfy FK.
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at
            ) VALUES (X'AA112233445566778899AABBCCDDEEFF', 'subj', 'Person',
                      1.0, 1.0, 1.0, 1.0)",
            [],
        )
        .expect("insert subj");
        // Bad: object_kind='entity' but no object_entity_id.
        let res = conn.execute(
            "INSERT INTO graph_edges (
                id, subject_id, predicate_kind, predicate_label,
                object_kind, recorded_at, resolution_method, created_at
            ) VALUES (X'BB112233445566778899AABBCCDDEEFF',
                      X'AA112233445566778899AABBCCDDEEFF',
                      'canonical', 'KnownAs', 'entity', 1.0, 'manual', 1.0)",
            [],
        );
        assert!(
            res.is_err(),
            "object_kind='entity' with NULL object_entity_id should violate CHECK"
        );
    }

    #[test]
    fn graph_edges_object_xor_check_rejects_literal_with_entity_id() {
        // Symmetric to the test above: the *other* invalid combo of the XOR.
        // object_kind='literal' but object_entity_id is also set → must fail.
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init");
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at
            ) VALUES (X'AA112233445566778899AABBCCDDEEFF', 'subj', 'Person',
                      1.0, 1.0, 1.0, 1.0)",
            [],
        )
        .expect("insert subj");
        let res = conn.execute(
            "INSERT INTO graph_edges (
                id, subject_id, predicate_kind, predicate_label,
                object_kind, object_entity_id, object_literal,
                recorded_at, resolution_method, created_at
            ) VALUES (X'CC112233445566778899AABBCCDDEEFF',
                      X'AA112233445566778899AABBCCDDEEFF',
                      'canonical', 'HasName', 'literal',
                      X'AA112233445566778899AABBCCDDEEFF',
                      '\"oops\"',
                      1.0, 'manual', 1.0)",
            [],
        );
        assert!(
            res.is_err(),
            "object_kind='literal' with non-NULL object_entity_id should violate CHECK"
        );
    }

    #[test]
    fn graph_edges_valid_from_to_check_enforced() {
        // valid_from must be <= valid_to when both are set (bi-temporal sanity).
        let conn = fresh_conn();
        init_graph_tables(&conn).expect("init");
        conn.execute(
            "INSERT INTO graph_entities (
                id, canonical_name, kind, first_seen, last_seen,
                created_at, updated_at
            ) VALUES (X'AA112233445566778899AABBCCDDEEFF', 'subj', 'Person',
                      1.0, 1.0, 1.0, 1.0)",
            [],
        )
        .expect("insert subj");
        // Bad: valid_from (10.0) > valid_to (5.0).
        let res = conn.execute(
            "INSERT INTO graph_edges (
                id, subject_id, predicate_kind, predicate_label,
                object_kind, object_literal, valid_from, valid_to,
                recorded_at, resolution_method, created_at
            ) VALUES (X'DD112233445566778899AABBCCDDEEFF',
                      X'AA112233445566778899AABBCCDDEEFF',
                      'canonical', 'HasName', 'literal', '\"x\"',
                      10.0, 5.0,
                      1.0, 'manual', 1.0)",
            [],
        );
        assert!(
            res.is_err(),
            "valid_from > valid_to should violate CHECK"
        );

        // Good: valid_from <= valid_to (boundary equal is allowed).
        let ok = conn.execute(
            "INSERT INTO graph_edges (
                id, subject_id, predicate_kind, predicate_label,
                object_kind, object_literal, valid_from, valid_to,
                recorded_at, resolution_method, created_at
            ) VALUES (X'EE112233445566778899AABBCCDDEEFF',
                      X'AA112233445566778899AABBCCDDEEFF',
                      'canonical', 'HasName', 'literal', '\"x\"',
                      5.0, 5.0,
                      1.0, 'manual', 1.0)",
            [],
        );
        assert!(ok.is_ok(), "valid_from == valid_to should pass: {ok:?}");
    }
}
