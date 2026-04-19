//! FeedbackProcessor — processes user feedback on topic pages and integrates
//! it into the compilation loop (design doc §3.5).

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::compiler::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TRAIT
// ═══════════════════════════════════════════════════════════════════════════════

/// Storage trait for feedback entries.
pub trait FeedbackStore {
    fn save(&self, entry: &FeedbackEntry) -> Result<(), KcError>;
    fn get_for_topic(&self, topic_id: &TopicId) -> Result<Vec<FeedbackEntry>, KcError>;
    fn mark_resolved(&self, topic_id: &TopicId, compilation_id: &str) -> Result<usize, KcError>;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SQLite IMPLEMENTATION
// ═══════════════════════════════════════════════════════════════════════════════

/// SQLite-backed [`FeedbackStore`].
pub struct SqliteFeedbackStore {
    conn: rusqlite::Connection,
}

impl SqliteFeedbackStore {
    pub fn new(conn: rusqlite::Connection) -> Result<Self, KcError> {
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, KcError> {
        Self::new(
            rusqlite::Connection::open_in_memory()
                .map_err(|e| KcError::Storage(e.to_string()))?,
        )
    }

    fn init(&self) -> Result<(), KcError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS kc_feedback (
                    id INTEGER PRIMARY KEY,
                    topic_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    comment TEXT,
                    timestamp TEXT NOT NULL,
                    resolved INTEGER NOT NULL DEFAULT 0,
                    resolved_by TEXT
                );",
            )
            .map_err(|e| KcError::Storage(format!("init kc_feedback: {e}")))?;
        Ok(())
    }
}

impl FeedbackStore for SqliteFeedbackStore {
    fn save(&self, entry: &FeedbackEntry) -> Result<(), KcError> {
        let kind_json = serde_json::to_string(&entry.kind)
            .map_err(|e| KcError::Storage(format!("serialize kind: {e}")))?;
        let ts = entry.timestamp.to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO kc_feedback (topic_id, kind, comment, timestamp, resolved)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    entry.topic_id.0,
                    kind_json,
                    entry.comment,
                    ts,
                    entry.resolved as i32,
                ],
            )
            .map_err(|e| KcError::Storage(format!("save feedback: {e}")))?;
        Ok(())
    }

    fn get_for_topic(&self, topic_id: &TopicId) -> Result<Vec<FeedbackEntry>, KcError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT topic_id, kind, comment, timestamp, resolved
                 FROM kc_feedback
                 WHERE topic_id = ?1
                 ORDER BY timestamp DESC",
            )
            .map_err(|e| KcError::Storage(format!("get_for_topic prepare: {e}")))?;

        let rows = stmt
            .query_map(params![topic_id.0], |row| {
                let tid: String = row.get(0)?;
                let kind_json: String = row.get(1)?;
                let comment: Option<String> = row.get(2)?;
                let ts_str: String = row.get(3)?;
                let resolved: i32 = row.get(4)?;

                Ok((tid, kind_json, comment, ts_str, resolved))
            })
            .map_err(|e| KcError::Storage(format!("get_for_topic query: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            let (tid, kind_json, comment, ts_str, resolved) =
                row.map_err(|e| KcError::Storage(format!("get_for_topic row: {e}")))?;

            let kind: FeedbackKind = serde_json::from_str(&kind_json)
                .map_err(|e| KcError::Storage(format!("deserialize kind: {e}")))?;

            let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_default();

            entries.push(FeedbackEntry {
                topic_id: TopicId(tid),
                kind,
                comment,
                timestamp,
                resolved: resolved != 0,
            });
        }
        Ok(entries)
    }

    fn mark_resolved(&self, topic_id: &TopicId, compilation_id: &str) -> Result<usize, KcError> {
        let changed = self
            .conn
            .execute(
                "UPDATE kc_feedback SET resolved = 1, resolved_by = ?1
                 WHERE topic_id = ?2 AND resolved = 0",
                params![compilation_id, topic_id.0],
            )
            .map_err(|e| KcError::Storage(format!("mark_resolved: {e}")))?;
        Ok(changed)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  FEEDBACK PROCESSOR
// ═══════════════════════════════════════════════════════════════════════════════

pub struct FeedbackProcessor {
    _config: KcConfig,
}

impl FeedbackProcessor {
    pub fn new(config: KcConfig) -> Self {
        Self { _config: config }
    }

    /// Record feedback for a topic. Converts `TopicFeedback` → `FeedbackEntry`
    /// and persists it via the given store.
    pub fn record(
        &self,
        feedback: TopicFeedback,
        store: &impl FeedbackStore,
    ) -> Result<FeedbackEntry, KcError> {
        let entry = FeedbackEntry {
            topic_id: feedback.topic_id,
            kind: feedback.kind,
            comment: feedback.comment,
            timestamp: feedback.timestamp,
            resolved: false,
        };
        store.save(&entry)?;
        Ok(entry)
    }

    /// Build a feedback context string for inclusion in compilation prompts.
    /// Returns formatted text describing unresolved feedback to incorporate.
    pub fn build_prompt_context(
        &self,
        topic_id: &TopicId,
        store: &impl FeedbackStore,
    ) -> Result<String, KcError> {
        let entries = store.get_for_topic(topic_id)?;
        let unresolved: Vec<_> = entries.iter().filter(|e| !e.resolved).collect();

        if unresolved.is_empty() {
            return Ok(String::new());
        }

        let mut lines = vec!["User feedback to incorporate:".to_string()];
        for entry in &unresolved {
            let line = match &entry.kind {
                FeedbackKind::Correction(text) => format!("- [CORRECTION]: \"{}\"", text),
                FeedbackKind::ThumbsDown => {
                    if let Some(comment) = &entry.comment {
                        format!("- [NEGATIVE]: \"{}\"", comment)
                    } else {
                        "- [NEGATIVE]: User marked this section as incorrect".to_string()
                    }
                }
                FeedbackKind::ThumbsUp => {
                    if let Some(comment) = &entry.comment {
                        format!("- [POSITIVE — preserve]: \"{}\"", comment)
                    } else {
                        "- [POSITIVE — preserve]: User validated this section".to_string()
                    }
                }
                FeedbackKind::TitleSuggestion(title) => {
                    format!("- [TITLE SUGGESTION]: \"{}\"", title)
                }
                FeedbackKind::MergeRequest(other_id) => {
                    format!(
                        "- [MERGE REQUEST]: User suggests merging with topic \"{}\"",
                        other_id
                    )
                }
                FeedbackKind::SplitRequest(parts) => {
                    format!(
                        "- [SPLIT REQUEST]: User suggests splitting into: {}",
                        parts.join(", ")
                    )
                }
            };
            lines.push(line);
        }

        Ok(lines.join("\n"))
    }

    /// Determine if feedback warrants immediate recompilation.
    pub fn should_trigger_recompile(&self, feedback: &TopicFeedback) -> bool {
        matches!(
            feedback.kind,
            FeedbackKind::ThumbsDown
                | FeedbackKind::Correction(_)
                | FeedbackKind::TitleSuggestion(_)
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::types::KcConfig;
    use chrono::Utc;

    fn make_store() -> SqliteFeedbackStore {
        SqliteFeedbackStore::in_memory().unwrap()
    }

    fn make_processor() -> FeedbackProcessor {
        FeedbackProcessor::new(KcConfig::default())
    }

    fn make_feedback(topic_id: &str, kind: FeedbackKind, comment: Option<&str>) -> TopicFeedback {
        TopicFeedback {
            topic_id: TopicId(topic_id.to_string()),
            kind,
            comment: comment.map(|s| s.to_string()),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_record_and_retrieve() {
        let store = make_store();
        let proc = make_processor();

        let fb = make_feedback("t1", FeedbackKind::ThumbsDown, Some("not great"));
        let entry = proc.record(fb, &store).unwrap();
        assert!(!entry.resolved);

        let entries = store.get_for_topic(&TopicId("t1".to_string())).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].resolved);
        assert_eq!(entries[0].topic_id.0, "t1");
        assert_eq!(entries[0].comment, Some("not great".to_string()));
    }

    #[test]
    fn test_mark_resolved() {
        let store = make_store();
        let proc = make_processor();

        // Record 3 feedbacks for the same topic
        proc.record(
            make_feedback("t2", FeedbackKind::ThumbsDown, None),
            &store,
        )
        .unwrap();
        proc.record(
            make_feedback("t2", FeedbackKind::Correction("fix this".into()), None),
            &store,
        )
        .unwrap();
        proc.record(
            make_feedback("t2", FeedbackKind::ThumbsUp, None),
            &store,
        )
        .unwrap();

        let count = store
            .mark_resolved(&TopicId("t2".to_string()), "compile-42")
            .unwrap();
        assert_eq!(count, 3);

        let entries = store.get_for_topic(&TopicId("t2".to_string())).unwrap();
        assert!(entries.iter().all(|e| e.resolved));
    }

    #[test]
    fn test_should_trigger_recompile() {
        let proc = make_processor();

        assert!(proc.should_trigger_recompile(&make_feedback(
            "t",
            FeedbackKind::ThumbsDown,
            None
        )));
        assert!(proc.should_trigger_recompile(&make_feedback(
            "t",
            FeedbackKind::Correction("x".into()),
            None
        )));
        assert!(proc.should_trigger_recompile(&make_feedback(
            "t",
            FeedbackKind::TitleSuggestion("New Title".into()),
            None
        )));
        assert!(!proc.should_trigger_recompile(&make_feedback(
            "t",
            FeedbackKind::ThumbsUp,
            None
        )));
    }

    #[test]
    fn test_build_prompt_context_empty() {
        let store = make_store();
        let proc = make_processor();

        let ctx = proc
            .build_prompt_context(&TopicId("t-none".to_string()), &store)
            .unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_build_prompt_context_mixed() {
        let store = make_store();
        let proc = make_processor();
        let tid = TopicId("t3".to_string());

        proc.record(
            make_feedback("t3", FeedbackKind::Correction("wrong date".into()), None),
            &store,
        )
        .unwrap();
        proc.record(
            make_feedback("t3", FeedbackKind::ThumbsUp, Some("good section")),
            &store,
        )
        .unwrap();
        proc.record(
            make_feedback("t3", FeedbackKind::ThumbsDown, Some("confusing")),
            &store,
        )
        .unwrap();

        let ctx = proc.build_prompt_context(&tid, &store).unwrap();
        assert!(ctx.contains("[CORRECTION]"));
        assert!(ctx.contains("[POSITIVE"));
        assert!(ctx.contains("[NEGATIVE]"));
        assert!(ctx.contains("wrong date"));
        assert!(ctx.contains("good section"));
        assert!(ctx.contains("confusing"));
    }

    #[test]
    fn test_build_prompt_context_resolved_excluded() {
        let store = make_store();
        let proc = make_processor();
        let tid = TopicId("t4".to_string());

        // Record two feedbacks
        proc.record(
            make_feedback("t4", FeedbackKind::Correction("old issue".into()), None),
            &store,
        )
        .unwrap();
        proc.record(
            make_feedback("t4", FeedbackKind::ThumbsDown, Some("bad")),
            &store,
        )
        .unwrap();

        // Resolve them
        store.mark_resolved(&tid, "compile-99").unwrap();

        // Context should be empty since all are resolved
        let ctx = proc.build_prompt_context(&tid, &store).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_record_converts_correctly() {
        let store = make_store();
        let proc = make_processor();

        let correction_text = "The year should be 2025, not 2024".to_string();
        let fb = TopicFeedback {
            topic_id: TopicId("t5".to_string()),
            kind: FeedbackKind::Correction(correction_text.clone()),
            comment: Some("please fix".to_string()),
            timestamp: Utc::now(),
        };

        let entry = proc.record(fb, &store).unwrap();

        assert_eq!(entry.topic_id.0, "t5");
        assert!(!entry.resolved);
        assert_eq!(entry.comment, Some("please fix".to_string()));
        match &entry.kind {
            FeedbackKind::Correction(text) => assert_eq!(text, &correction_text),
            _ => panic!("Expected Correction kind"),
        }
    }
}
