//! KnowledgeStore trait and SQLite implementation.
//!
//! Provides persistent storage for topic pages, compilation records,
//! and source memory references used by the Knowledge Compiler.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TRAIT
// ═══════════════════════════════════════════════════════════════════════════════

/// Persistence layer for Knowledge Compiler data.
pub trait KnowledgeStore {
    /// Create all required tables and indices.
    fn init_schema(&self) -> Result<(), KcError>;

    /// Insert a new topic page.
    fn create_topic_page(&self, page: &TopicPage) -> Result<(), KcError>;

    /// Update an existing topic page (matched by `page.id`).
    fn update_topic_page(&self, page: &TopicPage) -> Result<(), KcError>;

    /// Fetch a topic page by id, returning `None` if it doesn't exist.
    fn get_topic_page(&self, id: &TopicId) -> Result<Option<TopicPage>, KcError>;

    /// List every topic page in the store.
    fn list_topic_pages(&self) -> Result<Vec<TopicPage>, KcError>;

    /// Delete a topic page. Returns `true` if a row was actually removed.
    fn delete_topic_page(&self, id: &TopicId) -> Result<bool, KcError>;

    /// Persist a compilation record.
    fn save_compilation_record(&self, record: &CompilationRecord) -> Result<(), KcError>;

    /// Retrieve all compilation records for a given topic.
    fn get_compilation_records(&self, topic_id: &TopicId) -> Result<Vec<CompilationRecord>, KcError>;

    /// Fetch all pages with a given status.
    fn get_pages_by_status(&self, status: TopicStatus) -> Result<Vec<TopicPage>, KcError>;

    /// Set the quality/activity score and bump `updated_at`.
    fn update_activity_score(&self, id: &TopicId, score: f64) -> Result<(), KcError>;

    /// Archive a topic page, recording a reason.
    fn mark_archived(&self, id: &TopicId, reason: &str) -> Result<(), KcError>;

    /// Return active pages whose `updated_at` is older than `stale_days` days.
    fn get_stale_pages(&self, stale_days: u32) -> Result<Vec<TopicPage>, KcError>;

    /// Replace the set of source-memory references for a topic.
    fn save_source_refs(&self, topic_id: &TopicId, refs: &[SourceMemoryRef]) -> Result<(), KcError>;

    /// Retrieve all source-memory references for a topic.
    fn get_source_refs(&self, topic_id: &TopicId) -> Result<Vec<SourceMemoryRef>, KcError>;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HELPERS — TopicStatus ↔ string
// ═══════════════════════════════════════════════════════════════════════════════

fn status_to_str(s: &TopicStatus) -> &'static str {
    match s {
        TopicStatus::Active => "Active",
        TopicStatus::Stale => "Stale",
        TopicStatus::Archived => "Archived",
        TopicStatus::FailedPermanent => "FailedPermanent",
    }
}

fn str_to_status(s: &str) -> TopicStatus {
    match s {
        "Active" => TopicStatus::Active,
        "Stale" => TopicStatus::Stale,
        "Archived" => TopicStatus::Archived,
        "FailedPermanent" => TopicStatus::FailedPermanent,
        _ => TopicStatus::Active, // fallback
    }
}

fn to_rfc3339(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn json_tags(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_owned())
}

fn parse_json_vec(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SQLite IMPLEMENTATION
// ═══════════════════════════════════════════════════════════════════════════════

/// SQLite-backed [`KnowledgeStore`].
pub struct SqliteKnowledgeStore {
    conn: Connection,
}

impl SqliteKnowledgeStore {
    /// Wrap an already-opened [`Connection`].
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Open (or create) a database at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self, KcError> {
        let conn =
            Connection::open(path).map_err(|e| KcError::Storage(format!("open: {e}")))?;
        Ok(Self { conn })
    }

    /// Create an in-memory store (useful for tests).
    pub fn in_memory() -> Result<Self, KcError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| KcError::Storage(format!("in_memory: {e}")))?;
        Ok(Self { conn })
    }

    // ── row → TopicPage ──────────────────────────────────────────────────

    fn row_to_topic_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<TopicPage> {
        let id: String = row.get("id")?;
        let title: String = row.get("title")?;
        let content: String = row.get("content")?;
        let summary: String = row.get("summary")?;
        let status_str: String = row.get("status")?;
        let version: u32 = row.get("version")?;
        let quality_score: Option<f64> = row.get("quality_score")?;
        let compilation_count: u32 = row.get("compilation_count")?;
        let tags_json: String = row.get("tags")?;
        let source_ids_json: String = row.get("source_memory_ids")?;
        let created_at_str: String = row.get("created_at")?;
        let updated_at_str: String = row.get("updated_at")?;

        // Parse datetimes — fall back to epoch on error to avoid panics inside
        // the rusqlite row-mapping closure (which requires rusqlite::Result).
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_default();
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_default();

        Ok(TopicPage {
            id: TopicId(id),
            title,
            content,
            sections: Vec::new(),
            summary,
            status: str_to_status(&status_str),
            version,
            metadata: TopicMetadata {
                created_at,
                updated_at,
                compilation_count,
                source_memory_ids: parse_json_vec(&source_ids_json),
                tags: parse_json_vec(&tags_json),
                quality_score,
            },
        })
    }
}

impl KnowledgeStore for SqliteKnowledgeStore {
    // ── schema ───────────────────────────────────────────────────────────

    fn init_schema(&self) -> Result<(), KcError> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS kc_topic_pages (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    content TEXT NOT NULL,
                    summary TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL DEFAULT 'Active',
                    version INTEGER NOT NULL DEFAULT 1,
                    quality_score REAL,
                    compilation_count INTEGER NOT NULL DEFAULT 0,
                    tags TEXT NOT NULL DEFAULT '[]',
                    source_memory_ids TEXT NOT NULL DEFAULT '[]',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS kc_compilation_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    topic_id TEXT NOT NULL,
                    compiled_at TEXT NOT NULL,
                    source_count INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    quality_score REAL NOT NULL,
                    recompile_reason TEXT,
                    FOREIGN KEY (topic_id) REFERENCES kc_topic_pages(id)
                );

                CREATE TABLE IF NOT EXISTS kc_compilation_sources (
                    topic_id TEXT NOT NULL,
                    memory_id TEXT NOT NULL,
                    relevance_score REAL NOT NULL DEFAULT 1.0,
                    added_at TEXT NOT NULL,
                    PRIMARY KEY (topic_id, memory_id),
                    FOREIGN KEY (topic_id) REFERENCES kc_topic_pages(id)
                );

                CREATE INDEX IF NOT EXISTS idx_kc_topics_status
                    ON kc_topic_pages(status);
                CREATE INDEX IF NOT EXISTS idx_kc_topics_updated
                    ON kc_topic_pages(updated_at);
                CREATE INDEX IF NOT EXISTS idx_kc_compilation_topic
                    ON kc_compilation_records(topic_id);
                ",
            )
            .map_err(|e| KcError::Storage(format!("init_schema: {e}")))?;
        Ok(())
    }

    // ── CRUD: topic pages ────────────────────────────────────────────────

    fn create_topic_page(&self, page: &TopicPage) -> Result<(), KcError> {
        self.conn
            .execute(
                "INSERT INTO kc_topic_pages
                    (id, title, content, summary, status, version,
                     quality_score, compilation_count, tags, source_memory_ids,
                     created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    page.id.0,
                    page.title,
                    page.content,
                    page.summary,
                    status_to_str(&page.status),
                    page.version,
                    page.metadata.quality_score,
                    page.metadata.compilation_count,
                    json_tags(&page.metadata.tags),
                    json_tags(&page.metadata.source_memory_ids),
                    to_rfc3339(&page.metadata.created_at),
                    to_rfc3339(&page.metadata.updated_at),
                ],
            )
            .map_err(|e| KcError::Storage(format!("create_topic_page: {e}")))?;
        Ok(())
    }

    fn update_topic_page(&self, page: &TopicPage) -> Result<(), KcError> {
        let changed = self
            .conn
            .execute(
                "UPDATE kc_topic_pages SET
                    title = ?1, content = ?2, summary = ?3, status = ?4,
                    version = ?5, quality_score = ?6, compilation_count = ?7,
                    tags = ?8, source_memory_ids = ?9, updated_at = ?10
                 WHERE id = ?11",
                params![
                    page.title,
                    page.content,
                    page.summary,
                    status_to_str(&page.status),
                    page.version,
                    page.metadata.quality_score,
                    page.metadata.compilation_count,
                    json_tags(&page.metadata.tags),
                    json_tags(&page.metadata.source_memory_ids),
                    to_rfc3339(&page.metadata.updated_at),
                    page.id.0,
                ],
            )
            .map_err(|e| KcError::Storage(format!("update_topic_page: {e}")))?;

        if changed == 0 {
            return Err(KcError::NotFound(format!("topic page '{}'", page.id)));
        }
        Ok(())
    }

    fn get_topic_page(&self, id: &TopicId) -> Result<Option<TopicPage>, KcError> {
        self.conn
            .query_row(
                "SELECT * FROM kc_topic_pages WHERE id = ?1",
                params![id.0],
                Self::row_to_topic_page,
            )
            .optional()
            .map_err(|e| KcError::Storage(format!("get_topic_page: {e}")))
    }

    fn list_topic_pages(&self) -> Result<Vec<TopicPage>, KcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM kc_topic_pages ORDER BY updated_at DESC")
            .map_err(|e| KcError::Storage(format!("list_topic_pages prepare: {e}")))?;

        let rows = stmt
            .query_map([], Self::row_to_topic_page)
            .map_err(|e| KcError::Storage(format!("list_topic_pages query: {e}")))?;

        let mut pages = Vec::new();
        for row in rows {
            pages.push(row.map_err(|e| KcError::Storage(format!("list_topic_pages row: {e}")))?);
        }
        Ok(pages)
    }

    fn delete_topic_page(&self, id: &TopicId) -> Result<bool, KcError> {
        // Also clean up related rows.
        self.conn
            .execute(
                "DELETE FROM kc_compilation_records WHERE topic_id = ?1",
                params![id.0],
            )
            .map_err(|e| KcError::Storage(format!("delete_topic_page (records): {e}")))?;

        self.conn
            .execute(
                "DELETE FROM kc_compilation_sources WHERE topic_id = ?1",
                params![id.0],
            )
            .map_err(|e| KcError::Storage(format!("delete_topic_page (sources): {e}")))?;

        let changed = self
            .conn
            .execute(
                "DELETE FROM kc_topic_pages WHERE id = ?1",
                params![id.0],
            )
            .map_err(|e| KcError::Storage(format!("delete_topic_page: {e}")))?;

        Ok(changed > 0)
    }

    // ── compilation records ──────────────────────────────────────────────

    fn save_compilation_record(&self, record: &CompilationRecord) -> Result<(), KcError> {
        self.conn
            .execute(
                "INSERT INTO kc_compilation_records
                    (topic_id, compiled_at, source_count, duration_ms,
                     quality_score, recompile_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.topic_id.0,
                    to_rfc3339(&record.compiled_at),
                    record.source_count as i64,
                    record.duration_ms as i64,
                    record.quality_score,
                    record.recompile_reason,
                ],
            )
            .map_err(|e| KcError::Storage(format!("save_compilation_record: {e}")))?;
        Ok(())
    }

    fn get_compilation_records(
        &self,
        topic_id: &TopicId,
    ) -> Result<Vec<CompilationRecord>, KcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT topic_id, compiled_at, source_count, duration_ms,
                        quality_score, recompile_reason
                 FROM kc_compilation_records
                 WHERE topic_id = ?1
                 ORDER BY compiled_at DESC",
            )
            .map_err(|e| KcError::Storage(format!("get_compilation_records prepare: {e}")))?;

        let rows = stmt
            .query_map(params![topic_id.0], |row| {
                let tid: String = row.get(0)?;
                let compiled_at_str: String = row.get(1)?;
                let source_count: i64 = row.get(2)?;
                let duration_ms: i64 = row.get(3)?;
                let quality_score: f64 = row.get(4)?;
                let recompile_reason: Option<String> = row.get(5)?;

                let compiled_at = DateTime::parse_from_rfc3339(&compiled_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_default();

                Ok(CompilationRecord {
                    topic_id: TopicId(tid),
                    compiled_at,
                    source_count: source_count as usize,
                    duration_ms: duration_ms as u64,
                    quality_score,
                    recompile_reason,
                })
            })
            .map_err(|e| KcError::Storage(format!("get_compilation_records query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records.push(
                row.map_err(|e| KcError::Storage(format!("get_compilation_records row: {e}")))?,
            );
        }
        Ok(records)
    }

    // ── filtered queries ─────────────────────────────────────────────────

    fn get_pages_by_status(&self, status: TopicStatus) -> Result<Vec<TopicPage>, KcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM kc_topic_pages WHERE status = ?1 ORDER BY updated_at DESC")
            .map_err(|e| KcError::Storage(format!("get_pages_by_status prepare: {e}")))?;

        let rows = stmt
            .query_map(params![status_to_str(&status)], Self::row_to_topic_page)
            .map_err(|e| KcError::Storage(format!("get_pages_by_status query: {e}")))?;

        let mut pages = Vec::new();
        for row in rows {
            pages.push(
                row.map_err(|e| KcError::Storage(format!("get_pages_by_status row: {e}")))?,
            );
        }
        Ok(pages)
    }

    fn update_activity_score(&self, id: &TopicId, score: f64) -> Result<(), KcError> {
        let now = to_rfc3339(&Utc::now());
        let changed = self
            .conn
            .execute(
                "UPDATE kc_topic_pages SET quality_score = ?1, updated_at = ?2 WHERE id = ?3",
                params![score, now, id.0],
            )
            .map_err(|e| KcError::Storage(format!("update_activity_score: {e}")))?;

        if changed == 0 {
            return Err(KcError::NotFound(format!("topic page '{id}'")));
        }
        Ok(())
    }

    fn mark_archived(&self, id: &TopicId, reason: &str) -> Result<(), KcError> {
        let now = to_rfc3339(&Utc::now());
        let changed = self
            .conn
            .execute(
                "UPDATE kc_topic_pages SET status = 'Archived', updated_at = ?1 WHERE id = ?2",
                params![now, id.0],
            )
            .map_err(|e| KcError::Storage(format!("mark_archived: {e}")))?;

        if changed == 0 {
            return Err(KcError::NotFound(format!("topic page '{id}'")));
        }

        // Record the reason as a compilation record with recompile_reason.
        self.conn
            .execute(
                "INSERT INTO kc_compilation_records
                    (topic_id, compiled_at, source_count, duration_ms,
                     quality_score, recompile_reason)
                 VALUES (?1, ?2, 0, 0, 0.0, ?3)",
                params![id.0, now, reason],
            )
            .map_err(|e| KcError::Storage(format!("mark_archived (record): {e}")))?;

        Ok(())
    }

    fn get_stale_pages(&self, stale_days: u32) -> Result<Vec<TopicPage>, KcError> {
        let sql = format!(
            "SELECT * FROM kc_topic_pages \
             WHERE status = 'Active' \
               AND updated_at < datetime('now', '-{stale_days} days') \
             ORDER BY updated_at ASC"
        );

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| KcError::Storage(format!("get_stale_pages prepare: {e}")))?;

        let rows = stmt
            .query_map([], Self::row_to_topic_page)
            .map_err(|e| KcError::Storage(format!("get_stale_pages query: {e}")))?;

        let mut pages = Vec::new();
        for row in rows {
            pages
                .push(row.map_err(|e| KcError::Storage(format!("get_stale_pages row: {e}")))?);
        }
        Ok(pages)
    }

    // ── source refs ──────────────────────────────────────────────────────

    fn save_source_refs(
        &self,
        topic_id: &TopicId,
        refs: &[SourceMemoryRef],
    ) -> Result<(), KcError> {
        // Replace strategy: delete existing then insert new.
        self.conn
            .execute(
                "DELETE FROM kc_compilation_sources WHERE topic_id = ?1",
                params![topic_id.0],
            )
            .map_err(|e| KcError::Storage(format!("save_source_refs delete: {e}")))?;

        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO kc_compilation_sources
                    (topic_id, memory_id, relevance_score, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| KcError::Storage(format!("save_source_refs prepare: {e}")))?;

        for r in refs {
            stmt.execute(params![
                topic_id.0,
                r.memory_id,
                r.relevance_score,
                to_rfc3339(&r.added_at),
            ])
            .map_err(|e| KcError::Storage(format!("save_source_refs insert: {e}")))?;
        }
        Ok(())
    }

    fn get_source_refs(&self, topic_id: &TopicId) -> Result<Vec<SourceMemoryRef>, KcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, relevance_score, added_at
                 FROM kc_compilation_sources
                 WHERE topic_id = ?1
                 ORDER BY relevance_score DESC",
            )
            .map_err(|e| KcError::Storage(format!("get_source_refs prepare: {e}")))?;

        let rows = stmt
            .query_map(params![topic_id.0], |row| {
                let memory_id: String = row.get(0)?;
                let relevance_score: f64 = row.get(1)?;
                let added_at_str: String = row.get(2)?;

                let added_at = DateTime::parse_from_rfc3339(&added_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_default();

                Ok(SourceMemoryRef {
                    memory_id,
                    relevance_score,
                    added_at,
                })
            })
            .map_err(|e| KcError::Storage(format!("get_source_refs query: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| KcError::Storage(format!("get_source_refs row: {e}")))?,
            );
        }
        Ok(out)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_store() -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        store
    }

    fn sample_page(id: &str) -> TopicPage {
        let now = Utc::now();
        TopicPage {
            id: TopicId(id.to_owned()),
            title: format!("Topic {id}"),
            content: "Some content".to_owned(),
            sections: Vec::new(),
            summary: "A summary".to_owned(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 0,
                source_memory_ids: vec!["mem-1".to_owned()],
                tags: vec!["rust".to_owned()],
                quality_score: Some(0.85),
            },
        }
    }

    #[test]
    fn create_and_get() {
        let store = make_store();
        let page = sample_page("t1");
        store.create_topic_page(&page).unwrap();

        let got = store.get_topic_page(&TopicId("t1".into())).unwrap().unwrap();
        assert_eq!(got.id, page.id);
        assert_eq!(got.title, page.title);
        assert_eq!(got.metadata.tags, vec!["rust".to_owned()]);
    }

    #[test]
    fn update_and_list() {
        let store = make_store();
        let mut page = sample_page("t2");
        store.create_topic_page(&page).unwrap();

        page.title = "Updated Title".to_owned();
        page.version = 2;
        store.update_topic_page(&page).unwrap();

        let all = store.list_topic_pages().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Updated Title");
        assert_eq!(all[0].version, 2);
    }

    #[test]
    fn delete() {
        let store = make_store();
        store.create_topic_page(&sample_page("d1")).unwrap();
        assert!(store.delete_topic_page(&TopicId("d1".into())).unwrap());
        assert!(!store.delete_topic_page(&TopicId("d1".into())).unwrap());
        assert!(store.get_topic_page(&TopicId("d1".into())).unwrap().is_none());
    }

    #[test]
    fn compilation_records() {
        let store = make_store();
        store.create_topic_page(&sample_page("cr1")).unwrap();

        let rec = CompilationRecord {
            topic_id: TopicId("cr1".into()),
            compiled_at: Utc::now(),
            source_count: 5,
            duration_ms: 120,
            quality_score: 0.9,
            recompile_reason: Some("new memories".into()),
        };
        store.save_compilation_record(&rec).unwrap();

        let recs = store.get_compilation_records(&TopicId("cr1".into())).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].source_count, 5);
    }

    #[test]
    fn pages_by_status() {
        let store = make_store();
        store.create_topic_page(&sample_page("s1")).unwrap();

        let mut archived = sample_page("s2");
        archived.status = TopicStatus::Archived;
        store.create_topic_page(&archived).unwrap();

        let active = store.get_pages_by_status(TopicStatus::Active).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id.0, "s1");
    }

    #[test]
    fn source_refs_round_trip() {
        let store = make_store();
        store.create_topic_page(&sample_page("sr1")).unwrap();

        let refs = vec![
            SourceMemoryRef {
                memory_id: "m-a".into(),
                relevance_score: 0.95,
                added_at: Utc::now(),
            },
            SourceMemoryRef {
                memory_id: "m-b".into(),
                relevance_score: 0.80,
                added_at: Utc::now(),
            },
        ];
        store.save_source_refs(&TopicId("sr1".into()), &refs).unwrap();

        let got = store.get_source_refs(&TopicId("sr1".into())).unwrap();
        assert_eq!(got.len(), 2);
        // ordered by relevance desc
        assert_eq!(got[0].memory_id, "m-a");
    }

    #[test]
    fn mark_archived_and_activity_score() {
        let store = make_store();
        store.create_topic_page(&sample_page("ma1")).unwrap();

        store.update_activity_score(&TopicId("ma1".into()), 0.42).unwrap();
        let p = store.get_topic_page(&TopicId("ma1".into())).unwrap().unwrap();
        assert!((p.metadata.quality_score.unwrap() - 0.42).abs() < 1e-9);

        store.mark_archived(&TopicId("ma1".into()), "no longer relevant").unwrap();
        let p2 = store.get_topic_page(&TopicId("ma1".into())).unwrap().unwrap();
        assert_eq!(p2.status, TopicStatus::Archived);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let store = make_store();
        let result = store.get_topic_page(&TopicId("does-not-exist".into())).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn create_duplicate_id_errors() {
        let store = make_store();
        let page = sample_page("dup1");
        store.create_topic_page(&page).unwrap();
        // Second create with same id should fail
        let result = store.create_topic_page(&page);
        assert!(result.is_err());
    }

    #[test]
    fn list_empty_store() {
        let store = make_store();
        let all = store.list_topic_pages().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn list_multiple_pages() {
        let store = make_store();
        for i in 0..5 {
            store.create_topic_page(&sample_page(&format!("multi-{i}"))).unwrap();
        }
        let all = store.list_topic_pages().unwrap();
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn compilation_records_empty() {
        let store = make_store();
        store.create_topic_page(&sample_page("no-recs")).unwrap();
        let recs = store.get_compilation_records(&TopicId("no-recs".into())).unwrap();
        assert!(recs.is_empty());
    }

    #[test]
    fn compilation_records_multiple() {
        let store = make_store();
        store.create_topic_page(&sample_page("multi-rec")).unwrap();

        for i in 0..3 {
            let rec = CompilationRecord {
                topic_id: TopicId("multi-rec".into()),
                compiled_at: Utc::now(),
                source_count: i + 1,
                duration_ms: 100 * (i as u64 + 1),
                quality_score: 0.7 + (i as f64 * 0.1),
                recompile_reason: if i == 0 { None } else { Some(format!("reason-{i}")) },
            };
            store.save_compilation_record(&rec).unwrap();
        }

        let recs = store.get_compilation_records(&TopicId("multi-rec".into())).unwrap();
        assert_eq!(recs.len(), 3);
    }

    #[test]
    fn pages_by_status_empty_result() {
        let store = make_store();
        store.create_topic_page(&sample_page("only-active")).unwrap();
        let stale = store.get_pages_by_status(TopicStatus::Stale).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn source_refs_replace_on_save() {
        let store = make_store();
        store.create_topic_page(&sample_page("ref-replace")).unwrap();
        let tid = TopicId("ref-replace".into());

        // Save first batch
        let refs1 = vec![SourceMemoryRef {
            memory_id: "m-old".into(),
            relevance_score: 0.5,
            added_at: Utc::now(),
        }];
        store.save_source_refs(&tid, &refs1).unwrap();
        assert_eq!(store.get_source_refs(&tid).unwrap().len(), 1);

        // Save second batch — should replace, not append
        let refs2 = vec![
            SourceMemoryRef {
                memory_id: "m-new-a".into(),
                relevance_score: 0.9,
                added_at: Utc::now(),
            },
            SourceMemoryRef {
                memory_id: "m-new-b".into(),
                relevance_score: 0.8,
                added_at: Utc::now(),
            },
        ];
        store.save_source_refs(&tid, &refs2).unwrap();
        let got = store.get_source_refs(&tid).unwrap();
        assert_eq!(got.len(), 2);
        // Ordered by relevance desc
        assert_eq!(got[0].memory_id, "m-new-a");
    }

    #[test]
    fn source_refs_empty_save() {
        let store = make_store();
        store.create_topic_page(&sample_page("ref-empty")).unwrap();
        let tid = TopicId("ref-empty".into());
        store.save_source_refs(&tid, &[]).unwrap();
        let got = store.get_source_refs(&tid).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn sections_not_persisted_in_storage() {
        // Note: sections are reconstructed by the compilation pipeline,
        // not stored directly in the kc_topic_pages table.
        // Storage always returns Vec::new() for sections.
        let store = make_store();
        let mut page = sample_page("with-sections");
        page.sections = vec![
            TopicSection {
                heading: "Intro".into(),
                body: "First section".into(),
                user_edited: false,
                edited_at: None,
            },
        ];
        store.create_topic_page(&page).unwrap();

        let got = store.get_topic_page(&TopicId("with-sections".into())).unwrap().unwrap();
        // Sections are not persisted — always empty on read from storage
        assert!(got.sections.is_empty());
        // But other fields round-trip fine
        assert_eq!(got.title, "Topic with-sections");
        assert_eq!(got.content, "Some content");
    }
}
