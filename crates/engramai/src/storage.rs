//! SQLite storage backend for Engram.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Result as SqlResult};
use std::path::Path;

use crate::synthesis::types::{GateScores, ProvenanceRecord};
use crate::types::{AclEntry, CrossLink, HebbianLink, MemoryLayer, MemoryRecord, MemoryType, Permission};

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
fn now_f64() -> f64 {
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

    /// Migrate entity tables: add unique constraints needed for upsert operations.
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
        
        let metadata_json = record.metadata.as_ref().map(|m| serde_json::to_string(m).ok()).flatten();
        
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
        let mut stmt = self.conn.prepare("SELECT * FROM memories")?;
        let rows = stmt.query_map([], |row| {
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
        let metadata_json = record.metadata.as_ref().map(|m| serde_json::to_string(m).ok()).flatten();
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
        let metadata_json = metadata.map(|m| serde_json::to_string(&m).ok()).flatten();
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
                "SELECT * FROM memories WHERE memory_type = ? ORDER BY importance DESC LIMIT ?"
            )?;
            
            let rows = stmt.query_map(params![memory_type.to_string(), limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE memory_type = ? AND namespace = ? ORDER BY importance DESC LIMIT ?"
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
            WHERE memories_fts MATCH ?
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
                "SELECT * FROM memories ORDER BY created_at DESC LIMIT ?"
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                let id: String = row.get("id")?;
                let access_times = self.get_access_times(&id).unwrap_or_default();
                self.row_to_record(row, access_times)
            })?;
            rows.collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM memories WHERE namespace = ? ORDER BY created_at DESC LIMIT ?"
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
            .prepare("SELECT * FROM memories WHERE memory_type = ?")?;
        
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
                WHERE memories_fts MATCH ?
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
                WHERE memories_fts MATCH ? AND m.namespace = ?
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
        
        let mut stmt = self.conn.prepare("SELECT * FROM memories WHERE namespace = ?")?;
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
            "SELECT memory_id, embedding FROM memory_embeddings WHERE model = ?"
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
            WHERE m.namespace = ? AND e.model = ?
            "#
        )?;
        
        let rows = stmt.query_map(params![ns, model], |row| {
            let memory_id: String = row.get(0)?;
            let bytes: Vec<u8> = row.get(1)?;
            Ok((memory_id, bytes_to_f32_vec(&bytes)))
        })?;
        
        rows.collect()
    }
    
    /// Delete embedding for a specific (memory_id, model) pair.
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
    
    /// Merge a duplicate memory's metadata into an existing memory.
    ///
    /// Strategy (from ISS-003):
    /// - access_count: add new access to existing memory's access log
    /// - importance: max(existing, new)
    /// - created_at: keep existing (older)
    /// - content: keep existing (already stored, presumably equivalent)
    ///
    /// Does NOT create a new memory — just boosts the existing one.
    pub fn merge_memory_into(
        &mut self,
        existing_id: &str,
        new_importance: f64,
    ) -> Result<(), rusqlite::Error> {
        // Insert a new access_log entry for the existing memory (now)
        self.conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)",
            params![existing_id, now_f64()],
        )?;
        
        // Update importance = MAX(existing, new)
        self.conn.execute(
            "UPDATE memories SET importance = MAX(importance, ?) WHERE id = ?",
            params![new_importance, existing_id],
        )?;
        
        log::info!(
            "Merged duplicate into memory {}: boosted access + importance(max {})",
            existing_id,
            new_importance
        );
        
        Ok(())
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
}
