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
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, rusqlite::Error> {
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

        // v0.3 graph layer schema (additive; never touches v0.2 tables).
        // Maps GraphError back to rusqlite::Error to keep this constructor's
        // return type stable.
        crate::graph::init_graph_tables(&conn).map_err(|e| match e {
            crate::graph::GraphError::Sqlite(inner) => inner,
            other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
        })?;

        Ok(Self { conn })
    }
    
    /// Get a reference to the underlying database connection.
    pub fn connection(&self) -> &Connection {
        &self.conn
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
                contradicts, contradicted_by, metadata, namespace
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        
        tx.commit()?;
        Ok(())
    }

    /// Get a memory by ID.
    pub fn get(&self, id: &str) -> Result<Option<MemoryRecord>, rusqlite::Error> {
        let access_times = self.get_access_times(id)?;
        
        self.conn
            .query_row(
                "SELECT * FROM memories WHERE id = ?",
                params![id],
                |row| self.row_to_record(row, access_times.clone()),
            )
            .optional()
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

    /// Get Hebbian neighbors for a memory.
    pub fn get_hebbian_neighbors(&self, memory_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT target_id FROM hebbian_links WHERE source_id = ? AND strength > 0"
        )?;
        
        let rows = stmt.query_map(params![memory_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Get Hebbian neighbors with their link weights.
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
    pub fn record_coactivation(
        &mut self,
        id1: &str,
        id2: &str,
        threshold: i32,
    ) -> Result<bool, rusqlite::Error> {
        let (id1, id2) = if id1 < id2 { (id1, id2) } else { (id2, id1) };
        
        // Check existing link
        let existing: Option<(f64, i32)> = self.conn
            .query_row(
                "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                params![id1, id2],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        
        match existing {
            Some((strength, _count)) if strength > 0.0 => {
                // Link already formed, strengthen it
                let new_strength = (strength + 0.1).min(1.0);
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id1, id2],
                )?;
                // Also update reverse link
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id2, id1],
                )?;
                Ok(false)
            }
            Some((_, count)) => {
                // Tracking phase, increment count
                let new_count = count + 1;
                if new_count >= threshold {
                    // Threshold reached, form link
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    // Create reverse link
                    self.conn.execute(
                        "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?, ?, 1.0, ?, ?)",
                        params![id2, id1, new_count, now_f64()],
                    )?;
                    Ok(true)
                } else {
                    self.conn.execute(
                        "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    Ok(false)
                }
            }
            None => {
                // First co-activation, create tracking record
                self.conn.execute(
                    "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at) VALUES (?, ?, 0.0, 1, ?)",
                    params![id1, id2, now_f64()],
                )?;
                Ok(false)
            }
        }
    }

    /// Decay all Hebbian links by a factor.
    pub fn decay_hebbian_links(&mut self, factor: f64) -> Result<usize, rusqlite::Error> {
        // Decay all links
        self.conn.execute(
            "UPDATE hebbian_links SET strength = strength * ? WHERE strength > 0",
            params![factor],
        )?;
        
        // Prune very weak links
        let pruned = self.conn.execute(
            "DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1",
            [],
        )?;
        
        Ok(pruned)
    }

    /// Transfer Hebbian links from donor to target during merge.
    /// - Repoints donor links to target
    /// - If link already exists on target, keeps max weight
    /// - Drops self-links (source==target after repoint)
    /// - Deletes all donor links after transfer
    pub fn merge_hebbian_links(
        &mut self,
        donor_id: &str,
        target_id: &str,
    ) -> Result<usize, rusqlite::Error> {
        // Get all links involving the donor
        let links = self.get_hebbian_links_weighted(donor_id)?;
        let mut transferred = 0;
        
        for (other_id, weight) in &links {
            // Skip self-links
            if other_id == target_id {
                continue;
            }
            
            // Check if target already has a link to this other memory
            let existing_weight: Option<f64> = self.conn.query_row(
                "SELECT strength FROM hebbian_links WHERE \
                 (source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1)",
                params![target_id, other_id],
                |row| row.get(0),
            ).optional()?;
            
            match existing_weight {
                Some(existing) => {
                    // Update to max weight
                    let max_weight = existing.max(*weight);
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = ?1 WHERE \
                         (source_id = ?2 AND target_id = ?3) OR (source_id = ?3 AND target_id = ?2)",
                        params![max_weight, target_id, other_id],
                    )?;
                }
                None => {
                    // Create new link from target to other
                    self.conn.execute(
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
        
        // Delete all donor links
        self.conn.execute(
            "DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1",
            params![donor_id],
        )?;
        
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
        // Apply differential decay rates based on signal_source
        self.conn.execute(
            "UPDATE hebbian_links SET strength = strength * CASE \
                WHEN signal_source = 'corecall' THEN ?1 \
                WHEN signal_source = 'multi' THEN ?2 \
                ELSE ?3 \
            END \
            WHERE strength > 0",
            params![decay_corecall, decay_multi, decay_single],
        )?;

        // Prune very weak links (same threshold as uniform decay)
        let pruned = self.conn.execute(
            "DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1",
            [],
        )?;

        Ok(pruned)
    }

    fn row_to_record(
        &self,
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
        
        let contradicts_str: String = row.get("contradicts")?;
        let contradicted_by_str: String = row.get("contradicted_by")?;
        let superseded_by_str: String = row.get("superseded_by").unwrap_or_default();
        
        let metadata = metadata_str
            .and_then(|s| serde_json::from_str(&s).ok());
        
        Ok(MemoryRecord {
            id: row.get("id")?,
            content: row.get("content")?,
            memory_type,
            layer,
            created_at,
            access_times,
            working_strength: row.get("working_strength")?,
            core_strength: row.get("core_strength")?,
            importance: row.get("importance")?,
            pinned: row.get::<_, i32>("pinned")? != 0,
            consolidation_count: row.get("consolidation_count")?,
            last_consolidated,
            source: row.get("source")?,
            contradicts: if contradicts_str.is_empty() { None } else { Some(contradicts_str) },
            contradicted_by: if contradicted_by_str.is_empty() { None } else { Some(contradicted_by_str) },
            superseded_by: if superseded_by_str.is_empty() { None } else { Some(superseded_by_str) },
            metadata,
        })
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

        self.conn.execute(
            "UPDATE memories SET superseded_by = ? WHERE id = ?",
            params![new_id, old_id],
        ).map_err(SupersessionError::Db)?;
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
            for &old_id in old_ids {
                self.conn.execute(
                    "UPDATE memories SET superseded_by = ? WHERE id = ?",
                    params![new_id, old_id],
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

        self.conn.execute(
            "UPDATE memories SET superseded_by = '' WHERE id = ?",
            params![id],
        ).map_err(SupersessionError::Db)?;
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
        
        let now = chrono::Utc::now().to_rfc3339();
        
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO memory_embeddings (memory_id, model, embedding, dimensions, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            params![memory_id, model, bytes, dimensions as i64, now],
        )?;
        
        Ok(())
    }
    
    /// Get embedding for a memory using a specific model.
    ///
    /// Returns None if no embedding exists for this (memory_id, model) pair.
    pub fn get_embedding(&self, memory_id: &str, model: &str) -> Result<Option<Vec<f32>>, rusqlite::Error> {
        let model = Self::normalize_model_id(model);
        let result: Option<Vec<u8>> = self.conn
            .query_row(
                "SELECT embedding FROM memory_embeddings WHERE memory_id = ? AND model = ?",
                params![memory_id, model],
                |row| row.get(0),
            )
            .optional()?;
        
        Ok(result.map(|bytes| bytes_to_f32_vec(&bytes)))
    }
    
    /// Get all embeddings for a specific model.
    ///
    /// Returns (memory_id, embedding) pairs for the given model only.
    /// Cross-model comparison is undefined behavior per protocol.
    pub fn get_all_embeddings(&self, model: &str) -> Result<Vec<(String, Vec<f32>)>, rusqlite::Error> {
        let model = Self::normalize_model_id(model);
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

    /// Hard delete with full cascade across all related tables.
    pub fn hard_delete_cascade(&self, id: &str) -> Result<(), rusqlite::Error> {
        // Delete from all related tables first
        self.conn.execute("DELETE FROM memory_embeddings WHERE memory_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM access_log WHERE memory_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM memory_entities WHERE memory_id = ?1", params![id])?;
        self.conn.execute("DELETE FROM synthesis_provenance WHERE source_id = ?1 OR insight_id = ?1", params![id])?;
        // FTS cleanup
        let rowid: Result<i64, _> = self.conn.query_row(
            "SELECT rowid FROM memories WHERE id = ?", params![id], |row| row.get(0),
        );
        if let Ok(rowid) = rowid {
            let _ = self.conn.execute("DELETE FROM memories_fts WHERE rowid = ?", params![rowid]);
        }
        // Finally the memory itself
        self.conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
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
    pub fn delete_all_embeddings(&mut self, memory_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM memory_embeddings WHERE memory_id = ?",
            params![memory_id],
        )?;
        Ok(())
    }
    
    /// Get memory IDs that don't have embeddings for a specific model.
    ///
    /// Used to find memories that need (re)embedding when switching models
    /// or during backfill operations.
    pub fn get_memories_without_embeddings(&self, model: &str) -> Result<Vec<String>, rusqlite::Error> {
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
    
    /// Get embedding statistics, optionally filtered by model.
    pub fn embedding_stats(&self) -> Result<EmbeddingStats, rusqlite::Error> {
        let total_memories: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM memories",
            [],
            |row| row.get(0),
        )?;
        
        // Count distinct memory_ids with any embedding
        let embedded_count: usize = self.conn.query_row(
            "SELECT COUNT(DISTINCT memory_id) FROM memory_embeddings",
            [],
            |row| row.get(0),
        )?;
        
        // Get the most common model
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
    pub fn record_coactivation_ns(
        &mut self,
        id1: &str,
        id2: &str,
        threshold: i32,
        namespace: &str,
    ) -> Result<bool, rusqlite::Error> {
        let (id1, id2) = if id1 < id2 { (id1, id2) } else { (id2, id1) };
        
        // Check existing link
        let existing: Option<(f64, i32)> = self.conn
            .query_row(
                "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                params![id1, id2],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        
        match existing {
            Some((strength, _count)) if strength > 0.0 => {
                // Link already formed, strengthen it
                let new_strength = (strength + 0.1).min(1.0);
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id1, id2],
                )?;
                // Also update reverse link
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id2, id1],
                )?;
                Ok(false)
            }
            Some((_, count)) => {
                // Tracking phase, increment count
                let new_count = count + 1;
                if new_count >= threshold {
                    // Threshold reached, form link
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    // Create reverse link
                    self.conn.execute(
                        "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 1.0, ?, ?, ?)",
                        params![id2, id1, new_count, now_f64(), namespace],
                    )?;
                    Ok(true)
                } else {
                    self.conn.execute(
                        "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    Ok(false)
                }
            }
            None => {
                // First co-activation, create tracking record
                self.conn.execute(
                    "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 0.0, 1, ?, ?)",
                    params![id1, id2, now_f64(), namespace],
                )?;
                Ok(false)
            }
        }
    }
    
    // === Cross-Namespace Hebbian Methods (Phase 3) ===
    
    /// Record cross-namespace co-activation.
    ///
    /// When memories from different namespaces are recalled together,
    /// this creates a Hebbian link that spans namespaces.
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
        
        // Check existing link
        let existing: Option<(f64, i32)> = self.conn
            .query_row(
                "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id = ? AND target_id = ?",
                params![id1, id2],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        
        // Use "cross" as special namespace marker for cross-namespace links
        let cross_ns = format!("{}:{}", ns1, ns2);
        
        match existing {
            Some((strength, _count)) if strength > 0.0 => {
                // Link already formed, strengthen it
                let new_strength = (strength + 0.1).min(1.0);
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id1, id2],
                )?;
                // Also update reverse link
                self.conn.execute(
                    "UPDATE hebbian_links SET strength = ?, coactivation_count = coactivation_count + 1 WHERE source_id = ? AND target_id = ?",
                    params![new_strength, id2, id1],
                )?;
                Ok(false)
            }
            Some((_, count)) => {
                // Tracking phase, increment count
                let new_count = count + 1;
                if new_count >= threshold {
                    // Threshold reached, form link
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = 1.0, coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    // Create reverse link
                    self.conn.execute(
                        "INSERT OR REPLACE INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 1.0, ?, ?, ?)",
                        params![id2, id1, new_count, now_f64(), &cross_ns],
                    )?;
                    Ok(true)
                } else {
                    self.conn.execute(
                        "UPDATE hebbian_links SET coactivation_count = ? WHERE source_id = ? AND target_id = ?",
                        params![new_count, id1, id2],
                    )?;
                    Ok(false)
                }
            }
            None => {
                // First co-activation, create tracking record
                self.conn.execute(
                    "INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at, namespace) VALUES (?, ?, 0.0, 1, ?, ?)",
                    params![id1, id2, now_f64(), &cross_ns],
                )?;
                Ok(false)
            }
        }
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
        Ok(())
    }

    /// Get all source provenance records for a given insight.
    pub fn get_insight_sources(&self, insight_id: &str) -> Result<Vec<ProvenanceRecord>, Box<dyn std::error::Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, source_original_importance FROM synthesis_provenance WHERE insight_id = ?1"
        )?;
        let records = stmt.query_map([insight_id], |row| {
            Self::row_to_provenance(row)
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Get all insights derived from a source memory.
    pub fn get_memory_insights(&self, source_id: &str) -> Result<Vec<ProvenanceRecord>, Box<dyn std::error::Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, insight_id, source_id, cluster_id, synthesis_timestamp, gate_decision, gate_scores, confidence, source_original_importance FROM synthesis_provenance WHERE source_id = ?1"
        )?;
        let records = stmt.query_map([source_id], |row| {
            Self::row_to_provenance(row)
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
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
    pub fn check_coverage(&self, member_ids: &[String]) -> Result<f64, Box<dyn std::error::Error>> {
        if member_ids.is_empty() {
            return Ok(0.0);
        }
        let mut covered = 0usize;
        for id in member_ids {
            let count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM synthesis_provenance WHERE source_id = ?1",
                [id],
                |row| row.get(0),
            )?;
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
        let result: Option<Vec<u8>> = self.conn
            .query_row(
                "SELECT embedding FROM memory_embeddings WHERE memory_id = ?1 LIMIT 1",
                params![memory_id],
                |row| row.get(0),
            )
            .optional()?;
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
    pub fn record_association(
        &self,
        source_id: &str,
        target_id: &str,
        strength: f64,
        signal_source: &str,
        signal_detail: &str,
        namespace: &str,
    ) -> Result<bool, rusqlite::Error> {
        // Check for existing link (either direction)
        let existing: Option<(String, String, f64)> = self.conn
            .query_row(
                "SELECT source_id, target_id, strength FROM hebbian_links \
                 WHERE (source_id = ?1 AND target_id = ?2) OR (source_id = ?2 AND target_id = ?1) \
                 LIMIT 1",
                params![source_id, target_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        
        match existing {
            Some((existing_src, existing_tgt, existing_strength)) => {
                // Update if new strength is higher
                let new_strength = existing_strength.max(strength);
                if strength > existing_strength {
                    // New link is stronger — update strength and signal_source
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = ?1, signal_source = ?2, signal_detail = ?3 \
                         WHERE source_id = ?4 AND target_id = ?5",
                        params![new_strength, signal_source, signal_detail, existing_src, existing_tgt],
                    )?;
                } else {
                    // Just update strength (keep existing signal_source)
                    self.conn.execute(
                        "UPDATE hebbian_links SET strength = ?1 \
                         WHERE source_id = ?2 AND target_id = ?3",
                        params![new_strength, existing_src, existing_tgt],
                    )?;
                }
                Ok(false)
            }
            None => {
                // Create new link
                self.conn.execute(
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
                Ok(true)
            }
        }
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
}
