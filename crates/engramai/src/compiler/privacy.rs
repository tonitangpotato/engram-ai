//! Privacy guard for access control, redaction, and audit logging.
//!
//! Provides:
//! - Access decision logic based on [`PrivacyLevel`] and context
//! - Entity redaction (emails, API keys, IP addresses, file paths)
//! - Audit logging to a SQLite-backed trail

use chrono::Utc;
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// Access decision from the privacy guard.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessDecision {
    /// Full access allowed.
    Allow,
    /// Access allowed but content must be redacted.
    AllowRedacted,
    /// Access denied.
    Deny { reason: String },
}

/// Context for access checks.
#[derive(Debug, Clone)]
pub struct AccessContext {
    /// Who is accessing (CLI user, API caller).
    pub accessor: String,
    /// Explicitly requesting private topics.
    pub include_private: bool,
    /// Export context (stricter redaction).
    pub is_export: bool,
}

/// Audit action types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    Query,
    Inspect,
    Export,
    PrivacyLevelChange { from: String, to: String },
}

/// Redacted version of a topic page.
#[derive(Debug, Clone)]
pub struct RedactedTopic {
    pub id: TopicId,
    pub title: String,
    pub content: String,
    pub redaction_count: usize,
}

/// Entity types that can be detected and redacted.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityType {
    PersonName,
    Email,
    ApiKey,
    IpAddress,
    FilePath,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TopicPage HELPER
// ═══════════════════════════════════════════════════════════════════════════════

/// Extract a [`PrivacyLevel`] from a [`TopicPage`] by inspecting its metadata
/// tags. Defaults to [`PrivacyLevel::Public`] when no privacy tag is present.
pub fn topic_privacy_level(page: &TopicPage) -> PrivacyLevel {
    for tag in &page.metadata.tags {
        match tag.as_str() {
            "privacy:private" => return PrivacyLevel::Private,
            "privacy:sensitive" => return PrivacyLevel::Sensitive,
            "privacy:internal" => return PrivacyLevel::Internal,
            _ => {}
        }
    }
    PrivacyLevel::Public
}

// ═══════════════════════════════════════════════════════════════════════════════
//  PRIVACY GUARD
// ═══════════════════════════════════════════════════════════════════════════════

/// Enforces access control, redacts sensitive entities, and maintains an
/// audit trail of all access events.
pub struct PrivacyGuard {
    /// Audit log connection (separate from main knowledge store).
    audit_conn: Connection,
    /// Entity types to redact.
    redact_entities: Vec<EntityType>,
    /// Whether to redact file paths.
    redact_paths: bool,
}

impl PrivacyGuard {
    /// Create a new privacy guard with its own audit log DB connection.
    pub fn new(audit_conn: Connection) -> Result<Self, KcError> {
        let guard = Self {
            audit_conn,
            redact_entities: vec![
                EntityType::Email,
                EntityType::ApiKey,
                EntityType::IpAddress,
            ],
            redact_paths: true,
        };
        guard.init_audit_schema()?;
        Ok(guard)
    }

    /// Create for testing with in-memory DB.
    pub fn in_memory() -> Result<Self, KcError> {
        let conn =
            Connection::open_in_memory().map_err(|e| KcError::Storage(e.to_string()))?;
        Self::new(conn)
    }

    fn init_audit_schema(&self) -> Result<(), KcError> {
        self.audit_conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS kc_audit_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    topic_id TEXT,
                    actor TEXT NOT NULL,
                    details TEXT NOT NULL DEFAULT ''
                );
                CREATE INDEX IF NOT EXISTS idx_kc_audit_topic
                    ON kc_audit_log(topic_id);
                CREATE INDEX IF NOT EXISTS idx_kc_audit_time
                    ON kc_audit_log(timestamp);
                ",
            )
            .map_err(|e| KcError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Check access to a topic based on its privacy level and context.
    pub fn check_access(&self, page: &TopicPage, context: &AccessContext) -> AccessDecision {
        let privacy = topic_privacy_level(page);
        match privacy {
            PrivacyLevel::Public => AccessDecision::Allow,
            PrivacyLevel::Internal => {
                if context.is_export {
                    AccessDecision::AllowRedacted
                } else {
                    AccessDecision::Allow
                }
            }
            PrivacyLevel::Sensitive => {
                if context.is_export {
                    AccessDecision::AllowRedacted
                } else {
                    AccessDecision::Allow
                }
            }
            PrivacyLevel::Private => {
                if context.include_private {
                    if context.is_export {
                        AccessDecision::AllowRedacted
                    } else {
                        AccessDecision::Allow
                    }
                } else {
                    AccessDecision::Deny {
                        reason: "Private topic requires --include-private flag".to_string(),
                    }
                }
            }
        }
    }

    /// Redact sensitive entities from topic content.
    pub fn redact(&self, page: &TopicPage) -> RedactedTopic {
        let mut content = page.content.clone();
        let mut title = page.title.clone();
        let mut count = 0;

        // Email pattern
        let email_re =
            Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").unwrap();
        // API key pattern (long hex/base64 strings)
        let api_key_re =
            Regex::new(r"(?:sk-|key-|api[_-]?key[=: ]+)[a-zA-Z0-9_-]{20,}").unwrap();
        // IP address pattern
        let ip_re = Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap();
        // File path pattern (Unix paths)
        let path_re = Regex::new(r"(?:/[a-zA-Z0-9._-]+){2,}").unwrap();

        if self.redact_entities.contains(&EntityType::Email) {
            let (new_content, n) = redact_pattern(&content, &email_re, "EMAIL");
            content = new_content;
            count += n;
            let (new_title, n) = redact_pattern(&title, &email_re, "EMAIL");
            title = new_title;
            count += n;
        }

        if self.redact_entities.contains(&EntityType::ApiKey) {
            let (new_content, n) = redact_pattern(&content, &api_key_re, "API_KEY");
            content = new_content;
            count += n;
            let (new_title, n) = redact_pattern(&title, &api_key_re, "API_KEY");
            title = new_title;
            count += n;
        }

        if self.redact_entities.contains(&EntityType::IpAddress) {
            let (new_content, n) = redact_pattern(&content, &ip_re, "IP_ADDR");
            content = new_content;
            count += n;
        }

        if self.redact_paths {
            let (new_content, n) = redact_pattern(&content, &path_re, "PATH");
            content = new_content;
            count += n;
        }

        RedactedTopic {
            id: page.id.clone(),
            title,
            content,
            redaction_count: count,
        }
    }

    /// Log an access event to the audit log.
    ///
    /// Maps to the [`AuditEntry`] schema: `operation` receives the serialised
    /// [`AuditAction`], `actor` is the accessor identity, and `details`
    /// captures privacy level plus redaction status.
    pub fn log_access(
        &self,
        topic_id: &TopicId,
        action: &AuditAction,
        accessor: &str,
        privacy_level: &PrivacyLevel,
        was_redacted: bool,
    ) -> Result<(), KcError> {
        let operation =
            serde_json::to_string(action).map_err(|e| KcError::Storage(e.to_string()))?;
        let details = format!(
            "privacy_level={:?}, was_redacted={}",
            privacy_level, was_redacted
        );

        self.audit_conn
            .execute(
                "INSERT INTO kc_audit_log (timestamp, operation, topic_id, actor, details)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    Utc::now().to_rfc3339(),
                    operation,
                    topic_id.as_ref(),
                    accessor,
                    details,
                ],
            )
            .map_err(|e| KcError::Storage(e.to_string()))?;

        Ok(())
    }

    /// Query the audit log, optionally filtering by topic.
    pub fn query_audit_log(
        &self,
        topic_id: Option<&TopicId>,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, KcError> {
        let mut results = Vec::new();

        if let Some(tid) = topic_id {
            let mut stmt = self
                .audit_conn
                .prepare(
                    "SELECT timestamp, operation, topic_id, actor, details
                     FROM kc_audit_log WHERE topic_id = ?1
                     ORDER BY timestamp DESC LIMIT ?2",
                )
                .map_err(|e| KcError::Storage(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![tid.as_ref(), limit], row_to_audit_entry)
                .map_err(|e| KcError::Storage(e.to_string()))?;

            for row in rows {
                results.push(row.map_err(|e| KcError::Storage(e.to_string()))?);
            }
        } else {
            let mut stmt = self
                .audit_conn
                .prepare(
                    "SELECT timestamp, operation, topic_id, actor, details
                     FROM kc_audit_log
                     ORDER BY timestamp DESC LIMIT ?1",
                )
                .map_err(|e| KcError::Storage(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![limit], row_to_audit_entry)
                .map_err(|e| KcError::Storage(e.to_string()))?;

            for row in rows {
                results.push(row.map_err(|e| KcError::Storage(e.to_string()))?);
            }
        }

        Ok(results)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Map a SQLite row to an [`AuditEntry`].
fn row_to_audit_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    let timestamp_str: String = row.get(0)?;
    let operation: String = row.get(1)?;
    let topic_id: Option<String> = row.get(2)?;
    let actor: String = row.get(3)?;
    let details: String = row.get(4)?;

    let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_default();

    Ok(AuditEntry {
        timestamp,
        operation,
        topic_id: topic_id.map(TopicId),
        actor,
        details,
    })
}

/// Replace all occurrences of `pattern` in `text` with numbered placeholders
/// like `[EMAIL-1]`, `[EMAIL-2]`, etc. Returns the new string and the count
/// of replacements made.
fn redact_pattern(text: &str, pattern: &Regex, entity_type: &str) -> (String, usize) {
    let mut count = 0usize;
    let result = pattern.replace_all(text, |_caps: &regex::Captures| {
        count += 1;
        format!("[{}-{}]", entity_type, count)
    });
    (result.into_owned(), count)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_page(id: &str, tags: Vec<&str>) -> TopicPage {
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
                source_memory_ids: vec![],
                tags: tags.into_iter().map(|s| s.to_owned()).collect(),
                quality_score: None,
            },
        }
    }

    fn default_context() -> AccessContext {
        AccessContext {
            accessor: "test-user".to_string(),
            include_private: false,
            is_export: false,
        }
    }

    #[test]
    fn test_access_public() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let page = sample_page("pub1", vec![]);
        let decision = guard.check_access(&page, &default_context());
        assert_eq!(decision, AccessDecision::Allow);
    }

    #[test]
    fn test_access_private_denied() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let page = sample_page("priv1", vec!["privacy:private"]);
        let ctx = default_context(); // include_private = false
        let decision = guard.check_access(&page, &ctx);
        assert!(matches!(decision, AccessDecision::Deny { .. }));
    }

    #[test]
    fn test_access_sensitive_export() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let page = sample_page("sens1", vec!["privacy:sensitive"]);
        let ctx = AccessContext {
            accessor: "test-user".to_string(),
            include_private: false,
            is_export: true,
        };
        let decision = guard.check_access(&page, &ctx);
        assert_eq!(decision, AccessDecision::AllowRedacted);
    }

    #[test]
    fn test_redaction() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let now = Utc::now();
        let page = TopicPage {
            id: TopicId("redact1".to_owned()),
            title: "Contact Info".to_owned(),
            content: "Email me at alice@example.com, server at 192.168.1.1".to_owned(),
            sections: Vec::new(),
            summary: String::new(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 0,
                source_memory_ids: vec![],
                tags: vec![],
                quality_score: None,
            },
        };

        let redacted = guard.redact(&page);
        assert!(
            redacted.content.contains("[EMAIL-1]"),
            "Expected [EMAIL-1] in: {}",
            redacted.content
        );
        assert!(
            redacted.content.contains("[IP_ADDR-1]"),
            "Expected [IP_ADDR-1] in: {}",
            redacted.content
        );
        assert!(!redacted.content.contains("alice@example.com"));
        assert!(!redacted.content.contains("192.168.1.1"));
        assert!(redacted.redaction_count >= 2);
    }

    #[test]
    fn test_audit_log() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let topic_id = TopicId("audit1".to_owned());

        guard
            .log_access(
                &topic_id,
                &AuditAction::Query,
                "cli-user",
                &PrivacyLevel::Sensitive,
                false,
            )
            .unwrap();

        let entries = guard.query_audit_log(Some(&topic_id), 10).unwrap();
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.topic_id.as_ref().unwrap().0, "audit1");
        assert_eq!(entry.actor, "cli-user");
        assert!(entry.operation.contains("Query"));
        assert!(entry.details.contains("Sensitive"));
    }

    #[test]
    fn test_topic_privacy_level_detection() {
        // No privacy tag → Public
        let public = sample_page("pl-1", vec!["rust", "memory"]);
        assert_eq!(topic_privacy_level(&public), PrivacyLevel::Public);

        // privacy:private tag
        let private = sample_page("pl-2", vec!["privacy:private"]);
        assert_eq!(topic_privacy_level(&private), PrivacyLevel::Private);

        // privacy:sensitive tag
        let sensitive = sample_page("pl-3", vec!["privacy:sensitive"]);
        assert_eq!(topic_privacy_level(&sensitive), PrivacyLevel::Sensitive);

        // privacy:internal tag
        let internal = sample_page("pl-4", vec!["privacy:internal"]);
        assert_eq!(topic_privacy_level(&internal), PrivacyLevel::Internal);
    }

    #[test]
    fn test_access_private_allowed_with_flag() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let page = sample_page("priv-allow", vec!["privacy:private"]);
        let ctx = AccessContext {
            accessor: "admin".to_string(),
            include_private: true,
            is_export: false,
        };
        let decision = guard.check_access(&page, &ctx);
        assert_eq!(decision, AccessDecision::Allow);
    }

    #[test]
    fn test_redaction_no_sensitive_content() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let now = Utc::now();
        let page = TopicPage {
            id: TopicId("no-redact".to_owned()),
            title: "Safe Topic".to_owned(),
            content: "Nothing sensitive here, just plain text".to_owned(),
            sections: Vec::new(),
            summary: String::new(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 0,
                source_memory_ids: vec![],
                tags: vec![],
                quality_score: None,
            },
        };

        let redacted = guard.redact(&page);
        assert_eq!(redacted.redaction_count, 0);
        assert_eq!(redacted.content, page.content);
    }

    #[test]
    fn test_audit_log_multiple_entries() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let topic_id = TopicId("multi-audit".to_owned());

        for action in &[AuditAction::Query, AuditAction::Export, AuditAction::Inspect] {
            guard.log_access(&topic_id, action, "user", &PrivacyLevel::Public, false).unwrap();
        }

        let entries = guard.query_audit_log(Some(&topic_id), 10).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_audit_log_limit() {
        let guard = PrivacyGuard::in_memory().unwrap();
        let topic_id = TopicId("limit-audit".to_owned());

        for _ in 0..10 {
            guard.log_access(&topic_id, &AuditAction::Query, "user", &PrivacyLevel::Public, false).unwrap();
        }

        let entries = guard.query_audit_log(Some(&topic_id), 3).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_audit_log_no_topic_filter() {
        let guard = PrivacyGuard::in_memory().unwrap();

        guard.log_access(&TopicId("a".into()), &AuditAction::Query, "user", &PrivacyLevel::Public, false).unwrap();
        guard.log_access(&TopicId("b".into()), &AuditAction::Export, "user", &PrivacyLevel::Private, false).unwrap();

        // No topic filter → returns all
        let entries = guard.query_audit_log(None, 10).unwrap();
        assert_eq!(entries.len(), 2);
    }
}
