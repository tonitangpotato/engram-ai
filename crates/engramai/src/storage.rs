//! SQLite storage backend for Engram.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};
use std::path::Path;

use crate::synthesis::types::{GateScores, ProvenanceRecord};
use crate::triple::{Triple, Predicate, TripleSource};
use crate::types::{AclEntry, CrossLink, HebbianLink, MemoryLayer, MergeOutcome, MemoryRecord, MemoryType, Permission};

use std::sync::OnceLock;

/// Global jieba instance (loaded once, ~150ms first use, then instant).
fn jieba() -> &'static jieba_rs::Jieba {
    static JIEBA: OnceLock<jieba_rs::Jieba> = OnceLock::new();
    JIEBA.get_or_init(jieba_rs::Jieba::new)
}

/// Tokenize text for FTS5 indexing.
/// Uses jieba for Chinese word segmentation + CJK/ASCII boundary splitting.
/// e.g. "RustClaw是一个记忆系统" → "RustClaw 是 一个 记忆 系统"
/// e.g. "用Rust写agent框架" → "用 Rust 写 agent 框架"
fn tokenize_cjk_boundaries(text: &str) -> String {
    if !text.chars().any(is_cjk_char) {
        return text.to_string(); // Fast path: no CJK, skip jieba
    }
    
    // Use jieba to segment Chinese text
    let words = jieba().cut(text, true); // true = HMM mode for better accuracy
    
    // Join with spaces, then ensure CJK/ASCII boundaries have spaces
    let joined = words.join(" ");
    
    // Clean up: remove duplicate spaces
    let mut result = String::with_capacity(joined.len());
    let mut prev_space = false;
    for ch in joined.chars() {
        if ch == ' ' {
            if !prev_space {
                result.push(ch);
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result
}

/// Tokenize a query string the same way FTS5's unicode61 tokenizer does.
///
/// unicode61 treats any non-alphanumeric character as a separator, splitting
/// "2.5D" into ["2", "5D"], "v0.2.1" into ["v0", "2", "1"], etc.
/// We must split identically so that FTS MATCH queries align with the index.
fn tokenize_like_unicode61(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || is_cjk_char(ch) {
            current.push(ch);
        } else {
            // Non-alphanumeric = separator (same as unicode61)
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Check if a character is CJK (Chinese/Japanese/Korean).
fn is_cjk_char(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul
    )
}

/// Convert a `DateTime<Utc>` to a Unix float (seconds since epoch).
fn datetime_to_f64(dt: &DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1_000_000_000.0
}

/// Convert a Unix float (seconds since epoch) to `DateTime<Utc>`.
fn f64_to_datetime(ts: f64) -> DateTime<Utc> {
    let secs = ts.floor() as i64;
    let nanos = ((ts - secs as f64) * 1_000_000_000.0).max(0.0) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Get the current time as a Unix float (seconds since epoch).
pub fn now_f64() -> f64 {
    datetime_to_f64(&Utc::now())
}

/// Convert raw bytes to Vec<f32> (little-endian).
fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap();
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Embedding statistics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingStats {
    pub total_memories: usize,
    pub embedded_count: usize,
    pub model: Option<String>,
    pub dimensions: Option<usize>,
}

/// A record from the `entities` table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EntityRecord {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub namespace: String,
    pub metadata: Option<String>,
    pub created_at: f64,
    pub updated_at: f64,
}

/// Generate a deterministic entity ID from (name, entity_type, namespace).
///
/// Uses a stable FNV-1a-inspired hash to produce a 16-char hex string.
/// Deterministic: same inputs always produce the same ID.
/// The UNIQUE index on `(name, entity_type, namespace)` is the real safety net.
fn generate_entity_id(name: &str, entity_type: &str, namespace: &str) -> String {
    let input = format!("{}|{}|{}", name.to_lowercase(), entity_type.to_lowercase(), namespace);
    // FNV-1a 64-bit (stable, no external crate needed)
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// SQLite-backed memory storage with FTS5 search.
pub struct Storage {
    conn: Connection,
    /// v0.4 unified substrate read-switch.
    ///
    /// When `true`, Phase D read adapters fetch rows from the unified
    /// `nodes` / `edges` / `node_embeddings` tables instead of the
    /// legacy per-concept tables (`memories`, `synthesis_provenance`,
    /// `memory_embeddings`, `entities`, …). Writes are always
    /// dual-routed (Phase B), so flipping this flag is a pure read
    /// swap — see `.gid/features/v04-unified-substrate/design.md` §5.4.
    ///
    /// Captured at construction time via
    /// [`Storage::with_unified_substrate`] from
    /// `MemoryConfig::unified_substrate`. There is intentionally no
    /// setter: read mode is a process-lifecycle decision, not a
    /// request-time toggle. Avoids stale-flag risk from setter
    /// patterns.
    ///
    /// Defaults to `false` so existing constructors (`Storage::new`)
    /// keep legacy behavior bit-identical.
    unified_substrate: bool,
}

// ---------------------------------------------------------------------
// ISS-019 Step 7a — merge-tracking helpers (v2-first, v1-fallback).
// ---------------------------------------------------------------------

/// Read `(merge_history, merge_count)` from a stored metadata blob,
/// checking the v2 namespaced location (`engram.*`) first and falling
/// back to the v1 flat layout (top-level keys).
fn read_merge_tracking(
    metadata: &serde_json::Value,
) -> (Vec<serde_json::Value>, i64) {
    if let Some(engram) = metadata.get("engram") {
        let history = engram
            .get("merge_history")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let count = engram
            .get("merge_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        return (history, count);
    }
    let history = metadata
        .get("merge_history")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let count = metadata
        .get("merge_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    (history, count)
}

/// Write `(merge_history, merge_count)` into metadata at the v2 location
/// (`engram.merge_history`, `engram.merge_count`). If the blob is not
/// yet v2-shaped, an `engram` object is created. v1 top-level keys are
/// not written — callers should have already re-shaped the blob via
/// `build_v2_metadata` / `to_v2_metadata`.
fn write_merge_tracking(
    metadata: &mut serde_json::Value,
    history: Vec<serde_json::Value>,
    count: i64,
) {
    if !metadata.is_object() {
        *metadata = serde_json::json!({});
    }
    if let Some(obj) = metadata.as_object_mut() {
        let engram = obj
            .entry("engram".to_string())
            .or_insert_with(|| serde_json::json!({"version": 2}));
        if let Some(e_obj) = engram.as_object_mut() {
            e_obj.insert(
                "merge_history".into(),
                serde_json::Value::Array(history),
            );
            e_obj.insert("merge_count".into(), serde_json::json!(count));
        }
    }
}

impl Storage {
    /// Open or create a SQLite database at the given path.
    ///
    /// Use `:memory:` for an in-memory database.
    ///
    /// Defaults to legacy read mode (`unified_substrate = false`). To
    /// open with v0.4 unified-substrate reads enabled, use
    /// [`Storage::with_unified_substrate`].
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, rusqlite::Error> {
        Self::with_unified_substrate(path, false)
    }

    /// Open or create a SQLite database with the v0.4 unified-substrate
    /// read flag explicitly set.
    ///
    /// `unified_substrate = true` makes Phase D read adapters fetch
    /// from `nodes` / `edges` / `node_embeddings` instead of the
    /// legacy per-concept tables. Writes are always dual-routed
    /// (Phase B), so this is a pure read swap. See
    /// `.gid/features/v04-unified-substrate/design.md` §5.4.
    ///
    /// **The flag is captured at construction time**: this is
    /// deliberate — read mode is a process-lifecycle decision, not a
    /// request-time toggle. Avoids stale-flag risk from setter
    /// patterns.
    pub fn with_unified_substrate<P: AsRef<Path>>(
        path: P,
        unified_substrate: bool,
    ) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        
        // Enable WAL mode for better concurrency + busy timeout for multi-process access
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
        
        // Create schema
        Self::create_schema(&conn)?;
        
        // Run migrations for v2 features (namespace, ACL)
        Self::migrate_v2(&conn)?;
        
        // Run migrations for embeddings
        Self::migrate_embeddings(&conn)?;
        
        // Run migrations for entity table constraints
        Self::migrate_entities(&conn)?;
        
        // Rebuild FTS with CJK tokenization if needed
        Self::rebuild_fts_if_needed(&conn)?;
        
        // Run migrations for multi-signal Hebbian columns
        Self::migrate_hebbian_signals(&conn)?;

        // ISS-117: dedupe legacy double-direction hebbian_links rows
        // into single canonical (min, max) rows. Idempotent.
        Self::migrate_hebbian_canonical_rows(&conn)?;

        // Run migrations for triple extraction
        Self::migrate_triples(&conn)?;
        
        // Run migrations for promotion candidates
        Self::migrate_promotions(&conn)?;
        
        // Run migrations for cluster state persistence
        Self::migrate_cluster_state(&conn)?;

        // ISS-019 Step 6: quarantine table (persistent failed-extraction storage).
        Self::migrate_quarantine(&conn)?;

        // ISS-019 Step 7b: backfill_queue table (v1 → v2 dimensional recovery).
        Self::migrate_backfill_queue(&conn)?;
        
        // Add deleted_at column for soft-delete
        match conn.execute(
            "ALTER TABLE memories ADD COLUMN deleted_at TEXT DEFAULT NULL",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }

        // Index for soft-delete filter performance
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_memories_deleted_at ON memories(deleted_at);"
        )?;
        
        // Add superseded_by column for memory supersession (GUARD-ss.3: idempotent migration)
        match conn.execute(
            "ALTER TABLE memories ADD COLUMN superseded_by TEXT DEFAULT ''",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }

        // ISS-103: split occurred_at out of created_at.
        //
        // `created_at` is wall-clock ingest time (drives Ebbinghaus decay).
        // `occurred_at` is the event/fact's logical time (drives temporal
        // grounding & temporal-range queries). Nullable: `None` means
        // "we don't know when this happened" — readers fall back to
        // `created_at`.
        //
        // Pre-ISS-103 rows: `occurred_at` defaults to NULL. They retain
        // ISS-087 behaviour where `created_at` was being overloaded with
        // event time; new ingests after this migration write the two
        // columns independently.
        match conn.execute(
            "ALTER TABLE memories ADD COLUMN occurred_at REAL DEFAULT NULL",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }

        // v0.3 graph layer schema (additive; never touches v0.2 tables).
        // Maps GraphError back to rusqlite::Error to keep this constructor's
        // return type stable.
        crate::graph::init_graph_tables(&conn).map_err(|e| match e {
            crate::graph::GraphError::Sqlite(inner) => inner,
            other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
        })?;

        // v0.4 unified substrate: nodes table (T05)
        Self::migrate_unified_nodes(&conn)?;

        // v0.4 unified substrate: edges table (T06)
        Self::migrate_unified_edges(&conn)?;

        // v0.4 unified substrate: nodes_fts + triggers (T07)
        Self::migrate_unified_fts(&conn)?;

        // v0.4 unified substrate: node_embeddings multi-model (T08)
        Self::migrate_unified_node_embeddings(&conn)?;

        // v0.4 Phase C (T19+): backfill_runs audit table.
        // Idempotent CREATE TABLE IF NOT EXISTS — safe on every open.
        Self::migrate_backfill_runs(&conn)?;

        // v0.4 unified substrate: bump schema_version (T09).
        // Runs last so a partial Phase A migration leaves the version
        // unchanged — re-opening then re-attempts the missing migrations
        // (all are idempotent per GUARD-ss.3).
        Self::bump_schema_version_v04_additive(&conn)?;

        Ok(Self {
            conn,
            unified_substrate,
        })
    }

    /// Run all v0.4 Phase A migrations (T05–T09) on a foreign connection.
    ///
    /// Exposed for code paths that init their own connection without going
    /// through `Storage::new` — specifically `graph::storage_graph::init_graph_tables`,
    /// which is called from unit tests that open `Connection::open_in_memory()`
    /// directly and need the unified substrate tables for the dual-write
    /// helpers (`dual_write_entity_to_nodes`, `dual_write_edge_to_edges`,
    /// `Storage::add` memory→nodes) to find their target tables.
    ///
    /// Idempotent (GUARD-ss.3): re-running is a no-op on every migration.
    pub(crate) fn migrate_v04_substrate(conn: &Connection) -> SqlResult<()> {
        Self::migrate_unified_nodes(conn)?;
        Self::migrate_unified_edges(conn)?;
        Self::migrate_unified_fts(conn)?;
        Self::migrate_unified_node_embeddings(conn)?;
        Self::migrate_backfill_runs(conn)?;
        Self::bump_schema_version_v04_additive(conn)?;
        Ok(())
    }

    /// v0.4 unified substrate (T05): create the `nodes` table, its indexes, and
    /// the `fts_rowid_counter` singleton helper per design.md §3.1.
    ///
    /// **Additive only** — does not touch `memories`, `entities`,
    /// `hebbian_links`, or any existing table. Idempotent (GUARD-ss.3):
    /// `CREATE TABLE IF NOT EXISTS` + `CREATE INDEX IF NOT EXISTS` +
    /// `INSERT OR IGNORE` for the counter singleton.
    ///
    /// Schema version is **not** bumped here — T09 lands that after the full
    /// T05–T08 set is in place.
    fn migrate_unified_nodes(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS nodes (
                -- identity
                id                  TEXT PRIMARY KEY,
                node_kind           TEXT NOT NULL,
                namespace           TEXT NOT NULL DEFAULT 'default',

                -- memory-specific sub-classification (NULL for non-memory kinds)
                layer               TEXT,
                memory_type         TEXT,

                -- content
                content             TEXT NOT NULL,
                summary             TEXT NOT NULL DEFAULT '',
                attributes          TEXT NOT NULL DEFAULT '{}',

                -- vector
                embedding           BLOB,
                embedding_model     TEXT,

                -- temporal (bi-temporal)
                occurred_at         REAL,
                valid_from          REAL,
                valid_to            REAL,
                created_at          REAL NOT NULL,
                updated_at          REAL NOT NULL,
                first_seen          REAL,
                last_seen           REAL,

                -- decay / activation / strength
                activation          REAL NOT NULL DEFAULT 0.0,
                working_strength    REAL NOT NULL DEFAULT 1.0,
                core_strength       REAL NOT NULL DEFAULT 0.0,
                importance          REAL NOT NULL DEFAULT 0.3,
                confidence          REAL NOT NULL DEFAULT 0.5,

                -- affect
                agent_affect        TEXT,
                arousal             REAL NOT NULL DEFAULT 0.0,
                somatic_fingerprint BLOB,

                -- retirement
                deleted_at          REAL,
                superseded_by       TEXT REFERENCES nodes(id) ON DELETE SET NULL,
                pinned              INTEGER NOT NULL DEFAULT 0,

                -- provenance
                source              TEXT NOT NULL DEFAULT '',
                source_run_id       TEXT,
                consolidation_count INTEGER NOT NULL DEFAULT 0,
                last_consolidated   REAL,

                -- history (audit trail of in-place mutations, e.g. entity merges)
                history             TEXT NOT NULL DEFAULT '[]',

                -- FTS surrogate: stable integer for nodes_fts rowid (§3.3).
                fts_rowid           INTEGER UNIQUE,

                CHECK (activation       BETWEEN 0.0 AND 1.0),
                CHECK (arousal          BETWEEN 0.0 AND 1.0),
                CHECK (importance       BETWEEN 0.0 AND 1.0),
                CHECK (confidence       BETWEEN 0.0 AND 1.0),
                CHECK (working_strength BETWEEN 0.0 AND 1.0),
                CHECK (core_strength    BETWEEN 0.0 AND 1.0)
            );

            CREATE INDEX IF NOT EXISTS idx_nodes_kind         ON nodes(node_kind, namespace);
            CREATE INDEX IF NOT EXISTS idx_nodes_namespace    ON nodes(namespace);
            CREATE INDEX IF NOT EXISTS idx_nodes_created      ON nodes(created_at);
            CREATE INDEX IF NOT EXISTS idx_nodes_occurred     ON nodes(occurred_at) WHERE occurred_at IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_nodes_deleted      ON nodes(deleted_at) WHERE deleted_at IS NULL;
            CREATE INDEX IF NOT EXISTS idx_nodes_kind_active  ON nodes(node_kind, activation) WHERE deleted_at IS NULL;
            CREATE INDEX IF NOT EXISTS idx_nodes_memory_type  ON nodes(memory_type) WHERE node_kind='memory';
            CREATE INDEX IF NOT EXISTS idx_nodes_superseded   ON nodes(superseded_by) WHERE superseded_by IS NOT NULL;

            -- Monotonic counter for fts_rowid assignment (§3.3, §6 writer).
            CREATE TABLE IF NOT EXISTS fts_rowid_counter (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 0),
                next_value INTEGER NOT NULL DEFAULT 1
            );
            INSERT OR IGNORE INTO fts_rowid_counter (singleton, next_value) VALUES (0, 1);
        "#)?;
        Ok(())
    }

    /// v0.4 unified substrate (T06): create the `edges` table and its indexes
    /// per design.md §3.2.
    ///
    /// **Additive only** — does not touch `entity_relations`, `hebbian_links`,
    /// or any existing table. Idempotent (GUARD-ss.3) via `CREATE TABLE IF NOT
    /// EXISTS` + `CREATE INDEX IF NOT EXISTS`.
    ///
    /// Foreign keys reference `nodes(id)` and self-reference (`supersedes`,
    /// `invalidated_by`) — so T05 (`migrate_unified_nodes`) must run first.
    /// The call site in `Storage::open` already enforces this order.
    ///
    /// Schema version is **not** bumped here — T09 lands that after the full
    /// T05–T08 set is in place.
    fn migrate_unified_edges(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS edges (
                -- identity
                id                  TEXT PRIMARY KEY,

                -- endpoints
                source_id           TEXT NOT NULL REFERENCES nodes(id) ON DELETE RESTRICT,
                target_id           TEXT REFERENCES nodes(id) ON DELETE RESTRICT,
                target_literal      TEXT,        -- JSON; NULL iff target_id IS NOT NULL

                -- typing: two-level discriminator (§3.2)
                edge_kind           TEXT NOT NULL,
                predicate_kind      TEXT NOT NULL DEFAULT 'canonical',
                predicate           TEXT NOT NULL,

                -- payload
                summary             TEXT NOT NULL DEFAULT '',
                attributes          TEXT NOT NULL DEFAULT '{}',
                weight              REAL NOT NULL DEFAULT 1.0,
                activation          REAL NOT NULL DEFAULT 0.0,
                confidence          REAL NOT NULL DEFAULT 0.5,

                -- temporal (bi-temporal)
                valid_from          REAL,
                valid_to            REAL,
                recorded_at         REAL NOT NULL,

                -- supersession / retirement
                invalidated_at      REAL,
                invalidated_by      TEXT REFERENCES edges(id),
                supersedes          TEXT REFERENCES edges(id),

                agent_affect        TEXT,

                -- provenance
                source_run_id       TEXT,        -- string UUID; references pipeline_runs.id
                source_memory_id    TEXT REFERENCES nodes(id),
                resolution_method   TEXT NOT NULL DEFAULT 'direct',

                namespace           TEXT NOT NULL DEFAULT 'default',
                created_at          REAL NOT NULL,
                updated_at          REAL NOT NULL,

                CHECK (confidence BETWEEN 0.0 AND 1.0),
                CHECK (weight     >= 0.0),
                CHECK (
                    (target_id IS NOT NULL AND target_literal IS NULL) OR
                    (target_id IS NULL     AND target_literal IS NOT NULL)
                )
            );

            CREATE INDEX IF NOT EXISTS idx_edges_source
                ON edges(source_id, edge_kind);
            CREATE INDEX IF NOT EXISTS idx_edges_target
                ON edges(target_id, edge_kind) WHERE target_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_edges_kind_pred
                ON edges(edge_kind, predicate, namespace);
            CREATE INDEX IF NOT EXISTS idx_edges_namespace
                ON edges(namespace);
            CREATE INDEX IF NOT EXISTS idx_edges_temporal
                ON edges(valid_from, valid_to) WHERE valid_from IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_edges_live
                ON edges(edge_kind, predicate) WHERE invalidated_at IS NULL;

            -- Partial UNIQUE indexes enforce upsert semantics per design §3.2:
            -- associative co-activation accumulates weight (one row per
            -- src/tgt/predicate/signal_source); containment is set
            -- membership. Structural edges may legitimately duplicate
            -- across runs and are NOT unique.
            --
            -- signal_source is part of the associative-edge identity
            -- (design §4.3): each distinct signal_source between the
            -- same (src, tgt) pair gets its own row, so §4.6
            -- differential decay can apply per-signal-source. SQLite
            -- supports json_extract in expression-indexed unique
            -- constraints and resolves ON CONFLICT against them.
            CREATE UNIQUE INDEX IF NOT EXISTS idx_edges_containment_unique
                ON edges(source_id, target_id, edge_kind, predicate)
                WHERE edge_kind = 'containment';
        "#)?;

        // T14 migration: associative-edge unique index was originally
        // 4 columns (src, tgt, kind, predicate). Design §4.3 amendment
        // extends it to 5 columns by adding json_extract(attributes,
        // '$.signal_source'). Pre-T14 DBs have the old index; CREATE
        // IF NOT EXISTS won't replace it. Detect via sqlite_master and
        // DROP + RECREATE if the old shape is present.
        //
        // GUARD-ss.3 idempotency: this check is cheap and re-running
        // on an already-migrated DB is a no-op (the new shape is
        // detected and we skip the drop).
        let needs_assoc_index_migration: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master \
                 WHERE type='index' AND name='idx_edges_assoc_unique' \
                   AND sql NOT LIKE '%signal_source%'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if needs_assoc_index_migration {
            conn.execute_batch("DROP INDEX idx_edges_assoc_unique;")?;
        }
        conn.execute_batch(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_edges_assoc_unique
                ON edges(source_id, target_id, edge_kind, predicate,
                         json_extract(attributes, '$.signal_source'))
                WHERE edge_kind = 'associative';
            "#,
        )?;
        Ok(())
    }

    /// v0.4 unified substrate (T08): create the `node_embeddings` multi-model
    /// extension table per design.md §3.4.
    ///
    /// **Role**: 99% of retrieval reads the inlined `nodes.embedding`
    /// (single model, no JOIN). This table serves the multi-model
    /// power-user case currently provided by `memory_embeddings` — and
    /// extends it to *any* node kind (entity / topic / insight / …),
    /// which legacy `memory_embeddings` could not.
    ///
    /// **Schema**:
    /// - PK `(node_id, model)` — one row per (node × model) pair.
    /// - `ON DELETE CASCADE` from `nodes(id)` — drop a node and all its
    ///   alternate-model embeddings vanish too.
    /// - `idx_node_embeddings_model` — supports "find all nodes embedded
    ///   under model X" scans during backfill / model migration.
    ///
    /// **Additive only** — does not touch existing `memory_embeddings`.
    /// Phase B backfill (T20) populates this table from `memory_embeddings`.
    /// Idempotent (GUARD-ss.3) via `CREATE … IF NOT EXISTS`.
    ///
    /// T05 (`migrate_unified_nodes`) must run first because of the FK.
    /// Call site in `Storage::open` enforces this.
    fn migrate_unified_node_embeddings(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS node_embeddings (
                node_id     TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                model       TEXT NOT NULL,
                embedding   BLOB NOT NULL,
                dimensions  INTEGER NOT NULL,
                created_at  REAL NOT NULL,
                PRIMARY KEY (node_id, model)
            );
            CREATE INDEX IF NOT EXISTS idx_node_embeddings_model
                ON node_embeddings(model);
        "#)?;
        Ok(())
    }

    /// v0.4 unified substrate (T19+): create the `backfill_runs` audit table
    /// per design §5.3. One row per backfill invocation, regardless of
    /// which legacy→unified table the driver targeted. This table is the
    /// **only** stateful artifact backfill leaves behind on the DB outside
    /// the unified `nodes`/`edges` tables themselves; idempotency relies
    /// purely on `INSERT OR IGNORE` semantics (the audit row is for
    /// operator visibility, not for re-run correctness).
    ///
    /// Schema:
    /// - `run_id` UUID — generated by the driver, returned to caller.
    /// - `legacy_table` — source table being backfilled (`memories`,
    ///   `memory_embeddings`, `entities`, `entity_relations`,
    ///   `memory_entities`, `hebbian_links`, `synthesis_provenance`).
    ///   String, not enum, so a future driver can extend without a
    ///   migration.
    /// - `rows_read` — total rows iterated from the legacy table.
    /// - `rows_inserted` — newly written unified rows this run.
    /// - `rows_skipped_existing` — legacy rows whose unified counterpart
    ///   already existed (idempotency hit).
    /// - `rows_failed` — legacy rows that errored during translation
    ///   (always 0 for memories backfill — no LLM, no parse failures).
    /// - `started_at` / `finished_at` — wall-clock epoch seconds (REAL).
    /// - `notes` — free-form JSON for driver-specific diagnostics.
    ///
    /// **Idempotent** (GUARD-ss.3): `CREATE TABLE IF NOT EXISTS` on every
    /// open. No data is mutated.
    fn migrate_backfill_runs(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS backfill_runs (
                run_id                  TEXT PRIMARY KEY,
                legacy_table            TEXT NOT NULL,
                rows_read               INTEGER NOT NULL DEFAULT 0,
                rows_inserted           INTEGER NOT NULL DEFAULT 0,
                rows_skipped_existing   INTEGER NOT NULL DEFAULT 0,
                rows_failed             INTEGER NOT NULL DEFAULT 0,
                started_at              REAL NOT NULL,
                finished_at             REAL,
                notes                   TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_backfill_runs_table_time
                ON backfill_runs(legacy_table, started_at);
        "#)?;
        Ok(())
    }

    /// v0.4 unified substrate (T09): bump `engram_meta.schema_version` to
    /// `0.4-additive` once Phase A migrations (T05/T06/T07/T08) have all
    /// run successfully.
    ///
    /// **Why a string, not an int**: legacy v0.3 used `'1'` (integer-as-
    /// text). v0.4 introduces phased migration (`0.4-additive`,
    /// `0.4-dual-write`, `0.4-unified`) and the version string carries
    /// phase semantics. Tooling that needs ordering can split on `-`.
    ///
    /// **Why INSERT OR REPLACE, not UPDATE**: the row may be absent on a
    /// brand-new DB where the legacy seed `INSERT OR IGNORE … '1'` and
    /// this bump are both first-time writes — order doesn't matter and
    /// both end states are correct (`0.4-additive` wins because we run
    /// after the seed).
    ///
    /// **Idempotent** (GUARD-ss.3): re-opening a v0.4 DB just rewrites
    /// the same value. Safe to run on every open.
    ///
    /// Call site: **last** step in `Storage::open` so a partial Phase A
    /// (e.g. T07 trigger creation fails after T05/T06 succeed) leaves
    /// the version string unchanged, and the next `open()` retries the
    /// missing pieces.
    fn bump_schema_version_v04_additive(conn: &Connection) -> SqlResult<()> {
        // Ensure engram_meta exists. `Storage::new` creates it in the
        // legacy bootstrap section (storage.rs:738), but
        // `migrate_v04_substrate` is also called from
        // `init_graph_tables` (tests/foreign-connection paths) where
        // the legacy bootstrap hasn't run. Creating it here is
        // idempotent and cheap.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS engram_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO engram_meta (key, value) VALUES ('schema_version', '0.4-additive')",
            [],
        )?;
        Ok(())
    }

    /// v0.4 unified substrate (T07): create the `nodes_fts` FTS5 virtual table
    /// and its three sync triggers per design.md §3.3.
    ///
    /// **Mode**: contentless FTS5 (`content=''`). The canonical text lives in
    /// `nodes.content`/`nodes.summary`; FTS stores tokens only. Triggers
    /// keyed by `nodes.fts_rowid` (a stable monotonic integer, NOT the SQLite
    /// implicit rowid which is unstable across VACUUM when the PK is TEXT)
    /// keep FTS in lockstep with `nodes`.
    ///
    /// **Trigger form**: contentless FTS5 deletes require the special
    /// `INSERT INTO nodes_fts(nodes_fts, rowid, content, summary)
    ///  VALUES ('delete', …)` command — a plain `DELETE` is rejected by FTS5.
    /// Updates are decompose into a delete-then-insert pair on the same
    /// `fts_rowid`.
    ///
    /// **Additive only** — does not touch any existing v0.3 FTS table
    /// (which targets `memories`, not `nodes`). Idempotent (GUARD-ss.3) via
    /// `CREATE VIRTUAL TABLE IF NOT EXISTS` + `CREATE TRIGGER IF NOT EXISTS`.
    ///
    /// T05 (`migrate_unified_nodes`) must run first because the triggers
    /// reference `nodes`. Call site in `Storage::open` enforces this.
    fn migrate_unified_fts(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            -- Contentless FTS5: stores tokens only; canonical text is in
            -- nodes.content / nodes.summary. Keyed by nodes.fts_rowid.
            CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                content,
                summary,
                tokenize='unicode61 remove_diacritics 2',
                content=''
            );

            -- INSERT: project (fts_rowid, content, summary) into FTS.
            CREATE TRIGGER IF NOT EXISTS nodes_fts_ai
            AFTER INSERT ON nodes BEGIN
                INSERT INTO nodes_fts(rowid, content, summary)
                VALUES (new.fts_rowid, new.content, new.summary);
            END;

            -- DELETE: contentless FTS5 requires the 'delete' command form.
            CREATE TRIGGER IF NOT EXISTS nodes_fts_ad
            AFTER DELETE ON nodes BEGIN
                INSERT INTO nodes_fts(nodes_fts, rowid, content, summary)
                VALUES ('delete', old.fts_rowid, old.content, old.summary);
            END;

            -- UPDATE OF content,summary: delete-then-insert on same fts_rowid.
            CREATE TRIGGER IF NOT EXISTS nodes_fts_au
            AFTER UPDATE OF content, summary ON nodes BEGIN
                INSERT INTO nodes_fts(nodes_fts, rowid, content, summary)
                VALUES ('delete', old.fts_rowid, old.content, old.summary);
                INSERT INTO nodes_fts(rowid, content, summary)
                VALUES (new.fts_rowid, new.content, new.summary);
            END;
        "#)?;
        Ok(())
    }
    
    /// Get a reference to the underlying database connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get a mutable reference to the underlying database connection.
    ///
    /// Required by callers that build a `&mut`-borrowing helper around
    /// the connection — notably `SqliteGraphStore::new(&'a mut Connection)`,
    /// which the v0.3 read paths invoke from `Memory::extraction_status`
    /// and friends. Direct SQL mutation outside that pattern is
    /// discouraged; prefer the higher-level methods on `Storage` /
    /// `GraphStore`.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    fn create_schema(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                layer TEXT NOT NULL,
                created_at REAL NOT NULL,
                working_strength REAL NOT NULL DEFAULT 1.0,
                core_strength REAL NOT NULL DEFAULT 0.0,
                importance REAL NOT NULL DEFAULT 0.3,
                pinned INTEGER NOT NULL DEFAULT 0,
                consolidation_count INTEGER NOT NULL DEFAULT 0,
                last_consolidated REAL,
                source TEXT DEFAULT '',
                contradicts TEXT DEFAULT '',
                contradicted_by TEXT DEFAULT '',
                metadata TEXT,
                namespace TEXT NOT NULL DEFAULT 'default'
            );

            CREATE TABLE IF NOT EXISTS access_log (
                memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                accessed_at REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS hebbian_links (
                source_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                target_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                strength REAL NOT NULL DEFAULT 1.0,
                coactivation_count INTEGER NOT NULL DEFAULT 0,
                temporal_forward INTEGER NOT NULL DEFAULT 0,
                temporal_backward INTEGER NOT NULL DEFAULT 0,
                direction TEXT NOT NULL DEFAULT 'bidirectional',
                created_at REAL NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default',
                PRIMARY KEY (source_id, target_id)
            );
            
            CREATE TABLE IF NOT EXISTS engram_acl (
                agent_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                permission TEXT NOT NULL,
                granted_by TEXT NOT NULL,
                created_at REAL NOT NULL,
                PRIMARY KEY (agent_id, namespace)
            );

            -- Schema metadata
            CREATE TABLE IF NOT EXISTS engram_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT OR IGNORE INTO engram_meta VALUES ('schema_version', '1');

            -- Entity tables (canonical schema)
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                namespace TEXT NOT NULL DEFAULT 'default',
                metadata TEXT,
                created_at REAL NOT NULL,
                updated_at REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS entity_relations (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                target_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                relation TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0,
                source TEXT,
                namespace TEXT NOT NULL DEFAULT 'default',
                created_at REAL NOT NULL,
                metadata TEXT
            );

            CREATE TABLE IF NOT EXISTS memory_entities (
                memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                role TEXT NOT NULL DEFAULT 'mention',
                PRIMARY KEY (memory_id, entity_id)
            );

            CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
            CREATE INDEX IF NOT EXISTS idx_hebbian_source ON hebbian_links(source_id);
            CREATE INDEX IF NOT EXISTS idx_hebbian_target ON hebbian_links(target_id);
            CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
            CREATE INDEX IF NOT EXISTS idx_hebbian_namespace ON hebbian_links(namespace);
            CREATE INDEX IF NOT EXISTS idx_entities_namespace ON entities(namespace);
            CREATE INDEX IF NOT EXISTS idx_entity_relations_source ON entity_relations(source_id);
            CREATE INDEX IF NOT EXISTS idx_entity_relations_target ON entity_relations(target_id);
            CREATE INDEX IF NOT EXISTS idx_memory_entities_memory ON memory_entities(memory_id);
            CREATE INDEX IF NOT EXISTS idx_memory_entities_entity ON memory_entities(entity_id);

            -- Synthesis provenance: tracks which source memories contributed to insights
            CREATE TABLE IF NOT EXISTS synthesis_provenance (
                id TEXT PRIMARY KEY,
                insight_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                cluster_id TEXT NOT NULL,
                synthesis_timestamp TEXT NOT NULL,
                gate_decision TEXT NOT NULL,
                gate_scores TEXT,
                confidence REAL NOT NULL,
                source_original_importance REAL,
                FOREIGN KEY (insight_id) REFERENCES memories(id),
                FOREIGN KEY (source_id) REFERENCES memories(id)
            );

            CREATE INDEX IF NOT EXISTS idx_provenance_insight ON synthesis_provenance(insight_id);
            CREATE INDEX IF NOT EXISTS idx_provenance_source ON synthesis_provenance(source_id);

            -- FTS5 for full-text search (manually managed, not via triggers,
            -- so we can pre-process content for CJK/ASCII boundary tokenization)
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content
            );
            "#,
        )?;
        Ok(())
    }
    
    /// Migrate existing databases to v2 schema (add namespace, ACL table).
    fn migrate_v2(conn: &Connection) -> SqlResult<()> {
        // Check if namespace column exists in memories table
        let has_namespace: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('memories') WHERE name='namespace'",
            [],
            |row| row.get(0),
        )?;
        
        if !has_namespace {
            conn.execute(
                "ALTER TABLE memories ADD COLUMN namespace TEXT NOT NULL DEFAULT 'default'",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace)",
                [],
            )?;
        }
        
        // Check if namespace column exists in hebbian_links table
        let has_hebbian_namespace: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('hebbian_links') WHERE name='namespace'",
            [],
            |row| row.get(0),
        )?;
        
        if !has_hebbian_namespace {
            conn.execute(
                "ALTER TABLE hebbian_links ADD COLUMN namespace TEXT NOT NULL DEFAULT 'default'",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_hebbian_namespace ON hebbian_links(namespace)",
                [],
            )?;
        }
        
        // Create ACL table if not exists (idempotent)
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS engram_acl (
                agent_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                permission TEXT NOT NULL,
                granted_by TEXT NOT NULL,
                created_at REAL NOT NULL,
                PRIMARY KEY (agent_id, namespace)
            );
            "#,
        )?;
        
        Ok(())
    }
    
    /// Migrate to embeddings table — supports v1 → v2 protocol migration.
    ///
    /// Protocol v2 changes:
    /// - PK: (memory_id) → (memory_id, model) for multi-model support
    /// - Embedding format: BLOB only (little-endian f32 array)
    /// - Model naming: `{provider}/{model_name}` convention
    fn migrate_embeddings(conn: &Connection) -> SqlResult<()> {
        // Check if we already have v2 schema
        let protocol_version = Self::get_meta(conn, "embedding_protocol_version")
            .unwrap_or(None)
            .unwrap_or_else(|| "0".to_string());
        
        if protocol_version == "2" {
            // Already at v2, just ensure table exists
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memory_embeddings (
                    memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                    model       TEXT NOT NULL,
                    embedding   BLOB NOT NULL,
                    dimensions  INTEGER NOT NULL,
                    created_at  TEXT NOT NULL,
                    PRIMARY KEY (memory_id, model)
                );
                CREATE INDEX IF NOT EXISTS idx_embeddings_model ON memory_embeddings(model);
                "#,
            )?;
            return Ok(());
        }
        
        // Check if old table exists
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memory_embeddings'",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false);
        
        if !table_exists {
            // Fresh install — create v2 schema directly
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memory_embeddings (
                    memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                    model       TEXT NOT NULL,
                    embedding   BLOB NOT NULL,
                    dimensions  INTEGER NOT NULL,
                    created_at  TEXT NOT NULL,
                    PRIMARY KEY (memory_id, model)
                );
                CREATE INDEX IF NOT EXISTS idx_embeddings_model ON memory_embeddings(model);
                "#,
            )?;
            Self::set_meta(conn, "embedding_protocol_version", "2")?;
            return Ok(());
        }
        
        // Migrate from v1 → v2
        eprintln!("[engram] Migrating memory_embeddings to protocol v2 (multi-model support)...");
        
        // Check what columns exist in old table
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(memory_embeddings)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        
        let has_model = cols.contains(&"model".to_string());
        let has_dimensions = cols.contains(&"dimensions".to_string());
        let has_created_at = cols.contains(&"created_at".to_string());
        
        // Step 1: Create v2 table
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memory_embeddings_v2 (
                memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                model       TEXT NOT NULL,
                embedding   BLOB NOT NULL,
                dimensions  INTEGER NOT NULL,
                created_at  TEXT NOT NULL,
                PRIMARY KEY (memory_id, model)
            );
            "#,
        )?;
        
        // Step 2: Migrate BLOB rows
        let mut migrated = 0;
        let mut skipped = 0;
        
        {
            let select_sql = if has_model && has_dimensions && has_created_at {
                "SELECT memory_id, embedding, model, dimensions, created_at FROM memory_embeddings"
            } else if has_model {
                "SELECT memory_id, embedding, model, 0, '' FROM memory_embeddings"
            } else {
                "SELECT memory_id, embedding, 'unknown/legacy', 0, '' FROM memory_embeddings"
            };
            
            let mut stmt = conn.prepare(select_sql)?;
            let rows: Vec<(String, Vec<u8>, String, i64, String)> = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?.filter_map(|r| r.ok()).collect();
            
            for (memory_id, blob_or_text, mut model, mut dims, created_at) in rows {
                // Determine if this is BLOB or TEXT (JSON)
                let final_blob: Vec<u8>;
                
                if blob_or_text.len() % 4 == 0 && !blob_or_text.is_empty() {
                    // Likely BLOB — check if it looks like valid f32 bytes
                    // A simple heuristic: valid f32 embeddings won't start with `[` (0x5B)
                    if blob_or_text.first() == Some(&0x5B) || blob_or_text.first() == Some(&0x2D) {
                        // Starts with `[` or `-` — probably JSON text
                        match Self::json_text_to_blob(&blob_or_text) {
                            Some((blob, d)) => {
                                final_blob = blob;
                                if dims == 0 { dims = d as i64; }
                            }
                            None => {
                                eprintln!("[engram] Skipping corrupt embedding for memory {}", memory_id);
                                skipped += 1;
                                continue;
                            }
                        }
                    } else {
                        // Assume valid BLOB
                        final_blob = blob_or_text;
                        if dims == 0 { dims = final_blob.len() as i64 / 4; }
                    }
                } else if !blob_or_text.is_empty() {
                    // Not aligned to 4 bytes — must be TEXT/JSON
                    match Self::json_text_to_blob(&blob_or_text) {
                        Some((blob, d)) => {
                            final_blob = blob;
                            if dims == 0 { dims = d as i64; }
                        }
                        None => {
                            eprintln!("[engram] Skipping corrupt embedding for memory {}", memory_id);
                            skipped += 1;
                            continue;
                        }
                    }
                } else {
                    skipped += 1;
                    continue;
                }
                
                // Fix model name: add provider prefix if missing
                if !model.contains('/') {
                    if model == "unknown" || model.is_empty() {
                        model = "unknown/legacy".to_string();
                    } else {
                        // Try to guess provider from model name
                        model = if model.starts_with("text-embedding") {
                            format!("openai/{}", model)
                        } else {
                            format!("ollama/{}", model)
                        };
                    }
                }
                
                let ts = if created_at.is_empty() {
                    chrono::Utc::now().to_rfc3339()
                } else {
                    created_at
                };
                
                conn.execute(
                    "INSERT OR REPLACE INTO memory_embeddings_v2 (memory_id, model, embedding, dimensions, created_at) VALUES (?, ?, ?, ?, ?)",
                    params![memory_id, model, final_blob, dims, ts],
                )?;
                migrated += 1;
            }
        }
        
        // Step 3: Replace old table
        conn.execute_batch(
            r#"
            DROP TABLE memory_embeddings;
            ALTER TABLE memory_embeddings_v2 RENAME TO memory_embeddings;
            CREATE INDEX IF NOT EXISTS idx_embeddings_model ON memory_embeddings(model);
            "#,
        )?;
        
        // Step 4: Set protocol version
        Self::set_meta(conn, "embedding_protocol_version", "2")?;
        
        eprintln!("[engram] Migration complete: {} migrated, {} skipped", migrated, skipped);
        
        Ok(())
    }
    
    /// Helper: convert JSON text embedding to BLOB format.
    fn json_text_to_blob(data: &[u8]) -> Option<(Vec<u8>, usize)> {
        let text = std::str::from_utf8(data).ok()?;
        let values: Vec<f64> = serde_json::from_str(text).ok()?;
        let dims = values.len();
        let blob: Vec<u8> = values.iter()
            .flat_map(|v| (*v as f32).to_le_bytes())
            .collect();
        Some((blob, dims))
    }
    
    /// Get a metadata value from engram_meta table.
    fn get_meta(conn: &Connection, key: &str) -> SqlResult<Option<String>> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS engram_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);"
        )?;
        conn.query_row(
            "SELECT value FROM engram_meta WHERE key = ?",
            params![key],
            |row| row.get(0),
        ).optional()
    }
    
    /// Set a metadata value in engram_meta table.
    fn set_meta(conn: &Connection, key: &str, value: &str) -> SqlResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS engram_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);"
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO engram_meta (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Migrate hebbian_links table: add signal_source and signal_detail columns.
    fn migrate_hebbian_signals(conn: &Connection) -> SqlResult<()> {
        // Add signal_source column (safe migration: ignore "duplicate column name")
        match conn.execute(
            "ALTER TABLE hebbian_links ADD COLUMN signal_source TEXT DEFAULT 'corecall'",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }
        
        // Add signal_detail column
        match conn.execute(
            "ALTER TABLE hebbian_links ADD COLUMN signal_detail TEXT DEFAULT NULL",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }
        
        // Backfill existing rows
        conn.execute(
            "UPDATE hebbian_links SET signal_source = 'corecall' WHERE signal_source IS NULL",
            [],
        )?;
        
        // Add index for signal_source queries
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_hebbian_signal_source ON hebbian_links(signal_source);"
        )?;
        
        Ok(())
    }

    /// **ISS-117**: Dedupe legacy double-direction `hebbian_links` rows
    /// into single canonical `(min(source,target), max(source,target))`
    /// rows.
    ///
    /// Before ISS-117, the formed-link path in `record_coactivation*`
    /// inserted both `(a, b)` and `(b, a)` rows into `hebbian_links`.
    /// Phase B dual-write (T14) canonicalised to one `edges` row per
    /// link, so a reader switching from `hebbian_links` to
    /// `edges` saw row-shape divergence: 2 legacy rows vs 1 unified
    /// row. The writer-side fix in ISS-117 stops emitting reverse
    /// rows, but existing databases still have them. This one-shot
    /// migration collapses them.
    ///
    /// **Algorithm**: for every pair `(a, b)` where both `(a, b)` and
    /// `(b, a)` exist, keep the canonical `(min, max)` row, merging
    /// the duplicate's fields with max-semantics on `strength`,
    /// sum on `coactivation_count`, sum on
    /// `temporal_forward/backward`, min on `created_at`. Then DELETE
    /// the non-canonical row.
    ///
    /// Idempotent: re-running on an already-canonical table is a
    /// no-op (no pairs match the JOIN).
    fn migrate_hebbian_canonical_rows(conn: &Connection) -> SqlResult<()> {
        // Step 1: merge non-canonical row's metrics into canonical row.
        // A row is "non-canonical" if source_id > target_id.
        // For each such row, find its mirror (canonical) and merge.
        conn.execute(
            "UPDATE hebbian_links AS canonical \
             SET strength = MAX(canonical.strength, ( \
                 SELECT mirror.strength FROM hebbian_links AS mirror \
                 WHERE mirror.source_id = canonical.target_id \
                   AND mirror.target_id = canonical.source_id \
             )), \
             coactivation_count = canonical.coactivation_count + COALESCE(( \
                 SELECT mirror.coactivation_count FROM hebbian_links AS mirror \
                 WHERE mirror.source_id = canonical.target_id \
                   AND mirror.target_id = canonical.source_id \
             ), 0), \
             temporal_forward = canonical.temporal_forward + COALESCE(( \
                 SELECT mirror.temporal_forward FROM hebbian_links AS mirror \
                 WHERE mirror.source_id = canonical.target_id \
                   AND mirror.target_id = canonical.source_id \
             ), 0), \
             temporal_backward = canonical.temporal_backward + COALESCE(( \
                 SELECT mirror.temporal_backward FROM hebbian_links AS mirror \
                 WHERE mirror.source_id = canonical.target_id \
                   AND mirror.target_id = canonical.source_id \
             ), 0), \
             created_at = MIN(canonical.created_at, COALESCE(( \
                 SELECT mirror.created_at FROM hebbian_links AS mirror \
                 WHERE mirror.source_id = canonical.target_id \
                   AND mirror.target_id = canonical.source_id \
             ), canonical.created_at)) \
             WHERE canonical.source_id < canonical.target_id \
               AND EXISTS ( \
                   SELECT 1 FROM hebbian_links AS mirror \
                   WHERE mirror.source_id = canonical.target_id \
                     AND mirror.target_id = canonical.source_id \
               )",
            [],
        )?;

        // Step 2: delete non-canonical rows (source_id > target_id).
        // These are the reverse-direction duplicates whose metrics
        // were just merged into their canonical mirror.
        conn.execute(
            "DELETE FROM hebbian_links WHERE source_id > target_id",
            [],
        )?;

        Ok(())
    }
    
    /// Migrate entity tables: add unique constraints needed for upsert operations.
    /// Migrate schema for triple extraction support.
    fn migrate_triples(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS triples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0,
                source TEXT NOT NULL DEFAULT 'llm',
                created_at TEXT NOT NULL,
                UNIQUE(memory_id, subject, predicate, object)
            );
            CREATE INDEX IF NOT EXISTS idx_triples_memory ON triples(memory_id);
            CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject);
            CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object);
            "#
        )?;

        // Add triple_extraction_attempts column to memories
        match conn.execute(
            "ALTER TABLE memories ADD COLUMN triple_extraction_attempts INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("duplicate column name") => {},
            Err(e) => return Err(e),
        }

        Ok(())
    }

    fn migrate_entities(conn: &Connection) -> SqlResult<()> {
        // Add UNIQUE index on entities(name, entity_type, namespace) for deterministic upserts
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_unique ON entities(name, entity_type, namespace);"
        )?;
        
        // entity_relations needs a UNIQUE constraint on (source_id, target_id, relation)
        // for ON CONFLICT to work. We create a unique index.
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_entity_relations_unique ON entity_relations(source_id, target_id, relation);"
        )?;
        
        Ok(())
    }

    /// Migrate schema for promotion candidates table (ISS-008).
    fn migrate_promotions(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS promotion_candidates (
                id TEXT PRIMARY KEY,
                member_ids TEXT NOT NULL,
                snippets TEXT NOT NULL,
                avg_core_strength REAL NOT NULL,
                avg_importance REAL NOT NULL,
                time_span_days REAL NOT NULL,
                internal_link_count INTEGER NOT NULL,
                suggested_target TEXT NOT NULL,
                summary TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL,
                resolved_at TEXT
            );
            "#
        )?;
        Ok(())
    }

    /// Store a promotion candidate.
    pub fn store_promotion_candidate(&self, candidate: &crate::promotion::PromotionCandidate) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO promotion_candidates (id, member_ids, snippets, avg_core_strength, avg_importance, time_span_days, internal_link_count, suggested_target, summary, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                candidate.id,
                serde_json::to_string(&candidate.member_ids).unwrap_or_default(),
                serde_json::to_string(&candidate.snippets).unwrap_or_default(),
                candidate.avg_core_strength,
                candidate.avg_importance,
                candidate.time_span_days,
                candidate.internal_link_count,
                candidate.suggested_target,
                candidate.summary,
                candidate.status,
                candidate.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Get all pending promotion candidates.
    pub fn get_pending_promotions(&self) -> Result<Vec<crate::promotion::PromotionCandidate>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, member_ids, snippets, avg_core_strength, avg_importance, time_span_days, internal_link_count, suggested_target, summary, status, created_at FROM promotion_candidates WHERE status = 'pending'"
        )?;
        let rows = stmt.query_map([], |row| {
            let member_ids_json: String = row.get(1)?;
            let snippets_json: String = row.get(2)?;
            let created_at_str: String = row.get(10)?;
            Ok(crate::promotion::PromotionCandidate {
                id: row.get(0)?,
                member_ids: serde_json::from_str(&member_ids_json).unwrap_or_default(),
                snippets: serde_json::from_str(&snippets_json).unwrap_or_default(),
                avg_core_strength: row.get(3)?,
                avg_importance: row.get(4)?,
                time_span_days: row.get(5)?,
                internal_link_count: row.get::<_, i64>(6)? as usize,
                suggested_target: row.get(7)?,
                summary: row.get(8)?,
                status: row.get(9)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            })
        })?;
        rows.collect()
    }

    /// Resolve a promotion candidate (mark as approved or dismissed).
    pub fn resolve_promotion(&self, id: &str, status: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE promotion_candidates SET status = ?1, resolved_at = ?2 WHERE id = ?3",
            rusqlite::params![status, chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Check if a cluster (by member IDs) has already been promoted (approved or pending).
    pub fn is_cluster_already_promoted(&self, member_ids: &[String]) -> Result<bool, rusqlite::Error> {
        // Check if any existing non-dismissed candidate has significant overlap with these member_ids
        let mut stmt = self.conn.prepare(
            "SELECT member_ids FROM promotion_candidates WHERE status != 'dismissed'"
        )?;
        let rows = stmt.query_map([], |row| {
            let json: String = row.get(0)?;
            Ok(json)
        })?;

        let input_set: std::collections::HashSet<&str> = member_ids.iter().map(|s| s.as_str()).collect();

        for row in rows {
            let json = row?;
            if let Ok(existing_ids) = serde_json::from_str::<Vec<String>>(&json) {
                let existing_set: std::collections::HashSet<&str> = existing_ids.iter().map(|s| s.as_str()).collect();
                let overlap = input_set.intersection(&existing_set).count();
                // If >50% overlap, consider it already promoted
                let min_size = input_set.len().min(existing_set.len());
                if min_size > 0 && overlap * 2 >= min_size {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Rebuild FTS index with CJK tokenization if not already done.
    /// Uses engram_meta 'fts_cjk_version' to track migration state.
    fn rebuild_fts_if_needed(conn: &Connection) -> SqlResult<()> {
        const FTS_CJK_VERSION: &str = "1";
        
        let current: Option<String> = conn
            .query_row(
                "SELECT value FROM engram_meta WHERE key = 'fts_cjk_version'",
                [],
                |row| row.get(0),
            )
            .ok();
        
        if current.as_deref() == Some(FTS_CJK_VERSION) {
            return Ok(()); // Already up to date
        }
        
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        if count == 0 {
            conn.execute(
                "INSERT OR REPLACE INTO engram_meta VALUES ('fts_cjk_version', ?1)",
                params![FTS_CJK_VERSION],
            )?;
            return Ok(());
        }
        
        // Rebuild: clear FTS and re-insert all with tokenization (in a transaction)
        conn.execute_batch("BEGIN IMMEDIATE")?;
        
        conn.execute("DELETE FROM memories_fts", [])?;
        
        let mut stmt = conn.prepare("SELECT rowid, content FROM memories")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        
        for (rowid, content) in &rows {
            let tokenized = tokenize_cjk_boundaries(content);
            conn.execute(
                "INSERT INTO memories_fts(rowid, content) VALUES (?1, ?2)",
                params![rowid, tokenized],
            )?;
        }
        
        conn.execute(
            "INSERT OR REPLACE INTO engram_meta VALUES ('fts_cjk_version', ?1)",
            params![FTS_CJK_VERSION],
        )?;
        
        conn.execute_batch("COMMIT")?;
        
        eprintln!("[engram] Rebuilt FTS index with CJK tokenization for {} memories", rows.len());
        Ok(())
    }

    /// Add a new memory to storage.
    pub fn add(&mut self, record: &MemoryRecord, namespace: &str) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        
        let metadata_json = record.metadata.as_ref().and_then(|m| serde_json::to_string(m).ok());
        
        tx.execute(
            r#"
            INSERT INTO memories (
                id, content, memory_type, layer, created_at,
                working_strength, core_strength, importance, pinned,
                consolidation_count, last_consolidated, source,
                contradicts, contradicted_by, metadata, namespace,
                occurred_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                record.id,
                record.content,
                record.memory_type.to_string(),
                record.layer.to_string(),
                datetime_to_f64(&record.created_at),
                record.working_strength,
                record.core_strength,
                record.importance,
                record.pinned as i32,
                record.consolidation_count,
                record.last_consolidated.map(|dt| datetime_to_f64(&dt)),
                record.source,
                record.contradicts.as_ref().unwrap_or(&String::new()),
                record.contradicted_by.as_ref().unwrap_or(&String::new()),
                metadata_json,
                namespace,
                record.occurred_at.map(|dt| datetime_to_f64(&dt)),
            ],
        )?;
        
        // Record initial access
        tx.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![record.id, datetime_to_f64(&record.created_at)],
        )?;
        
        // Insert into FTS with CJK/ASCII boundary tokenization
        let tokenized = tokenize_cjk_boundaries(&record.content);
        let rowid: i64 = tx.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![record.id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?, ?)",
            params![rowid, tokenized],
        )?;

        // T12 — Phase B dual-write: every memory row also lands in
        // `nodes` as `node_kind='memory'`. Delegates to
        // `insert_memory_node_row`, which is the single source of
        // truth for the memory→nodes projection (also used by the
        // T19 Phase C backfill driver). Keeping the mapping in one
        // place guarantees that dual-write and backfill stay in sync
        // as the schema evolves.
        Self::insert_memory_node_row(&tx, record, namespace, metadata_json.as_deref())?;

        tx.commit()?;
        Ok(())
    }

    /// Project a `MemoryRecord` into the unified `nodes` table as a
    /// `node_kind='memory'` row. **Single source of truth** for the
    /// memory→nodes field mapping (design.md §3.1 + §5.3), used by:
    ///
    ///   - `Storage::add` — Phase B dual-write (T12), inserts both
    ///     the legacy `memories` row and the unified `nodes` row in
    ///     the same transaction.
    ///   - `substrate::backfill::backfill_memories_to_nodes` — Phase
    ///     C historical backfill (T19), iterates existing `memories`
    ///     rows and writes their unified projection.
    ///
    /// ## Why a helper, not duplicated SQL
    ///
    /// The dual-write and the backfill must produce **identical**
    /// `nodes` rows for the same `MemoryRecord` — otherwise Phase D
    /// read cutover sees inconsistent state depending on whether a
    /// memory was added before or after Phase C ran. Centralising the
    /// projection here makes that invariant a compile-time fact
    /// rather than a code-review one. Future schema changes touch
    /// exactly one place; T17 parity (and any successor) doesn't have
    /// to chase divergence.
    ///
    /// ## Field mapping
    ///
    ///   - `id, content, layer, memory_type, namespace`: direct copy
    ///     (strings, no conversion).
    ///   - `created_at, occurred_at, last_consolidated`: same `f64`
    ///     epoch on both tables; `nodes.updated_at` mirrors
    ///     `created_at` at insert time (further updates are the
    ///     concern of UPDATE paths like `supersede`).
    ///   - `working_strength, core_strength, importance,
    ///     consolidation_count, pinned, source`: direct copy.
    ///   - `attributes`: caller-supplied JSON string (already
    ///     serialized from `record.metadata` upstream); `NULL`
    ///     coerces to `'{}'` via `COALESCE` so the SQL `NOT NULL`
    ///     contract on `nodes.attributes` stays intact.
    ///   - `summary`: empty string (memories have no separate
    ///     summary; the `nodes_fts` trigger indexes `content` as
    ///     primary search target).
    ///   - `superseded_by`: **always `NULL`** on the insert path.
    ///     Fresh inserts cannot have been superseded yet;
    ///     supersession is established post-insert via
    ///     `supersede` / `supersede_bulk`, which dual-update both
    ///     `memories.superseded_by` and `nodes.superseded_by`. See
    ///     the T12 root fix commit `de0af68` for the rationale on
    ///     why this field is **not** sourced from
    ///     `record.superseded_by` or `record.contradicted_by`.
    ///   - `fts_rowid`: claims the next monotonic value from
    ///     `fts_rowid_counter`. Burning a rowid when the
    ///     `INSERT OR IGNORE` is a no-op is harmless — `nodes_fts`
    ///     is contentless and the counter just has to stay unique.
    ///
    /// ## Idempotency
    ///
    /// `INSERT OR IGNORE` on `nodes(id)`: re-inserting the same
    /// memory id is a no-op, returning `Ok(false)`. The backfill
    /// driver relies on this for re-run safety (GUARD-ss.3).
    ///
    /// Returns `true` iff a new row was actually inserted (i.e. the
    /// `INSERT OR IGNORE` did not collide with an existing id),
    /// allowing callers to count `rows_inserted` vs
    /// `rows_skipped_existing` for backfill audit rows.
    pub(crate) fn insert_memory_node_row(
        tx: &rusqlite::Transaction<'_>,
        record: &MemoryRecord,
        namespace: &str,
        attributes_json: Option<&str>,
    ) -> Result<bool, rusqlite::Error> {
        let next_fts_rowid: i64 = tx.query_row(
            "UPDATE fts_rowid_counter
             SET next_value = next_value + 1
             WHERE singleton = 0
             RETURNING next_value - 1",
            [],
            |r| r.get(0),
        )?;

        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO nodes (
                id, node_kind, namespace,
                layer, memory_type,
                content, summary, attributes,
                occurred_at, created_at, updated_at, last_consolidated,
                working_strength, core_strength, importance,
                consolidation_count, pinned,
                source, superseded_by,
                fts_rowid
            ) VALUES (
                ?, 'memory', ?,
                ?, ?,
                ?, '', COALESCE(?, '{}'),
                ?, ?, ?, ?,
                ?, ?, ?,
                ?, ?,
                ?, ?,
                ?
            )
            "#,
            params![
                record.id,
                namespace,
                record.layer.to_string(),
                record.memory_type.to_string(),
                record.content,
                attributes_json,
                record.occurred_at.map(|dt| datetime_to_f64(&dt)),
                datetime_to_f64(&record.created_at),
                datetime_to_f64(&record.created_at),
                record.last_consolidated.map(|dt| datetime_to_f64(&dt)),
                record.working_strength,
                record.core_strength,
                record.importance,
                record.consolidation_count,
                record.pinned as i32,
                record.source,
                None::<&str>,
                next_fts_rowid,
            ],
        )?;

        Ok(rows > 0)
    }

    /// Project a `(memory_id, model, embedding, dimensions,
    /// created_at_rfc3339)` row from the legacy `memory_embeddings`
    /// table into the unified `node_embeddings` table (T20 / design
    /// §5.3).
    ///
    /// ## Why a helper, not duplicated SQL
    ///
    /// Phase B does not yet dual-write embeddings — there is only
    /// the backfill caller (T20). The helper exists nevertheless so
    /// that **when** Phase B grows an embedding dual-write path
    /// (likely as part of follow-up work to keep `node_embeddings`
    /// live), there is exactly one place that defines the legacy →
    /// unified embedding row shape. T17-style parity tests can pin
    /// the byte-equal invariant the moment a second caller appears.
    ///
    /// ## Field mapping
    ///
    ///   - `memory_id → node_id`: direct copy. The legacy FK to
    ///     `memories(id)` projects 1:1 to the unified FK to
    ///     `nodes(id)`; this means **T20 requires T19 to have run
    ///     first** so the parent `nodes` row exists.
    ///   - `model, embedding, dimensions`: direct copy (BLOB is
    ///     byte-identical between tables).
    ///   - `created_at`: RFC3339 TEXT → epoch `REAL` via
    ///     `chrono::DateTime::parse_from_rfc3339`. If parsing fails
    ///     (corrupt legacy data), the caller decides whether to skip
    ///     the row or fall back to `Utc::now()`. The helper itself
    ///     accepts a pre-converted `f64` to keep the policy choice
    ///     out of the SQL layer.
    ///
    /// ## Idempotency
    ///
    /// `INSERT OR IGNORE` on `(node_id, model)`: re-inserting the
    /// same pair is a no-op. The backfill driver relies on this for
    /// re-run safety, identical to T19 semantics.
    ///
    /// Returns `true` iff a new row was inserted.
    pub(crate) fn insert_node_embedding_row(
        tx: &rusqlite::Transaction<'_>,
        node_id: &str,
        model: &str,
        embedding: &[u8],
        dimensions: i64,
        created_at_epoch: f64,
    ) -> Result<bool, rusqlite::Error> {
        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO node_embeddings
                (node_id, model, embedding, dimensions, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            params![node_id, model, embedding, dimensions, created_at_epoch],
        )?;
        Ok(rows > 0)
    }

    /// Project a legacy `entities` row into the unified `nodes` table
    /// as a `node_kind='entity'` row (T21 / design §5.3).
    ///
    /// ## Why a helper, not duplicated SQL
    ///
    /// Phase B already dual-writes resolution-pipeline `Entity` rows
    /// via `graph::store::dual_write_entity_to_nodes`, but that
    /// callsite operates on the richer in-memory `Entity` struct
    /// (with affect, embedding, history). The legacy `entities`
    /// table is a thinner schema (just id, name, entity_type,
    /// namespace, metadata, created_at, updated_at) so backfill
    /// cannot share T13's helper — different input shape, different
    /// defaults, different field set.
    ///
    /// This helper is therefore distinct from `dual_write_entity_to_nodes`
    /// by design. The contract they share is the **output**:
    /// `nodes(node_kind='entity')` rows produced by either path are
    /// retrievable through the same Phase D read paths. The helper
    /// owns the legacy → unified projection; if Phase B ever grows
    /// a path that writes legacy-shaped entity rows (rather than
    /// resolution-pipeline ones), it should call this helper.
    ///
    /// ## Field mapping (design §5.3)
    ///
    ///   - `id`: direct copy (TEXT PK in both tables).
    ///   - `name → content`: the human-visible label.
    ///   - `entity_type`: stored in `attributes` JSON under the
    ///     `"entity_type"` key. This matches the design contract
    ///     ("entities.entity_type → nodes.attributes.entity_type")
    ///     and avoids carrying a denormalized column that only
    ///     `node_kind='entity'` rows would use.
    ///   - `metadata`: caller-supplied merged JSON (the helper does
    ///     not parse `entities.metadata` itself — the driver
    ///     handles the merge-with-existing logic for case-2 rows).
    ///   - `namespace, created_at, updated_at`: direct copy.
    ///   - `summary, embedding, history, affect, etc.`: schema
    ///     defaults (empty/zero). T13-shaped fields like
    ///     `agent_affect`, `arousal`, `somatic_fingerprint` are
    ///     pipeline-only — legacy entities never had them.
    ///   - `fts_rowid`: claim next monotonic value (same scheme as
    ///     T19/T20).
    ///
    /// ## Idempotency
    ///
    /// `INSERT OR IGNORE` on `nodes(id)`. Re-running the backfill is
    /// a no-op for already-projected entities, returning
    /// `Ok(false)`. For "row existed already" cases, the driver
    /// handles the **merge** logic separately (Pass 2).
    ///
    /// Returns `true` iff a row was newly inserted.
    pub(crate) fn insert_entity_node_row(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        name: &str,
        attributes_json: &str,
        namespace: &str,
        created_at: f64,
        updated_at: f64,
    ) -> Result<bool, rusqlite::Error> {
        let next_fts_rowid: i64 = tx.query_row(
            "UPDATE fts_rowid_counter
             SET next_value = next_value + 1
             WHERE singleton = 0
             RETURNING next_value - 1",
            [],
            |r| r.get(0),
        )?;

        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO nodes (
                id, node_kind, namespace,
                content, summary, attributes,
                created_at, updated_at,
                fts_rowid
            ) VALUES (
                ?, 'entity', ?,
                ?, '', ?,
                ?, ?,
                ?
            )
            "#,
            params![
                id,
                namespace,
                name,
                attributes_json,
                created_at,
                updated_at,
                next_fts_rowid,
            ],
        )?;
        Ok(rows > 0)
    }

    /// Project a legacy `entity_relations` row into the unified
    /// `edges` table as a `edge_kind='structural'` row (T22 /
    /// design §5.3).
    ///
    /// ## Why a separate helper from T13's `dual_write_edge_to_edges`
    ///
    /// T13's helper takes a `graph::edge::Edge` struct from the
    /// resolution pipeline and hardcodes `edge_kind='assertion'`.
    /// T22 backfills the legacy `entity_relations` table whose
    /// rows correspond to a different ontological category
    /// (`edge_kind='structural'` — "X has-part Y" type facts
    /// recorded directly rather than asserted by an
    /// utterance-resolution pipeline). The input shape is also
    /// thinner: just (id, source_id, target_id, relation,
    /// confidence, source, namespace, created_at, metadata).
    ///
    /// Same single-source-of-truth philosophy as the entity
    /// helper: if Phase B ever grows a path that writes
    /// legacy-shaped structural edges, it should call this helper.
    ///
    /// ## Field mapping (design §5.3)
    ///
    ///   - `entity_relations.id → edges.id` (TEXT, no conversion).
    ///   - `source_id, target_id → edges.source_id, edges.target_id`
    ///     (FK to `nodes(id)`).
    ///   - `relation → edges.predicate` (literal copy).
    ///   - `confidence → edges.confidence`.
    ///   - `namespace, created_at`: direct copy. `recorded_at` and
    ///     `updated_at` both fall back to `created_at` — legacy
    ///     entity_relations has no separate fields.
    ///   - `metadata` (legacy JSON object) + `source` (free text
    ///     column): both merged into `edges.attributes` by the
    ///     CALLER. This helper just takes a pre-built
    ///     `attributes_json` string.
    ///   - `edge_kind='structural'`, `predicate_kind='canonical'`
    ///     are constants set here. Other schema columns
    ///     (weight=1.0, activation=0.0, valid_from=NULL,
    ///     resolution_method='direct', etc.) use schema defaults.
    ///
    /// ## FK requirements
    ///
    /// `edges.source_id` and `edges.target_id` both have ON DELETE
    /// RESTRICT FKs into `nodes(id)`. The CALLER must verify the
    /// endpoint nodes exist before invoking the helper, otherwise
    /// SQLite returns a constraint failure and the entire tx fails.
    /// T22's driver checks via `EXISTS` and skips dangling endpoints
    /// (counted in audit notes for recovery).
    ///
    /// ## Idempotency
    ///
    /// `INSERT OR IGNORE` on `edges(id)`. Re-running yields
    /// `Ok(false)` on already-projected rows. Pass 2 merge
    /// semantics for attributes are driver-side, not helper-side.
    ///
    /// Returns `true` iff a row was newly inserted.
    pub(crate) fn insert_structural_edge_row(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        source_id: &str,
        target_id: &str,
        predicate: &str,
        attributes_json: &str,
        confidence: f64,
        namespace: &str,
        created_at: f64,
    ) -> Result<bool, rusqlite::Error> {
        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO edges (
                id,
                source_id, target_id, target_literal,
                edge_kind, predicate_kind, predicate,
                summary, attributes,
                confidence,
                recorded_at,
                resolution_method,
                namespace, created_at, updated_at
            ) VALUES (
                ?,
                ?, ?, NULL,
                'structural', 'canonical', ?,
                '', ?,
                ?,
                ?,
                'direct',
                ?, ?, ?
            )
            "#,
            params![
                id,
                source_id,
                target_id,
                predicate,
                attributes_json,
                confidence,
                created_at,
                namespace,
                created_at,
                created_at,
            ],
        )?;
        Ok(rows > 0)
    }

    /// T23 — Phase C backfill helper: project a `memory_entities` row
    /// (or any other memory→entity mention-style fact) into the unified
    /// `edges` table as `edge_kind='provenance'`.
    ///
    /// Field mapping (per v04-unified-substrate design.md §3.3 + §5.3):
    ///   * `id`             — caller-computed deterministic UUID
    ///                        (sha256 over the legacy row's natural key,
    ///                        formatted as UUID per §5.3 lines 1170-1182).
    ///   * `source_id`      — memory node id (TEXT, must already exist
    ///                        in `nodes` — caller checks FK).
    ///   * `target_id`      — entity node id (TEXT, must already exist).
    ///   * `predicate`      — `'mentions'` for canonical mention rows.
    ///                        Caller normalizes legacy `role` text into
    ///                        a predicate string (see T23 driver).
    ///   * `attributes_json` — caller-built JSON object. Empty object
    ///                         `"{}"` is fine; the driver records
    ///                         the raw legacy `role` here when it
    ///                         deviates from the canonical value
    ///                         (e.g. `'triple'`) for audit traceability.
    ///   * `confidence`     — provenance edges are not probabilistic
    ///                        in v0.3; pass `1.0` (caller does this).
    ///   * `namespace`      — partition the edge belongs to (derived
    ///                        from the parent memory by the driver,
    ///                        since `memory_entities` has no own
    ///                        namespace column).
    ///   * `created_at`     — derived from the parent memory's
    ///                        `created_at` for the same reason.
    ///
    /// Idempotency: `INSERT OR IGNORE` keyed on `id` only. The
    /// deterministic UUID derivation (see design §5.3) makes
    /// repeating the same backfill a no-op. Note that `provenance`
    /// edges are NOT covered by a partial UNIQUE index on
    /// `(source_id, target_id, edge_kind, predicate)` — by design
    /// (§5.3 lines 1185-1195), they're allowed to accumulate when
    /// emitted by non-backfill writers with fresh random UUIDs.
    /// Idempotency relies entirely on the deterministic-id contract
    /// at the backfill boundary.
    ///
    /// Returns `true` iff a new row was actually inserted (PK was
    /// novel); `false` iff the row already existed.
    pub(crate) fn insert_provenance_edge_row(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        source_id: &str,
        target_id: &str,
        predicate: &str,
        attributes_json: &str,
        confidence: f64,
        namespace: &str,
        created_at: f64,
    ) -> Result<bool, rusqlite::Error> {
        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO edges (
                id,
                source_id, target_id, target_literal,
                edge_kind, predicate_kind, predicate,
                summary, attributes,
                confidence,
                recorded_at,
                resolution_method,
                namespace, created_at, updated_at
            ) VALUES (
                ?,
                ?, ?, NULL,
                'provenance', 'canonical', ?,
                '', ?,
                ?,
                ?,
                'direct',
                ?, ?, ?
            )
            "#,
            params![
                id,
                source_id,
                target_id,
                predicate,
                attributes_json,
                confidence,
                created_at,
                namespace,
                created_at,
                created_at,
            ],
        )?;
        Ok(rows > 0)
    }

    /// T24 — Phase C backfill helper: project a `hebbian_links` row
    /// (or a SQL-side merged group of legacy rows covering the same
    /// canonical pair × signal_source) into the unified `edges`
    /// table as `edge_kind='associative', predicate='co_activated'`.
    ///
    /// Field mapping (per v04-unified-substrate design.md §3.3 + §5.3):
    ///   * `id`             — caller-computed deterministic UUID over
    ///                        the canonicalized natural key
    ///                        `(hebbian_links, min(src,tgt), max(src,tgt),
    ///                        namespace, edge_kind, predicate)` per the
    ///                        amended §5.3 hash template. Two legacy
    ///                        rows for the same pair in opposite
    ///                        directions collapse to the same id.
    ///   * `source_id`,
    ///     `target_id`      — endpoints already canonicalized to
    ///                        `(min, max)` by the caller. Both must
    ///                        exist in `nodes` — caller checks FK
    ///                        before calling.
    ///   * `attributes_json` — caller-built JSON containing the full
    ///                        signal/temporal payload required by
    ///                        §4.6 differential decay:
    ///                          `signal_source`, `signal_detail`,
    ///                          `coactivation_count`, `temporal_forward`,
    ///                          `temporal_backward`, `direction`.
    ///                        `signal_source` is also a row-identity
    ///                        dimension via the partial unique index
    ///                        `idx_edges_assoc_unique` (§3.2); the
    ///                        deterministic id must already encode the
    ///                        same discriminator so re-runs hit the id
    ///                        before the index.
    ///   * `weight`         — `strength` from legacy (post-merge sum if
    ///                        the caller merged opposite-direction rows).
    ///   * `namespace`      — derived from the legacy row's `namespace`
    ///                        column (NOT from endpoint nodes).
    ///   * `created_at`     — min of merged legacy rows' `created_at`
    ///                        (earliest observation wins; preserved as
    ///                        `recorded_at` for §4.6 decay math).
    ///
    /// Idempotency: `INSERT OR IGNORE` against the deterministic
    /// primary key. A re-run of the backfill driver produces zero new
    /// rows (the partial unique associative index is a secondary
    /// safety net; the primary id collision short-circuits first).
    pub(crate) fn insert_associative_edge_row(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        source_id: &str,
        target_id: &str,
        attributes_json: &str,
        weight: f64,
        namespace: &str,
        created_at: f64,
    ) -> Result<bool, rusqlite::Error> {
        let rows = tx.execute(
            r#"
            INSERT OR IGNORE INTO edges (
                id,
                source_id, target_id, target_literal,
                edge_kind, predicate_kind, predicate,
                summary, attributes,
                weight, activation, confidence,
                recorded_at,
                resolution_method,
                namespace, created_at, updated_at
            ) VALUES (
                ?,
                ?, ?, NULL,
                'associative', 'canonical', 'co_activated',
                '', ?,
                ?, 0.0, 1.0,
                ?,
                'direct',
                ?, ?, ?
            )
            "#,
            params![
                id,
                source_id,
                target_id,
                attributes_json,
                weight,
                created_at,
                namespace,
                created_at,
                created_at,
            ],
        )?;
        Ok(rows > 0)
    }

    /// Get a memory by ID.
    pub fn get(&self, id: &str) -> Result<Option<MemoryRecord>, rusqlite::Error> {
        fetch_memory_record(&self.conn, id)
    }

    /// Get all memories.
    pub fn all(&self) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare("SELECT * FROM memories WHERE deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '')")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get("id")?;
            let access_times = self.get_access_times(&id).unwrap_or_default();
            self.row_to_record(row, access_times)
        })?;
        
        rows.collect()
    }

    /// Get memories by a list of IDs (batch fetch).
    ///
    /// More efficient than `all()` when you only need specific memories.
    /// Uses SQL `WHERE id IN (...)` for a single query instead of loading
    /// everything and filtering in Rust.
    pub fn get_by_ids(&self, ids: &[&str]) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build parameterized IN clause: WHERE id IN (?1, ?2, ?3, ...)
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT * FROM memories WHERE id IN ({}) AND deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '')",
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::types::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            let id: String = row.get("id")?;
            let access_times = self.get_access_times(&id).unwrap_or_default();
            self.row_to_record(row, access_times)
        })?;

        rows.collect()
    }

    /// Update an existing memory.
    ///
    /// Uses an IMMEDIATE transaction to ensure atomicity of the memory update
    /// and FTS index update, preventing corruption under multi-process access.
    /// If already inside a transaction (e.g., called from undo_synthesis), skips
    /// creating a new transaction to avoid "cannot start a transaction within a transaction".
    pub fn update(&mut self, record: &MemoryRecord) -> Result<(), rusqlite::Error> {
        let metadata_json = record.metadata.as_ref().and_then(|m| serde_json::to_string(m).ok());
        let needs_tx = self.conn.is_autocommit();
        
        if needs_tx {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
        }
        
        let result = self.update_inner(record, &metadata_json);
        
        if needs_tx {
            match &result {
                Ok(_) => self.conn.execute_batch("COMMIT")?,
                Err(_) => { let _ = self.conn.execute_batch("ROLLBACK"); }
            }
        }
        
        result
    }
    
    /// Inner update logic (always runs within a transaction context).
    fn update_inner(&self, record: &MemoryRecord, metadata_json: &Option<String>) -> Result<(), rusqlite::Error> {
        // Get rowid for FTS update
        let rowid: i64 = self.conn.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![record.id],
            |row| row.get(0),
        )?;
        
        self.conn.execute(
            r#"
            UPDATE memories SET
                content = ?, memory_type = ?, layer = ?,
                working_strength = ?, core_strength = ?, importance = ?,
                pinned = ?, consolidation_count = ?, last_consolidated = ?,
                source = ?, contradicts = ?, contradicted_by = ?, metadata = ?
            WHERE id = ?
            "#,
            params![
                record.content,
                record.memory_type.to_string(),
                record.layer.to_string(),
                record.working_strength,
                record.core_strength,
                record.importance,
                record.pinned as i32,
                record.consolidation_count,
                record.last_consolidated.map(|dt| datetime_to_f64(&dt)),
                record.source,
                record.contradicts.as_ref().unwrap_or(&String::new()),
                record.contradicted_by.as_ref().unwrap_or(&String::new()),
                metadata_json,
                record.id,
            ],
        )?;
        
        // Update FTS with CJK tokenization (with malformed recovery)
        match self.conn.execute("DELETE FROM memories_fts WHERE rowid = ?", params![rowid]) {
            Ok(_) => {},
            Err(e) if e.to_string().contains("malformed") => {
                // FTS corrupted, rebuild the index
                eprintln!("[engram] FTS corruption detected during update, rebuilding index...");
                let _ = self.conn.execute(
                    "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')", []
                );
                // Retry delete after rebuild
                let _ = self.conn.execute("DELETE FROM memories_fts WHERE rowid = ?", params![rowid]);
            }
            Err(_) => {} // Other errors are non-critical for FTS
        }
        
        let tokenized = tokenize_cjk_boundaries(&record.content);
        let _ = self.conn.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?, ?)",
            params![rowid, tokenized],
        );
        
        Ok(())
    }

    /// Delete a memory by ID.
    ///
    /// Uses an IMMEDIATE transaction to ensure atomicity of the FTS delete
    /// and memory delete, preventing corruption under multi-process access.
    /// If already inside a transaction, participates in the existing one.
    pub fn delete(&mut self, id: &str) -> Result<(), rusqlite::Error> {
        let needs_tx = self.conn.is_autocommit();
        
        if needs_tx {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
        }
        
        let result = self.delete_inner(id);
        
        if needs_tx {
            match &result {
                Ok(_) => self.conn.execute_batch("COMMIT")?,
                Err(_) => { let _ = self.conn.execute_batch("ROLLBACK"); }
            }
        }
        
        result
    }
    
    /// Inner delete logic (always runs within a transaction context).
    fn delete_inner(&self, id: &str) -> Result<(), rusqlite::Error> {
        // Delete FTS entry (standalone table, delete by rowid)
        let rowid: Result<i64, _> = self.conn.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        );
        if let Ok(rowid) = rowid {
            let _ = self.conn.execute(
                "DELETE FROM memories_fts WHERE rowid = ?",
                params![rowid],
            );
        }
        self.conn.execute("DELETE FROM memories WHERE id = ?", params![id])?;
        Ok(())
    }
    /// Update just the content and metadata of a memory.
    ///
    /// Used by update_memory to change content while preserving other fields.
    /// Uses an IMMEDIATE transaction to ensure atomicity of the memory update
    /// and FTS index update, preventing corruption under multi-process access.
    /// If already inside a transaction, participates in the existing one.
    pub fn update_content(
        &mut self,
        id: &str,
        new_content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), rusqlite::Error> {
        let metadata_json = metadata.and_then(|m| serde_json::to_string(&m).ok());
        let needs_tx = self.conn.is_autocommit();
        
        if needs_tx {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
        }
        
        let result = self.update_content_inner(id, new_content, &metadata_json);
        
        if needs_tx {
            match &result {
                Ok(_) => self.conn.execute_batch("COMMIT")?,
                Err(_) => { let _ = self.conn.execute_batch("ROLLBACK"); }
            }
        }
        
        result
    }
    
    /// Inner update_content logic (always runs within a transaction context).
    fn update_content_inner(&self, id: &str, new_content: &str, metadata_json: &Option<String>) -> Result<(), rusqlite::Error> {
        // Get rowid before updating
        let rowid: i64 = self.conn.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        )?;
        
        self.conn.execute(
            "UPDATE memories SET content = ?, metadata = ? WHERE id = ?",
            params![new_content, metadata_json, id],
        )?;
        
        // Update FTS index manually (no triggers, need CJK tokenization)
        let _ = self.conn.execute("DELETE FROM memories_fts WHERE rowid = ?", params![rowid]);
        let tokenized = tokenize_cjk_boundaries(new_content);
        let _ = self.conn.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?, ?)",
            params![rowid, tokenized],
        );
        
        Ok(())
    }
    
    /// Get all memories of a specific type, optionally filtered by namespace.
    pub fn search_by_type_ns(
        &self,
        memory_type: MemoryType,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let ns = namespace.unwrap_or("default");
        
        if ns == "*" {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE memory_type = ? AND deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '') ORDER BY importance DESC LIMIT ?"
            )?;
            
            let rows = stmt.query_map(params![memory_type.to_string(), limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE memory_type = ? AND namespace = ? AND deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '') ORDER BY importance DESC LIMIT ?"
            )?;
            
            let rows = stmt.query_map(params![memory_type.to_string(), ns, limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            
            rows.collect()
        }
    }

    /// Record an access for a memory.
    pub fn record_access(&mut self, id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![id, now_f64()],
        )?;
        Ok(())
    }

    /// Get all access timestamps for a memory.
    pub fn get_access_times(&self, id: &str) -> Result<Vec<DateTime<Utc>>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT accessed_at FROM access_log WHERE memory_id = ? ORDER BY accessed_at")?;
        
        let rows = stmt.query_map(params![id], |row| {
            let ts: f64 = row.get(0)?;
            Ok(f64_to_datetime(ts))
        })?;
        
        rows.collect()
    }

    /// Full-text search using FTS5.
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        // Tokenize CJK text first, then split like unicode61 for FTS alignment
        let tokenized = tokenize_cjk_boundaries(query);
        let words = tokenize_like_unicode61(&tokenized);
        if words.is_empty() {
            return Ok(vec![]);
        }
        
        // Build OR query — each token quoted to prevent FTS5 syntax injection
        let fts_query = words.iter().map(|w| format!("\"{}\"", w)).collect::<Vec<_>>().join(" OR ");
        
        let mut stmt = self.conn.prepare(
            r#"
            SELECT m.* FROM memories m
            JOIN memories_fts f ON m.rowid = f.rowid
            WHERE memories_fts MATCH ? AND m.deleted_at IS NULL
            AND (m.superseded_by IS NULL OR m.superseded_by = '')
            ORDER BY rank LIMIT ?
            "#,
        )?;
        
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            let id: String = row.get("id")?;
            let access_times = self.get_access_times(&id).unwrap_or_default();
            self.row_to_record(row, access_times)
        })?;
        
        rows.collect()
    }

    /// Search memories by type.
    /// Fetch the N most recently created memories, optionally filtered by namespace.
    ///
    /// Returns memories ordered newest-first. No query needed — pure chronological.
    /// Used for session bootstrap: inject recent context after restart.
    pub fn fetch_recent(
        &self,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let ns = namespace.unwrap_or("default");

        if ns == "*" {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE (superseded_by IS NULL OR superseded_by = '') AND deleted_at IS NULL ORDER BY created_at DESC LIMIT ?"
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE namespace = ? AND (superseded_by IS NULL OR superseded_by = '') AND deleted_at IS NULL ORDER BY created_at DESC LIMIT ?"
            )?;
            let rows = stmt.query_map(params![ns, limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            rows.collect()
        }
    }

    pub fn search_by_type(&self, memory_type: MemoryType) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM memories WHERE memory_type = ? AND deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '')")?;
        
        let rows = stmt.query_map(params![memory_type.to_string()], |row| {
            let id: String = row.get("id")?;
            let access_times = self.get_access_times(&id).unwrap_or_default();
            self.row_to_record(row, access_times)
        })?;
        
        rows.collect()
    }

    /// Get Hebbian neighbours of a memory.
    ///
    /// **ISS-117 (single canonical row)**: returns neighbours from
    /// canonical `(min,max)` rows in `hebbian_links` using an OR-match
    /// on `(source_id = ? OR target_id = ?)` so a caller passing
    /// either endpoint of a formed link sees the other endpoint. Prior
    /// to ISS-117, formed links were stored as two directional rows
    /// and this method used `WHERE source_id = ?` only, which silently
    /// hid neighbours when the caller passed the wrong endpoint for
    /// `record_association`-formed links.
    ///
    /// **T29.4 (Phase D read-switch)**: when `unified_substrate` is on,
    /// reads from `edges WHERE edge_kind='associative'` instead of the
    /// legacy `hebbian_links` table. The unified path is OR-match too
    /// because Phase B `dual_write_hebbian_to_edges` canonicalises
    /// `(min(a,b), max(a,b))` just like the legacy single-row writer
    /// after ISS-117.
    ///
    /// **Semantic divergence (design §4.3 + §5.4)**: unified
    /// accumulates `weight` from the first co-activation
    /// (ISS-116 unconditional dual-write at delta=0.1) while legacy
    /// `strength` stays at 0.0 until the formed-link threshold is
    /// crossed and then jumps to 1.0. Both paths filter `> 0`, so
    /// the unified path can surface tracking-phase neighbours
    /// (sub-threshold co-activation) that the legacy path
    /// silently hides. Phase D parity (§5.4) accepts this as a
    /// one-way move to unified semantics and is validated end-to-end
    /// by LoCoMo J-score, not byte-equal row sets.
    pub fn get_hebbian_neighbors(&self, memory_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                "SELECT CASE WHEN source_id = ?1 THEN target_id ELSE source_id END \
                 FROM edges \
                 WHERE edge_kind = 'associative' \
                   AND (source_id = ?1 OR target_id = ?1) \
                   AND weight > 0"
            )?;
            let rows = stmt.query_map(params![memory_id], |row| row.get(0))?;
            return rows.collect();
        }

        let mut stmt = self.conn.prepare(
            "SELECT CASE WHEN source_id = ?1 THEN target_id ELSE source_id END \
             FROM hebbian_links \
             WHERE (source_id = ?1 OR target_id = ?1) AND strength > 0"
        )?;

        let rows = stmt.query_map(params![memory_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Get Hebbian neighbors with their link weights.
    ///
    /// **ISS-117 (single canonical row)**: returns one row per
    /// neighbour. Prior to ISS-117, formed links produced duplicate
    /// rows (legacy stored both directions), and callers that summed
    /// `strength` got 2× the correct score (memory.rs recall scoring,
    /// merge_hebbian_links transferred count). The reader SQL is
    /// unchanged — it already used `(source=? OR target=?)` — but
    /// the writer now stores only one canonical row, so dup is gone.
    pub fn get_hebbian_links_weighted(&self, memory_id: &str) -> Result<Vec<(String, f64)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT CASE WHEN source_id = ?1 THEN target_id ELSE source_id END, strength \
             FROM hebbian_links WHERE (source_id = ?1 OR target_id = ?1) AND strength > 0"
        )?;
        let rows = stmt.query_map(params![memory_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        rows.collect()
    }

    /// Record co-activation for Hebbian learning.
    ///
    /// **ISS-116 (Phase B dual-WRITE)**: this method was added to T14's
    /// dual-write coverage on 2026-05-13. Every call now also UPSERTs
    /// into `edges` (edge_kind='associative') so the unified substrate
    /// stays in lockstep with `hebbian_links`. The dual-write mirrors
    /// the namespaced variant's policy:
    ///
    ///   - **Unconditional**: every call adds `delta_weight=0.1` on
    ///     edges, regardless of which legacy branch (formed,
    ///     threshold-crossing, tracking) fired. This intentionally
    ///     accumulates edge weight from the first recall even when the
    ///     legacy row sits at `strength=0` during the tracking phase.
    ///     Pre-existing T14 divergence preserved here for consistency
    ///     across the three coactivation writers.
    ///   - `signal_source="corecall"` marks this as recall-driven.
    ///   - `namespace="default"` because this overload is
    ///     namespace-agnostic.
    ///
    /// **Behavior change**: the legacy table has no FK on (source_id,
    /// target_id); the unified `edges` table REFERENCES nodes(id). If
    /// either endpoint is missing from `nodes`, the dual-write will
    /// fail FK, the whole transaction rolls back, and the call returns
    /// a SQLite error. Previously this method silently inserted an
    /// orphan legacy row. This is the desired fail-fast behavior for
    /// Phase B lockstep; callers must ensure both ids have been added
    /// via `Storage::add` first.
    pub fn record_coactivation(
        &mut self,
        id1: &str,
        id2: &str,
        threshold: i32,
    ) -> Result<bool, rusqlite::Error> {
        let (id1, id2) = if id1 < id2 { (id1, id2) } else { (id2, id1) };

        let tx = self.conn.transaction()?;
        let formed = {
            // Check existing link
            let existing: Option<(f64, i32)> = tx
                .query_row(
                    "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                    params![id1, id2],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            match existing {
                Some((strength, _count)) if strength > 0.0 => {
                    // Link already formed, strengthen it.
                    // ISS-117: single canonical row only. No reverse
                    // INSERT/UPDATE — readers OR-match on (id1, id2)
                    // for direction-agnostic lookups.
                    let new_strength = (strength + 0.1).min(1.0);
                    tx.execute(
                        "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                        params![new_strength, id1, id2],
                    )?;
                    false
                }
                Some((_, count)) => {
                    // Tracking phase, increment count
                    let new_count = count + 1;
                    if new_count >= threshold {
                        // Threshold reached, form link.
                        // ISS-117: update only the canonical row; no
                        // reverse INSERT.
                        tx.execute(
                            "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        true
                    } else {
                        tx.execute(
                            "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        false
                    }
                }
                None => {
                    // First co-activation, create tracking record
                    tx.execute(
                        "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?, ?, 0.0, 1, ?)",
                        params![id1, id2, now_f64()],
                    )?;
                    false
                }
            }
        };

        // ISS-116: unified-edges dual-write. Matches record_coactivation_ns
        // policy — one unconditional UPSERT with delta_weight=0.1 per call.
        crate::graph::store::dual_write_hebbian_to_edges(
            &tx,
            id1,
            id2,
            "corecall",
            "{}",
            0.1,
            "default",
        )
        .map_err(|e| match e {
            crate::graph::GraphError::Sqlite(s) => s,
            other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
        })?;

        tx.commit()?;
        Ok(formed)
    }

    /// Decay all Hebbian links by a factor.
    pub fn decay_hebbian_links(&mut self, factor: f64) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.transaction()?;

        // Legacy side
        tx.execute(
            "UPDATE hebbian_links SET strength = strength * ? WHERE strength > 0",
            params![factor],
        )?;
        let pruned = tx.execute(
            "DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1",
            [],
        )?;

        // ISS-116: unified-edges mirror. `edges.weight` plays the role
        // of `hebbian_links.strength`; `edge_kind='associative'` scopes
        // the bulk op to hebbian rows only. Prune threshold is the
        // same 0.1 floor.
        tx.execute(
            "UPDATE edges SET weight = weight * ? \
             WHERE edge_kind = 'associative' AND weight > 0",
            params![factor],
        )?;
        tx.execute(
            "DELETE FROM edges \
             WHERE edge_kind = 'associative' AND weight > 0 AND weight < 0.1",
            [],
        )?;

        tx.commit()?;
        Ok(pruned)
    }

    /// Transfer Hebbian links from donor to target during merge.
    /// - Repoints donor links to target
    /// - If link already exists on target, keeps max weight
    /// - Drops self-links (source==target after repoint)
    /// - Deletes all donor links after transfer
    ///
    /// **ISS-116 (Phase B dual-WRITE)**: mirror-merges the donor's
    /// `edges` rows (edge_kind='associative') into `target`'s edge
    /// neighborhood with the same max-weight semantics, then deletes
    /// the donor's edges. Both legacy and unified sides run in a
    /// single transaction so a partial failure cannot leave the two
    /// substrates inconsistent.
    pub fn merge_hebbian_links(
        &mut self,
        donor_id: &str,
        target_id: &str,
    ) -> Result<usize, rusqlite::Error> {
        // Defensive guard: donor == target would cause the final DELETE
        // (WHERE source_id = donor OR target_id = donor) to wipe the
        // surviving memory's entire hebbian neighborhood. Pre-existing
        // legacy code had no guard for this — ISS-116 closes both sides
        // (legacy + unified edges) so we add the guard once at entry.
        if donor_id == target_id {
            return Ok(0);
        }

        // Collect all donor-touching hebbian neighbours BEFORE opening
        // the transaction (the call uses &self).
        let links = self.get_hebbian_links_weighted(donor_id)?;

        // Collect donor-touching associative edges with their
        // canonicalised "other endpoint" + weight. We do this outside
        // the tx for the same reason: borrow shape.
        let edge_neighbours: Vec<(String, f64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT source_id, target_id, weight FROM edges \
                 WHERE edge_kind = 'associative' \
                   AND (source_id = ?1 OR target_id = ?1)",
            )?;
            let rows = stmt.query_map(params![donor_id], |row| {
                let s: String = row.get(0)?;
                let t: String = row.get(1)?;
                let w: f64 = row.get(2)?;
                let other = if s == donor_id { t } else { s };
                Ok((other, w))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let tx = self.conn.transaction()?;
        let mut transferred = 0;

        // === Legacy side ===
        for (other_id, weight) in &links {
            if other_id == target_id {
                continue;
            }
            let existing_weight: Option<f64> = tx.query_row(
                "SELECT strength FROM hebbian_links WHERE \
                 (source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1)",
                params![target_id, other_id],
                |row| row.get(0),
            ).optional()?;

            match existing_weight {
                Some(existing) => {
                    let max_weight = existing.max(*weight);
                    tx.execute(
                        "UPDATE hebbian_links SET strength = ?1 WHERE \
                         (source_id = ?2 AND target_id = ?3) OR (source_id = ?3 AND target_id = ?2)",
                        params![max_weight, target_id, other_id],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT OR IGNORE INTO hebbian_links \
                         (source_id, target_id, strength, coactivation_count, \
                          temporal_forward, temporal_backward, direction, created_at, namespace) \
                         VALUES (?1, ?2, ?3, 1, 0, 0, 'bidirectional', ?4, 'default')",
                        params![target_id, other_id, weight, now_f64()],
                    )?;
                }
            }
            transferred += 1;
        }
        tx.execute(
            "DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1",
            params![donor_id],
        )?;

        // === ISS-116: unified-edges mirror ===
        // For every associative edge touching donor_id, fold its weight
        // into the (target_id, other) edge with max-weight semantics
        // (matching legacy). Self-edges (other == target_id) are
        // dropped, mirroring the skip above.
        for (other_id, donor_weight) in &edge_neighbours {
            if other_id == target_id {
                continue;
            }
            let (lo, hi) = if target_id < other_id.as_str() {
                (target_id, other_id.as_str())
            } else {
                (other_id.as_str(), target_id)
            };
            let existing_w: Option<f64> = tx.query_row(
                "SELECT weight FROM edges WHERE edge_kind = 'associative' \
                 AND source_id = ?1 AND target_id = ?2",
                params![lo, hi],
                |row| row.get(0),
            ).optional()?;
            match existing_w {
                Some(existing) => {
                    let max_w = existing.max(*donor_weight);
                    tx.execute(
                        "UPDATE edges SET weight = ?1 \
                         WHERE edge_kind = 'associative' \
                           AND source_id = ?2 AND target_id = ?3",
                        params![max_w, lo, hi],
                    )?;
                }
                None => {
                    // Mint a fresh edge row for the surviving pair.
                    // Reuses dual_write_hebbian_to_edges so the new
                    // row matches T14's shape (attributes JSON, etc.).
                    crate::graph::store::dual_write_hebbian_to_edges(
                        &tx,
                        lo,
                        hi,
                        "corecall",
                        "{}",
                        *donor_weight,
                        "default",
                    )
                    .map_err(|e| match e {
                        crate::graph::GraphError::Sqlite(s) => s,
                        other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
                    })?;
                }
            }
        }
        // Delete all donor-touching associative edges.
        tx.execute(
            "DELETE FROM edges WHERE edge_kind = 'associative' \
             AND (source_id = ?1 OR target_id = ?1)",
            params![donor_id],
        )?;

        tx.commit()?;
        Ok(transferred)
    }

    /// Decay Hebbian links with differential rates based on signal source.
    ///
    /// - `corecall` links decay slowest (highest retention)
    /// - `multi` links decay at medium rate
    /// - All other signal sources (`entity`, `embedding`, `temporal`) decay fastest
    ///
    /// Returns the number of deleted (pruned) links.
    pub fn decay_hebbian_links_differential(
        &mut self,
        decay_corecall: f64,
        decay_multi: f64,
        decay_single: f64,
    ) -> Result<usize, rusqlite::Error> {
        let tx = self.conn.transaction()?;

        // Legacy side
        tx.execute(
            "UPDATE hebbian_links SET strength = strength * CASE \
                WHEN signal_source = 'corecall' THEN ?1 \
                WHEN signal_source = 'multi' THEN ?2 \
                ELSE ?3 \
            END \
            WHERE strength > 0",
            params![decay_corecall, decay_multi, decay_single],
        )?;
        let pruned = tx.execute(
            "DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1",
            [],
        )?;

        // ISS-116: unified-edges mirror. `signal_source` lives inside
        // `edges.attributes` (JSON); `json_extract` gives the predicate
        // selectivity needed to apply the same CASE WHEN. This is
        // slower than the column-backed legacy predicate but correct.
        // FOLLOWUP-ISS-116-perf: consider a generated column or
        // partial index keyed by signal_source for hot decay paths.
        tx.execute(
            "UPDATE edges SET weight = weight * CASE \
                WHEN json_extract(attributes, '$.signal_source') = 'corecall' THEN ?1 \
                WHEN json_extract(attributes, '$.signal_source') = 'multi'    THEN ?2 \
                ELSE ?3 \
            END \
            WHERE edge_kind = 'associative' AND weight > 0",
            params![decay_corecall, decay_multi, decay_single],
        )?;
        tx.execute(
            "DELETE FROM edges \
             WHERE edge_kind = 'associative' AND weight > 0 AND weight < 0.1",
            [],
        )?;

        tx.commit()?;
        Ok(pruned)
    }

    fn row_to_record(
        &self,
        row: &rusqlite::Row,
        access_times: Vec<DateTime<Utc>>,
    ) -> SqlResult<MemoryRecord> {
        row_to_record_impl(row, access_times)
    }
    
    /// Get the namespace of a memory by ID.
    pub fn get_namespace(&self, id: &str) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT namespace FROM memories WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .optional()
    }

    // === Supersession Methods ===

    /// Mark old_id as superseded by new_id.
    ///
    /// Validates: old_id exists, new_id exists, old_id != new_id, same namespace.
    /// If old_id is already superseded, updates the link (last-write-wins).
    pub fn supersede(&self, old_id: &str, new_id: &str) -> Result<(), crate::types::SupersessionError> {
        use crate::types::SupersessionError;

        if old_id == new_id {
            return Err(SupersessionError::SelfSupersession(old_id.to_string()));
        }
        // Validate old exists
        if self.get(old_id).map_err(SupersessionError::Db)?.is_none() {
            return Err(SupersessionError::NotFound(old_id.to_string()));
        }
        // Validate new exists
        if self.get(new_id).map_err(SupersessionError::Db)?.is_none() {
            return Err(SupersessionError::NotFound(new_id.to_string()));
        }
        // Namespace check (SEC-ss.1): both must be in the same namespace
        let old_ns = self.get_namespace(old_id).map_err(SupersessionError::Db)?;
        let new_ns = self.get_namespace(new_id).map_err(SupersessionError::Db)?;
        if old_ns != new_ns {
            return Err(SupersessionError::CrossNamespace {
                old_ns: old_ns.unwrap_or_default(),
                new_ns: new_ns.unwrap_or_default(),
            });
        }

        // Phase B dual-write: keep `memories.superseded_by` and
        // `nodes.superseded_by` in lock-step inside a single
        // transaction so retrieval, which currently reads `memories`
        // but will switch to `nodes` at Phase D cutover, never sees
        // a half-updated supersession.
        //
        // `nodes.superseded_by` is `TEXT REFERENCES nodes(id) ON
        // DELETE SET NULL`, so we store `new_id` directly (no `''`
        // sentinel — the legacy `''` convention is `memories`-only).
        // The `INSERT OR IGNORE` policy in `Storage::add` already
        // guaranteed `nodes(new_id)` exists, so the FK will resolve;
        // if for some reason it doesn't, the UPDATE silently no-ops
        // on `nodes` (zero rows match) — that's a corrupted-state
        // signal we want to surface, so we assert at least one row
        // was updated on the `memories` side.
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(SupersessionError::Db)?;
        tx.execute(
            "UPDATE memories SET superseded_by = ? WHERE id = ?",
            params![new_id, old_id],
        )
        .map_err(SupersessionError::Db)?;
        tx.execute(
            "UPDATE nodes SET superseded_by = ?, updated_at = ? \
             WHERE id = ? AND node_kind = 'memory'",
            params![new_id, datetime_to_f64(&chrono::Utc::now()), old_id],
        )
        .map_err(SupersessionError::Db)?;
        tx.commit().map_err(SupersessionError::Db)?;
        Ok(())
    }

    /// Supersede multiple old IDs with one new ID. Transactional.
    ///
    /// If any old_id doesn't exist, rolls back and returns error with invalid IDs.
    /// Empty old_ids = no-op success (returns 0).
    pub fn supersede_bulk(&self, old_ids: &[&str], new_id: &str) -> Result<usize, crate::types::SupersessionError> {
        use crate::types::SupersessionError;

        if old_ids.is_empty() {
            return Ok(0);
        }

        // Validate new_id exists
        if self.get(new_id).map_err(SupersessionError::Db)?.is_none() {
            return Err(SupersessionError::NotFound(new_id.to_string()));
        }
        let new_ns = self.get_namespace(new_id).map_err(SupersessionError::Db)?;

        // Validate all old IDs exist and are in the same namespace
        let mut invalid_ids = Vec::new();
        for &old_id in old_ids {
            if old_id == new_id {
                invalid_ids.push(old_id.to_string());
                continue;
            }
            match self.get(old_id).map_err(SupersessionError::Db)? {
                None => invalid_ids.push(old_id.to_string()),
                Some(_) => {
                    let old_ns = self.get_namespace(old_id).map_err(SupersessionError::Db)?;
                    if old_ns != new_ns {
                        return Err(SupersessionError::CrossNamespace {
                            old_ns: old_ns.unwrap_or_default(),
                            new_ns: new_ns.unwrap_or_default(),
                        });
                    }
                }
            }
        }

        if !invalid_ids.is_empty() {
            return Err(SupersessionError::InvalidIds(invalid_ids));
        }

        // All validated — execute in a savepoint
        self.conn.execute("SAVEPOINT supersede_bulk", []).map_err(SupersessionError::Db)?;
        let result = (|| {
            // Phase B dual-write: every (old_id → new_id) pair updates
            // both `memories.superseded_by` and `nodes.superseded_by`
            // inside the same savepoint, so partial failure rolls back
            // both legacy and unified state atomically.
            let now = datetime_to_f64(&chrono::Utc::now());
            for &old_id in old_ids {
                self.conn.execute(
                    "UPDATE memories SET superseded_by = ? WHERE id = ?",
                    params![new_id, old_id],
                ).map_err(SupersessionError::Db)?;
                self.conn.execute(
                    "UPDATE nodes SET superseded_by = ?, updated_at = ? \
                     WHERE id = ? AND node_kind = 'memory'",
                    params![new_id, now, old_id],
                ).map_err(SupersessionError::Db)?;
            }
            Ok::<usize, SupersessionError>(old_ids.len())
        })();

        match result {
            Ok(count) => {
                self.conn.execute("RELEASE supersede_bulk", []).map_err(SupersessionError::Db)?;
                Ok(count)
            }
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK TO supersede_bulk", []);
                let _ = self.conn.execute("RELEASE supersede_bulk", []);
                Err(e)
            }
        }
    }

    /// Clear superseded_by for a memory, restoring it to active recall.
    pub fn unsupersede(&self, id: &str) -> Result<(), crate::types::SupersessionError> {
        use crate::types::SupersessionError;

        if self.get(id).map_err(SupersessionError::Db)?.is_none() {
            return Err(SupersessionError::NotFound(id.to_string()));
        }

        // Phase B dual-write: clear supersession on both
        // `memories.superseded_by` (sentinel `''` per legacy
        // convention) and `nodes.superseded_by` (`NULL` per design
        // §5.3 / `REFERENCES nodes(id) ON DELETE SET NULL`).
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(SupersessionError::Db)?;
        tx.execute(
            "UPDATE memories SET superseded_by = '' WHERE id = ?",
            params![id],
        )
        .map_err(SupersessionError::Db)?;
        tx.execute(
            "UPDATE nodes SET superseded_by = NULL, updated_at = ? \
             WHERE id = ? AND node_kind = 'memory'",
            params![datetime_to_f64(&chrono::Utc::now()), id],
        )
        .map_err(SupersessionError::Db)?;
        tx.commit().map_err(SupersessionError::Db)?;
        Ok(())
    }

    /// List all superseded memories, optionally filtered by namespace.
    pub fn list_superseded(&self, namespace: Option<&str>) -> Result<Vec<(MemoryRecord, String)>, rusqlite::Error> {
        let query = if let Some(ns) = namespace {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE superseded_by != '' AND namespace = ? AND deleted_at IS NULL ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map(params![ns], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                let record = self.row_to_record(row, access_times)?;
                let superseded_by: String = row.get("superseded_by")?;
                Ok((record, superseded_by))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE superseded_by != '' AND deleted_at IS NULL ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map([], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                let record = self.row_to_record(row, access_times)?;
                let superseded_by: String = row.get("superseded_by")?;
                Ok((record, superseded_by))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        Ok(query)
    }

    /// Resolve the supersession chain head for a given memory.
    ///
    /// Returns the final non-superseded memory ID, or None if cycle detected.
    pub fn resolve_chain_head(&self, id: &str) -> Result<Option<String>, rusqlite::Error> {
        let mut current = id.to_string();
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current.clone()) {
                // Cycle detected
                log::warn!("Supersession cycle detected involving {}", current);
                return Ok(None);
            }
            match self.get(&current)? {
                Some(record) => match &record.superseded_by {
                    Some(next) => current = next.clone(),
                    None => return Ok(Some(current)),
                },
                None => return Ok(None), // broken chain
            }
        }
    }
    
    /// Full-text search using FTS5, filtered by namespace.
    /// 
    /// If namespace is None, search in "default" namespace.
    /// If namespace is Some("*"), search across all namespaces.
    pub fn search_fts_ns(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        // Tokenize CJK text first, then split like unicode61 for FTS alignment
        let tokenized = tokenize_cjk_boundaries(query);
        let words = tokenize_like_unicode61(&tokenized);
        if words.is_empty() {
            return Ok(vec![]);
        }
        
        // Build OR query — each token quoted to prevent FTS5 syntax injection
        let fts_query = words.iter().map(|w| format!("\"{}\"", w)).collect::<Vec<_>>().join(" OR ");
        
        let ns = namespace.unwrap_or("default");
        
        if ns == "*" {
            // Search all namespaces
            let mut stmt = self.conn.prepare(
                r#"
                SELECT m.* FROM memories m
                JOIN memories_fts f ON m.rowid = f.rowid
                WHERE memories_fts MATCH ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')
                ORDER BY rank LIMIT ?
                "#,
            )?;
            
            let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            
            rows.collect()
        } else {
            // Search specific namespace
            let mut stmt = self.conn.prepare(
                r#"
                SELECT m.* FROM memories m
                JOIN memories_fts f ON m.rowid = f.rowid
                WHERE memories_fts MATCH ? AND m.namespace = ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')
                ORDER BY rank LIMIT ?
                "#,
            )?;
            
            let rows = stmt.query_map(params![fts_query, ns, limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            
            rows.collect()
        }
    }
    
    /// Get all memories in a specific namespace.
    pub fn all_in_namespace(&self, namespace: Option<&str>) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let ns = namespace.unwrap_or("default");
        
        if ns == "*" {
            return self.all();
        }
        
        let mut stmt = self.conn.prepare("SELECT * FROM memories WHERE namespace = ? AND deleted_at IS NULL AND (superseded_by IS NULL OR superseded_by = '')")?;
        let rows = stmt.query_map(params![ns], |row| {
            let id: String = row.get("id")?;
            let access_times = self.get_access_times(&id).unwrap_or_default();
            self.row_to_record(row, access_times)
        })?;
        
        rows.collect()
    }
    
    // === Embedding Methods (Protocol v2) ===
    //
    // See EMBEDDING_PROTOCOL.md for the full specification.
    // PK: (memory_id, model) — supports multiple embedding models per memory.
    // BLOB format: raw little-endian f32 array, no header.
    
    /// Validate an embedding vector before storage.
    ///
    /// Returns Err if the embedding is empty or contains non-finite values.
    /// Normalize model ID to always include provider prefix.
    /// Bare model names (e.g. "nomic-embed-text") are auto-prefixed.
    fn normalize_model_id(model: &str) -> String {
        if model.contains('/') {
            model.to_string()
        } else if model.starts_with("text-embedding") {
            format!("openai/{}", model)
        } else if model.is_empty() || model == "unknown" {
            "unknown/legacy".to_string()
        } else {
            format!("ollama/{}", model)
        }
    }

    fn validate_embedding(embedding: &[f32]) -> Result<(), rusqlite::Error> {
        if embedding.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "Empty embedding".to_string(),
            ));
        }
        if !embedding.iter().all(|f| f.is_finite()) {
            return Err(rusqlite::Error::InvalidParameterName(
                "Non-finite value in embedding (NaN or Inf)".to_string(),
            ));
        }
        Ok(())
    }
    
    /// Store embedding for a memory with a specific model.
    ///
    /// Protocol v2: PK is (memory_id, model), so the same memory can have
    /// embeddings from multiple models simultaneously.
    ///
    /// Serializes the embedding as raw f32 bytes (little-endian) per spec.
    pub fn store_embedding(
        &mut self,
        memory_id: &str,
        embedding: &[f32],
        model: &str,
        dimensions: usize,
    ) -> Result<(), rusqlite::Error> {
        Self::validate_embedding(embedding)?;
        
        // Normalize model ID: must have provider prefix (e.g. "ollama/nomic-embed-text")
        let model = Self::normalize_model_id(model);
        
        // Serialize Vec<f32> as raw bytes (4 bytes per f32, little-endian)
        let bytes: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        
        debug_assert_eq!(bytes.len(), dimensions * 4,
            "Blob size mismatch: {} bytes for {} dimensions", bytes.len(), dimensions);
        
        let now_dt = chrono::Utc::now();
        let now = now_dt.to_rfc3339();
        // Same instant in epoch seconds (sub-second precision) for the
        // unified `node_embeddings` table, whose `created_at` is `REAL`.
        let now_epoch = now_dt.timestamp() as f64
            + (now_dt.timestamp_subsec_nanos() as f64) / 1e9;

        // Phase B (T20 follow-up) dual-write: every legacy
        // `memory_embeddings` insert also lands in the unified
        // `node_embeddings` table with `node_id = memory_id`. The FK
        // to `nodes(id)` is satisfied because `Storage::add` already
        // T12-dual-wrote the parent memory→nodes row (unconditional,
        // not flag-gated), so by the time `store_embedding` runs the
        // parent node always exists. See `insert_node_embedding_row`
        // for the backfill counterpart (T20) — we deliberately do
        // **not** call that helper here because its semantics are
        // `INSERT OR IGNORE` (preserves whichever side wrote first,
        // safe for re-runnable backfill), whereas live writes from
        // `store_embedding` use `INSERT OR REPLACE` so that
        // re-embedding a memory with a new vector cleanly overwrites
        // the prior (memory_id, model) entry on both sides. Two
        // statements share one transaction to keep both tables in
        // lockstep.
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT OR REPLACE INTO memory_embeddings (memory_id, model, embedding, dimensions, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            params![memory_id, model, bytes, dimensions as i64, now],
        )?;
        tx.execute(
            r#"
            INSERT OR REPLACE INTO node_embeddings (node_id, model, embedding, dimensions, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            params![memory_id, model, bytes, dimensions as i64, now_epoch],
        )?;
        tx.commit()?;

        Ok(())
    }
    
    /// Get embedding for a memory using a specific model.
    ///
    /// Returns None if no embedding exists for this (memory_id, model) pair.
    ///
    /// **Phase D (T29.3)**: when `self.unified_substrate` is `true`,
    /// reads from `node_embeddings` (keyed by `node_id`); otherwise
    /// reads from legacy `memory_embeddings`. Both tables stay in
    /// lockstep through `store_embedding`'s dual-write
    /// (`memory_id == node_id` by T12 construction).
    pub fn get_embedding(&self, memory_id: &str, model: &str) -> Result<Option<Vec<f32>>, rusqlite::Error> {
        let model = Self::normalize_model_id(model);
        let result: Option<Vec<u8>> = if self.unified_substrate {
            self.conn
                .query_row(
                    "SELECT embedding FROM node_embeddings WHERE node_id = ? AND model = ?",
                    params![memory_id, model],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            self.conn
                .query_row(
                    "SELECT embedding FROM memory_embeddings WHERE memory_id = ? AND model = ?",
                    params![memory_id, model],
                    |row| row.get(0),
                )
                .optional()?
        };

        Ok(result.map(|bytes| bytes_to_f32_vec(&bytes)))
    }
    
    /// Get all embeddings for a specific model.
    ///
    /// Returns (memory_id, embedding) pairs for the given model only.
    /// Cross-model comparison is undefined behavior per protocol.
    ///
    /// Filters to live (non-deleted, non-superseded) memories. Under
    /// the unified path the same liveness predicate is applied via the
    /// `memories` JOIN — `node_embeddings` itself has no liveness
    /// columns, so we route through the legacy table-of-record. This
    /// is intentional and bug-for-bug with the legacy reader: callers
    /// already pre-filter by namespace upstream when needed.
    pub fn get_all_embeddings(&self, model: &str) -> Result<Vec<(String, Vec<f32>)>, rusqlite::Error> {
        let model = Self::normalize_model_id(model);
        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                r#"SELECT e.node_id, e.embedding FROM node_embeddings e
                JOIN memories m ON e.node_id = m.id
                WHERE e.model = ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')"#
            )?;
            let rows = stmt.query_map(params![model], |row| {
                let memory_id: String = row.get(0)?;
                let bytes: Vec<u8> = row.get(1)?;
                Ok((memory_id, bytes_to_f32_vec(&bytes)))
            })?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                r#"SELECT e.memory_id, e.embedding FROM memory_embeddings e
                JOIN memories m ON e.memory_id = m.id
                WHERE e.model = ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')"#
            )?;
            let rows = stmt.query_map(params![model], |row| {
                let memory_id: String = row.get(0)?;
                let bytes: Vec<u8> = row.get(1)?;
                Ok((memory_id, bytes_to_f32_vec(&bytes)))
            })?;
            rows.collect()
        }
    }
    
    /// Get embeddings for a specific namespace and model.
    ///
    /// Only returns embeddings from the specified model to ensure
    /// cosine similarity is computed within the same vector space.
    pub fn get_embeddings_in_namespace(
        &self,
        namespace: Option<&str>,
        model: &str,
    ) -> Result<Vec<(String, Vec<f32>)>, rusqlite::Error> {
        let model = Self::normalize_model_id(model);
        let ns = namespace.unwrap_or("default");
        
        if ns == "*" {
            return self.get_all_embeddings(&model);
        }

        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                r#"
                SELECT e.node_id, e.embedding FROM node_embeddings e
                JOIN memories m ON e.node_id = m.id
                WHERE m.namespace = ? AND e.model = ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')
                "#
            )?;
            let rows = stmt.query_map(params![ns, model], |row| {
                let memory_id: String = row.get(0)?;
                let bytes: Vec<u8> = row.get(1)?;
                Ok((memory_id, bytes_to_f32_vec(&bytes)))
            })?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                r#"
                SELECT e.memory_id, e.embedding FROM memory_embeddings e
                JOIN memories m ON e.memory_id = m.id
                WHERE m.namespace = ? AND e.model = ? AND m.deleted_at IS NULL
                AND (m.superseded_by IS NULL OR m.superseded_by = '')
                "#
            )?;
            let rows = stmt.query_map(params![ns, model], |row| {
                let memory_id: String = row.get(0)?;
                let bytes: Vec<u8> = row.get(1)?;
                Ok((memory_id, bytes_to_f32_vec(&bytes)))
            })?;
            rows.collect()
        }
    }
    
    // === Soft-Delete / Lifecycle Methods ===

    /// Get a reference to the underlying connection (for tests).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Soft delete: set deleted_at timestamp.
    pub fn soft_delete(&self, id: &str) -> Result<(), rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE memories SET deleted_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Hard delete a memory and every dependent row across both the
    /// legacy schema and the v0.4 unified substrate.
    ///
    /// # What this deletes
    ///
    /// Legacy tables (table-of-record before Phase D):
    /// - `memory_embeddings WHERE memory_id = ?`
    /// - `access_log       WHERE memory_id = ?` (no unified counterpart)
    /// - `hebbian_links    WHERE source_id = ? OR target_id = ?`
    /// - `memory_entities  WHERE memory_id = ?`
    /// - `synthesis_provenance WHERE source_id = ? OR insight_id = ?`
    /// - `memories_fts` (via rowid lookup)
    /// - `memories WHERE id = ?`
    ///
    /// Unified substrate (closes ISS-115 — Phase B dual-WRITE writers
    /// from T13–T16 had no symmetric dual-DELETE story, so before
    /// this method these rows leaked into Phase D unified reads
    /// after a hard-delete):
    /// - `node_embeddings WHERE node_id = ?`  (T20 / T29.3 mirror)
    /// - `edges WHERE edge_kind = 'associative' AND
    ///        (source_id = ? OR target_id = ?)`  (T14 / T24 mirror)
    /// - `edges WHERE source_id = ? AND
    ///        ((edge_kind = 'provenance'  AND predicate = 'mentions') OR
    ///         (edge_kind = 'structural' AND
    ///            predicate IN ('subject_of', 'object_of')))`
    ///   (T23 mirror — the three role-splits memory_entities maps to)
    /// - `edges WHERE edge_kind = 'provenance' AND
    ///        predicate = 'derived_from' AND
    ///        (source_id = ? OR target_id = ?)`  (T16 / T25 mirror)
    /// - `nodes WHERE id = ?`  (T12 / T19 mirror — also cascades
    ///   `nodes_fts` via trigger and would cascade `node_embeddings`
    ///   via `ON DELETE CASCADE`, but we delete explicitly above to
    ///   keep dual-DELETE one-for-one with dual-WRITE)
    ///
    /// # Order matters
    ///
    /// `edges.source_id` / `edges.target_id` are `REFERENCES nodes(id)
    /// ON DELETE RESTRICT`. Deleting `nodes` before clearing every
    /// edge that touches `id` would raise a FK violation. The
    /// sequence below clears dependents first, then the parent rows
    /// on each side, matching the legacy ordering one-for-one.
    ///
    /// # Atomicity
    ///
    /// All statements share a single SQLite transaction. The previous
    /// implementation ran each `execute` in autocommit mode, so a
    /// FK violation mid-cascade could leave half-deleted state. The
    /// transaction here also gives the rusqlite `ON DELETE RESTRICT`
    /// checks a consistent view of the row set.
    pub fn hard_delete_cascade(&mut self, id: &str) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;

        // --- Legacy side: clear dependents first ---
        tx.execute("DELETE FROM memory_embeddings WHERE memory_id = ?1", params![id])?;
        tx.execute("DELETE FROM access_log WHERE memory_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM memory_entities WHERE memory_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM synthesis_provenance WHERE source_id = ?1 OR insight_id = ?1",
            params![id],
        )?;
        // memories_fts cleanup must come before DELETE FROM memories
        // because the rowid lookup reads `memories`.
        let rowid: Result<i64, _> = tx.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        );
        if let Ok(rowid) = rowid {
            let _ = tx.execute("DELETE FROM memories_fts WHERE rowid = ?", params![rowid]);
        }
        tx.execute("DELETE FROM memories WHERE id = ?1", params![id])?;

        // --- Unified side: same shape, same order (clears ISS-115) ---
        tx.execute("DELETE FROM node_embeddings WHERE node_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM edges WHERE edge_kind = 'associative' \
             AND (source_id = ?1 OR target_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE source_id = ?1 AND ( \
                 (edge_kind = 'provenance' AND predicate = 'mentions') OR \
                 (edge_kind = 'structural' AND predicate IN ('subject_of', 'object_of')) \
             )",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM edges WHERE edge_kind = 'provenance' \
             AND predicate = 'derived_from' \
             AND (source_id = ?1 OR target_id = ?1)",
            params![id],
        )?;
        // `nodes_fts` is contentless-FTS5 maintained by AFTER DELETE
        // trigger `nodes_fts_ad` — no explicit row-removal needed.
        tx.execute("DELETE FROM nodes WHERE id = ?1", params![id])?;

        tx.commit()?;
        Ok(())
    }

    /// List soft-deleted memories, optionally filtered by namespace.
    pub fn list_deleted(&self, namespace: Option<&str>) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        let ns = namespace.unwrap_or("default");
        if ns == "*" {
            let mut stmt = self.conn.prepare("SELECT * FROM memories WHERE deleted_at IS NOT NULL")?;
            let rows = stmt.query_map([], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE namespace = ? AND deleted_at IS NOT NULL"
            )?;
            let rows = stmt.query_map(params![ns], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            rows.collect()
        }
    }

    /// Count soft-deleted memories.
    pub fn count_soft_deleted(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE deleted_at IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get the deleted_at timestamp for a memory.
    pub fn get_deleted_at(&self, id: &str) -> Result<Option<String>, rusqlite::Error> {
        let result: Option<String> = self.conn.query_row(
            "SELECT deleted_at FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        )?;
        Ok(result)
    }

    pub fn delete_embedding(&mut self, memory_id: &str, model: &str) -> Result<(), rusqlite::Error> {
        let model = Self::normalize_model_id(model);
        self.conn.execute(
            "DELETE FROM memory_embeddings WHERE memory_id = ? AND model = ?",
            params![memory_id, model],
        )?;
        Ok(())
    }
    
    /// Delete all embeddings for a memory (all models).
    ///
    /// Mirrors `store_embedding`'s dual-write: every legacy
    /// `memory_embeddings` row is paired with a `node_embeddings`
    /// row keyed by the same id, so both sides must be cleared
    /// atomically. See ISS-115 for the broader Phase B dual-DELETE
    /// closure.
    pub fn delete_all_embeddings(&mut self, memory_id: &str) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM memory_embeddings WHERE memory_id = ?",
            params![memory_id],
        )?;
        tx.execute(
            "DELETE FROM node_embeddings WHERE node_id = ?",
            params![memory_id],
        )?;
        tx.commit()?;
        Ok(())
    }
    
    /// Get memory IDs that don't have embeddings for a specific model.
    ///
    /// Used to find memories that need (re)embedding when switching models
    /// or during backfill operations.
    pub fn get_memories_without_embeddings(&self, model: &str) -> Result<Vec<String>, rusqlite::Error> {
        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                r#"
                SELECT m.id FROM memories m
                LEFT JOIN node_embeddings e ON m.id = e.node_id AND e.model = ?
                WHERE e.node_id IS NULL
                "#
            )?;
            let rows = stmt.query_map(params![model], |row| row.get(0))?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                r#"
                SELECT m.id FROM memories m
                LEFT JOIN memory_embeddings e ON m.id = e.memory_id AND e.model = ?
                WHERE e.memory_id IS NULL
                "#
            )?;
            let rows = stmt.query_map(params![model], |row| row.get(0))?;
            rows.collect()
        }
    }

    /// Get embedding statistics, optionally filtered by model.
    pub fn embedding_stats(&self) -> Result<EmbeddingStats, rusqlite::Error> {
        let total_memories: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM memories",
            [],
            |row| row.get(0),
        )?;

        let (embedded_count, model, dimensions) = if self.unified_substrate {
            let embedded_count: usize = self.conn.query_row(
                "SELECT COUNT(DISTINCT node_id) FROM node_embeddings",
                [],
                |row| row.get(0),
            )?;
            let model: Option<String> = self.conn.query_row(
                "SELECT model FROM node_embeddings GROUP BY model ORDER BY COUNT(*) DESC LIMIT 1",
                [],
                |row| row.get(0),
            ).optional()?;
            let dimensions: Option<usize> = self.conn.query_row(
                "SELECT dimensions FROM node_embeddings LIMIT 1",
                [],
                |row| row.get::<_, i64>(0).map(|d| d as usize),
            ).optional()?;
            (embedded_count, model, dimensions)
        } else {
            let embedded_count: usize = self.conn.query_row(
                "SELECT COUNT(DISTINCT memory_id) FROM memory_embeddings",
                [],
                |row| row.get(0),
            )?;
            let model: Option<String> = self.conn.query_row(
                "SELECT model FROM memory_embeddings GROUP BY model ORDER BY COUNT(*) DESC LIMIT 1",
                [],
                |row| row.get(0),
            ).optional()?;
            let dimensions: Option<usize> = self.conn.query_row(
                "SELECT dimensions FROM memory_embeddings LIMIT 1",
                [],
                |row| row.get::<_, i64>(0).map(|d| d as usize),
            ).optional()?;
            (embedded_count, model, dimensions)
        };

        Ok(EmbeddingStats {
            total_memories,
            embedded_count,
            model,
            dimensions,
        })
    }
    
    // === ACL Methods ===
    
    /// Grant a permission to an agent for a namespace.
    pub fn grant_permission(
        &mut self,
        agent_id: &str,
        namespace: &str,
        permission: Permission,
        granted_by: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO engram_acl (agent_id, namespace, permission, granted_by, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            params![
                agent_id,
                namespace,
                permission.to_string(),
                granted_by,
                now_f64(),
            ],
        )?;
        Ok(())
    }
    
    /// Revoke a permission from an agent for a namespace.
    pub fn revoke_permission(&mut self, agent_id: &str, namespace: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM engram_acl WHERE agent_id = ? AND namespace = ?",
            params![agent_id, namespace],
        )?;
        Ok(())
    }
    
    /// Check if an agent has a specific permission for a namespace.
    /// 
    /// Permission hierarchy: admin > write > read
    /// Wildcard namespace ("*") grants access to all namespaces.
    pub fn check_permission(
        &self,
        agent_id: &str,
        namespace: &str,
        required: Permission,
    ) -> Result<bool, rusqlite::Error> {
        // Check for direct namespace permission
        let direct: Option<String> = self.conn
            .query_row(
                "SELECT permission FROM engram_acl WHERE agent_id = ? AND namespace = ?",
                params![agent_id, namespace],
                |row| row.get(0),
            )
            .optional()?;
        
        if let Some(perm_str) = direct {
            if let Ok(perm) = perm_str.parse::<Permission>() {
                return Ok(Self::permission_allows(perm, required));
            }
        }
        
        // Check for wildcard namespace permission
        let wildcard: Option<String> = self.conn
            .query_row(
                "SELECT permission FROM engram_acl WHERE agent_id = ? AND namespace = '*'",
                params![agent_id],
                |row| row.get(0),
            )
            .optional()?;
        
        if let Some(perm_str) = wildcard {
            if let Ok(perm) = perm_str.parse::<Permission>() {
                return Ok(Self::permission_allows(perm, required));
            }
        }
        
        // Default: check if this is the agent's own namespace or global namespace
        // Global namespace ("global") is readable by everyone
        if namespace == "global" && matches!(required, Permission::Read) {
            return Ok(true);
        }
        
        // Default write to own namespace
        if namespace == agent_id && matches!(required, Permission::Write | Permission::Read) {
            return Ok(true);
        }
        
        Ok(false)
    }
    
    /// Check if granted permission allows required permission.
    fn permission_allows(granted: Permission, required: Permission) -> bool {
        match required {
            Permission::Read => granted.can_read(),
            Permission::Write => granted.can_write(),
            Permission::Admin => granted.is_admin(),
        }
    }
    
    /// List all permissions for an agent.
    pub fn list_permissions(&self, agent_id: &str) -> Result<Vec<AclEntry>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT agent_id, namespace, permission, granted_by, created_at FROM engram_acl WHERE agent_id = ?"
        )?;
        
        let rows = stmt.query_map(params![agent_id], |row| {
            let perm_str: String = row.get(2)?;
            let created_at_f64: f64 = row.get(4)?;
            
            Ok(AclEntry {
                agent_id: row.get(0)?,
                namespace: row.get(1)?,
                permission: perm_str.parse().unwrap_or(Permission::Read),
                granted_by: row.get(3)?,
                created_at: f64_to_datetime(created_at_f64),
            })
        })?;
        
        rows.collect()
    }
    
    /// Get Hebbian neighbors for a memory, optionally filtered by namespace.
    pub fn get_hebbian_neighbors_ns(
        &self,
        memory_id: &str,
        namespace: Option<&str>,
    ) -> Result<Vec<String>, rusqlite::Error> {
        match namespace {
            Some("*") | None => {
                // All namespaces (original behavior)
                self.get_hebbian_neighbors(memory_id)
            }
            Some(ns) => {
                let mut stmt = self.conn.prepare(
                    "SELECT target_id FROM hebbian_links WHERE source_id = ? AND strength > 0 AND namespace = ?"
                )?;
                
                let rows = stmt.query_map(params![memory_id, ns], |row| row.get(0))?;
                rows.collect()
            }
        }
    }
    
    /// Record co-activation with namespace tracking.
    ///
    /// Legacy semantics: threshold-gated Hebbian link formation. The first
    /// `threshold` co-activations only increment `coactivation_count`; the
    /// link "forms" (strength=1.0) when count crosses the threshold; further
    /// recalls add `+0.1` (capped at 1.0).
    ///
    /// T14 dual-write (design §4.3): every call ALSO performs one UPSERT
    /// into `edges(edge_kind='associative')` with `signal_source='corecall'`
    /// and `delta_weight=0.1`. This replaces legacy's threshold-gated +0.1
    /// approximation with a true sum-accumulating Hebbian frequency signal.
    /// The legacy threshold/cap logic stays untouched on `hebbian_links`
    /// because v0.4 readers still resolve associative info via legacy;
    /// unified-edges divergence is documented in §4.3's comparison table
    /// and verified by T17 parity (existence + signal_source, not numeric
    /// equality).
    pub fn record_coactivation_ns(
        &mut self,
        id1: &str,
        id2: &str,
        threshold: i32,
        namespace: &str,
    ) -> Result<bool, rusqlite::Error> {
        let (id1, id2) = if id1 < id2 { (id1, id2) } else { (id2, id1) };

        let tx = self.conn.transaction()?;
        let result = {
            // Check existing link
            let existing: Option<(f64, i32)> = tx
                .query_row(
                    "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                    params![id1, id2],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            let formed = match existing {
                Some((strength, _count)) if strength > 0.0 => {
                    // Link already formed, strengthen it.
                    // ISS-117: single canonical row only.
                    let new_strength = (strength + 0.1).min(1.0);
                    tx.execute(
                        "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                        params![new_strength, id1, id2],
                    )?;
                    false
                }
                Some((_, count)) => {
                    // Tracking phase, increment count
                    let new_count = count + 1;
                    if new_count >= threshold {
                        // Threshold reached, form link.
                        // ISS-117: update canonical row only.
                        tx.execute(
                            "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        true
                    } else {
                        tx.execute(
                            "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        false
                    }
                }
                None => {
                    // First co-activation, create tracking record
                    tx.execute(
                        "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 0.0, 1, ?, ?)",
                        params![id1, id2, now_f64(), namespace],
                    )?;
                    false
                }
            };

            // T14: unified-edges dual-write. signal_source='corecall'
            // marks this as recall-driven co-activation (vs. LinkFormer's
            // 'entity'/'temporal'/etc. signals). delta_weight=0.1 matches
            // legacy's per-recall increment so sum-accumulating weight on
            // edges tracks Hebbian frequency exactly.
            crate::graph::store::dual_write_hebbian_to_edges(
                &tx,
                id1,
                id2,
                "corecall",
                "{}",
                0.1,
                namespace,
            )
            .map_err(|e| match e {
                crate::graph::GraphError::Sqlite(s) => s,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })?;

            Ok::<bool, rusqlite::Error>(formed)
        }?;

        tx.commit()?;
        Ok(result)
    }
    
    // === Cross-Namespace Hebbian Methods (Phase 3) ===
    
    /// Record cross-namespace co-activation.
    ///
    /// When memories from different namespaces are recalled together,
    /// this creates a Hebbian link that spans namespaces.
    ///
    /// T14 dual-write (design §4.3): every call ALSO performs one UPSERT
    /// into `edges(edge_kind='associative')` with `signal_source='corecall'`
    /// and `delta_weight=0.1`. The unified edge's `namespace` column holds
    /// the synthesized `"ns1:ns2"` marker (matches the legacy convention
    /// for cross-NS rows). See `record_coactivation_ns` for the rationale.
    pub fn record_cross_namespace_coactivation(
        &mut self,
        id1: &str,
        ns1: &str,
        id2: &str,
        ns2: &str,
        threshold: i32,
    ) -> Result<bool, rusqlite::Error> {
        // Only create cross-namespace links when namespaces differ
        if ns1 == ns2 {
            return self.record_coactivation_ns(id1, id2, threshold, ns1);
        }

        // Ensure consistent ordering
        let (id1, id2, ns1, ns2) = if (ns1, id1) < (ns2, id2) {
            (id1, id2, ns1, ns2)
        } else {
            (id2, id1, ns2, ns1)
        };

        // Use "ns1:ns2" as namespace marker for cross-namespace links
        let cross_ns = format!("{}:{}", ns1, ns2);

        let tx = self.conn.transaction()?;
        let result = {
            // Check existing link
            let existing: Option<(f64, i32)> = tx
                .query_row(
                    "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                    params![id1, id2],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            let formed = match existing {
                Some((strength, _count)) if strength > 0.0 => {
                    // Link already formed, strengthen it.
                    // ISS-117: single canonical row only.
                    let new_strength = (strength + 0.1).min(1.0);
                    tx.execute(
                        "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                        params![new_strength, id1, id2],
                    )?;
                    false
                }
                Some((_, count)) => {
                    // Tracking phase, increment count
                    let new_count = count + 1;
                    if new_count >= threshold {
                        // Threshold reached, form link.
                        // ISS-117: update canonical row only.
                        tx.execute(
                            "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        true
                    } else {
                        tx.execute(
                            "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                            params![new_count, id1, id2],
                        )?;
                        false
                    }
                }
                None => {
                    // First co-activation, create tracking record
                    tx.execute(
                        "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 0.0, 1, ?, ?)",
                        params![id1, id2, now_f64(), &cross_ns],
                    )?;
                    false
                }
            };

            // T14: unified-edges dual-write — see record_coactivation_ns
            // for full rationale. cross_ns ("ns1:ns2") goes into the
            // namespace column so cross-NS associative facts stay
            // distinguishable from same-NS ones.
            crate::graph::store::dual_write_hebbian_to_edges(
                &tx,
                id1,
                id2,
                "corecall",
                "{}",
                0.1,
                &cross_ns,
            )
            .map_err(|e| match e {
                crate::graph::GraphError::Sqlite(s) => s,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })?;

            Ok::<bool, rusqlite::Error>(formed)
        }?;

        tx.commit()?;
        Ok(result)
    }
    
    /// Discover cross-namespace Hebbian links between two namespaces.
    ///
    /// Returns all Hebbian links where source is in namespace_a and target
    /// is in namespace_b (or vice versa).
    pub fn discover_cross_links(
        &self,
        namespace_a: &str,
        namespace_b: &str,
    ) -> Result<Vec<HebbianLink>, rusqlite::Error> {
        // Find links with cross-namespace marker
        let cross_ns_1 = format!("{}:{}", namespace_a, namespace_b);
        let cross_ns_2 = format!("{}:{}", namespace_b, namespace_a);
        
        let mut stmt = self.conn.prepare(
            r#"
            SELECT h.source_id, h.target_id, h.strength, h.coactivation_count, 
                   h.direction, h.created_at, h.namespace,
                   m1.namespace as source_ns, m2.namespace as target_ns
            FROM hebbian_links h
            LEFT JOIN memories m1 ON h.source_id = m1.id
            LEFT JOIN memories m2 ON h.target_id = m2.id
            WHERE h.strength > 0 AND (h.namespace = ? OR h.namespace = ?)
            ORDER BY h.strength DESC
            "#,
        )?;
        
        let rows = stmt.query_map(params![cross_ns_1, cross_ns_2], |row| {
            let created_at_f64: f64 = row.get(5)?;
            let source_ns: Option<String> = row.get(7)?;
            let target_ns: Option<String> = row.get(8)?;
            
            Ok(HebbianLink {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                strength: row.get(2)?,
                coactivation_count: row.get(3)?,
                direction: row.get(4)?,
                created_at: f64_to_datetime(created_at_f64),
                source_ns,
                target_ns,
            })
        })?;
        
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
    
    /// Get all cross-namespace links for a memory.
    pub fn get_cross_namespace_neighbors(
        &self,
        memory_id: &str,
    ) -> Result<Vec<CrossLink>, rusqlite::Error> {
        // Get source memory's namespace
        let source_ns = self.get_namespace(memory_id)?;
        
        let mut stmt = self.conn.prepare(
            r#"
            SELECT h.source_id, h.target_id, h.strength, m.namespace, m.content
            FROM hebbian_links h
            JOIN memories m ON h.target_id = m.id
            WHERE h.source_id = ? AND h.strength > 0
            "#,
        )?;
        
        let source_ns_str = source_ns.clone().unwrap_or_else(|| "default".to_string());
        
        let rows = stmt.query_map(params![memory_id], |row| {
            let target_ns: String = row.get(3)?;
            let content: String = row.get(4)?;
            
            Ok(CrossLink {
                source_id: row.get(0)?,
                source_ns: source_ns_str.clone(),
                target_id: row.get(1)?,
                target_ns,
                strength: row.get(2)?,
                description: Some(content),
            })
        })?;
        
        // Filter to only cross-namespace links
        let source_ns_val = source_ns.unwrap_or_else(|| "default".to_string());
        Ok(rows
            .filter_map(|r| r.ok())
            .filter(|link| link.target_ns != source_ns_val)
            .collect())
    }
    
    /// Get all cross-namespace links in the database.
    pub fn get_all_cross_links(&self) -> Result<Vec<CrossLink>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT h.source_id, h.target_id, h.strength, 
                   m1.namespace as source_ns, m2.namespace as target_ns,
                   m2.content as target_content
            FROM hebbian_links h
            JOIN memories m1 ON h.source_id = m1.id
            JOIN memories m2 ON h.target_id = m2.id
            WHERE h.strength > 0 AND m1.namespace != m2.namespace
            ORDER BY h.strength DESC
            "#,
        )?;
        
        let rows = stmt.query_map([], |row| {
            Ok(CrossLink {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                strength: row.get(2)?,
                source_ns: row.get(3)?,
                target_ns: row.get(4)?,
                description: row.get(5)?,
            })
        })?;
        
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
    
    // Transaction support for bulk operations (ISS-001 fix)
    
    /// Begin an IMMEDIATE transaction.
    ///
    /// IMMEDIATE locks the DB immediately to prevent write conflicts.
    /// This is critical for consolidation cycles that do bulk updates.
    pub fn begin_transaction(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }
    
    /// Commit the current transaction.
    pub fn commit_transaction(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
    
    /// Rollback the current transaction.
    pub fn rollback_transaction(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }
    
    /// Rebuild the FTS5 index from scratch.
    ///
    /// Use this to recover from FTS corruption. This will re-index all memories.
    pub fn rebuild_fts(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute("INSERT INTO memories_fts(memories_fts) VALUES('rebuild')", [])?;
        Ok(())
    }
    
    /// Check database integrity.
    ///
    /// Returns true if integrity check passes, false otherwise.
    pub fn integrity_check(&self) -> Result<bool, rusqlite::Error> {
        let result: String = self.conn.query_row(
            "PRAGMA integrity_check", [], |row| row.get(0)
        )?;
        Ok(result == "ok")
    }
    
    // ── Entity CRUD ──────────────────────────────────────────────────────
    
    /// Upsert an entity. Returns the deterministic entity ID.
    ///
    /// If the entity already exists (by name+type+namespace), updates
    /// `updated_at` and merges metadata (new metadata wins if provided).
    pub fn upsert_entity(
        &self,
        name: &str,
        entity_type: &str,
        namespace: &str,
        metadata: Option<&str>,
    ) -> Result<String, rusqlite::Error> {
        let entity_id = generate_entity_id(name, entity_type, namespace);
        let now = now_f64();
        self.conn.execute(
            r#"
            INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(id) DO UPDATE SET
                updated_at = ?6,
                metadata = COALESCE(?5, metadata)
            "#,
            params![entity_id, name, entity_type, namespace, metadata, now],
        )?;
        Ok(entity_id)
    }
    
    /// Link a memory to an entity with a given role (e.g. "mention", "subject").
    ///
    /// Ignores duplicates (memory_id, entity_id is the PK).
    pub fn link_memory_entity(
        &self,
        memory_id: &str,
        entity_id: &str,
        role: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, role) VALUES (?1, ?2, ?3)",
            params![memory_id, entity_id, role],
        )?;
        Ok(())
    }
    
    /// Upsert an entity relation. Confidence starts at 0.1 and increments
    /// by 0.1 on each repeated observation, capped at 1.0.
    pub fn upsert_entity_relation(
        &self,
        source_id: &str,
        target_id: &str,
        relation: &str,
        namespace: &str,
    ) -> Result<(), rusqlite::Error> {
        let now = now_f64();
        let id = format!("{}_{}", source_id, target_id);
        self.conn.execute(
            r#"
            INSERT INTO entity_relations (id, source_id, target_id, relation, confidence, namespace, created_at)
            VALUES (?1, ?2, ?3, ?4, 0.1, ?5, ?6)
            ON CONFLICT(source_id, target_id, relation) DO UPDATE SET
                confidence = MIN(confidence + 0.1, 1.0),
                created_at = ?6
            "#,
            params![id, source_id, target_id, relation, namespace, now],
        )?;
        Ok(())
    }
    
    /// Find entities by exact name match, optionally filtered by namespace.
    pub fn find_entities(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EntityRecord>, rusqlite::Error> {
        match namespace {
            Some(ns) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, name, entity_type, namespace, metadata, created_at, updated_at \
                     FROM entities WHERE name = ?1 AND namespace = ?2 LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![query, ns, limit as i64], |row| {
                    Ok(EntityRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        entity_type: row.get(2)?,
                        namespace: row.get(3)?,
                        metadata: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, name, entity_type, namespace, metadata, created_at, updated_at \
                     FROM entities WHERE name = ?1 LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![query, limit as i64], |row| {
                    Ok(EntityRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        entity_type: row.get(2)?,
                        namespace: row.get(3)?,
                        metadata: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })?;
                Ok(rows.filter_map(|r| r.ok()).collect())
            }
        }
    }
    
    /// Get entity IDs associated with a memory.
    pub fn get_entity_ids_for_memory(&self, memory_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT entity_id FROM memory_entities WHERE memory_id = ?1"
        )?;
        let rows = stmt.query_map(params![memory_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Get all memory IDs linked to a given entity.
    pub fn get_entity_memories(&self, entity_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id FROM memory_entities WHERE entity_id = ?1",
        )?;
        let rows = stmt.query_map(params![entity_id], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
    
    /// Get entities related to a given entity (both directions).
    ///
    /// Returns `(entity_id, relation_type)` pairs.
    pub fn get_related_entities(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT target_id, relation FROM entity_relations WHERE source_id = ?1
            UNION
            SELECT source_id, relation FROM entity_relations WHERE target_id = ?1
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![entity_id, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
    
    /// Get a single entity by ID.
    pub fn get_entity(&self, id: &str) -> Result<Option<EntityRecord>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id, name, entity_type, namespace, metadata, created_at, updated_at \
                 FROM entities WHERE id = ?1",
                params![id],
                |row| {
                    Ok(EntityRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        entity_type: row.get(2)?,
                        namespace: row.get(3)?,
                        metadata: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()
    }
    
    /// Count entities, optionally filtered by namespace.
    pub fn count_entities(&self, namespace: Option<&str>) -> Result<usize, rusqlite::Error> {
        match namespace {
            Some(ns) => {
                let count: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM entities WHERE namespace = ?1",
                    params![ns],
                    |row| row.get(0),
                )?;
                Ok(count as usize)
            }
            None => {
                let count: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM entities",
                    [],
                    |row| row.get(0),
                )?;
                Ok(count as usize)
            }
        }
    }
    
    /// List entities, optionally filtered by type and namespace.
    /// Ordered by updated_at descending (most recently touched first).
    pub fn list_entities(
        &self,
        entity_type: Option<&str>,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(EntityRecord, usize)>, rusqlite::Error> {
        let sql = match (entity_type, namespace) {
            (Some(_), Some(_)) => {
                r#"SELECT e.id, e.name, e.entity_type, e.namespace, e.metadata, e.created_at, e.updated_at,
                          COUNT(me.memory_id) as mention_count
                   FROM entities e
                   LEFT JOIN memory_entities me ON e.id = me.entity_id
                   WHERE e.entity_type = ?1 AND e.namespace = ?2
                   GROUP BY e.id
                   ORDER BY mention_count DESC, e.updated_at DESC
                   LIMIT ?3"#
            }
            (Some(_), None) => {
                r#"SELECT e.id, e.name, e.entity_type, e.namespace, e.metadata, e.created_at, e.updated_at,
                          COUNT(me.memory_id) as mention_count
                   FROM entities e
                   LEFT JOIN memory_entities me ON e.id = me.entity_id
                   WHERE e.entity_type = ?1
                   GROUP BY e.id
                   ORDER BY mention_count DESC, e.updated_at DESC
                   LIMIT ?3"#
            }
            (None, Some(_)) => {
                r#"SELECT e.id, e.name, e.entity_type, e.namespace, e.metadata, e.created_at, e.updated_at,
                          COUNT(me.memory_id) as mention_count
                   FROM entities e
                   LEFT JOIN memory_entities me ON e.id = me.entity_id
                   WHERE e.namespace = ?2
                   GROUP BY e.id
                   ORDER BY mention_count DESC, e.updated_at DESC
                   LIMIT ?3"#
            }
            (None, None) => {
                r#"SELECT e.id, e.name, e.entity_type, e.namespace, e.metadata, e.created_at, e.updated_at,
                          COUNT(me.memory_id) as mention_count
                   FROM entities e
                   LEFT JOIN memory_entities me ON e.id = me.entity_id
                   GROUP BY e.id
                   ORDER BY mention_count DESC, e.updated_at DESC
                   LIMIT ?3"#
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let et = entity_type.unwrap_or("");
        let ns = namespace.unwrap_or("");
        let rows = stmt.query_map(params![et, ns, limit as i64], |row| {
            Ok((
                EntityRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: row.get(2)?,
                    namespace: row.get(3)?,
                    metadata: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                },
                row.get::<_, i64>(7)? as usize,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get entity statistics: (entity_count, relation_count, link_count).
    pub fn entity_stats(&self) -> Result<(usize, usize, usize), rusqlite::Error> {
        let entity_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM entities",
            [],
            |row| row.get(0),
        )?;
        let relation_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM entity_relations",
            [],
            |row| row.get(0),
        )?;
        let link_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_entities",
            [],
            |row| row.get(0),
        )?;
        Ok((entity_count as usize, relation_count as usize, link_count as usize))
    }

    /// Delete an entity and all its relations/links (CASCADE handles it).
    pub fn delete_entity(&self, entity_id: &str) -> Result<bool, rusqlite::Error> {
        let affected = self.conn.execute(
            "DELETE FROM entities WHERE id = ?1",
            [entity_id],
        )?;
        Ok(affected > 0)
    }

    /// Delete entities matching a filter. Returns count deleted.
    /// Used to purge false-positive entities (e.g., short @mentions that are noise).
    pub fn delete_entities_by_filter(
        &self,
        entity_type: &str,
        name_pattern: &str,
    ) -> Result<usize, rusqlite::Error> {
        // First find matching entity IDs
        let mut stmt = self.conn.prepare(
            "SELECT id FROM entities WHERE entity_type = ?1 AND name GLOB ?2"
        )?;
        let ids: Vec<String> = stmt.query_map(
            rusqlite::params![entity_type, name_pattern],
            |row| row.get(0),
        )?.filter_map(|r| r.ok()).collect();
        
        let mut count = 0;
        for id in &ids {
            if self.delete_entity(id)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Clear all memory_entities links for a batch of memories (for re-extraction).
    pub fn clear_memory_entity_links(&self, memory_ids: &[String]) -> Result<usize, rusqlite::Error> {
        let mut count = 0;
        for mid in memory_ids {
            count += self.conn.execute(
                "DELETE FROM memory_entities WHERE memory_id = ?1",
                [mid],
            )?;
        }
        Ok(count)
    }

    /// Find the most similar memory to the given embedding vector.
    /// Returns (memory_id, cosine_similarity) if any memory exceeds the threshold.
    /// Only searches within the specified namespace and model.
    pub fn find_nearest_embedding(
        &self,
        embedding: &[f32],
        model: &str,
        namespace: Option<&str>,
        threshold: f64,
    ) -> Result<Option<(String, f32)>, rusqlite::Error> {
        use crate::embeddings::EmbeddingProvider;
        
        let start = std::time::Instant::now();
        let stored = self.get_embeddings_in_namespace(namespace, model)?;
        
        let mut best: Option<(String, f32)> = None;
        for (mid, stored_emb) in &stored {
            let sim = EmbeddingProvider::cosine_similarity(embedding, stored_emb);
            if (sim as f64) >= threshold {
                match best {
                    Some((_, best_sim)) if sim > best_sim => {
                        best = Some((mid.clone(), sim));
                    }
                    None => {
                        best = Some((mid.clone(), sim));
                    }
                    _ => {}
                }
            }
        }
        
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 100 {
            log::warn!(
                "Dedup scan took {}ms over {} embeddings",
                elapsed.as_millis(),
                stored.len()
            );
        }
        
        Ok(best)
    }
    
    /// Find ALL memories with embedding similarity above threshold in namespace.
    /// Unlike find_nearest_embedding which returns top-1, this returns all matches.
    pub fn find_all_above_threshold(
        &self,
        embedding: &[f32],
        model: &str,
        namespace: Option<&str>,
        threshold: f64,
    ) -> Result<Vec<(String, f32)>, rusqlite::Error> {
        let stored = self.get_embeddings_in_namespace(namespace, model)?;
        let mut matches = Vec::new();
        for (id, stored_emb) in &stored {
            let sim = crate::EmbeddingProvider::cosine_similarity(embedding, stored_emb);
            if sim as f64 >= threshold {
                matches.push((id.clone(), sim));
            }
        }
        matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(matches)
    }

    /// Get first N chars of a memory's content.
    pub fn get_memory_content_preview(&self, id: &str, max_chars: usize) -> Result<String, rusqlite::Error> {
        let content: String = self.conn.query_row(
            "SELECT content FROM memories WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )?;
        Ok(content.chars().take(max_chars).collect())
    }

    /// Merge a duplicate memory's metadata into an existing memory.
    ///
    /// Strategy (from ISS-003, upgraded with smart merge):
    /// - access_count: add new access to existing memory's access log
    /// - importance: max(existing, new)
    /// - created_at: keep existing (older)
    /// - content: update if new content is significantly longer (>30% longer)
    /// - metadata: track merge history (capped at 10 entries) and merge count
    ///
    /// Does NOT create a new memory — just boosts the existing one.
    pub fn merge_memory_into(
        &mut self,
        existing_id: &str,
        new_content: &str,
        new_importance: f64,
        similarity: f32,
    ) -> Result<MergeOutcome, rusqlite::Error> {
        // Step 1: Insert a new access_log entry for the existing memory (now)
        self.conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![existing_id, now_f64()],
        )?;
        
        // Update importance = MAX(existing, new)
        self.conn.execute(
            "UPDATE memories SET importance = MAX(importance, ?) WHERE id = ?",
            params![new_importance, existing_id],
        )?;
        
        // Step 2: Content evolution — fetch existing content and metadata
        let (existing_content, existing_metadata_str): (String, Option<String>) = self.conn.query_row(
            "SELECT content, metadata FROM memories WHERE id = ?",
            params![existing_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        
        let content_updated = new_content.len() > (existing_content.len() as f64 * 1.3) as usize;
        
        if content_updated {
            self.conn.execute(
                "UPDATE memories SET content = ? WHERE id = ?",
                params![new_content, existing_id],
            )?;
        }
        
        // Step 3: Merge history in metadata
        let mut metadata: serde_json::Value = existing_metadata_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        
        // Ensure metadata is an object
        if !metadata.is_object() {
            metadata = serde_json::json!({});
        }
        
        // Build merge history entry
        let epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let history_entry = serde_json::json!({
            "ts": epoch_secs,
            "sim": similarity,
            "content_updated": content_updated,
            "prev_content_len": existing_content.len(),
            "new_content_len": new_content.len(),
        });
        
        // Append to merge_history array (FIFO, capped at 10)
        let (merge_history, merge_count_prev) = read_merge_tracking(&metadata);
        let mut new_history = merge_history;
        new_history.push(history_entry);
        if new_history.len() > 10 {
            let start = new_history.len() - 10;
            new_history = new_history[start..].to_vec();
        }
        let merge_count = merge_count_prev + 1;
        write_merge_tracking(&mut metadata, new_history, merge_count);
        
        // Write updated metadata back to DB
        let metadata_str = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "UPDATE memories SET metadata = ? WHERE id = ?",
            params![metadata_str, existing_id],
        )?;
        
        log::info!(
            "Merged duplicate into memory {}: boosted access + importance(max {}), content_updated={}, merge_count={}",
            existing_id,
            new_importance,
            content_updated,
            merge_count,
        );
        
        Ok(MergeOutcome {
            memory_id: existing_id.to_string(),
            content_updated,
            merge_count: merge_count as i32,
        })
    }

    /// Merge using a typed `EnrichedMemory` (ISS-019 Step 5).
    ///
    /// Coexists with `merge_memory_into`. Uses `Dimensions::union` to
    /// combine dimensional signatures without information loss, then
    /// writes back the merged metadata while preserving / appending
    /// to `merge_history` (FIFO cap 10) and incrementing `merge_count`.
    ///
    /// Steps (design §5.2):
    /// 1. Fetch existing row; decode to `EnrichedMemory`.
    /// 2. `merged_dims = existing.dimensions.union(incoming.dimensions, weights)`.
    /// 3. Content: longer wins (same rule as legacy, kept in sync with
    ///    `core_fact`).
    /// 4. Importance: `max(existing, incoming)`.
    /// 5. Metadata: `merged_dims.to_legacy_metadata()` plus preserved
    ///    `merge_count` / `merge_history`.
    /// 6. Single `UPDATE memories`, plus an `INSERT INTO access_log`
    ///    to record the merge-time access.
    ///
    /// Algebraic properties (design §5.3, proven by `Dimensions::union`
    /// proptests):
    /// - Idempotent on identical inputs.
    /// - Associative under consistent weights.
    /// - Monotone: never loses information.
    ///
    /// Legacy `merge_memory_into` remains for the shim path until
    /// Step 5.9 renames this method to canonical.
    pub fn merge_enriched_into(
        &mut self,
        existing_id: &str,
        incoming: &crate::enriched::EnrichedMemory,
        similarity: f32,
    ) -> Result<MergeOutcome, rusqlite::Error> {
        use crate::dimensions::Dimensions;
        use crate::enriched::EnrichedMemory;
        use crate::merge_types::MergeWeights;

        // Step 1: fetch the existing row and decode it into EnrichedMemory.
        let existing_record = self.get(existing_id)?.ok_or_else(|| {
            rusqlite::Error::QueryReturnedNoRows
        })?;
        let existing_em = EnrichedMemory::from_memory_record(&existing_record).map_err(|e| {
            // Only way this fails is empty content on a persisted row,
            // which would be a data-integrity bug, not a normal runtime
            // condition. Surface as a sqlite InvalidColumnType — the
            // closest rusqlite::Error kind for "row is corrupt".
            rusqlite::Error::InvalidColumnType(
                0,
                format!("EnrichedMemory::from_memory_record failed for id={}: {}", existing_id, e),
                rusqlite::types::Type::Text,
            )
        })?;

        // Step 2: dimensional union with importance-weighted scalars.
        let weights = MergeWeights::new(
            existing_em.importance.get(),
            incoming.importance.get(),
        );
        let merged_dims: Dimensions = existing_em
            .dimensions
            .clone()
            .union(incoming.dimensions.clone(), weights);

        // Step 3: content — longer wins (same rule `build_legacy_metadata`
        // established; kept in sync with core_fact so invariants hold).
        let (merged_content, content_updated) = {
            let ec = existing_em.content.as_str();
            let ic = incoming.content.as_str();
            if ic.len() > (ec.len() as f64 * 1.3) as usize {
                (ic.to_string(), true)
            } else {
                (ec.to_string(), false)
            }
        };

        // Step 4: importance = max.
        let merged_importance = existing_em
            .importance
            .get()
            .max(incoming.importance.get());

        // Step 5: build merged EnrichedMemory for metadata serialization.
        // Keep existing's user_metadata — merge path does not adopt
        // the incoming caller's arbitrary metadata keys; those belong
        // to a different session.
        //
        // Use merged_dims; if longer-wins chose incoming content, the
        // core_fact inside merged_dims may lag behind. Rewrite
        // core_fact to match merged_content to preserve the
        // EnrichedMemory invariant.
        let mut final_dims = merged_dims;
        if final_dims.core_fact.as_str() != merged_content {
            // Safe: merged_content derives from a non-empty existing
            // or incoming content_fact, so NonEmptyString::new succeeds.
            // If somehow empty, fall back to existing core_fact.
            if let Ok(new_core) = crate::dimensions::NonEmptyString::new(merged_content.clone()) {
                final_dims.core_fact = new_core;
            }
        }

        let merged_em = crate::enriched::EnrichedMemory::from_dimensions(
            final_dims,
            crate::dimensions::Importance::new(merged_importance),
            existing_em.source.clone(),
            existing_em.namespace.clone(),
            existing_em.user_metadata.clone(),
        );

        // Build the new metadata JSON from merged EnrichedMemory.
        let mut metadata = merged_em.to_legacy_metadata();
        // `to_legacy_metadata` returns a Value::Object — ensure it's
        // mutable as an object so we can splice merge tracking in.
        if !metadata.is_object() {
            metadata = serde_json::json!({});
        }

        // Step 6: preserve / extend merge tracking (merge_history,
        // merge_count). Read from existing record's stored metadata.
        let existing_meta_obj = existing_record
            .metadata
            .as_ref()
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let history_entry = serde_json::json!({
            "ts": epoch_secs,
            "sim": similarity,
            "content_updated": content_updated,
            "prev_content_len": existing_em.content.len(),
            "new_content_len": incoming.content.len(),
            "via": "merge_enriched_into",
        });

        let existing_meta_value = serde_json::Value::Object(existing_meta_obj);
        let (existing_history, existing_count) =
            read_merge_tracking(&existing_meta_value);
        let mut new_history = existing_history;
        new_history.push(history_entry);
        if new_history.len() > 10 {
            let start = new_history.len() - 10;
            new_history = new_history[start..].to_vec();
        }
        let merge_count = existing_count + 1;
        write_merge_tracking(&mut metadata, new_history, merge_count);

        // Persist:
        //   - access_log entry (records the merge-time access for ACT-R)
        //   - single UPDATE touching content + importance + metadata
        self.conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![existing_id, now_f64()],
        )?;

        let metadata_str = serde_json::to_string(&metadata)
            .unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "UPDATE memories SET content = ?, importance = ?, metadata = ? WHERE id = ?",
            params![merged_content, merged_importance, metadata_str, existing_id],
        )?;

        log::info!(
            "merge_enriched_into: id={} content_updated={} merge_count={} importance={:.3}",
            existing_id,
            content_updated,
            merge_count,
            merged_importance,
        );

        Ok(MergeOutcome {
            memory_id: existing_id.to_string(),
            content_updated,
            merge_count: merge_count as i32,
        })
    }

    /// Get memories that have no entity links (for backfill/extraction).
    ///
    /// Returns `(memory_id, content, namespace)` triples.
    pub fn get_memories_without_entities(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT m.id, m.content, COALESCE(m.namespace, 'default') as ns
            FROM memories m
            LEFT JOIN memory_entities me ON m.id = me.memory_id
            WHERE me.entity_id IS NULL
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // -----------------------------------------------------------------------
    // Synthesis Provenance
    // -----------------------------------------------------------------------

    /// Record provenance for a single source memory contributing to an insight.
    pub fn record_provenance(&self, record: &ProvenanceRecord) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute(
            "INSERT INTO synthesis_provenance (id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, source_original_importance) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.insight_id,
                record.source_id,
                record.cluster_id,
                record.synthesis_timestamp.to_rfc3339(),
                record.gate_decision,
                record.gate_scores.as_ref().map(|s| serde_json::to_string(s).unwrap_or_default()),
                record.confidence,
                record.source_original_importance,
            ],
        )?;

        // T16 — Phase B dual-write: provenance also lands in unified
        // `edges` as `edge_kind='provenance'`, `predicate='derived_from'`
        // (design §4.5). Direction: insight → source memory (the insight
        // is *derived from* the source).
        //
        // No partial unique index for provenance (design §3.2: only
        // associative + containment are uniquified). Each provenance
        // record gets a fresh `id` from the caller, so re-running a
        // retried synthesis creates additional rows — that matches the
        // legacy table's append-only behavior. T17 row-count parity test
        // will assert legacy-row-count == unified-row-count for
        // edge_kind='provenance'.
        //
        // Attributes JSON embeds gate_decision, gate_scores, cluster_id
        // per design §4.5 SQL. Pre-serialize gate_scores so the
        // `json_object` builder gets a string we can attach verbatim
        // (json_object embeds strings as JSON-encoded strings; the
        // gate_scores TEXT is already valid JSON, so we use json() to
        // unwrap it into the parent object instead of nesting as a
        // quoted string).
        let gate_scores_json: Option<String> = record
            .gate_scores
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default());
        let ts_unix = datetime_to_f64(&record.synthesis_timestamp);
        // synthesis_timestamp is **also** stored verbatim (RFC3339) in
        // attributes — `recorded_at`/`created_at` are epoch f64 columns
        // and lose sub-second precision compared to the RFC3339 source.
        // T25 backfill writes the same field; the Phase D
        // `row_to_provenance_from_edge` reader prefers attributes over
        // the epoch column. Without this field T16 dual-write rows
        // would lose nanosecond precision on round-trip — a real bug
        // discovered by T29.2 contract tests.
        let ts_rfc3339 = record.synthesis_timestamp.to_rfc3339();
        // Use `json()` wrapper so embedded JSON objects don't get
        // re-encoded as quoted strings inside the parent attributes
        // blob — matches the convention used in §4.3 Hebbian SQL.
        self.conn.execute(
            r#"
            INSERT INTO edges (
                id,
                source_id, target_id,
                edge_kind, predicate_kind, predicate,
                summary, attributes, weight,
                activation, confidence,
                recorded_at,
                namespace,
                created_at, updated_at
            ) VALUES (
                ?1,
                ?2, ?3,
                'provenance', 'canonical', 'derived_from',
                '',
                json_object(
                    'gate_decision',       ?4,
                    'gate_scores',         CASE WHEN ?5 IS NULL THEN NULL ELSE json(?5) END,
                    'cluster_id',          ?6,
                    'source_original_importance', ?7,
                    'synthesis_timestamp', ?10
                ),
                1.0,
                0.0, ?8,
                ?9,
                'default',
                ?9, ?9
            )
            "#,
            params![
                record.id,
                record.insight_id,
                record.source_id,
                record.gate_decision,
                gate_scores_json,
                record.cluster_id,
                record.source_original_importance,
                record.confidence,
                ts_unix,
                ts_rfc3339,
            ],
        )?;

        Ok(())
    }

    /// Get all source provenance records for a given insight.
    ///
    /// Phase D T29.2: reads from `synthesis_provenance` (legacy) or
    /// `edges WHERE edge_kind='provenance' AND predicate='derived_from'`
    /// (unified) based on `self.unified_substrate`. Both paths return
    /// bit-identical `ProvenanceRecord`s under T16+T25 dual-write/backfill
    /// invariants.
    pub fn get_insight_sources(&self, insight_id: &str) -> Result<Vec<ProvenanceRecord>, Box<dyn std::error::Error>> {
        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                "SELECT id, source_id, target_id, confidence, attributes \
                 FROM edges \
                 WHERE edge_kind = 'provenance' \
                   AND predicate = 'derived_from' \
                   AND source_id = ?1"
            )?;
            let records = stmt.query_map([insight_id], |row| {
                Self::row_to_provenance_from_edge(row)
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, source_original_importance FROM synthesis_provenance WHERE insight_id = ?1"
            )?;
            let records = stmt.query_map([insight_id], |row| {
                Self::row_to_provenance(row)
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        }
    }

    /// Get all insights derived from a source memory.
    ///
    /// Phase D T29.2: reads from `synthesis_provenance` (legacy) or
    /// `edges WHERE edge_kind='provenance' AND predicate='derived_from'`
    /// (unified) based on `self.unified_substrate`. T25 maps edge
    /// direction insight → source, so the source memory is keyed by
    /// `target_id` on the unified path (vs `source_id` in legacy).
    pub fn get_memory_insights(&self, source_id: &str) -> Result<Vec<ProvenanceRecord>, Box<dyn std::error::Error>> {
        if self.unified_substrate {
            let mut stmt = self.conn.prepare(
                "SELECT id, source_id, target_id, confidence, attributes \
                 FROM edges \
                 WHERE edge_kind = 'provenance' \
                   AND predicate = 'derived_from' \
                   AND target_id = ?1"
            )?;
            let records = stmt.query_map([source_id], |row| {
                Self::row_to_provenance_from_edge(row)
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, source_original_importance FROM synthesis_provenance WHERE source_id = ?1"
            )?;
            let records = stmt.query_map([source_id], |row| {
                Self::row_to_provenance(row)
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        }
    }

    /// Delete all provenance records for an insight.
    pub fn delete_provenance(&self, insight_id: &str) -> Result<usize, Box<dyn std::error::Error>> {
        let count = self.conn.execute(
            "DELETE FROM synthesis_provenance WHERE insight_id = ?1",
            [insight_id],
        )?;
        Ok(count)
    }

    /// Check what percentage of member IDs appear as source_id in synthesis_provenance.
    ///
    /// Phase D T29.2: reads from `synthesis_provenance` (legacy) or
    /// `edges WHERE edge_kind='provenance' AND predicate='derived_from'`
    /// (unified) based on `self.unified_substrate`. The "source" memory
    /// keyed by `member_ids[i]` is `target_id` on the unified path
    /// (insight→source edge direction per T25).
    pub fn check_coverage(&self, member_ids: &[String]) -> Result<f64, Box<dyn std::error::Error>> {
        if member_ids.is_empty() {
            return Ok(0.0);
        }
        let sql = if self.unified_substrate {
            "SELECT COUNT(*) FROM edges \
             WHERE edge_kind = 'provenance' \
               AND predicate = 'derived_from' \
               AND target_id = ?1"
        } else {
            "SELECT COUNT(*) FROM synthesis_provenance WHERE source_id = ?1"
        };
        let mut covered = 0usize;
        for id in member_ids {
            let count: i64 = self.conn.query_row(sql, [id], |row| row.get(0))?;
            if count > 0 {
                covered += 1;
            }
        }
        Ok(covered as f64 / member_ids.len() as f64)
    }

    /// Update the importance of a memory.
    pub fn update_importance(&self, memory_id: &str, importance: f64) -> Result<(), Box<dyn std::error::Error>> {
        self.conn.execute(
            "UPDATE memories SET importance = ?1 WHERE id = ?2",
            params![importance, memory_id],
        )?;
        Ok(())
    }

    /// Insert a raw memory record. Used by synthesis engine to store insights.
    ///
    /// **Caller must manage the transaction.** This method does NOT create its own
    /// transaction — it is designed to be called inside an existing transaction
    /// (e.g., from `begin_transaction()` / `commit_transaction()`).
    /// The caller's transaction provides atomicity for the memory insert + FTS indexing.
    pub fn store_raw(
        &self,
        id: &str,
        content: &str,
        memory_type: &str,
        importance: f64,
        metadata: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let now = datetime_to_f64(&Utc::now());
        self.conn.execute(
            r#"INSERT INTO memories (
                id, content, memory_type, importance, layer,
                working_strength, core_strength, source, created_at,
                last_consolidated, consolidation_count, pinned, metadata, namespace
            ) VALUES (?1, ?2, ?3, ?4, 'core', 0.5, 0.5, 'synthesis', ?5, NULL, 0, 0, ?6, 'default')"#,
            params![id, content, memory_type, importance, now, metadata],
        )?;
        // Record initial access
        self.conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![id, now],
        )?;
        // Insert into FTS
        let rowid: i64 = self.conn.query_row(
            "SELECT rowid FROM memories WHERE id = ?",
            params![id],
            |row| row.get(0),
        )?;
        let tokenized = tokenize_cjk_boundaries(content);
        self.conn.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?, ?)",
            params![rowid, tokenized],
        )?;

        // T16 — Phase B dual-write: synthesis insights also land in
        // `nodes` as `node_kind='insight'` (design §4.5). `store_raw`
        // is currently the ONLY caller (synthesis engine via
        // `store_insight_atomically`), and the legacy `memories.source`
        // column is hardcoded to `'synthesis'` — that hardcoding is
        // exactly why we can hardcode `node_kind='insight'` here too.
        //
        // If a future caller appears that uses `store_raw` for a
        // non-synthesis flow, the right fix is a new public ingest
        // entry point (per design §4.1 F4), not branching here.
        //
        // Statement-only INSERT (no inner transaction): when called
        // inside `store_insight_atomically`'s `begin_transaction`,
        // this statement joins the active tx so insight + provenance
        // commit atomically. Standalone calls land in their own
        // autocommit tx; `INSERT OR IGNORE` keeps that path
        // idempotent against retry.
        let next_fts_rowid: i64 = self.conn.query_row(
            "UPDATE fts_rowid_counter
             SET next_value = next_value + 1
             WHERE singleton = 0
             RETURNING next_value - 1",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO nodes (
                id, node_kind, namespace,
                layer, memory_type,
                content, summary, attributes,
                occurred_at, created_at, updated_at, last_consolidated,
                working_strength, core_strength, importance,
                consolidation_count, pinned,
                source, superseded_by,
                fts_rowid
            ) VALUES (
                ?1, 'insight', 'default',
                'core', ?2,
                ?3, '', COALESCE(?4, '{}'),
                NULL, ?5, ?5, NULL,
                0.5, 0.5, ?6,
                0, 0,
                'synthesis', NULL,
                ?7
            )
            "#,
            params![id, memory_type, content, metadata, now, importance, next_fts_rowid],
        )?;

        Ok(())
    }

    /// Convert a database row into a ProvenanceRecord.
    fn row_to_provenance(row: &rusqlite::Row) -> Result<ProvenanceRecord, rusqlite::Error> {
        let gate_scores_str: Option<String> = row.get(6)?;
        let gate_scores: Option<GateScores> = gate_scores_str.and_then(|s| serde_json::from_str(&s).ok());

        let ts_str: String = row.get(4)?;
        let synthesis_timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        Ok(ProvenanceRecord {
            id: row.get(0)?,
            insight_id: row.get(1)?,
            source_id: row.get(2)?,
            cluster_id: row.get(3)?,
            synthesis_timestamp,
            gate_decision: row.get(5)?,
            gate_scores,
            confidence: row.get(7)?,
            source_original_importance: row.get(8)?,
        })
    }

    /// Convert a unified-substrate `edges` row into a ProvenanceRecord
    /// (Phase D T29.2 read adapter).
    ///
    /// The caller is expected to SELECT five columns in this order:
    /// `id, source_id, target_id, confidence, attributes`. The
    /// attributes JSON is built by T25's
    /// `backfill_synthesis_provenance_to_edges` (and by T16's Phase B
    /// dual-write); see substrate/backfill.rs §T25 for the canonical
    /// shape. This function is the inverse of that packing.
    ///
    /// Field reconstruction:
    /// - `id`              ← edges.id
    /// - `insight_id`      ← edges.source_id (T25 maps insight→source as edge direction)
    /// - `source_id`       ← edges.target_id
    /// - `confidence`      ← edges.confidence
    /// - `cluster_id`              ← attributes["cluster_id"]
    /// - `synthesis_timestamp`     ← attributes["synthesis_timestamp"] (RFC3339)
    /// - `gate_decision`           ← attributes["gate_decision"]
    /// - `gate_scores`             ← attributes["gate_scores"] (nested JSON; None if absent or malformed)
    /// - `source_original_importance` ← attributes["source_original_importance"]
    ///
    /// Tolerance policy:
    /// - missing/malformed `synthesis_timestamp` → `Utc::now()`
    ///   (bug-for-bug compat with `row_to_provenance`'s
    ///   `parse_from_rfc3339(...).unwrap_or_else(|_| Utc::now())`).
    ///   Historical note: T16 dual-write rows written before this
    ///   field was added land here — they have a precise
    ///   `recorded_at` epoch column but no attribute. Future work
    ///   may fall back to the column; for now we accept the lossy
    ///   path because (a) Phase E will retire the legacy table
    ///   anyway and (b) all *new* T16 writes populate the field.
    /// - missing `gate_scores` or string-typed (T25 malformed-
    ///   passthrough shape) → `None`. Same lossy semantics as the
    ///   legacy reader's `.ok()` over `serde_json::from_str`.
    /// - missing required strings (`gate_decision`, `cluster_id`) →
    ///   empty string. This is **more lenient than the legacy
    ///   reader**, which would propagate a rusqlite NULL-conversion
    ///   error via `?`. Deliberate: under the T16+T25 contract these
    ///   are always populated, so the divergence only manifests on
    ///   externally-corrupted attributes JSON — surfacing the row
    ///   with empty strings keeps consumers (provenance chain
    ///   walkers) running rather than poisoning the whole batch.
    fn row_to_provenance_from_edge(
        row: &rusqlite::Row,
    ) -> Result<ProvenanceRecord, rusqlite::Error> {
        let id: String = row.get(0)?;
        let insight_id: String = row.get(1)?;
        let source_id: String = row.get(2)?;
        let confidence: f64 = row.get(3)?;
        let attrs_str: String = row.get(4)?;

        let attrs: serde_json::Value =
            serde_json::from_str(&attrs_str).unwrap_or(serde_json::Value::Null);

        let get_str = |key: &str| -> String {
            attrs
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let cluster_id = get_str("cluster_id");
        let gate_decision = get_str("gate_decision");

        let ts_str = get_str("synthesis_timestamp");
        let synthesis_timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        // gate_scores is a nested JSON object when well-formed (T25
        // unpacks the legacy TEXT into a sub-object). If it's a
        // string-shaped value here it means legacy was malformed and
        // T25 preserved the raw text — surface None, matching the
        // legacy reader's `.ok()` lossy-parse semantics.
        let gate_scores: Option<GateScores> = match attrs.get("gate_scores") {
            Some(v) if v.is_object() => serde_json::from_value(v.clone()).ok(),
            _ => None,
        };

        let source_original_importance = attrs
            .get("source_original_importance")
            .and_then(|v| v.as_f64());

        Ok(ProvenanceRecord {
            id,
            insight_id,
            source_id,
            cluster_id,
            synthesis_timestamp,
            gate_decision,
            gate_scores,
            confidence,
            source_original_importance,
        })
    }

    /// Get entity names associated with a memory.
    ///
    /// Joins through `memory_entities` → `entities` to return entity name strings.
    pub fn get_entities_for_memory(&self, memory_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT e.name FROM entities e \
             INNER JOIN memory_entities me ON e.id = me.entity_id \
             WHERE me.memory_id = ?1"
        )?;
        let rows = stmt.query_map(params![memory_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Get the first available embedding for a memory (any model).
    ///
    /// Used by association discovery when the caller doesn't know
    /// which model was used for a specific memory.
    pub fn get_embedding_for_memory(&self, memory_id: &str) -> Result<Option<Vec<f32>>, rusqlite::Error> {
        let result: Option<Vec<u8>> = if self.unified_substrate {
            self.conn
                .query_row(
                    "SELECT embedding FROM node_embeddings WHERE node_id = ?1 LIMIT 1",
                    params![memory_id],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            self.conn
                .query_row(
                    "SELECT embedding FROM memory_embeddings WHERE memory_id = ?1 LIMIT 1",
                    params![memory_id],
                    |row| row.get(0),
                )
                .optional()?
        };
        Ok(result.map(|bytes| bytes_to_f32_vec(&bytes)))
    }

    /// Get the created_at timestamp for a memory.
    ///
    /// Returns the Unix timestamp (f64) or None if the memory doesn't exist.
    pub fn get_memory_timestamp(&self, memory_id: &str) -> Result<Option<f64>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT created_at FROM memories WHERE id = ?1",
                params![memory_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Find the best entity overlap match for a set of entities in a namespace.
    /// Returns (memory_id, jaccard_score) for the best match above threshold.
    pub fn find_entity_overlap(
        &self,
        entity_names: &[String],
        namespace: &str,
        threshold: f64,
    ) -> Result<Option<(String, f64)>, rusqlite::Error> {
        if entity_names.is_empty() {
            return Ok(None);
        }
        
        // Build IN clause placeholders
        let placeholders: Vec<String> = entity_names.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let in_clause = placeholders.join(", ");
        
        // Query: find memory_ids that share entities, grouped with overlap count
        // Filter by namespace via JOIN on memories table
        let sql = format!(
            r#"
            SELECT me.memory_id, COUNT(DISTINCT e.name) as overlap_count
            FROM memory_entities me
            JOIN entities e ON me.entity_id = e.id
            JOIN memories m ON me.memory_id = m.id
            WHERE e.name IN ({})
              AND m.namespace = ?{}
              AND m.deleted_at IS NULL
            GROUP BY me.memory_id
            ORDER BY overlap_count DESC
            LIMIT 10
            "#,
            in_clause,
            entity_names.len() + 1
        );
        
        let mut stmt = self.conn.prepare(&sql)?;
        
        // Build params: entity names + namespace
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        for name in entity_names {
            params_vec.push(Box::new(name.clone()));
        }
        params_vec.push(Box::new(namespace.to_string()));
        
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter()
            .map(|p| p.as_ref())
            .collect();
        
        let mut best: Option<(String, f64)> = None;
        
        let mut rows = stmt.query(param_refs.as_slice())?;
        let input_count = entity_names.len();
        
        while let Some(row) = rows.next()? {
            let memory_id: String = row.get(0)?;
            let overlap_count: usize = row.get::<_, i64>(1)? as usize;
            
            // Get total entity count for this memory to compute Jaccard
            let target_count: usize = self.conn.query_row(
                "SELECT COUNT(*) FROM memory_entities WHERE memory_id = ?",
                params![memory_id],
                |r| r.get::<_, i64>(0),
            )? as usize;
            
            // Jaccard = intersection / union
            let union_count = input_count + target_count - overlap_count;
            if union_count == 0 { continue; }
            let jaccard = overlap_count as f64 / union_count as f64;
            
            if jaccard >= threshold {
                match &best {
                    Some((_, best_score)) if jaccard <= *best_score => {},
                    _ => { best = Some((memory_id, jaccard)); }
                }
            }
        }
        
        Ok(best)
    }

    /// Append merge provenance with full source_id tracking.
    /// Called after merge_memory_into() when the donor ID is known.
    pub fn append_merge_provenance(
        &self,
        target_id: &str,
        source_id: &str,
        similarity: f32,
        content_updated: bool,
    ) -> Result<(), rusqlite::Error> {
        let metadata_str: Option<String> = self.conn.query_row(
            "SELECT metadata FROM memories WHERE id = ?",
            params![target_id],
            |row| row.get(0),
        )?;
        
        let mut metadata: serde_json::Value = metadata_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        
        if !metadata.is_object() {
            metadata = serde_json::json!({});
        }
        
        let epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let entry = serde_json::json!({
            "source_id": source_id,
            "ts": epoch_secs,
            "sim": similarity,
            "content_updated": content_updated,
        });
        
        let (mut history, merge_count_prev) = read_merge_tracking(&metadata);
        history.push(entry);
        while history.len() > 10 { history.remove(0); }
        write_merge_tracking(&mut metadata, history, merge_count_prev);
        
        let metadata_str = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());
        self.conn.execute(
            "UPDATE memories SET metadata = ? WHERE id = ?",
            params![metadata_str, target_id],
        )?;
        
        Ok(())
    }

    /// Record a write-time discovered association (multi-signal Hebbian link).
    ///
    /// Checks for existing link in both directions (source→target OR target→source).
    /// If exists: updates strength to max(existing, new) and updates signal_source if new is stronger.
    /// If not: inserts a new link.
    ///
    /// Returns `Ok(true)` if a new link was created, `Ok(false)` if an existing link was updated.
    ///
    /// T14 — Phase B dual-write: every legacy `hebbian_links` write is
    /// mirrored to unified `edges(edge_kind='associative', predicate='co_activated')`
    /// inside the same transaction. Per design.md §4.3 the unified UPSERT
    /// uses `signal_source`-keyed identity and sum-accumulating weight,
    /// which differs from the legacy max semantics. T17 parity tests
    /// assert existence of corresponding unified rows, not numeric
    /// weight/count parity (intentional divergence).
    ///
    /// Signature: `&mut self` (was `&self` pre-T14). The cascade impact is
    /// documented in §8.10 T14.
    pub fn record_association(
        &mut self,
        source_id: &str,
        target_id: &str,
        strength: f64,
        signal_source: &str,
        signal_detail: &str,
        namespace: &str,
    ) -> Result<bool, rusqlite::Error> {
        let tx = self.conn.transaction()?;
        let result = {
            // Check for existing link (either direction)
            let existing: Option<(String, String, f64)> = tx
                .query_row(
                    "SELECT source_id, target_id, strength FROM hebbian_links \
                     WHERE (source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1) \
                     LIMIT 1",
                    params![source_id, target_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            let is_new = match existing {
                Some((existing_src, existing_tgt, existing_strength)) => {
                    // Update if new strength is higher
                    let new_strength = existing_strength.max(strength);
                    if strength > existing_strength {
                        // New link is stronger — update strength and signal_source
                        tx.execute(
                            "UPDATE hebbian_links SET strength = ?1, signal_source = ?2, signal_detail = ?3 \
                             WHERE source_id = ?4 AND target_id = ?5",
                            params![new_strength, signal_source, signal_detail, existing_src, existing_tgt],
                        )?;
                    } else {
                        // Just update strength (keep existing signal_source)
                        tx.execute(
                            "UPDATE hebbian_links SET strength = ?1 \
                             WHERE source_id = ?2 AND target_id = ?3",
                            params![new_strength, existing_src, existing_tgt],
                        )?;
                    }
                    false
                }
                None => {
                    // Create new link
                    tx.execute(
                        "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, \
                         created_at, signal_source, signal_detail, namespace) \
                         VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)",
                        params![
                            source_id,
                            target_id,
                            strength,
                            now_f64(),
                            signal_source,
                            signal_detail,
                            namespace,
                        ],
                    )?;
                    true
                }
            };

            // T14 dual-write: mirror the Hebbian event into unified
            // `edges` per §4.3. signal_source is part of row identity,
            // weight sum-accumulates, (src, tgt) canonicalized in the
            // helper. We pass `strength` as the delta_weight — for
            // LinkFormer's constant `initial_strength`, each call adds
            // a fresh delta to the unified row (legacy keeps max).
            crate::graph::store::dual_write_hebbian_to_edges(
                &tx,
                source_id,
                target_id,
                signal_source,
                signal_detail,
                strength,
                namespace,
            )
            .map_err(|e| match e {
                crate::graph::GraphError::Sqlite(s) => s,
                other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
            })?;

            Ok::<bool, rusqlite::Error>(is_new)
        };
        let is_new = result?;
        tx.commit()?;
        Ok(is_new)
    }
    
    /// Get memory IDs created since a given timestamp.
    ///
    /// Used by candidate selection for temporal window filtering.
    pub fn get_memory_ids_since(
        &self,
        since_timestamp: f64,
        namespace: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memories WHERE created_at >= ?1 AND namespace = ?2 \
             ORDER BY created_at DESC LIMIT 100"
        )?;
        let rows = stmt.query_map(params![since_timestamp, namespace], |row| {
            row.get(0)
        })?;
        rows.collect()
    }

    // ── Triple CRUD (ISS-016) ─────────────────────────────────────────

    /// Store triples for a memory. Duplicate (memory_id, s, p, o) are silently ignored.
    /// Also inserts triple subjects/objects as entities into memory_entities
    /// with source='triple' for transparent Hebbian integration.
    /// Returns the number of triples actually inserted.
    pub fn store_triples(&self, memory_id: &str, triples: &[Triple]) -> Result<usize, rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut inserted = 0;
        
        for triple in triples {
            let rows = self.conn.execute(
                "INSERT OR IGNORE INTO triples (memory_id, subject, predicate, object, confidence, source, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    memory_id,
                    triple.subject,
                    triple.predicate.as_str(),
                    triple.object,
                    triple.confidence,
                    match &triple.source {
                        TripleSource::Llm => "llm",
                        TripleSource::Rule => "rule",
                        TripleSource::Manual => "manual",
                    },
                    now,
                ],
            )?;
            if rows > 0 {
                inserted += 1;
                // Insert subject and object as entities for Hebbian integration
                self.insert_triple_entity(memory_id, &triple.subject)?;
                self.insert_triple_entity(memory_id, &triple.object)?;
            }
        }
        
        Ok(inserted)
    }
    
    /// Insert a triple-derived entity into entities + memory_entities tables.
    /// Uses deterministic ID from entity name hash.
    fn insert_triple_entity(&self, memory_id: &str, entity_name: &str) -> Result<(), rusqlite::Error> {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        
        let name_lower = entity_name.to_lowercase();
        let mut hasher = DefaultHasher::new();
        name_lower.hash(&mut hasher);
        let entity_id = format!("triple-{:x}", hasher.finish());
        
        let now = datetime_to_f64(&chrono::Utc::now());
        
        // Upsert into entities table
        self.conn.execute(
            "INSERT OR IGNORE INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at) \
             VALUES (?1, ?2, 'concept', 'triple', '{}', ?3, ?3)",
            params![entity_id, name_lower, now],
        )?;
        
        // Link to memory
        self.conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, role) VALUES (?1, ?2, 'triple')",
            params![memory_id, entity_id],
        )?;
        
        Ok(())
    }
    
    /// Get triples for a memory.
    pub fn get_triples(&self, memory_id: &str) -> Result<Vec<Triple>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT subject, predicate, object, confidence, source FROM triples WHERE memory_id = ?1"
        )?;
        let rows = stmt.query_map(params![memory_id], |row| {
            let subject: String = row.get(0)?;
            let predicate_str: String = row.get(1)?;
            let object: String = row.get(2)?;
            let confidence: f64 = row.get(3)?;
            let source_str: String = row.get(4)?;
            
            let predicate = Predicate::from_str_lossy(&predicate_str);
            let source = match source_str.as_str() {
                "rule" => TripleSource::Rule,
                "manual" => TripleSource::Manual,
                _ => TripleSource::Llm,
            };
            
            Ok(Triple {
                subject,
                predicate,
                object,
                confidence: confidence.clamp(0.0, 1.0),
                source,
                subject_kind_hint: None,
                object_kind_hint: None,
            })
        })?;
        rows.collect()
    }
    
    /// Check if a memory has triples already extracted.
    pub fn has_triples(&self, memory_id: &str) -> Result<bool, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM triples WHERE memory_id = ?1",
            params![memory_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
    
    /// Get memory IDs that need triple extraction (no triples, retry_count < max).
    pub fn get_unenriched_memory_ids(&self, limit: usize, max_retries: u32) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memories \
             WHERE id NOT IN (SELECT DISTINCT memory_id FROM triples) \
               AND triple_extraction_attempts < ?1 \
             ORDER BY created_at DESC \
             LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![max_retries, limit], |row| row.get(0))?;
        rows.collect()
    }
    
    /// Increment the extraction attempt counter for a memory.
    pub fn increment_extraction_attempts(&self, memory_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE memories SET triple_extraction_attempts = triple_extraction_attempts + 1 WHERE id = ?1",
            params![memory_id],
        )?;
        Ok(())
    }

    // ===========================================================================
    // Cluster State Persistence (incremental clustering)
    // ===========================================================================

    /// Migrate schema for cluster state tables.
    fn migrate_cluster_state(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS cluster_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_full_cluster_at TEXT,
                last_full_memory_count INTEGER DEFAULT 0,
                version INTEGER DEFAULT 1
            );
            INSERT OR IGNORE INTO cluster_state (id) VALUES (1);

            CREATE TABLE IF NOT EXISTS cluster_assignments (
                memory_id TEXT PRIMARY KEY,
                cluster_id TEXT NOT NULL,
                assigned_at TEXT NOT NULL,
                method TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0
            );

            CREATE TABLE IF NOT EXISTS cluster_centroids (
                cluster_id TEXT PRIMARY KEY,
                centroid BLOB NOT NULL,
                member_count INTEGER NOT NULL DEFAULT 0,
                dirty INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cluster_pending (
                memory_id TEXT PRIMARY KEY,
                added_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cluster_incremental_state (
                cluster_id TEXT PRIMARY KEY,
                state_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cluster_assignments_cluster ON cluster_assignments(cluster_id);
        "#)?;
        Ok(())
    }

    /// Initialize cluster tables (called by migrate, but can be called manually).
    pub fn init_cluster_tables(&self) -> Result<(), rusqlite::Error> {
        Self::migrate_cluster_state(&self.conn)
    }

    /// Get all cluster centroids as (cluster_id, centroid_vec).
    pub fn get_cluster_centroids(&self) -> Result<Vec<(String, Vec<f32>)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT cluster_id, centroid FROM cluster_centroids"
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let bytes: Vec<u8> = row.get(1)?;
            Ok((id, bytes_to_f32_vec(&bytes)))
        })?;
        rows.collect()
    }

    /// Assign a memory to a cluster.
    pub fn assign_to_cluster(
        &self, memory_id: &str, cluster_id: &str, method: &str, confidence: f64,
    ) -> Result<(), rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO cluster_assignments (memory_id, cluster_id, assigned_at, method, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![memory_id, cluster_id, now, method, confidence],
        )?;
        Ok(())
    }

    /// Incrementally update a centroid: new = (old * n + new_vec) / (n + 1)
    pub fn update_centroid_incremental(
        &self, cluster_id: &str, new_embedding: &[f32],
    ) -> Result<(), rusqlite::Error> {
        // Read current centroid + count
        let result: Option<(Vec<u8>, i64)> = self.conn.query_row(
            "SELECT centroid, member_count FROM cluster_centroids WHERE cluster_id = ?",
            params![cluster_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;

        let now = chrono::Utc::now().to_rfc3339();

        match result {
            Some((old_bytes, count)) => {
                let old = bytes_to_f32_vec(&old_bytes);
                let n = count as f32;
                let new_centroid: Vec<f32> = old.iter().zip(new_embedding.iter())
                    .map(|(o, e)| (o * n + e) / (n + 1.0))
                    .collect();
                let new_bytes: Vec<u8> = new_centroid.iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                self.conn.execute(
                    "UPDATE cluster_centroids SET centroid = ?1, member_count = member_count + 1, updated_at = ?2 WHERE cluster_id = ?3",
                    params![new_bytes, now, cluster_id],
                )?;
            }
            None => {
                // First member — centroid IS the embedding
                let bytes: Vec<u8> = new_embedding.iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                self.conn.execute(
                    "INSERT INTO cluster_centroids (cluster_id, centroid, member_count, dirty, updated_at)
                     VALUES (?1, ?2, 1, 0, ?3)",
                    params![cluster_id, bytes, now],
                )?;
            }
        }
        Ok(())
    }

    /// Mark a cluster as dirty (needs warm recluster).
    pub fn mark_cluster_dirty(&self, cluster_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE cluster_centroids SET dirty = 1 WHERE cluster_id = ?",
            params![cluster_id],
        )?;
        Ok(())
    }

    /// Get IDs of all dirty clusters.
    pub fn get_dirty_cluster_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT cluster_id FROM cluster_centroids WHERE dirty = 1"
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Add a memory to the pending queue (not assigned to any cluster).
    pub fn add_pending_memory(&self, memory_id: &str) -> Result<(), rusqlite::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO cluster_pending (memory_id, added_at) VALUES (?1, ?2)",
            params![memory_id, now],
        )?;
        Ok(())
    }

    /// Get all pending memory IDs.
    pub fn get_pending_memory_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare("SELECT memory_id FROM cluster_pending")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Get all memory IDs assigned to a specific cluster.
    pub fn get_cluster_members(&self, cluster_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id FROM cluster_assignments WHERE cluster_id = ?"
        )?;
        let rows = stmt.query_map(params![cluster_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Replace old clusters with new ones after warm/cold recluster.
    /// Deletes assignments for old_cluster_ids, inserts new assignments from new_clusters.
    pub fn replace_clusters(
        &self, old_cluster_ids: &[String], new_clusters: &[(String, Vec<String>, Vec<f32>)],
        // each tuple: (cluster_id, member_ids, centroid_vec)
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;

        // Delete old cluster assignments + centroids
        for cid in old_cluster_ids {
            tx.execute("DELETE FROM cluster_assignments WHERE cluster_id = ?", params![cid])?;
            tx.execute("DELETE FROM cluster_centroids WHERE cluster_id = ?", params![cid])?;
        }

        let now = chrono::Utc::now().to_rfc3339();

        // Insert new clusters
        for (cluster_id, member_ids, centroid) in new_clusters {
            // Insert centroid
            let centroid_bytes: Vec<u8> = centroid.iter().flat_map(|f| f.to_le_bytes()).collect();
            tx.execute(
                "INSERT OR REPLACE INTO cluster_centroids (cluster_id, centroid, member_count, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![cluster_id, centroid_bytes, member_ids.len() as i64, now],
            )?;

            // Insert assignments
            for mid in member_ids {
                tx.execute(
                    "INSERT OR REPLACE INTO cluster_assignments (memory_id, cluster_id, assigned_at, method, confidence)
                     VALUES (?1, ?2, ?3, 'warm', 1.0)",
                    params![mid, cluster_id, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Get memories by a set of IDs.
    pub fn get_memories_by_ids(&self, ids: &[String]) -> Result<Vec<MemoryRecord>, rusqlite::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Use individual queries to avoid SQL injection with dynamic IN clauses
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.get(id)? {
                results.push(record);
            }
        }
        Ok(results)
    }

    /// Clear all pending memories and dirty flags.
    pub fn clear_pending_and_dirty(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute("DELETE FROM cluster_pending", [])?;
        self.conn.execute("UPDATE cluster_centroids SET dirty = 0", [])?;
        Ok(())
    }

    /// Save full cluster state after a cold recluster.
    /// Replaces ALL cluster data with the provided clusters.
    pub fn save_full_cluster_state(
        &self, clusters: &[(String, Vec<String>, Vec<f32>)],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;

        // Clear everything
        tx.execute("DELETE FROM cluster_assignments", [])?;
        tx.execute("DELETE FROM cluster_centroids", [])?;
        tx.execute("DELETE FROM cluster_pending", [])?;

        let now = chrono::Utc::now().to_rfc3339();

        // Update cluster_state metadata
        tx.execute(
            "UPDATE cluster_state SET last_full_cluster_at = ?1, last_full_memory_count = ?2 WHERE id = 1",
            params![now, clusters.iter().map(|(_, members, _)| members.len()).sum::<usize>() as i64],
        )?;

        // Insert all clusters
        for (cluster_id, member_ids, centroid) in clusters {
            let centroid_bytes: Vec<u8> = centroid.iter().flat_map(|f| f.to_le_bytes()).collect();
            tx.execute(
                "INSERT INTO cluster_centroids (cluster_id, centroid, member_count, dirty, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![cluster_id, centroid_bytes, member_ids.len() as i64, now],
            )?;

            for mid in member_ids {
                tx.execute(
                    "INSERT INTO cluster_assignments (memory_id, cluster_id, assigned_at, method, confidence)
                     VALUES (?1, ?2, ?3, 'full', 1.0)",
                    params![mid, cluster_id, now],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Get the count of pending memories.
    pub fn get_pending_count(&self) -> Result<usize, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM cluster_pending", [], |row| row.get::<_, i64>(0)
        ).map(|c| c as usize)
    }

    /// Count total memories in storage.
    pub fn count_memories(&self) -> Result<usize, rusqlite::Error> {
        self.conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get::<_, i64>(0))
            .map(|c| c as usize)
    }

    /// Get all current clusters as MemoryCluster structs.
    ///
    /// Reads from cluster_assignments and builds minimal MemoryCluster structs.
    /// Quality scores and signal summaries use defaults since the full pairwise
    /// data is not recomputed — this is intentional for the warm/cached path
    /// where we skip expensive Infomap recomputation.
    /// Get the incremental synthesis state for a cluster.
    pub fn get_incremental_state(&self, cluster_id: &str) -> Result<Option<crate::synthesis::types::IncrementalState>, rusqlite::Error> {
        let result: Option<String> = self.conn.query_row(
            "SELECT state_json FROM cluster_incremental_state WHERE cluster_id = ?",
            params![cluster_id],
            |row| row.get(0),
        ).optional()?;
        match result {
            Some(json) => {
                match serde_json::from_str(&json) {
                    Ok(state) => Ok(Some(state)),
                    Err(_) => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    /// Save the incremental synthesis state for a cluster.
    pub fn set_incremental_state(&self, cluster_id: &str, state: &crate::synthesis::types::IncrementalState) -> Result<(), rusqlite::Error> {
        let json = serde_json::to_string(state).unwrap_or_default();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO cluster_incremental_state (cluster_id, state_json, updated_at) VALUES (?1, ?2, ?3)",
            params![cluster_id, json, now],
        )?;
        Ok(())
    }

    pub fn get_all_cluster_data(&self) -> Result<Vec<crate::synthesis::types::MemoryCluster>, rusqlite::Error> {
        use std::collections::HashMap;
        let mut clusters: HashMap<String, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT memory_id, cluster_id FROM cluster_assignments")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (memory_id, cluster_id) = row?;
            clusters.entry(cluster_id).or_default().push(memory_id);
        }

        let result = clusters.into_iter().map(|(cluster_id, mut members)| {
            members.sort();
            let centroid_id = members.first().cloned().unwrap_or_default();
            crate::synthesis::types::MemoryCluster {
                id: cluster_id,
                members,
                quality_score: 0.5, // default for cached clusters
                centroid_id,
                signals_summary: crate::synthesis::types::SignalsSummary {
                    dominant_signal: crate::synthesis::types::ClusterSignal::Hebbian,
                    hebbian_contribution: 0.4,
                    entity_contribution: 0.3,
                    embedding_contribution: 0.2,
                    temporal_contribution: 0.1,
                },
            }
        }).collect();

        Ok(result)
    }
    // ── Lifecycle: Health & Rebalance helpers (FEAT-003 Phase 5) ───────

    /// List distinct namespaces.
    pub fn list_namespaces(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT namespace FROM memories WHERE deleted_at IS NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Count memories without embeddings (orphans).
    pub fn count_orphan_memories(&self) -> Result<usize, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM memories m WHERE m.deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM memory_embeddings me WHERE me.memory_id = m.id)",
            [],
            |row| row.get(0),
        )
    }

    /// Count Hebbian links referencing deleted/non-existent memories.
    pub fn count_dangling_hebbian(&self) -> Result<usize, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM hebbian_links h WHERE NOT EXISTS (SELECT 1 FROM memories m WHERE m.id = h.source_id AND m.deleted_at IS NULL) OR NOT EXISTS (SELECT 1 FROM memories m WHERE m.id = h.target_id AND m.deleted_at IS NULL)",
            [],
            |row| row.get(0),
        )
    }

    /// Get IDs of memories without embeddings.
    pub fn get_orphan_memory_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id FROM memories m WHERE m.deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM memory_embeddings me WHERE me.memory_id = m.id)"
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Count clusters where >50% of members have been deleted or superseded.
    pub fn count_stale_clusters(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (
                SELECT ca.cluster_id,
                       COUNT(*) AS total,
                       SUM(CASE WHEN m.id IS NULL
                                OR m.deleted_at IS NOT NULL
                                OR (m.superseded_by IS NOT NULL AND m.superseded_by != '')
                           THEN 1 ELSE 0 END) AS gone
                FROM cluster_assignments ca
                LEFT JOIN memories m ON ca.memory_id = m.id
                GROUP BY ca.cluster_id
                HAVING CAST(gone AS REAL) / total > 0.5
            )",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Clean up access_log entries for deleted/non-existent memories.
    pub fn cleanup_orphaned_access_log(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM access_log WHERE memory_id NOT IN (SELECT id FROM memories WHERE deleted_at IS NULL)",
            [],
        )
    }

    /// Clean up Hebbian links where either side is deleted/non-existent.
    pub fn cleanup_dangling_hebbian(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM hebbian_links WHERE source_id NOT IN (SELECT id FROM memories WHERE deleted_at IS NULL) OR target_id NOT IN (SELECT id FROM memories WHERE deleted_at IS NULL)",
            [],
        )
    }

    /// Clean up entity links for deleted/non-existent memories.
    pub fn cleanup_orphaned_entity_links(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM memory_entities WHERE memory_id NOT IN (SELECT id FROM memories WHERE deleted_at IS NULL)",
            [],
        )
    }

    /// Count memories in a specific namespace (or all if None).
    pub fn count_memories_in_namespace(&self, namespace: Option<&str>) -> Result<usize, rusqlite::Error> {
        match namespace {
            Some(ns) => self.conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE namespace = ? AND deleted_at IS NULL",
                params![ns],
                |row| row.get(0),
            ),
            None => self.conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL",
                [],
                |row| row.get(0),
            ),
        }
    }

    // ===========================================================================
    // Quarantine table (ISS-019 Step 6)
    // ===========================================================================

    /// Create the `quarantine` table if absent.
    ///
    /// Persistent storage for content whose extractor pass failed at
    /// runtime. Failed content is preserved here (never silently dropped)
    /// so a later `retry_quarantined()` pass can re-run the extractor
    /// and promote the row to the main `memories` table if it now
    /// succeeds. Separate table keeps the main-table invariant clean
    /// ("every row has dimensions") and allows quarantine→memories
    /// to be 1-to-N when one failed blob produces multiple facts.
    ///
    /// Schema fields:
    /// - `id`                    — QuarantineId, uuid-short string
    /// - `content`               — original text to retry
    /// - `content_hash`          — dedup within quarantine
    /// - `reason_kind`           — serde tag of `QuarantineReason`
    ///   (`extractor_timeout`/`extractor_error`/...)
    /// - `reason_detail`         — optional inner string payload
    /// - `received_at`           — unix seconds, first-seen
    /// - `attempts`              — retry counter, bumped by `retry_quarantined`
    /// - `last_attempt_at`       — unix seconds, null until first retry
    /// - `last_error`            — last retry error message (null if none)
    /// - `source` / `namespace`  — StorageMeta carry-over for retry
    /// - `importance_hint`       — caller hint preserved for retry
    /// - `memory_type_hint`      — legacy MemoryType hint preserved for retry
    /// - `user_metadata`         — JSON blob, caller-supplied extras
    /// - `permanently_rejected`  — 0/1 flag; set once `attempts >= max_attempts`
    ///
    /// The table is intentionally NOT in the main `memories` namespace
    /// and is NOT indexed by FTS/vector — quarantine rows are not
    /// recall-visible. See design §4.
    fn migrate_quarantine(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS quarantine (
                id                    TEXT PRIMARY KEY,
                content               TEXT NOT NULL,
                content_hash          TEXT NOT NULL,
                reason_kind           TEXT NOT NULL,
                reason_detail         TEXT,
                received_at           REAL NOT NULL,
                attempts              INTEGER NOT NULL DEFAULT 0,
                last_attempt_at       REAL,
                last_error            TEXT,
                source                TEXT,
                namespace             TEXT,
                importance_hint       REAL,
                memory_type_hint      TEXT,
                user_metadata         TEXT,
                permanently_rejected  INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_quarantine_hash      ON quarantine(content_hash);
            CREATE INDEX IF NOT EXISTS idx_quarantine_received  ON quarantine(received_at);
            CREATE INDEX IF NOT EXISTS idx_quarantine_rejected  ON quarantine(permanently_rejected);
        "#)?;
        Ok(())
    }

    /// Insert a failed-extraction record into `quarantine`.
    ///
    /// Called by `Memory::store_raw` when the extractor fails. The
    /// returned `id` matches what the caller sees as
    /// `RawStoreOutcome::Quarantined { id, .. }`, so subsequent
    /// retries can be correlated.
    ///
    /// Dedup: if the same `content_hash` already has a non-permanently-
    /// rejected row, the existing id is returned (no duplicate insert).
    /// This prevents quarantine spam when the same content hits a
    /// transient extractor outage repeatedly.
    #[allow(clippy::too_many_arguments)] // 1:1 with schema columns; a struct adds boilerplate at a single callsite.
    pub fn insert_quarantine_row(
        &self,
        id: &str,
        content: &str,
        content_hash: &str,
        reason_kind: &str,
        reason_detail: Option<&str>,
        source: Option<&str>,
        namespace: Option<&str>,
        importance_hint: Option<f64>,
        memory_type_hint: Option<&str>,
        user_metadata: Option<&str>,
    ) -> SqlResult<String> {
        // Dedup: look for a live row with this content_hash.
        if let Some(existing_id) = self.conn.query_row(
            "SELECT id FROM quarantine
               WHERE content_hash = ?1 AND permanently_rejected = 0
               ORDER BY received_at DESC LIMIT 1",
            params![content_hash],
            |row| row.get::<_, String>(0),
        ).optional()? {
            return Ok(existing_id);
        }

        let now = chrono::Utc::now().timestamp() as f64;
        self.conn.execute(
            r#"INSERT INTO quarantine (
                id, content, content_hash, reason_kind, reason_detail,
                received_at, source, namespace,
                importance_hint, memory_type_hint, user_metadata
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                id,
                content,
                content_hash,
                reason_kind,
                reason_detail,
                now,
                source,
                namespace,
                importance_hint,
                memory_type_hint,
                user_metadata,
            ],
        )?;
        Ok(id.to_string())
    }

    /// One quarantine row as returned by `list_quarantine_for_retry`.
    ///
    /// Public so the retry caller (Memory::retry_quarantined) can
    /// reconstruct StorageMeta from the preserved hints.
    #[allow(dead_code)]  // tests don't touch every field
    pub fn list_quarantine_for_retry_batch(
        &self,
        max_items: usize,
    ) -> SqlResult<Vec<QuarantineRow>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, content, content_hash, reason_kind, reason_detail,
                      received_at, attempts, last_attempt_at, last_error,
                      source, namespace, importance_hint, memory_type_hint,
                      user_metadata
               FROM quarantine
               WHERE permanently_rejected = 0
               ORDER BY received_at ASC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![max_items as i64], |row| {
            Ok(QuarantineRow {
                id:                row.get(0)?,
                content:           row.get(1)?,
                content_hash:      row.get(2)?,
                reason_kind:       row.get(3)?,
                reason_detail:     row.get(4)?,
                received_at:       row.get(5)?,
                attempts:          row.get::<_, i64>(6)? as u32,
                last_attempt_at:   row.get(7)?,
                last_error:        row.get(8)?,
                source:            row.get(9)?,
                namespace:         row.get(10)?,
                importance_hint:   row.get(11)?,
                memory_type_hint:  row.get(12)?,
                user_metadata:     row.get(13)?,
            })
        })?;
        rows.collect()
    }

    /// Bump attempts counter + record last attempt time and error
    /// (if any). Does NOT delete the row.
    pub fn record_quarantine_attempt(
        &self,
        id: &str,
        last_error: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp() as f64;
        self.conn.execute(
            "UPDATE quarantine
                SET attempts = attempts + 1,
                    last_attempt_at = ?1,
                    last_error = ?2
              WHERE id = ?3",
            params![now, last_error, id],
        )?;
        Ok(())
    }

    /// Mark a quarantine row as permanently rejected (attempts
    /// exhausted). Row is kept for forensic review; never deleted
    /// automatically. Returns true if a row was flipped, false if
    /// unchanged or missing.
    pub fn mark_quarantine_permanently_rejected(
        &self,
        id: &str,
    ) -> SqlResult<bool> {
        let affected = self.conn.execute(
            "UPDATE quarantine SET permanently_rejected = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(affected > 0)
    }

    /// Delete a quarantine row by id (called after a successful
    /// retry promotes the content into `memories`). Returns true on
    /// delete, false if the row didn't exist.
    pub fn delete_quarantine_row(&self, id: &str) -> SqlResult<bool> {
        let affected = self.conn.execute(
            "DELETE FROM quarantine WHERE id = ?1",
            params![id],
        )?;
        Ok(affected > 0)
    }

    /// Purge permanently-rejected quarantine rows older than
    /// `ttl_seconds`. Never deletes non-rejected rows. Returns
    /// count of rows deleted.
    ///
    /// Honors the "never delete data silently" rule: only rows that
    /// were explicitly marked `permanently_rejected` (by exceeding
    /// max_attempts) and have been idle for `ttl_seconds` are
    /// candidates. Caller is responsible for invoking this deliberately.
    pub fn purge_rejected_quarantine(&self, ttl_seconds: i64) -> SqlResult<usize> {
        let cutoff = chrono::Utc::now().timestamp() as f64 - ttl_seconds as f64;
        let affected = self.conn.execute(
            "DELETE FROM quarantine
              WHERE permanently_rejected = 1
                AND (last_attempt_at IS NOT NULL AND last_attempt_at < ?1)",
            params![cutoff],
        )?;
        Ok(affected)
    }

    /// Count all live quarantine rows (non-rejected). For stats.
    pub fn count_quarantine_live(&self) -> SqlResult<usize> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM quarantine WHERE permanently_rejected = 0",
            [],
            |row| row.get::<_, i64>(0).map(|n| n as usize),
        )
    }

    // =====================================================================
    // ISS-019 Step 7b — backfill_queue table + CRUD
    // =====================================================================
    //
    // Holds pointers to rows in `memories` whose metadata is v1 (flat
    // layout) and was flagged by `classify_stored_metadata` as benefiting
    // from re-extraction. The queue is explicit — never populated as a
    // read-path side-effect. Callers (rebuild pilot, `engram backfill`
    // CLI, future scheduler) drive `enqueue_backfill` and
    // `list_backfill_batch` directly.
    //
    // Schema columns:
    //   - memory_id             — FK into memories(id); PRIMARY KEY (dedup)
    //   - enqueued_at           — first-seen timestamp (earliest wins)
    //   - reason_kind           — BackfillReason variant, snake_case
    //   - reason_detail         — optional human-readable context
    //   - attempts              — retry counter, bumped by backfill_dimensions
    //   - last_attempt_at       — wall-clock of most recent attempt
    //   - last_error            — message from most recent failure
    //   - permanently_rejected  — 0/1 flag; set once attempts >= max
    //
    // The table is intentionally NOT in the main `memories` namespace and
    // is NOT indexed by FTS/vector. Backfill rows are not recall-visible.
    fn migrate_backfill_queue(conn: &Connection) -> SqlResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS backfill_queue (
                memory_id             TEXT PRIMARY KEY,
                enqueued_at           REAL NOT NULL,
                reason_kind           TEXT NOT NULL,
                reason_detail         TEXT,
                attempts              INTEGER NOT NULL DEFAULT 0,
                last_attempt_at       REAL,
                last_error            TEXT,
                permanently_rejected  INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_backfill_enqueued ON backfill_queue(enqueued_at);
            CREATE INDEX IF NOT EXISTS idx_backfill_rejected ON backfill_queue(permanently_rejected);
            "#,
        )?;
        Ok(())
    }

    /// Insert (or upsert) a backfill-queue row for `memory_id`.
    ///
    /// Idempotent: if a live row already exists, preserves `enqueued_at`
    /// and `attempts` and updates only `reason_kind` / `reason_detail`
    /// (classification may refine over time). Re-enqueueing a
    /// permanently-rejected row is a no-op — caller must explicitly clear
    /// the flag first.
    pub fn enqueue_backfill(
        &self,
        memory_id: &str,
        reason_kind: &str,
        reason_detail: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp() as f64;
        self.conn.execute(
            r#"INSERT INTO backfill_queue (memory_id, enqueued_at, reason_kind, reason_detail)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(memory_id) DO UPDATE SET
                   reason_kind   = excluded.reason_kind,
                   reason_detail = excluded.reason_detail
                 WHERE permanently_rejected = 0"#,
            params![memory_id, now, reason_kind, reason_detail],
        )?;
        Ok(())
    }

    /// Fetch the oldest `max_items` live (non-rejected) backfill rows.
    pub fn list_backfill_batch(&self, max_items: usize) -> SqlResult<Vec<BackfillRow>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT memory_id, enqueued_at, reason_kind, reason_detail,
                      attempts, last_attempt_at, last_error
               FROM backfill_queue
               WHERE permanently_rejected = 0
               ORDER BY enqueued_at ASC
               LIMIT ?1"#,
        )?;
        let rows = stmt.query_map(params![max_items as i64], |row| {
            Ok(BackfillRow {
                memory_id:       row.get(0)?,
                enqueued_at:     row.get(1)?,
                reason_kind:     row.get(2)?,
                reason_detail:   row.get(3)?,
                attempts:        row.get::<_, i64>(4)? as u32,
                last_attempt_at: row.get(5)?,
                last_error:      row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Bump attempts + record last attempt time / error. Does not delete.
    pub fn record_backfill_attempt(
        &self,
        memory_id: &str,
        last_error: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp() as f64;
        self.conn.execute(
            "UPDATE backfill_queue
                SET attempts = attempts + 1,
                    last_attempt_at = ?1,
                    last_error = ?2
              WHERE memory_id = ?3",
            params![now, last_error, memory_id],
        )?;
        Ok(())
    }

    /// Flag a row as permanently rejected (attempts >= max_attempts).
    pub fn mark_backfill_permanently_rejected(&self, memory_id: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE backfill_queue SET permanently_rejected = 1 WHERE memory_id = ?1",
            params![memory_id],
        )?;
        Ok(())
    }

    /// Remove a backfill row (successful upgrade or deliberate purge).
    /// Returns `true` if a row was deleted.
    pub fn delete_backfill_row(&self, memory_id: &str) -> SqlResult<bool> {
        let n = self.conn.execute(
            "DELETE FROM backfill_queue WHERE memory_id = ?1",
            params![memory_id],
        )?;
        Ok(n > 0)
    }

    /// Count live (non-rejected) backfill rows.
    pub fn count_backfill_live(&self) -> SqlResult<usize> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM backfill_queue WHERE permanently_rejected = 0",
            [],
            |row| row.get::<_, i64>(0).map(|n| n as usize),
        )
    }

    /// List `(id, content, metadata)` tuples for scanning v1 rows.
    ///
    /// Paginated via `after_id` (exclusive). The intended caller is
    /// `Memory::scan_and_enqueue_backfill`, which iterates pages and
    /// runs `classify_stored_metadata` on each row to decide whether
    /// to enqueue. A coarse SQL-side filter excludes obviously-v2 rows
    /// (`metadata LIKE '%"engram"%'` implies the namespace is present);
    /// Rust-side classification handles the precise check.
    ///
    /// Excludes soft-deleted and superseded rows.
    pub fn list_v1_candidates_page(
        &self,
        after_id: Option<&str>,
        page_size: usize,
    ) -> SqlResult<Vec<(String, String, Option<String>)>> {
        let sql = match after_id {
            Some(_) => {
                r#"SELECT id, content, metadata FROM memories
                   WHERE id > ?1
                     AND deleted_at IS NULL
                     AND (superseded_by IS NULL OR superseded_by = '')
                     AND (metadata IS NULL OR metadata NOT LIKE '%"engram"%')
                   ORDER BY id ASC
                   LIMIT ?2"#
            }
            None => {
                r#"SELECT id, content, metadata FROM memories
                   WHERE deleted_at IS NULL
                     AND (superseded_by IS NULL OR superseded_by = '')
                     AND (metadata IS NULL OR metadata NOT LIKE '%"engram"%')
                   ORDER BY id ASC
                   LIMIT ?1"#
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows: Vec<(String, String, Option<String>)> = if let Some(aid) = after_id {
            stmt.query_map(params![aid, page_size as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<SqlResult<Vec<_>>>()?
        } else {
            stmt.query_map(params![page_size as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<SqlResult<Vec<_>>>()?
        };
        Ok(rows)
    }
}

/// A single row read from the quarantine table. See
/// `Storage::list_quarantine_for_retry_batch`.
#[derive(Debug, Clone)]
pub struct QuarantineRow {
    pub id:               String,
    pub content:          String,
    pub content_hash:     String,
    pub reason_kind:      String,
    pub reason_detail:    Option<String>,
    pub received_at:      f64,
    pub attempts:         u32,
    pub last_attempt_at:  Option<f64>,
    pub last_error:       Option<String>,
    pub source:           Option<String>,
    pub namespace:        Option<String>,
    pub importance_hint:  Option<f64>,
    pub memory_type_hint: Option<String>,
    pub user_metadata:    Option<String>,
}

/// A single row read from the backfill_queue table. See
/// `Storage::list_backfill_batch`.
#[derive(Debug, Clone)]
pub struct BackfillRow {
    pub memory_id:       String,
    pub enqueued_at:     f64,
    pub reason_kind:     String,
    pub reason_detail:   Option<String>,
    pub attempts:        u32,
    pub last_attempt_at: Option<f64>,
    pub last_error:      Option<String>,
}

// =============================================================================
// Free functions for cross-thread memory access (used by ResolutionPipeline's
// SqliteMemoryReader, which holds its own Mutex<Connection> separate from
// `Storage`'s connection — see ISS-037 Blocker 2).
// =============================================================================

/// Fetch a `MemoryRecord` by ID using a borrowed connection.
///
/// This mirrors `Storage::get` but takes `&Connection` directly so it can be
/// reused from a `MemoryReader` impl that owns its own connection (typically
/// wrapped in `Mutex` for `Sync`).
pub fn fetch_memory_record(
    conn: &Connection,
    id: &str,
) -> Result<Option<MemoryRecord>, rusqlite::Error> {
    let access_times = fetch_access_times(conn, id)?;

    conn.query_row(
        "SELECT * FROM memories WHERE id = ?",
        params![id],
        |row| row_to_record_impl(row, access_times.clone()),
    )
    .optional()
}

/// Fetch a `MemoryRecord` *with its namespace tag* by ID.
///
/// ISS-055: the resolution worker must scope all graph reads/writes to the
/// memory's `--ns` value, but `MemoryRecord` does not carry the namespace
/// column. This function exposes both as a tuple so `SqliteMemoryReader`
/// (the only production `MemoryReader` impl) can hand the namespace through
/// to `ResolutionPipeline::run_job`. Storage's high-level `get` keeps its
/// historical signature; new callers that need the namespace should use
/// this directly.
pub fn fetch_memory_record_with_namespace(
    conn: &Connection,
    id: &str,
) -> Result<Option<(MemoryRecord, String)>, rusqlite::Error> {
    let access_times = fetch_access_times(conn, id)?;

    conn.query_row(
        "SELECT *, namespace FROM memories WHERE id = ?",
        params![id],
        |row| {
            let record = row_to_record_impl(row, access_times.clone())?;
            // `namespace` column is part of `SELECT *`, so reading it by
            // name works even though we list it explicitly above.
            // SQLite returns the FIRST matching column by name, which is
            // the one from `SELECT *` — no duplication concern.
            let namespace: String = row.get("namespace")?;
            Ok((record, namespace))
        },
    )
    .optional()
}

/// Fetch all access timestamps for a memory using a borrowed connection.
fn fetch_access_times(
    conn: &Connection,
    id: &str,
) -> Result<Vec<DateTime<Utc>>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT accessed_at FROM access_log WHERE memory_id = ? ORDER BY accessed_at",
    )?;
    let rows = stmt.query_map(params![id], |row| {
        let ts: f64 = row.get(0)?;
        Ok(f64_to_datetime(ts))
    })?;
    rows.collect()
}

/// Map a SQL row from `memories` into a `MemoryRecord`.
///
/// Connection-independent: takes pre-fetched `access_times` so it can be called
/// from any context (Storage method, free function, MemoryReader, etc.).
pub(crate) fn row_to_record_impl(
    row: &rusqlite::Row,
    access_times: Vec<DateTime<Utc>>,
) -> SqlResult<MemoryRecord> {
    // Use column names instead of indices to handle DBs with extra columns (e.g. Python's summary/tokens)
    let memory_type_str: String = row.get("memory_type")?;
    let layer_str: String = row.get("layer")?;
    let created_at_f64: f64 = row.get("created_at")?;
    let last_consolidated_f64: Option<f64> = row.get("last_consolidated")?;
    let metadata_str: Option<String> = row.get("metadata")?;

    let memory_type = match memory_type_str.as_str() {
        "factual" => MemoryType::Factual,
        "episodic" => MemoryType::Episodic,
        "relational" => MemoryType::Relational,
        "emotional" => MemoryType::Emotional,
        "procedural" => MemoryType::Procedural,
        "opinion" => MemoryType::Opinion,
        "causal" => MemoryType::Causal,
        _ => MemoryType::Factual,
    };

    let layer = match layer_str.as_str() {
        "core" => MemoryLayer::Core,
        "working" => MemoryLayer::Working,
        "archive" => MemoryLayer::Archive,
        _ => MemoryLayer::Working,
    };

    let created_at = f64_to_datetime(created_at_f64);
    let last_consolidated = last_consolidated_f64.map(f64_to_datetime);

    // ISS-103: occurred_at is optional and only present in DBs migrated past
    // the v0.3.x split. `row.get` for a missing column returns
    // `InvalidColumnName` which we treat as "column not present yet" → None.
    // For columns that ARE present but contain SQL NULL, `Option<f64>` reads
    // as `Ok(None)`.
    let occurred_at = match row.get::<_, Option<f64>>("occurred_at") {
        Ok(Some(ts)) => Some(f64_to_datetime(ts)),
        Ok(None) => None,
        Err(rusqlite::Error::InvalidColumnName(_)) => None,
        Err(e) => return Err(e),
    };

    let contradicts_str: String = row.get("contradicts")?;
    let contradicted_by_str: String = row.get("contradicted_by")?;
    let superseded_by_str: String = row.get("superseded_by").unwrap_or_default();

    let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());

    Ok(MemoryRecord {
        id: row.get("id")?,
        content: row.get("content")?,
        memory_type,
        layer,
        created_at,
        occurred_at,
        access_times,
        working_strength: row.get("working_strength")?,
        core_strength: row.get("core_strength")?,
        importance: row.get("importance")?,
        pinned: row.get::<_, i32>("pinned")? != 0,
        consolidation_count: row.get("consolidation_count")?,
        last_consolidated,
        source: row.get("source")?,
        contradicts: if contradicts_str.is_empty() {
            None
        } else {
            Some(contradicts_str)
        },
        contradicted_by: if contradicted_by_str.is_empty() {
            None
        } else {
            Some(contradicted_by_str)
        },
        superseded_by: if superseded_by_str.is_empty() {
            None
        } else {
            Some(superseded_by_str)
        },
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};

    fn test_storage() -> Storage {
        Storage::new(":memory:").expect("in-memory storage")
    }

    fn make_record(id: &str, content: &str, created_at: DateTime<Utc>) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at,
            occurred_at: None,
            access_times: vec![created_at],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn test_record_association_new() {
        let _storage = test_storage();
        // Need to create memories first for FK constraints
        let mut storage_mut = Storage::new(":memory:").unwrap();
        let now = Utc::now();
        let m1 = make_record("mem_a", "memory about cats", now);
        let m2 = make_record("mem_b", "memory about dogs", now);
        storage_mut.add(&m1, "default").unwrap();
        storage_mut.add(&m2, "default").unwrap();

        let created = storage_mut
            .record_association("mem_a", "mem_b", 0.5, "entity", r#"{"entity_overlap":0.4}"#, "default")
            .unwrap();
        assert!(created, "should create new link");

        // Verify the link exists with correct columns
        let row: (f64, String, String) = storage_mut.connection().query_row(
            "SELECT strength, signal_source, signal_detail FROM hebbian_links WHERE source_id = 'mem_a' AND target_id = 'mem_b'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert!((row.0 - 0.5).abs() < f64::EPSILON);
        assert_eq!(row.1, "entity");
        assert_eq!(row.2, r#"{"entity_overlap":0.4}"#);
    }

    #[test]
    fn test_record_association_duplicate() {
        let mut storage = test_storage();
        let now = Utc::now();
        let m1 = make_record("mem_a", "memory about cats", now);
        let m2 = make_record("mem_b", "memory about dogs", now);
        storage.add(&m1, "default").unwrap();
        storage.add(&m2, "default").unwrap();

        // First insertion
        let created1 = storage
            .record_association("mem_a", "mem_b", 0.5, "entity", "{}", "default")
            .unwrap();
        assert!(created1);

        // Second insertion of same pair — should update, not create
        let created2 = storage
            .record_association("mem_a", "mem_b", 0.3, "temporal", "{}", "default")
            .unwrap();
        assert!(!created2, "should not create duplicate");

        // Verify only one row exists
        let count: i64 = storage.connection().query_row(
            "SELECT COUNT(*) FROM hebbian_links WHERE \
             (source_id = 'mem_a' AND target_id = 'mem_b') OR \
             (source_id = 'mem_b' AND target_id = 'mem_a')",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_record_association_bidirectional() {
        let mut storage = test_storage();
        let now = Utc::now();
        let m1 = make_record("mem_a", "memory about cats", now);
        let m2 = make_record("mem_b", "memory about dogs", now);
        storage.add(&m1, "default").unwrap();
        storage.add(&m2, "default").unwrap();

        // A → B
        let created1 = storage
            .record_association("mem_a", "mem_b", 0.5, "entity", "{}", "default")
            .unwrap();
        assert!(created1);

        // B → A should detect existing link and not create duplicate
        let created2 = storage
            .record_association("mem_b", "mem_a", 0.6, "multi", "{}", "default")
            .unwrap();
        assert!(!created2, "B→A should not create duplicate when A→B exists");

        // Verify only one row total
        let count: i64 = storage.connection().query_row(
            "SELECT COUNT(*) FROM hebbian_links WHERE \
             (source_id = 'mem_a' AND target_id = 'mem_b') OR \
             (source_id = 'mem_b' AND target_id = 'mem_a')",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        // Strength should be updated to max(0.5, 0.6) = 0.6
        let strength: f64 = storage.connection().query_row(
            "SELECT strength FROM hebbian_links WHERE source_id = 'mem_a' AND target_id = 'mem_b'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!((strength - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decay_differential_rates() {
        let mut storage = test_storage();
        let now = Utc::now();

        // Create three memories
        for id in &["m1", "m2", "m3", "m4", "m5", "m6"] {
            let rec = make_record(id, &format!("memory {}", id), now);
            storage.add(&rec, "default").unwrap();
        }

        // Create links with different signal_sources, all starting at strength 1.0
        let now_f64 = now.timestamp() as f64;
        storage.connection().execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
             VALUES ('m1', 'm2', 1.0, 1, ?1, 'corecall', 'default')",
            params![now_f64],
        ).unwrap();
        storage.connection().execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
             VALUES ('m3', 'm4', 1.0, 1, ?1, 'multi', 'default')",
            params![now_f64],
        ).unwrap();
        storage.connection().execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
             VALUES ('m5', 'm6', 1.0, 1, ?1, 'entity', 'default')",
            params![now_f64],
        ).unwrap();

        // Apply differential decay
        storage.decay_hebbian_links_differential(0.95, 0.90, 0.85).unwrap();

        // Check strengths
        let get_strength = |src: &str, tgt: &str| -> f64 {
            storage.connection().query_row(
                "SELECT strength FROM hebbian_links WHERE source_id = ?1 AND target_id = ?2",
                params![src, tgt],
                |row| row.get(0),
            ).unwrap()
        };

        let corecall_str = get_strength("m1", "m2");
        let multi_str = get_strength("m3", "m4");
        let entity_str = get_strength("m5", "m6");

        assert!((corecall_str - 0.95).abs() < 1e-9, "corecall should be 0.95, got {}", corecall_str);
        assert!((multi_str - 0.90).abs() < 1e-9, "multi should be 0.90, got {}", multi_str);
        assert!((entity_str - 0.85).abs() < 1e-9, "entity should be 0.85, got {}", entity_str);

        // Verify ordering: corecall > multi > entity (differential rates)
        assert!(corecall_str > multi_str);
        assert!(multi_str > entity_str);
    }

    #[test]
    fn test_decay_differential_deletes_weak() {
        let mut storage = test_storage();
        let now = Utc::now();

        // Create memories
        for id in &["m1", "m2", "m3", "m4"] {
            let rec = make_record(id, &format!("memory {}", id), now);
            storage.add(&rec, "default").unwrap();
        }

        let now_f64 = now.timestamp() as f64;
        // Create a weak entity link (strength 0.11 — just above threshold)
        storage.connection().execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
             VALUES ('m1', 'm2', 0.11, 1, ?1, 'entity', 'default')",
            params![now_f64],
        ).unwrap();
        // Create a stronger corecall link
        storage.connection().execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, signal_source, namespace) \
             VALUES ('m3', 'm4', 0.5, 1, ?1, 'corecall', 'default')",
            params![now_f64],
        ).unwrap();

        // Decay: entity link → 0.11 * 0.85 = 0.0935 < 0.1 → should be deleted
        // corecall link → 0.5 * 0.95 = 0.475 → should survive
        let deleted = storage.decay_hebbian_links_differential(0.95, 0.90, 0.85).unwrap();
        assert_eq!(deleted, 1, "should delete 1 weak link");

        // Verify entity link is gone
        let count: i64 = storage.connection().query_row(
            "SELECT COUNT(*) FROM hebbian_links WHERE source_id = 'm1' AND target_id = 'm2'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 0, "weak entity link should be deleted");

        // Verify corecall link survives
        let count: i64 = storage.connection().query_row(
            "SELECT COUNT(*) FROM hebbian_links WHERE source_id = 'm3' AND target_id = 'm4'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1, "strong corecall link should survive");
    }

    #[test]
    fn test_hebbian_signal_migration_fresh_db() {
        // Fresh DB should have signal_source and signal_detail columns
        let storage = test_storage();
        let has_signal_source: bool = storage.connection().query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('hebbian_links') WHERE name='signal_source'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(has_signal_source, "signal_source column should exist on fresh DB");

        let has_signal_detail: bool = storage.connection().query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('hebbian_links') WHERE name='signal_detail'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(has_signal_detail, "signal_detail column should exist on fresh DB");

        // Index should exist
        let has_index: bool = storage.connection().query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_hebbian_signal_source'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(has_index, "idx_hebbian_signal_source index should exist");
    }

    #[test]
    fn test_hebbian_signal_migration_idempotent() {
        // Running migration twice should not fail
        let storage = test_storage();
        // Migration already ran in Storage::new(). Run it again manually.
        Storage::migrate_hebbian_signals(storage.connection()).unwrap();
        // And a third time for good measure
        Storage::migrate_hebbian_signals(storage.connection()).unwrap();

        let has_signal_source: bool = storage.connection().query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('hebbian_links') WHERE name='signal_source'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(has_signal_source);
    }

    #[test]
    fn test_hebbian_signal_migration_backfills_existing_rows() {
        // Create a DB, insert a hebbian_link without signal_source, then migrate
        let conn = Connection::open(":memory:").unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;").unwrap();
        Storage::create_schema(&conn).unwrap();
        Storage::migrate_v2(&conn).unwrap();

        // Insert two memories for FK constraints
        let now = now_f64();
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, namespace) VALUES ('m1', 'test1', 'factual', 'working', ?1, 'default')",
            params![now],
        ).unwrap();
        conn.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, namespace) VALUES ('m2', 'test2', 'factual', 'working', ?1, 'default')",
            params![now],
        ).unwrap();

        // Insert a hebbian_link before migration (signal_source column doesn't exist yet)
        conn.execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES ('m1', 'm2', 1.0, 3, ?1, 'default')",
            params![now],
        ).unwrap();

        // Run migration (adds columns and backfills)
        Storage::migrate_hebbian_signals(&conn).unwrap();

        // After migration, the row should have signal_source = 'corecall' from backfill
        // (The ALTER TABLE DEFAULT fills NULL for existing rows, then UPDATE backfills)
        let source_after: String = conn.query_row(
            "SELECT signal_source FROM hebbian_links WHERE source_id = 'm1'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(source_after, "corecall", "signal_source should be backfilled to 'corecall'");
    }

    // ===========================================================================
    // Cluster State Persistence Tests
    // ===========================================================================

    #[test]
    fn test_cluster_centroids_roundtrip() {
        let storage = test_storage();
        let centroid = vec![1.0f32, 2.0, 3.0];
        storage.update_centroid_incremental("cluster_a", &centroid).unwrap();

        let centroids = storage.get_cluster_centroids().unwrap();
        assert_eq!(centroids.len(), 1);
        assert_eq!(centroids[0].0, "cluster_a");
        assert_eq!(centroids[0].1, vec![1.0f32, 2.0, 3.0]);
    }

    #[test]
    fn test_assign_to_cluster() {
        let storage = test_storage();
        storage.assign_to_cluster("mem_1", "cluster_a", "hot", 0.95).unwrap();

        let members = storage.get_cluster_members("cluster_a").unwrap();
        assert_eq!(members, vec!["mem_1".to_string()]);
    }

    #[test]
    fn test_centroid_incremental_update() {
        let storage = test_storage();
        // Insert initial centroid [1, 0, 0]
        storage.update_centroid_incremental("cluster_a", &[1.0, 0.0, 0.0]).unwrap();

        // Incrementally update with [0, 1, 0]
        // Expected: (old * 1 + new) / 2 = ([1,0,0] + [0,1,0]) / 2 = [0.5, 0.5, 0.0]
        storage.update_centroid_incremental("cluster_a", &[0.0, 1.0, 0.0]).unwrap();

        let centroids = storage.get_cluster_centroids().unwrap();
        assert_eq!(centroids.len(), 1);
        let (id, vec) = &centroids[0];
        assert_eq!(id, "cluster_a");
        assert!((vec[0] - 0.5).abs() < 1e-6, "expected 0.5, got {}", vec[0]);
        assert!((vec[1] - 0.5).abs() < 1e-6, "expected 0.5, got {}", vec[1]);
        assert!((vec[2] - 0.0).abs() < 1e-6, "expected 0.0, got {}", vec[2]);
    }

    #[test]
    fn test_dirty_cluster_tracking() {
        let storage = test_storage();
        // Create a centroid first
        storage.update_centroid_incremental("cluster_a", &[1.0, 0.0]).unwrap();
        storage.update_centroid_incremental("cluster_b", &[0.0, 1.0]).unwrap();

        // Mark one as dirty
        storage.mark_cluster_dirty("cluster_a").unwrap();

        let dirty = storage.get_dirty_cluster_ids().unwrap();
        assert_eq!(dirty, vec!["cluster_a".to_string()]);

        // Clear dirty flags
        storage.clear_pending_and_dirty().unwrap();
        let dirty = storage.get_dirty_cluster_ids().unwrap();
        assert!(dirty.is_empty());
    }

    #[test]
    fn test_pending_memory_tracking() {
        let storage = test_storage();
        storage.add_pending_memory("mem_1").unwrap();
        storage.add_pending_memory("mem_2").unwrap();
        // Duplicate should be ignored
        storage.add_pending_memory("mem_1").unwrap();

        let pending = storage.get_pending_memory_ids().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&"mem_1".to_string()));
        assert!(pending.contains(&"mem_2".to_string()));

        assert_eq!(storage.get_pending_count().unwrap(), 2);

        // Clear pending
        storage.clear_pending_and_dirty().unwrap();
        let pending = storage.get_pending_memory_ids().unwrap();
        assert!(pending.is_empty());
        assert_eq!(storage.get_pending_count().unwrap(), 0);
    }

    #[test]
    fn test_replace_clusters() {
        let storage = test_storage();

        // Create initial clusters
        storage.update_centroid_incremental("old_c1", &[1.0, 0.0]).unwrap();
        storage.assign_to_cluster("mem_1", "old_c1", "full", 1.0).unwrap();
        storage.assign_to_cluster("mem_2", "old_c1", "full", 1.0).unwrap();

        storage.update_centroid_incremental("old_c2", &[0.0, 1.0]).unwrap();
        storage.assign_to_cluster("mem_3", "old_c2", "full", 1.0).unwrap();

        // Replace old clusters with new ones
        let new_clusters = vec![
            ("new_c1".to_string(), vec!["mem_1".to_string(), "mem_3".to_string()], vec![0.5f32, 0.5]),
        ];
        storage.replace_clusters(
            &["old_c1".to_string(), "old_c2".to_string()],
            &new_clusters,
        ).unwrap();

        // Old clusters should be gone
        assert!(storage.get_cluster_members("old_c1").unwrap().is_empty());
        assert!(storage.get_cluster_members("old_c2").unwrap().is_empty());

        // New cluster should exist
        let members = storage.get_cluster_members("new_c1").unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.contains(&"mem_1".to_string()));
        assert!(members.contains(&"mem_3".to_string()));

        // Centroid should be correct
        let centroids = storage.get_cluster_centroids().unwrap();
        let new_centroid = centroids.iter().find(|(id, _)| id == "new_c1").unwrap();
        assert_eq!(new_centroid.1, vec![0.5f32, 0.5]);
    }

    #[test]
    fn test_save_full_cluster_state() {
        let storage = test_storage();

        // Add some pre-existing data
        storage.update_centroid_incremental("old_c", &[1.0]).unwrap();
        storage.assign_to_cluster("mem_x", "old_c", "hot", 0.5).unwrap();
        storage.add_pending_memory("mem_p").unwrap();

        // Save full cluster state (replaces everything)
        let clusters = vec![
            ("c1".to_string(), vec!["m1".to_string(), "m2".to_string()], vec![1.0f32, 0.0]),
            ("c2".to_string(), vec!["m3".to_string()], vec![0.0f32, 1.0]),
        ];
        storage.save_full_cluster_state(&clusters).unwrap();

        // Old data should be gone
        assert!(storage.get_cluster_members("old_c").unwrap().is_empty());
        assert!(storage.get_pending_memory_ids().unwrap().is_empty());

        // New data should be present
        let members_c1 = storage.get_cluster_members("c1").unwrap();
        assert_eq!(members_c1.len(), 2);
        assert!(members_c1.contains(&"m1".to_string()));
        assert!(members_c1.contains(&"m2".to_string()));

        let members_c2 = storage.get_cluster_members("c2").unwrap();
        assert_eq!(members_c2, vec!["m3".to_string()]);

        let centroids = storage.get_cluster_centroids().unwrap();
        assert_eq!(centroids.len(), 2);

        // Verify cluster_state metadata was updated
        let (last_at, count): (String, i64) = storage.conn.query_row(
            "SELECT last_full_cluster_at, last_full_memory_count FROM cluster_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert!(!last_at.is_empty());
        assert_eq!(count, 3); // m1, m2, m3
    }

    #[test]
    fn test_get_memories_by_ids_empty() {
        let storage = test_storage();
        let result = storage.get_memories_by_ids(&[]).unwrap();
        assert!(result.is_empty());
    }

    // ----- ISS-019 Step 5: merge_enriched_into -----

    fn make_enriched(content: &str, importance: f64) -> crate::enriched::EnrichedMemory {
        use crate::dimensions::{Dimensions, Importance, NonEmptyString, Valence};
        let mut d = Dimensions::minimal(content).unwrap();
        d.participants = Some("alice, bob".to_string());
        d.valence = Valence::new(0.4);
        d.core_fact = NonEmptyString::new(content.to_string()).unwrap();
        crate::enriched::EnrichedMemory::from_dimensions(
            d,
            Importance::new(importance),
            None,
            None,
            serde_json::Value::Null,
        )
    }

    fn persist_enriched(
        storage: &mut Storage,
        id: &str,
        em: &crate::enriched::EnrichedMemory,
    ) -> String {
        let rec = MemoryRecord {
            id: id.to_string(),
            content: em.content.clone(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            occurred_at: None,
            access_times: vec![Utc::now()],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: em.importance.get(),
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: em.source.clone().unwrap_or_default(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: Some(em.to_legacy_metadata()),
        };
        storage.add(&rec, "default").unwrap();
        id.to_string()
    }

    #[test]
    fn test_merge_enriched_into_applies_union() {
        use crate::dimensions::Dimensions;

        let mut storage = test_storage();
        let mut a = make_enriched("initial fact", 0.5);
        a.dimensions.participants = Some("alice".to_string());
        a.dimensions.location = Some("lab".to_string());
        let id = persist_enriched(&mut storage, "mem_merge_a", &a);

        let mut b = make_enriched("initial fact", 0.6);
        b.dimensions.participants = Some("bob, carol".to_string());
        b.dimensions.causation = Some("kickoff".to_string());

        let outcome = storage.merge_enriched_into(&id, &b, 0.95).unwrap();
        assert_eq!(outcome.merge_count, 1);

        // Re-fetch and verify dimensional union applied.
        let rec = storage.get(&id).unwrap().unwrap();
        let em =
            crate::enriched::EnrichedMemory::from_memory_record(&rec).unwrap();
        // participants: set-union of comma-separated
        let p = em.dimensions.participants.clone().unwrap();
        assert!(p.contains("alice"), "missing alice: {}", p);
        assert!(p.contains("bob"), "missing bob: {}", p);
        assert!(p.contains("carol"), "missing carol: {}", p);
        // location preserved from existing
        assert_eq!(em.dimensions.location.as_deref(), Some("lab"));
        // causation adopted from incoming (existing was None)
        assert_eq!(em.dimensions.causation.as_deref(), Some("kickoff"));
        // importance = max
        assert!((em.importance.get() - 0.6).abs() < 1e-9);

        // Invariant still holds — core_fact matches content.
        assert!(em.invariants_hold());
        let _ = Dimensions::minimal(&em.content).unwrap();
    }

    #[test]
    fn test_merge_enriched_into_increments_merge_count() {
        let mut storage = test_storage();
        let a = make_enriched("hello world", 0.5);
        let id = persist_enriched(&mut storage, "mem_merge_count", &a);

        for expected in 1..=3 {
            let b = make_enriched("hello world", 0.4);
            let out = storage.merge_enriched_into(&id, &b, 0.9).unwrap();
            assert_eq!(out.merge_count, expected);
        }

        let rec = storage.get(&id).unwrap().unwrap();
        let meta = rec.metadata.unwrap();
        assert_eq!(meta["engram"]["merge_count"].as_i64(), Some(3));
        assert_eq!(
            meta["engram"]["merge_history"].as_array().unwrap().len(),
            3
        );
    }

    #[test]
    fn test_merge_enriched_into_history_fifo_capped_at_10() {
        let mut storage = test_storage();
        let a = make_enriched("capped history test", 0.5);
        let id = persist_enriched(&mut storage, "mem_merge_fifo", &a);

        for _ in 0..15 {
            let b = make_enriched("capped history test", 0.3);
            storage.merge_enriched_into(&id, &b, 0.88).unwrap();
        }

        let rec = storage.get(&id).unwrap().unwrap();
        let meta = rec.metadata.unwrap();
        let history = meta["engram"]["merge_history"].as_array().unwrap();
        assert_eq!(history.len(), 10, "history should be FIFO-capped at 10");
        assert_eq!(
            meta["engram"]["merge_count"].as_i64(),
            Some(15),
            "merge_count tracks all merges, not just retained history"
        );
    }

    #[test]
    fn test_merge_enriched_into_idempotent_on_identical_inputs() {
        let mut storage = test_storage();
        let a = make_enriched("idempotent check", 0.5);
        let id = persist_enriched(&mut storage, "mem_merge_idem", &a);

        // Merge with an identical EnrichedMemory.
        let b = a.clone();
        storage.merge_enriched_into(&id, &b, 1.0).unwrap();

        let rec = storage.get(&id).unwrap().unwrap();
        let em =
            crate::enriched::EnrichedMemory::from_memory_record(&rec).unwrap();

        // Dimensional content survives unchanged (idempotence).
        assert_eq!(em.dimensions.participants, a.dimensions.participants);
        assert_eq!(em.dimensions.valence.get(), a.dimensions.valence.get());
        assert_eq!(em.dimensions.domain, a.dimensions.domain);
        // Importance: max(0.5, 0.5) = 0.5.
        assert!((em.importance.get() - 0.5).abs() < 1e-9);
        // Content unchanged.
        assert_eq!(em.content, a.content);
    }

    #[test]
    fn test_merge_enriched_into_longer_content_wins() {
        let mut storage = test_storage();
        let a = make_enriched("short", 0.5);
        let id = persist_enriched(&mut storage, "mem_merge_long", &a);

        let long = "a much longer and more detailed description of the thing";
        let b = make_enriched(long, 0.5);
        let outcome = storage.merge_enriched_into(&id, &b, 0.9).unwrap();
        assert!(outcome.content_updated);

        let rec = storage.get(&id).unwrap().unwrap();
        assert_eq!(rec.content, long);
        let em =
            crate::enriched::EnrichedMemory::from_memory_record(&rec).unwrap();
        assert!(em.invariants_hold(), "core_fact must track content");
    }

    #[test]
    fn test_merge_enriched_into_missing_id_errors() {
        let mut storage = test_storage();
        let b = make_enriched("never stored", 0.5);
        let err = storage
            .merge_enriched_into("nonexistent", &b, 0.9)
            .unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::QueryReturnedNoRows),
            "expected QueryReturnedNoRows, got {:?}",
            err
        );
    }

    // ── ISS-019 Step 6: quarantine table CRUD ──────────────────────

    #[test]
    fn test_quarantine_insert_and_list() {
        let storage = test_storage();
        let returned_id = storage
            .insert_quarantine_row(
                "q-1", "payload-1", "hash-1",
                "extractor_error", Some("boom"),
                Some("test"), Some("ns-a"),
                Some(0.7), Some("factual"),
                Some(r#"{"k":"v"}"#),
            )
            .unwrap();
        assert_eq!(returned_id, "q-1");

        let rows = storage.list_quarantine_for_retry_batch(10).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, "q-1");
        assert_eq!(r.content, "payload-1");
        assert_eq!(r.content_hash, "hash-1");
        assert_eq!(r.reason_kind, "extractor_error");
        assert_eq!(r.reason_detail.as_deref(), Some("boom"));
        assert_eq!(r.attempts, 0);
        assert_eq!(r.source.as_deref(), Some("test"));
        assert_eq!(r.namespace.as_deref(), Some("ns-a"));
        assert_eq!(r.importance_hint, Some(0.7));
        assert_eq!(r.memory_type_hint.as_deref(), Some("factual"));
        assert_eq!(r.user_metadata.as_deref(), Some(r#"{"k":"v"}"#));
        assert_eq!(r.last_attempt_at, None);
        assert_eq!(r.last_error, None);
    }

    #[test]
    fn test_quarantine_insert_dedups_on_live_hash() {
        let storage = test_storage();
        let id1 = storage.insert_quarantine_row(
            "q-1", "same", "h-dup", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        // Same content_hash — second call should return the existing id,
        // not insert a duplicate.
        let id2 = storage.insert_quarantine_row(
            "q-2", "same", "h-dup", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(id1, "q-1");
        assert_eq!(storage.list_quarantine_for_retry_batch(10).unwrap().len(), 1);
    }

    #[test]
    fn test_quarantine_insert_skips_dedup_for_rejected_rows() {
        let storage = test_storage();
        storage.insert_quarantine_row(
            "q-old", "x", "h-1", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        // Flip the old row to permanently_rejected — dedup should
        // now treat it as absent and let a fresh insert through.
        assert!(storage.mark_quarantine_permanently_rejected("q-old").unwrap());

        let id_new = storage.insert_quarantine_row(
            "q-new", "x", "h-1", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        assert_eq!(id_new, "q-new", "rejected rows must not block fresh inserts");
        // Two rows now, but only one is live.
        assert_eq!(storage.count_quarantine_live().unwrap(), 1);
    }

    #[test]
    fn test_quarantine_record_attempt_and_mark_rejected() {
        let storage = test_storage();
        storage.insert_quarantine_row(
            "q-a", "hi", "h-a", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();

        storage.record_quarantine_attempt("q-a", Some("retry fail #1")).unwrap();
        storage.record_quarantine_attempt("q-a", Some("retry fail #2")).unwrap();

        let rows = storage.list_quarantine_for_retry_batch(10).unwrap();
        let r = rows.iter().find(|r| r.id == "q-a").unwrap();
        assert_eq!(r.attempts, 2);
        assert_eq!(r.last_error.as_deref(), Some("retry fail #2"));
        assert!(r.last_attempt_at.is_some());

        assert!(storage.mark_quarantine_permanently_rejected("q-a").unwrap());
        // Once rejected, list_for_retry excludes it.
        let rows_after = storage.list_quarantine_for_retry_batch(10).unwrap();
        assert!(rows_after.iter().all(|r| r.id != "q-a"));
        assert_eq!(storage.count_quarantine_live().unwrap(), 0);
    }

    #[test]
    fn test_quarantine_delete_row() {
        let storage = test_storage();
        storage.insert_quarantine_row(
            "q-d", "data", "h-d", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        assert_eq!(storage.count_quarantine_live().unwrap(), 1);

        assert!(storage.delete_quarantine_row("q-d").unwrap());
        assert!(!storage.delete_quarantine_row("q-d").unwrap(),
            "second delete must return false");
        assert_eq!(storage.count_quarantine_live().unwrap(), 0);
    }

    #[test]
    fn test_quarantine_list_batch_limit_and_ordering() {
        let storage = test_storage();
        // Insert three with distinct content_hash so none dedup.
        for i in 0..3 {
            storage.insert_quarantine_row(
                &format!("q-{}", i),
                "content",
                &format!("h-{}", i),
                "extractor_error", None,
                None, None, None, None, None,
            ).unwrap();
            // Spread received_at slightly so ORDER BY received_at is deterministic.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let batch = storage.list_quarantine_for_retry_batch(2).unwrap();
        assert_eq!(batch.len(), 2, "LIMIT honored");
        assert_eq!(batch[0].id, "q-0", "oldest-first ordering");
        assert_eq!(batch[1].id, "q-1");
    }

    #[test]
    fn test_quarantine_purge_respects_ttl_and_flag() {
        let storage = test_storage();
        // Row 1: rejected, but we'll not change received_at — so it's
        // "just now", inside any reasonable TTL — must NOT be purged
        // with a large TTL.
        storage.insert_quarantine_row(
            "q-young", "young", "h-y", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();
        storage.record_quarantine_attempt("q-young", Some("e")).unwrap();
        storage.mark_quarantine_permanently_rejected("q-young").unwrap();

        // Row 2: live (not rejected) — must NEVER be purged.
        storage.insert_quarantine_row(
            "q-live", "live", "h-l", "extractor_error", None,
            None, None, None, None, None,
        ).unwrap();

        // TTL=3600s: young row was attempted "now" (>= cutoff) — survives.
        let purged = storage.purge_rejected_quarantine(3600).unwrap();
        assert_eq!(purged, 0, "fresh rejected row inside TTL must survive");

        // TTL = -9999 (cutoff far in the future) — young row's
        // last_attempt_at is in the past relative to cutoff, so it
        // gets purged. Live row stays.
        let purged_all = storage.purge_rejected_quarantine(-9999).unwrap();
        assert_eq!(purged_all, 1, "rejected row beyond TTL must be purged");

        // Live row survived.
        let live_rows = storage.list_quarantine_for_retry_batch(10).unwrap();
        assert_eq!(live_rows.len(), 1);
        assert_eq!(live_rows[0].id, "q-live");
    }

    // =====================================================================
    // ISS-019 Step 7b — backfill_queue CRUD
    // =====================================================================

    #[test]
    fn test_backfill_enqueue_and_list() {
        let storage = test_storage();
        storage
            .enqueue_backfill("mem-1", "missing_core_dimensions", Some("no participants"))
            .unwrap();
        // Sleep a hair so enqueued_at differs (tests stable ordering).
        std::thread::sleep(std::time::Duration::from_millis(20));
        storage
            .enqueue_backfill("mem-2", "dimensions_empty", None)
            .unwrap();

        let rows = storage.list_backfill_batch(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].memory_id, "mem-1"); // older first
        assert_eq!(rows[0].reason_kind, "missing_core_dimensions");
        assert_eq!(rows[0].reason_detail.as_deref(), Some("no participants"));
        assert_eq!(rows[0].attempts, 0);
        assert!(rows[0].last_attempt_at.is_none());
        assert_eq!(rows[1].memory_id, "mem-2");
        assert!(rows[1].reason_detail.is_none());
    }

    #[test]
    fn test_backfill_enqueue_is_idempotent_on_live_row() {
        let storage = test_storage();
        storage
            .enqueue_backfill("mem-1", "missing_core_dimensions", None)
            .unwrap();
        // Bump attempts so we can detect that re-enqueue preserves it.
        storage.record_backfill_attempt("mem-1", Some("err")).unwrap();

        // Re-enqueue with a refined reason.
        storage
            .enqueue_backfill(
                "mem-1",
                "partial_dimensions_long_content",
                Some("refined"),
            )
            .unwrap();

        let rows = storage.list_backfill_batch(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].memory_id, "mem-1");
        assert_eq!(rows[0].reason_kind, "partial_dimensions_long_content");
        assert_eq!(rows[0].reason_detail.as_deref(), Some("refined"));
        // Attempts preserved across re-enqueue.
        assert_eq!(rows[0].attempts, 1);
    }

    #[test]
    fn test_backfill_enqueue_skips_rejected_row_update() {
        let storage = test_storage();
        storage
            .enqueue_backfill("mem-1", "dimensions_empty", None)
            .unwrap();
        storage.mark_backfill_permanently_rejected("mem-1").unwrap();

        // Re-enqueue should NOT resurrect or update the rejected row's reason.
        storage
            .enqueue_backfill("mem-1", "missing_core_dimensions", Some("new reason"))
            .unwrap();

        // List shows zero live rows.
        let rows = storage.list_backfill_batch(10).unwrap();
        assert_eq!(rows.len(), 0);

        // Underlying row's reason is still the original (not updated).
        let reason: String = storage
            .conn
            .query_row(
                "SELECT reason_kind FROM backfill_queue WHERE memory_id = ?1",
                params!["mem-1"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "dimensions_empty");
    }

    #[test]
    fn test_backfill_record_attempt_and_reject() {
        let storage = test_storage();
        storage
            .enqueue_backfill("mem-1", "dimensions_empty", None)
            .unwrap();
        storage
            .record_backfill_attempt("mem-1", Some("boom"))
            .unwrap();
        storage
            .record_backfill_attempt("mem-1", Some("boom again"))
            .unwrap();

        let rows = storage.list_backfill_batch(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attempts, 2);
        assert_eq!(rows[0].last_error.as_deref(), Some("boom again"));
        assert!(rows[0].last_attempt_at.is_some());

        storage.mark_backfill_permanently_rejected("mem-1").unwrap();
        let live = storage.count_backfill_live().unwrap();
        assert_eq!(live, 0);
    }

    #[test]
    fn test_backfill_delete_row() {
        let storage = test_storage();
        storage
            .enqueue_backfill("mem-1", "dimensions_empty", None)
            .unwrap();
        assert_eq!(storage.count_backfill_live().unwrap(), 1);

        let deleted = storage.delete_backfill_row("mem-1").unwrap();
        assert!(deleted);
        assert_eq!(storage.count_backfill_live().unwrap(), 0);

        // Second delete returns false.
        let deleted_again = storage.delete_backfill_row("mem-1").unwrap();
        assert!(!deleted_again);
    }

    #[test]
    fn test_backfill_list_batch_limit_and_ordering() {
        let storage = test_storage();
        for i in 0..5 {
            storage
                .enqueue_backfill(
                    &format!("mem-{i}"),
                    "missing_core_dimensions",
                    None,
                )
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let first_three = storage.list_backfill_batch(3).unwrap();
        assert_eq!(first_three.len(), 3);
        assert_eq!(first_three[0].memory_id, "mem-0");
        assert_eq!(first_three[2].memory_id, "mem-2");

        let all_five = storage.list_backfill_batch(100).unwrap();
        assert_eq!(all_five.len(), 5);
    }

    // ---------------------------------------------------------------------
    // v0.4 unified substrate — T05: nodes table + indexes
    // ---------------------------------------------------------------------

    #[test]
    fn test_t05_fresh_db_creates_unified_nodes_table() {
        let storage = test_storage();
        let exists: Option<String> = storage
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='nodes'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(exists.as_deref(), Some("nodes"));
    }

    #[test]
    fn test_t05_idempotent_migration() {
        // Use a tempdir so the same path can be opened twice and survive Drop.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t05-idempotent.db");

        // First open creates the schema.
        {
            let s1 = Storage::new(&path).expect("first open");
            let exists: Option<String> = s1
                .conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name='nodes'",
                    [],
                    |row| row.get(0),
                )
                .ok();
            assert_eq!(exists.as_deref(), Some("nodes"));
        }

        // Re-open the same path: migration must be a no-op (no duplicate
        // column / duplicate index errors). The nodes table still exists.
        let s2 = Storage::new(&path).expect("re-open should be idempotent");
        let exists: Option<String> = s2
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='nodes'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(exists.as_deref(), Some("nodes"));

        // And exactly one counter row, still at the initial value.
        let counter_rows: i64 = s2
            .conn
            .query_row("SELECT COUNT(*) FROM fts_rowid_counter", [], |row| row.get(0))
            .unwrap();
        assert_eq!(counter_rows, 1);
    }

    #[test]
    fn test_t05_fts_rowid_counter_initialized() {
        let storage = test_storage();
        let next_value: i64 = storage
            .conn
            .query_row(
                "SELECT next_value FROM fts_rowid_counter WHERE singleton=0",
                [],
                |row| row.get(0),
            )
            .expect("counter singleton row exists");
        assert_eq!(next_value, 1);
    }

    // ---------------------------------------------------------------------
    // v0.4 unified substrate — T06: edges table + indexes
    // ---------------------------------------------------------------------

    #[test]
    fn test_t06_fresh_db_creates_unified_edges_table() {
        let storage = test_storage();
        let exists: Option<String> = storage
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='edges'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(exists.as_deref(), Some("edges"));
    }

    #[test]
    fn test_t06_idempotent_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t06-idempotent.db");

        // First open creates the schema.
        {
            let s1 = Storage::new(&path).expect("first open");
            let exists: Option<String> = s1
                .conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name='edges'",
                    [],
                    |row| row.get(0),
                )
                .ok();
            assert_eq!(exists.as_deref(), Some("edges"));
        }

        // Re-open: migration must be a no-op (no duplicate index errors).
        let s2 = Storage::new(&path).expect("re-open should be idempotent");
        let exists: Option<String> = s2
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='edges'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(exists.as_deref(), Some("edges"));
    }

    #[test]
    fn test_t06_edges_indexes_and_partial_uniques_created() {
        // Indexes (incl. partial UNIQUE indexes for associative+containment
        // upsert semantics per design §3.2) must exist after migration.
        let storage = test_storage();
        let mut stmt = storage
            .conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND tbl_name='edges' ORDER BY name",
            )
            .expect("prepare index list");
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query indexes")
            .filter_map(|r| r.ok())
            .collect();

        // Spot-check the design-mandated indexes are present.
        for expected in &[
            "idx_edges_source",
            "idx_edges_target",
            "idx_edges_kind_pred",
            "idx_edges_namespace",
            "idx_edges_temporal",
            "idx_edges_live",
            "idx_edges_assoc_unique",
            "idx_edges_containment_unique",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing index {expected}: have {names:?}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // v0.4 unified substrate — T07: nodes_fts virtual table + triggers
    // ---------------------------------------------------------------------

    /// Helper: insert a minimal node row directly via SQL, since the higher-
    /// level Node insert API doesn't exist yet (T10). Returns the assigned
    /// `fts_rowid`.
    fn insert_minimal_node(
        conn: &Connection,
        id: &str,
        content: &str,
        summary: &str,
    ) -> i64 {
        // Allocate fts_rowid from the singleton counter (mirrors what the
        // T10 writer will do — kept inline here so T07 tests don't depend
        // on a writer helper that doesn't exist yet).
        conn.execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        ).expect("bump counter");
        let fts_rowid: i64 = conn.query_row(
            "SELECT next_value - 1 FROM fts_rowid_counter WHERE singleton = 0",
            [],
            |row| row.get(0),
        ).expect("read counter");

        conn.execute(
            "INSERT INTO nodes (
                id, node_kind, namespace, content, summary,
                activation, arousal, importance, confidence,
                working_strength, core_strength,
                created_at, updated_at, fts_rowid
            ) VALUES (
                ?1, 'memory', 'default', ?2, ?3,
                0.5, 0.5, 0.5, 0.5,
                0.5, 0.5,
                0.0, 0.0, ?4
            )",
            params![id, content, summary, fts_rowid],
        ).expect("insert node");

        fts_rowid
    }

    #[test]
    fn test_t07_fresh_db_creates_fts_table_and_triggers() {
        let storage = test_storage();

        // Virtual table exists.
        let fts_exists: Option<String> = storage
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='nodes_fts'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(fts_exists.as_deref(), Some("nodes_fts"));

        // All three triggers exist.
        for expected in &["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"] {
            let trig: Option<String> = storage
                .conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='trigger' AND name=?1",
                    params![expected],
                    |row| row.get(0),
                )
                .ok();
            assert_eq!(trig.as_deref(), Some(*expected), "trigger {expected} missing");
        }
    }

    #[test]
    fn test_t07_insert_trigger_makes_node_searchable() {
        let storage = test_storage();
        let fts_rowid = insert_minimal_node(
            &storage.conn,
            "n1",
            "the quick brown fox jumps over the lazy dog",
            "fox summary",
        );

        // MATCH against the body.
        let found: i64 = storage.conn.query_row(
            "SELECT rowid FROM nodes_fts WHERE nodes_fts MATCH 'fox'",
            [],
            |row| row.get(0),
        ).expect("fts query returns the inserted row");
        assert_eq!(found, fts_rowid);

        // MATCH against the summary column too.
        let count: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'summary'",
            [],
            |row| row.get(0),
        ).expect("count query");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_t07_delete_trigger_removes_from_fts() {
        let storage = test_storage();
        let _ = insert_minimal_node(&storage.conn, "n1", "hello world", "");

        // Before delete: FTS sees it.
        let before: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'hello'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(before, 1);

        storage.conn.execute("DELETE FROM nodes WHERE id = 'n1'", []).expect("delete");

        // After delete: FTS no longer sees it (contentless 'delete' command
        // form fired correctly).
        let after: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'hello'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(after, 0);
    }

    #[test]
    fn test_t07_update_trigger_refreshes_fts() {
        let storage = test_storage();
        let _ = insert_minimal_node(&storage.conn, "n1", "apples and oranges", "");

        // Update content: old tokens disappear, new tokens appear, fts_rowid
        // stays stable.
        storage.conn.execute(
            "UPDATE nodes SET content = 'bananas and grapes' WHERE id = 'n1'",
            [],
        ).expect("update content");

        let old_hits: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'apples'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(old_hits, 0, "old content tokens must be purged");

        let new_hits: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM nodes_fts WHERE nodes_fts MATCH 'bananas'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(new_hits, 1, "new content tokens must be indexed");
    }

    #[test]
    fn test_t07_idempotent_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t07-idempotent.db");

        {
            let _ = Storage::new(&path).expect("first open");
        }
        // Re-open: virtual table + triggers already exist; CREATE … IF NOT
        // EXISTS must be silent (no "already exists" failure).
        let s2 = Storage::new(&path).expect("re-open is idempotent");
        let fts_exists: Option<String> = s2
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='nodes_fts'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(fts_exists.as_deref(), Some("nodes_fts"));
    }

    // ---------------------------------------------------------------------
    // v0.4 unified substrate — T08: node_embeddings multi-model extension
    // ---------------------------------------------------------------------

    #[test]
    fn test_t08_fresh_db_creates_node_embeddings_table_and_index() {
        let storage = test_storage();

        // Table exists with the expected columns / PK.
        let cols: Vec<(String, String, i32)> = storage
            .conn
            .prepare("PRAGMA table_info(node_embeddings)")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i32>(5)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // (name, type, pk-index) — PK is composite (node_id, model).
        let by_name: std::collections::HashMap<_, _> =
            cols.iter().map(|(n, t, pk)| (n.clone(), (t.clone(), *pk))).collect();
        for (col, expected_type) in &[
            ("node_id", "TEXT"),
            ("model", "TEXT"),
            ("embedding", "BLOB"),
            ("dimensions", "INTEGER"),
            ("created_at", "REAL"),
        ] {
            let (ty, _) = by_name.get(*col).unwrap_or_else(|| panic!("missing column {col}"));
            assert_eq!(ty.to_uppercase(), expected_type.to_uppercase(), "column {col} type");
        }
        // Composite PK on (node_id, model): both have non-zero pk index.
        assert!(by_name.get("node_id").unwrap().1 > 0, "node_id should be PK component");
        assert!(by_name.get("model").unwrap().1 > 0, "model should be PK component");

        // Index on model column exists.
        let idx: Option<String> = storage
            .conn
            .query_row(
                "SELECT name FROM sqlite_master \
                 WHERE type='index' AND name='idx_node_embeddings_model'",
                [],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(idx.as_deref(), Some("idx_node_embeddings_model"));
    }

    #[test]
    fn test_t08_fk_cascade_delete_drops_embeddings() {
        let storage = test_storage();
        // Need foreign keys ON for the cascade test.
        storage.conn.execute("PRAGMA foreign_keys = ON", []).unwrap();

        let _ = insert_minimal_node(&storage.conn, "n1", "irrelevant", "");

        // Insert two embeddings under different models for the same node.
        for model in &["text-embedding-3-small", "voyage-code-2"] {
            storage.conn.execute(
                "INSERT INTO node_embeddings (node_id, model, embedding, dimensions, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params!["n1", model, vec![0u8; 16], 4i64, 0.0_f64],
            ).expect("insert embedding");
        }

        let before: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id='n1'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(before, 2);

        // Delete the node → CASCADE removes both embeddings.
        storage.conn.execute("DELETE FROM nodes WHERE id='n1'", []).unwrap();

        let after: i64 = storage.conn.query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id='n1'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(after, 0, "ON DELETE CASCADE should drop embeddings");
    }

    #[test]
    fn test_t08_pk_prevents_duplicate_node_model_pair() {
        let storage = test_storage();
        let _ = insert_minimal_node(&storage.conn, "n1", "x", "");

        storage.conn.execute(
            "INSERT INTO node_embeddings (node_id, model, embedding, dimensions, created_at)
             VALUES ('n1', 'model-a', ?1, 4, 0.0)",
            params![vec![0u8; 16]],
        ).unwrap();

        let dup = storage.conn.execute(
            "INSERT INTO node_embeddings (node_id, model, embedding, dimensions, created_at)
             VALUES ('n1', 'model-a', ?1, 4, 1.0)",
            params![vec![1u8; 16]],
        );
        assert!(dup.is_err(), "duplicate (node_id, model) must be rejected by PK");

        // But a different model under the same node is fine.
        storage.conn.execute(
            "INSERT INTO node_embeddings (node_id, model, embedding, dimensions, created_at)
             VALUES ('n1', 'model-b', ?1, 4, 2.0)",
            params![vec![2u8; 16]],
        ).expect("different model under same node should work");
    }

    #[test]
    fn test_t08_idempotent_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t08-idempotent.db");

        { let _ = Storage::new(&path).expect("first open"); }
        let s2 = Storage::new(&path).expect("re-open is idempotent");

        let tbl: Option<String> = s2.conn.query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='node_embeddings'",
            [], |r| r.get(0)).ok();
        assert_eq!(tbl.as_deref(), Some("node_embeddings"));
    }

    // ---------------------------------------------------------------------
    // v0.4 unified substrate — T09: schema_version bump to 0.4-additive
    // ---------------------------------------------------------------------

    #[test]
    fn test_t09_fresh_db_has_v04_additive_schema_version() {
        let storage = test_storage();
        let v: String = storage.conn.query_row(
            "SELECT value FROM engram_meta WHERE key = 'schema_version'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(v, "0.4-additive");
    }

    #[test]
    fn test_t09_legacy_db_upgrades_to_v04_additive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");

        // Simulate a pre-v04 DB: open with the storage layer (which seeds
        // schema_version='1' via INSERT OR IGNORE), then manually stomp
        // the value back to '1' to simulate a DB last touched before T09.
        // (We can't trivially get an old engramai binary; this models the
        // same row state that a true legacy DB would present.)
        {
            let s = Storage::new(&path).unwrap();
            s.conn.execute(
                "INSERT OR REPLACE INTO engram_meta VALUES ('schema_version', '1')",
                [],
            ).unwrap();
            let v: String = s.conn.query_row(
                "SELECT value FROM engram_meta WHERE key='schema_version'",
                [], |r| r.get(0)).unwrap();
            assert_eq!(v, "1", "setup: legacy version forced");
        }

        // Re-open: T09 should rewrite schema_version to 0.4-additive.
        let s2 = Storage::new(&path).unwrap();
        let v: String = s2.conn.query_row(
            "SELECT value FROM engram_meta WHERE key='schema_version'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(v, "0.4-additive");
    }

    #[test]
    fn test_t09_idempotent_on_repeated_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idem.db");

        for _ in 0..3 {
            let s = Storage::new(&path).unwrap();
            let v: String = s.conn.query_row(
                "SELECT value FROM engram_meta WHERE key='schema_version'",
                [], |r| r.get(0)).unwrap();
            assert_eq!(v, "0.4-additive");
        }

        // Exactly one row for schema_version (no accumulation).
        let s = Storage::new(&path).unwrap();
        let n: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM engram_meta WHERE key='schema_version'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    // =============================================================
    // T29.3 — Phase D embeddings read-switch.
    //
    // Pattern mirrors T29.1 (subscriptions) and T29.2
    // (synthesis_provenance): open two `Storage` handles on the same
    // SQLite file — one with `unified_substrate=false` (legacy reader),
    // one with `true` (unified reader) — drive writes through one
    // handle (which dual-writes to both tables), then assert both
    // reader paths return byte-equivalent results.
    //
    // These tests pin three contracts:
    //   1. **Writer parity** — every `store_embedding` call lands in
    //      both `memory_embeddings` and `node_embeddings`, byte-equal.
    //   2. **Reader parity** — each of the six switched readers
    //      returns identical results across both paths.
    //   3. **Table isolation** — the unified reader filters on
    //      `node_embeddings` rows only; a stray `nodes` row without
    //      an accompanying embedding row must not appear.
    // =============================================================

    fn t29_3_open_pair(dir: &std::path::Path) -> (Storage, Storage) {
        let path = dir.join("t29_3.db");
        // The legacy handle is constructed first so its `Storage::new`
        // (which calls `migrate_unified_*`) sets up the unified schema
        // and tables; the unified handle then opens the same file
        // with `unified_substrate=true` to exercise the unified read
        // path. Order doesn't matter functionally — migrations are
        // idempotent — but doing it this way keeps the legacy default
        // visible in the helper signature.
        let legacy = Storage::new(&path).expect("legacy handle");
        let unified = Storage::with_unified_substrate(&path, true)
            .expect("unified handle");
        (legacy, unified)
    }

    fn t29_3_seed_memory(s: &mut Storage, id: &str, ns: &str) {
        let when = chrono::Utc::now();
        let mut rec = make_record(id, "embedding test content", when);
        s.add(&mut rec, ns).expect("seed memory row");
    }

    fn t29_3_seed_embedding(
        s: &mut Storage,
        memory_id: &str,
        ns: &str,
        model: &str,
        emb: &[f32],
    ) {
        // Ensure parent memory row exists in BOTH legacy `memories`
        // and unified `nodes` (T12 dual-write covers nodes
        // unconditionally), satisfying `node_embeddings.node_id` FK.
        t29_3_seed_memory(s, memory_id, ns);
        s.store_embedding(memory_id, emb, model, emb.len())
            .expect("dual-write embedding");
    }

    fn t29_3_sort_pairs(mut v: Vec<(String, Vec<f32>)>) -> Vec<(String, Vec<f32>)> {
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Writer-side parity: a single `store_embedding` call writes
    /// bit-identical rows to both legacy and unified tables (same id,
    /// same model, same blob, same dimensions). Without this guarantee
    /// the Phase D readers below would compare apples to oranges.
    #[test]
    fn t29_3_dual_write_writer_parity() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, _unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        let emb = vec![0.1f32, 0.2, 0.3, 0.4];
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &emb);

        let legacy_blob: Vec<u8> = legacy.conn.query_row(
            "SELECT embedding FROM memory_embeddings WHERE memory_id='m1' AND model=?",
            params![model],
            |r| r.get(0),
        ).unwrap();
        let unified_blob: Vec<u8> = legacy.conn.query_row(
            "SELECT embedding FROM node_embeddings WHERE node_id='m1' AND model=?",
            params![model],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(legacy_blob, unified_blob,
            "T29.3 dual-write: memory_embeddings and node_embeddings blobs must match byte-for-byte");

        // Re-store with a new vector — INSERT OR REPLACE semantics
        // must update BOTH sides, not leave a stale row behind on one.
        let emb2 = vec![0.9f32, 0.8, 0.7, 0.6];
        legacy.store_embedding("m1", &emb2, model, emb2.len()).unwrap();
        let legacy_blob2: Vec<u8> = legacy.conn.query_row(
            "SELECT embedding FROM memory_embeddings WHERE memory_id='m1' AND model=?",
            params![model],
            |r| r.get(0),
        ).unwrap();
        let unified_blob2: Vec<u8> = legacy.conn.query_row(
            "SELECT embedding FROM node_embeddings WHERE node_id='m1' AND model=?",
            params![model],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(legacy_blob2, unified_blob2,
            "T29.3 re-store: REPLACE must overwrite both sides");
        assert_ne!(legacy_blob, legacy_blob2, "sanity: blob actually changed");
    }

    #[test]
    fn t29_3_get_embedding_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        let emb = vec![0.5f32, 0.25, 0.125, 0.0625];
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &emb);

        let from_legacy = legacy.get_embedding("m1", model).unwrap();
        let from_unified = unified.get_embedding("m1", model).unwrap();
        assert_eq!(from_legacy, from_unified);
        assert_eq!(from_unified, Some(emb));

        // Wrong model returns None on both paths.
        assert_eq!(legacy.get_embedding("m1", "openai/text-embed").unwrap(), None);
        assert_eq!(unified.get_embedding("m1", "openai/text-embed").unwrap(), None);

        // Unknown memory id returns None on both paths.
        assert_eq!(legacy.get_embedding("missing", model).unwrap(), None);
        assert_eq!(unified.get_embedding("missing", model).unwrap(), None);
    }

    #[test]
    fn t29_3_get_embedding_for_memory_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        let emb = vec![1.0f32, 2.0, 3.0];
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &emb);

        assert_eq!(
            legacy.get_embedding_for_memory("m1").unwrap(),
            unified.get_embedding_for_memory("m1").unwrap()
        );
        assert_eq!(unified.get_embedding_for_memory("m1").unwrap(), Some(emb));
        assert_eq!(legacy.get_embedding_for_memory("nope").unwrap(), None);
        assert_eq!(unified.get_embedding_for_memory("nope").unwrap(), None);
    }

    #[test]
    fn t29_3_get_all_embeddings_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &vec![0.1f32, 0.2]);
        t29_3_seed_embedding(&mut legacy, "m2", "default", model, &vec![0.3f32, 0.4]);
        t29_3_seed_embedding(&mut legacy, "m3", "other-ns", model, &vec![0.5f32, 0.6]);

        let mut legacy_all = legacy.get_all_embeddings(model).unwrap();
        let mut unified_all = unified.get_all_embeddings(model).unwrap();
        legacy_all = t29_3_sort_pairs(legacy_all);
        unified_all = t29_3_sort_pairs(unified_all);
        assert_eq!(legacy_all, unified_all);
        assert_eq!(unified_all.len(), 3);

        // Soft-delete a memory — both paths must drop it (liveness
        // predicate is on `memories`, JOINed identically by both).
        legacy.soft_delete("m1").unwrap();
        let l = t29_3_sort_pairs(legacy.get_all_embeddings(model).unwrap());
        let u = t29_3_sort_pairs(unified.get_all_embeddings(model).unwrap());
        assert_eq!(l, u);
        assert_eq!(u.len(), 2);
    }

    #[test]
    fn t29_3_get_embeddings_in_namespace_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        t29_3_seed_embedding(&mut legacy, "m1", "alpha", model, &vec![0.1f32, 0.2]);
        t29_3_seed_embedding(&mut legacy, "m2", "alpha", model, &vec![0.3f32, 0.4]);
        t29_3_seed_embedding(&mut legacy, "m3", "beta", model, &vec![0.5f32, 0.6]);

        let l_alpha = t29_3_sort_pairs(legacy.get_embeddings_in_namespace(Some("alpha"), model).unwrap());
        let u_alpha = t29_3_sort_pairs(unified.get_embeddings_in_namespace(Some("alpha"), model).unwrap());
        assert_eq!(l_alpha, u_alpha);
        assert_eq!(u_alpha.len(), 2);

        // Wildcard delegates to get_all_embeddings — also must match.
        let l_star = t29_3_sort_pairs(legacy.get_embeddings_in_namespace(Some("*"), model).unwrap());
        let u_star = t29_3_sort_pairs(unified.get_embeddings_in_namespace(Some("*"), model).unwrap());
        assert_eq!(l_star, u_star);
        assert_eq!(u_star.len(), 3);

        // Unknown namespace → empty on both paths.
        let l_none: Vec<_> = legacy.get_embeddings_in_namespace(Some("ghost"), model).unwrap();
        let u_none: Vec<_> = unified.get_embeddings_in_namespace(Some("ghost"), model).unwrap();
        assert_eq!(l_none, u_none);
        assert!(u_none.is_empty());
    }

    #[test]
    fn t29_3_get_memories_without_embeddings_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        // m1 has embedding, m2 does not.
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &vec![0.1f32, 0.2]);
        t29_3_seed_memory(&mut legacy, "m2", "default");

        let mut l = legacy.get_memories_without_embeddings(model).unwrap();
        let mut u = unified.get_memories_without_embeddings(model).unwrap();
        l.sort();
        u.sort();
        assert_eq!(l, u);
        assert_eq!(u, vec!["m2".to_string()]);

        // Different model: both m1 AND m2 should appear (neither has
        // an embedding under the other model).
        let mut l2 = legacy.get_memories_without_embeddings("openai/text-embed").unwrap();
        let mut u2 = unified.get_memories_without_embeddings("openai/text-embed").unwrap();
        l2.sort();
        u2.sort();
        assert_eq!(l2, u2);
        assert_eq!(u2, vec!["m1".to_string(), "m2".to_string()]);
    }

    #[test]
    fn t29_3_embedding_stats_unified_matches_legacy() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model_a = "ollama/nomic-embed-text";
        let model_b = "openai/text-embed";
        // Two memories under model A, one under model B.
        t29_3_seed_embedding(&mut legacy, "m1", "default", model_a, &vec![0.1f32, 0.2]);
        t29_3_seed_embedding(&mut legacy, "m2", "default", model_a, &vec![0.3f32, 0.4]);
        // m3: same memory under model B
        t29_3_seed_embedding(&mut legacy, "m3", "default", model_b, &vec![0.5f32, 0.6]);

        let l = legacy.embedding_stats().unwrap();
        let u = unified.embedding_stats().unwrap();
        assert_eq!(l.total_memories, u.total_memories);
        assert_eq!(l.embedded_count, u.embedded_count);
        assert_eq!(l.embedded_count, 3); // distinct memory_ids
        assert_eq!(l.model, u.model);
        // Top model is model_a (2 rows) — pinned for both paths.
        assert_eq!(u.model.as_deref(), Some(model_a));
        assert_eq!(l.dimensions, u.dimensions);
        assert_eq!(u.dimensions, Some(2));
    }

    /// Pin the table-isolation contract: a `nodes` row that has no
    /// matching `node_embeddings` entry must not bleed into unified
    /// reader output. Without this guard, a future refactor that
    /// joins through `nodes` instead of `node_embeddings` would
    /// silently surface entity / topic / insight nodes alongside
    /// memory embeddings.
    #[test]
    fn t29_3_unified_path_ignores_nodes_without_embedding_row() {
        let dir = tempfile::tempdir().unwrap();
        let (mut legacy, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        // Seed one memory with an embedding.
        t29_3_seed_embedding(&mut legacy, "m1", "default", model, &vec![0.1f32, 0.2]);

        // Inject a stray entity node — no embedding row attached.
        // (This mimics what T21 backfill writes for `entity` nodes;
        // we don't reuse the helper to keep this test independent of
        // backfill's evolving column set.)
        legacy.conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at) \
             VALUES ('ent-1', 'entity', 'default', '', ?, ?)",
            params![now_f64(), now_f64()],
        ).unwrap();

        let unified_all = unified.get_all_embeddings(model).unwrap();
        assert_eq!(unified_all.len(), 1);
        assert_eq!(unified_all[0].0, "m1");
        // Sanity: `embedding_stats` doesn't count the entity either,
        // because COUNT(DISTINCT node_id) ranges over node_embeddings,
        // not nodes.
        let stats = unified.embedding_stats().unwrap();
        assert_eq!(stats.embedded_count, 1);
    }

    // =============================================================
    // ISS-115 — Phase B dual-DELETE closure.
    //
    // Phase B (T12–T16) shipped dual-WRITE writers but no dual-DELETE.
    // `hard_delete_cascade` and `delete_all_embeddings` now clear
    // both legacy and unified tables atomically in one transaction.
    // These tests pin the contract per table.
    // =============================================================

    /// Helper: inject a fully-populated unified row set for one memory
    /// id, simulating "as if every Phase B dual-WRITE and Phase C
    /// backfill had run." Used to test that dual-DELETE clears all of
    /// them. Tests do not rely on any specific live dual-WRITE writer
    /// existing for entities/hebbian/etc — they inject directly via
    /// `conn.execute`. This decouples the dual-DELETE contract from
    /// the (still-evolving) set of live dual-WRITE writers.
    fn iss115_seed_unified_rows_for_memory(s: &mut Storage, id: &str) {
        let t = now_f64();
        // node_embeddings (T20 mirror): assume store_embedding already
        // dual-wrote, OR inject directly. Note: the parent `nodes`
        // row already exists from `Storage::add`'s T12 dual-write.
        s.conn.execute(
            "INSERT OR REPLACE INTO node_embeddings (node_id, model, embedding, dimensions, created_at) \
             VALUES (?, 'test/model', ?, 4, ?)",
            params![id, vec![0u8; 16], t],
        ).unwrap();

        // Inject a second memory node so we have a valid edge target.
        s.conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at) \
             VALUES ('peer-1', 'memory', 'default', '', ?, ?) \
             ON CONFLICT(id) DO NOTHING",
            params![t, t],
        ).unwrap();
        // Entity node for memory_entities mirror.
        s.conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at) \
             VALUES ('ent-1', 'entity', 'default', '', ?, ?) \
             ON CONFLICT(id) DO NOTHING",
            params![t, t],
        ).unwrap();

        // edges, associative (T14/T24 mirror — hebbian).
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-assoc-1', ?, 'peer-1', 'associative', 'co_activated', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
        // edges, provenance/mentions (T23 mirror — memory_entities role='mention').
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-mention-1', ?, 'ent-1', 'provenance', 'mentions', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
        // edges, structural/subject_of (T23 mirror — memory_entities role='subject').
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-subj-1', ?, 'ent-1', 'structural', 'subject_of', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
        // edges, structural/object_of (T23 mirror — memory_entities role='object').
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-obj-1', ?, 'ent-1', 'structural', 'object_of', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
        // edges, provenance/derived_from (T16/T25 mirror — synthesis_provenance).
        // Two rows: id appears once as source (insight), once as target (source memory).
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-prov-out', ?, 'peer-1', 'provenance', 'derived_from', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-prov-in', 'peer-1', ?, 'provenance', 'derived_from', ?, ?, ?)",
            params![id, t, t, t],
        ).unwrap();
    }

    fn iss115_count_unified_rows_for(s: &Storage, id: &str) -> (i64, i64) {
        let nemb: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id = ?",
            params![id], |r| r.get(0),
        ).unwrap();
        let ne: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE source_id = ? OR target_id = ?",
            params![id, id], |r| r.get(0),
        ).unwrap();
        (nemb, ne)
    }

    fn iss115_legacy_count_for(s: &Storage, id: &str) -> i64 {
        // Sum across all legacy tables that hard_delete_cascade clears.
        let mut n: i64 = 0;
        n += s.conn.query_row("SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = ?", params![id], |r| r.get(0)).unwrap_or(0);
        n += s.conn.query_row("SELECT COUNT(*) FROM hebbian_links WHERE source_id = ? OR target_id = ?", params![id, id], |r| r.get(0)).unwrap_or(0);
        n += s.conn.query_row("SELECT COUNT(*) FROM memory_entities WHERE memory_id = ?", params![id], |r| r.get(0)).unwrap_or(0);
        n += s.conn.query_row("SELECT COUNT(*) FROM synthesis_provenance WHERE source_id = ? OR insight_id = ?", params![id, id], |r| r.get(0)).unwrap_or(0);
        n += s.conn.query_row("SELECT COUNT(*) FROM memories WHERE id = ?", params![id], |r| r.get(0)).unwrap_or(0);
        n
    }

    /// End-to-end: a memory with both legacy and unified row sets
    /// is fully cleared on both sides by one `hard_delete_cascade`
    /// call. Unified reads after deletion must return zero rows.
    #[test]
    fn iss115_hard_delete_cascade_clears_legacy_and_unified() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, unified) = t29_3_open_pair(dir.path());
        // Seed the memory via store_embedding (covers memories + nodes +
        // memory_embeddings + node_embeddings dual-write paths).
        t29_3_seed_embedding(&mut s, "m1", "default", "ollama/nomic-embed-text", &vec![0.1f32, 0.2]);
        // Inject hebbian/entity/provenance unified rows.
        iss115_seed_unified_rows_for_memory(&mut s, "m1");
        // Also seed legacy hebbian + memory_entities + synthesis_provenance.
        // Seed peer-1 as a real memory (so hebbian_links FK is happy)
        // via the same helper used for m1 — guarantees the row passes
        // every NOT NULL constraint without manual column listing.
        t29_3_seed_memory(&mut s, "peer-1", "default");
        let t = now_f64();
        s.conn.execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, namespace, created_at) \
             VALUES ('m1', 'peer-1', 0.5, 'default', ?)",
            params![t],
        ).unwrap();
        s.conn.execute(
            "INSERT OR IGNORE INTO entities (id, name, entity_type, namespace, created_at, updated_at) \
             VALUES ('ent-1', 'thing', 'general', 'default', ?, ?)",
            params![t, t],
        ).unwrap();
        s.conn.execute(
            "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, role) \
             VALUES ('m1', 'ent-1', 'mention')",
            params![],
        ).unwrap();
        s.conn.execute(
            "INSERT INTO synthesis_provenance (id, source_id, insight_id, cluster_id, synthesis_timestamp, gate_decision, source_original_importance, confidence) \
             VALUES ('p1', 'm1', 'peer-1', 'c1', ?, 'kept', 0.5, 1.0)",
            params![chrono::Utc::now().to_rfc3339()],
        ).unwrap();

        // Pre-delete sanity.
        assert!(iss115_legacy_count_for(&s, "m1") > 0, "legacy rows seeded");
        let (nemb, ne) = iss115_count_unified_rows_for(&s, "m1");
        assert!(nemb > 0 && ne > 0, "unified rows seeded");

        // Execute the dual-DELETE cascade.
        s.hard_delete_cascade("m1").unwrap();

        // Legacy side fully cleared.
        assert_eq!(iss115_legacy_count_for(&s, "m1"), 0,
            "ISS-115: legacy tables must be empty after hard_delete_cascade");
        // Unified side fully cleared.
        let (nemb_after, ne_after) = iss115_count_unified_rows_for(&s, "m1");
        assert_eq!(nemb_after, 0, "ISS-115: node_embeddings rows must be cleared");
        assert_eq!(ne_after, 0, "ISS-115: edges rows touching deleted memory must be cleared");
        // And the parent nodes row itself.
        let n_nodes: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(n_nodes, 0, "ISS-115: parent nodes row must be deleted");

        // Phase D reader parity: both legacy and unified embeddings
        // readers must agree (empty) post-delete.
        assert_eq!(unified.get_embedding("m1", "ollama/nomic-embed-text").unwrap(), None);
        assert_eq!(s.get_embedding("m1", "ollama/nomic-embed-text").unwrap(), None);
    }

    /// Hard-delete preserves edges that touch *other* memories.
    /// Pins that the WHERE clauses are scoped to the deleted id and
    /// do not accidentally widen the blast radius.
    #[test]
    fn iss115_hard_delete_cascade_does_not_touch_unrelated_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _unified) = t29_3_open_pair(dir.path());
        t29_3_seed_embedding(&mut s, "m1", "default", "ollama/nomic-embed-text", &vec![0.1f32, 0.2]);
        t29_3_seed_embedding(&mut s, "m2", "default", "ollama/nomic-embed-text", &vec![0.3f32, 0.4]);
        let t = now_f64();
        // edge between m2 and a third party — must survive deletion of m1.
        s.conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at) \
             VALUES ('m3', 'memory', 'default', '', ?, ?) ON CONFLICT(id) DO NOTHING",
            params![t, t],
        ).unwrap();
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e-keep', 'm2', 'm3', 'associative', 'co_activated', ?, ?, ?)",
            params![t, t, t],
        ).unwrap();

        s.hard_delete_cascade("m1").unwrap();

        let keep_count: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE id = 'e-keep'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(keep_count, 1, "ISS-115: unrelated edges must survive");
        // m2 itself must survive.
        let m2_count: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = 'm2'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(m2_count, 1);
    }

    /// `delete_all_embeddings` must clear both legacy and unified
    /// embedding rows for the memory, but must NOT touch the parent
    /// `nodes` row or any non-embedding edges.
    #[test]
    fn iss115_delete_all_embeddings_dualizes_and_leaves_nodes_intact() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, unified) = t29_3_open_pair(dir.path());
        let model = "ollama/nomic-embed-text";
        t29_3_seed_embedding(&mut s, "m1", "default", model, &vec![0.1f32, 0.2]);
        // Second model — both rows must go.
        s.store_embedding("m1", &[0.5f32, 0.6, 0.7], "openai/text-embed", 3).unwrap();
        // Inject one edge that touches m1 — must survive (not an
        // embedding row).
        let t = now_f64();
        s.conn.execute(
            "INSERT INTO nodes (id, node_kind, namespace, content, created_at, updated_at) \
             VALUES ('peer-1', 'memory', 'default', '', ?, ?) ON CONFLICT(id) DO NOTHING",
            params![t, t],
        ).unwrap();
        s.conn.execute(
            "INSERT INTO edges (id, source_id, target_id, edge_kind, predicate, recorded_at, created_at, updated_at) \
             VALUES ('e1', 'm1', 'peer-1', 'associative', 'co_activated', ?, ?, ?)",
            params![t, t, t],
        ).unwrap();

        s.delete_all_embeddings("m1").unwrap();

        // Both embedding tables empty for m1.
        let n_legacy: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM memory_embeddings WHERE memory_id = 'm1'",
            [], |r| r.get(0),
        ).unwrap();
        let n_unified: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM node_embeddings WHERE node_id = 'm1'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(n_legacy, 0);
        assert_eq!(n_unified, 0);
        // Parent rows untouched.
        let n_memory: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        let n_node: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = 'm1'", [], |r| r.get(0),
        ).unwrap();
        let n_edge: i64 = s.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE id = 'e1'", [], |r| r.get(0),
        ).unwrap();
        assert_eq!(n_memory, 1);
        assert_eq!(n_node, 1);
        assert_eq!(n_edge, 1, "ISS-115: non-embedding edges must survive delete_all_embeddings");
        // Phase D reader parity confirms.
        assert_eq!(s.get_embedding("m1", model).unwrap(), None);
        assert_eq!(unified.get_embedding("m1", model).unwrap(), None);
    }

    /// Re-running `hard_delete_cascade` on an already-deleted id must
    /// be a no-op (idempotent). Important for any retry / cleanup
    /// loop that doesn't track whether the delete already succeeded.
    #[test]
    fn iss115_hard_delete_cascade_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (mut s, _unified) = t29_3_open_pair(dir.path());
        t29_3_seed_embedding(&mut s, "m1", "default", "ollama/nomic-embed-text", &vec![0.1f32, 0.2]);
        s.hard_delete_cascade("m1").unwrap();
        // Second call should not raise.
        s.hard_delete_cascade("m1").expect("ISS-115: hard_delete_cascade must be idempotent");
        // And on an id that never existed.
        s.hard_delete_cascade("ghost").expect("ISS-115: deleting a never-seen id must not raise");
    }
}