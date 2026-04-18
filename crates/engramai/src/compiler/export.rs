//! Export engine for serializing compiled knowledge into portable formats.
//!
//! Loads topic pages from a [`KnowledgeStore`], applies privacy enforcement
//! via [`PrivacyGuard`], and serializes to JSON or Markdown.

use super::privacy::{AccessContext, AccessDecision, PrivacyGuard};
use super::storage::KnowledgeStore;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  OUTPUT TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of an export operation.
#[derive(Debug, Clone)]
pub enum ExportOutput {
    /// Pretty-printed JSON string.
    Json(String),
    /// Collection of Markdown files, one per topic.
    Markdown(Vec<MarkdownFile>),
}

/// A single Markdown file produced by the export engine.
#[derive(Debug, Clone)]
pub struct MarkdownFile {
    /// Relative file path, e.g. `{topic_id}.md`.
    pub path: String,
    /// Full Markdown content including YAML frontmatter.
    pub content: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  EXPORT ENGINE
// ═══════════════════════════════════════════════════════════════════════════════

/// Serializes compiled knowledge into portable formats with privacy enforcement.
pub struct ExportEngine;

impl ExportEngine {
    /// Export topic pages matching a filter to the specified format.
    ///
    /// 1. Loads all topic pages from `store` and filters by `filter` criteria.
    /// 2. For each page, calls `privacy.check_access(page, ctx)`:
    ///    - `Deny` → skip silently
    ///    - `AllowRedacted` → redact content before export
    ///    - `Allow` → include as-is
    /// 3. Serializes to the requested format.
    pub fn export(
        store: &dyn KnowledgeStore,
        privacy: &PrivacyGuard,
        ctx: &AccessContext,
        filter: &ExportFilter,
        format: ExportFormat,
    ) -> Result<ExportOutput, KcError> {
        // Step 1: Load and filter topic pages.
        let all_pages = store.list_topic_pages()?;
        let filtered = Self::apply_filter(all_pages, filter);

        // Step 2: Apply privacy checks, collecting exportable pages.
        let mut export_pages: Vec<TopicPage> = Vec::new();

        for page in filtered {
            match privacy.check_access(&page, ctx) {
                AccessDecision::Deny { .. } => {
                    // Skip silently.
                }
                AccessDecision::AllowRedacted => {
                    let redacted = privacy.redact(&page);
                    let mut redacted_page = page.clone();
                    redacted_page.title = redacted.title;
                    redacted_page.content = redacted.content;
                    export_pages.push(redacted_page);
                }
                AccessDecision::Allow => {
                    export_pages.push(page);
                }
            }
        }

        // Step 3: Serialize to the requested format.
        match format {
            ExportFormat::Json => Self::to_json(&export_pages),
            ExportFormat::Markdown => Ok(Self::to_markdown(&export_pages)),
            ExportFormat::Html => Err(KcError::ExportError(
                "HTML export is not yet supported".to_string(),
            )),
        }
    }

    /// Filter topic pages by the criteria in [`ExportFilter`].
    fn apply_filter(pages: Vec<TopicPage>, filter: &ExportFilter) -> Vec<TopicPage> {
        pages
            .into_iter()
            .filter(|page| {
                // Filter by topic IDs.
                if let Some(ref topics) = filter.topics {
                    if !topics.contains(&page.id) {
                        return false;
                    }
                }

                // Filter by status.
                if let Some(ref statuses) = filter.status {
                    if !statuses.contains(&page.status) {
                        return false;
                    }
                }

                // Filter by tags (page must have at least one matching tag).
                if let Some(ref tags) = filter.tags {
                    let has_match = page.metadata.tags.iter().any(|t| tags.contains(t));
                    if !has_match {
                        return false;
                    }
                }

                // Filter by updated_at >= since.
                if let Some(since) = filter.since {
                    if page.metadata.updated_at < since {
                        return false;
                    }
                }

                true
            })
            .collect()
    }

    /// Serialize topic pages to pretty-printed JSON.
    fn to_json(pages: &[TopicPage]) -> Result<ExportOutput, KcError> {
        let json = serde_json::to_string_pretty(pages)
            .map_err(|e| KcError::ExportError(format!("JSON serialization failed: {e}")))?;
        Ok(ExportOutput::Json(json))
    }

    /// Serialize topic pages to Markdown files with YAML frontmatter.
    fn to_markdown(pages: &[TopicPage]) -> ExportOutput {
        let files = pages
            .iter()
            .map(|page| {
                let tags_str = page
                    .metadata
                    .tags
                    .iter()
                    .map(|t| format!("  - {t}"))
                    .collect::<Vec<_>>()
                    .join("\n");

                let frontmatter = format!(
                    "---\n\
                     id: {id}\n\
                     title: \"{title}\"\n\
                     status: {status:?}\n\
                     version: {version}\n\
                     created_at: {created_at}\n\
                     updated_at: {updated_at}\n\
                     tags:\n\
                     {tags}\n\
                     ---",
                    id = page.id.0,
                    title = page.title.replace('"', "\\\""),
                    status = page.status,
                    version = page.version,
                    created_at = page.metadata.created_at.to_rfc3339(),
                    updated_at = page.metadata.updated_at.to_rfc3339(),
                    tags = tags_str,
                );

                let content = format!(
                    "{frontmatter}\n\n# {title}\n\n{summary}\n\n{body}",
                    frontmatter = frontmatter,
                    title = page.title,
                    summary = page.summary,
                    body = page.content,
                );

                MarkdownFile {
                    path: format!("{}.md", page.id.0),
                    content,
                }
            })
            .collect();

        ExportOutput::Markdown(files)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::privacy::{AccessContext, PrivacyGuard};
    use crate::compiler::storage::SqliteKnowledgeStore;
    use chrono::Utc;

    fn make_store() -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        store
    }

    fn make_privacy() -> PrivacyGuard {
        PrivacyGuard::in_memory().unwrap()
    }

    fn export_context() -> AccessContext {
        AccessContext {
            accessor: "test-user".to_string(),
            include_private: false,
            is_export: true,
        }
    }

    fn sample_page(id: &str, title: &str, tags: Vec<&str>) -> TopicPage {
        let now = Utc::now();
        TopicPage {
            id: TopicId(id.to_owned()),
            title: title.to_owned(),
            content: format!("Content for {title}"),
            sections: Vec::new(),
            summary: format!("Summary of {title}"),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: vec!["mem-1".to_owned()],
                tags: tags.into_iter().map(|s| s.to_owned()).collect(),
                quality_score: Some(0.85),
            },
        }
    }

    #[test]
    fn test_export_json() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        store
            .create_topic_page(&sample_page("t1", "Rust Basics", vec!["rust"]))
            .unwrap();
        store
            .create_topic_page(&sample_page("t2", "Error Handling", vec!["rust"]))
            .unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();

        match output {
            ExportOutput::Json(json) => {
                assert!(json.contains("Rust Basics"), "JSON should contain 'Rust Basics'");
                assert!(
                    json.contains("Error Handling"),
                    "JSON should contain 'Error Handling'"
                );
                // Verify it's valid JSON array with 2 elements.
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.len(), 2);
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_markdown() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        store
            .create_topic_page(&sample_page("t1", "Rust Basics", vec!["rust"]))
            .unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Markdown)
                .unwrap();

        match output {
            ExportOutput::Markdown(files) => {
                assert_eq!(files.len(), 1);
                let file = &files[0];
                assert_eq!(file.path, "t1.md");
                assert!(file.content.contains("---"), "Should have frontmatter delimiters");
                assert!(file.content.contains("id: t1"), "Should have id in frontmatter");
                assert!(
                    file.content.contains("title: \"Rust Basics\""),
                    "Should have title in frontmatter"
                );
                assert!(
                    file.content.contains("# Rust Basics"),
                    "Should have H1 heading"
                );
                assert!(
                    file.content.contains("Content for Rust Basics"),
                    "Should have body content"
                );
                assert!(
                    file.content.contains("Summary of Rust Basics"),
                    "Should have summary"
                );
                assert!(
                    file.content.contains("  - rust"),
                    "Should have tags in frontmatter"
                );
            }
            _ => panic!("Expected Markdown output"),
        }
    }

    #[test]
    fn test_export_filter_by_status() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        store
            .create_topic_page(&sample_page("active1", "Active Topic", vec!["rust"]))
            .unwrap();

        let mut archived = sample_page("archived1", "Archived Topic", vec!["rust"]);
        archived.status = TopicStatus::Archived;
        store.create_topic_page(&archived).unwrap();

        let filter = ExportFilter {
            topics: None,
            status: Some(vec![TopicStatus::Active]),
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();

        match output {
            ExportOutput::Json(json) => {
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.len(), 1, "Only the active topic should be exported");
                assert_eq!(parsed[0]["title"], "Active Topic");
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_privacy_deny() {
        let store = make_store();
        let privacy = make_privacy();
        // Context without include_private — private topics will be denied.
        let ctx = AccessContext {
            accessor: "test-user".to_string(),
            include_private: false,
            is_export: true,
        };

        store
            .create_topic_page(&sample_page("pub1", "Public Topic", vec![]))
            .unwrap();
        store
            .create_topic_page(&sample_page(
                "priv1",
                "Private Topic",
                vec!["privacy:private"],
            ))
            .unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();

        match output {
            ExportOutput::Json(json) => {
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.len(), 1, "Private topic should be excluded");
                assert_eq!(parsed[0]["title"], "Public Topic");
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_privacy_redacted() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = AccessContext {
            accessor: "test-user".to_string(),
            include_private: false,
            is_export: true,
        };

        let now = Utc::now();
        let sensitive_page = TopicPage {
            id: TopicId("sens1".to_owned()),
            title: "Sensitive Topic".to_owned(),
            content: "Contact alice@example.com for details".to_owned(),
            sections: Vec::new(),
            summary: "Has sensitive info".to_owned(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: vec![],
                tags: vec!["privacy:sensitive".to_owned()],
                quality_score: None,
            },
        };
        store.create_topic_page(&sensitive_page).unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();

        match output {
            ExportOutput::Json(json) => {
                assert!(
                    !json.contains("alice@example.com"),
                    "Email should be redacted"
                );
                assert!(json.contains("[EMAIL-1]"), "Should have redaction placeholder");
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_empty() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        // Filter by a topic ID that doesn't exist.
        let filter = ExportFilter {
            topics: Some(vec![TopicId("nonexistent".to_owned())]),
            status: None,
            tags: None,
            since: None,
        };

        let output =
            ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();

        match output {
            ExportOutput::Json(json) => {
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
                assert!(parsed.is_empty(), "Should have no results");
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_filter_by_topic_ids() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        store.create_topic_page(&sample_page("pick-1", "First", vec![])).unwrap();
        store.create_topic_page(&sample_page("pick-2", "Second", vec![])).unwrap();
        store.create_topic_page(&sample_page("skip-3", "Third", vec![])).unwrap();

        let filter = ExportFilter {
            topics: Some(vec![TopicId("pick-1".into()), TopicId("pick-2".into())]),
            status: None,
            tags: None,
            since: None,
        };

        let output = ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Json).unwrap();
        match output {
            ExportOutput::Json(json) => {
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.len(), 2);
            }
            _ => panic!("Expected JSON output"),
        }
    }

    #[test]
    fn test_export_markdown_multiple() {
        let store = make_store();
        let privacy = make_privacy();
        let ctx = export_context();

        store.create_topic_page(&sample_page("md-1", "First Topic", vec!["tag1"])).unwrap();
        store.create_topic_page(&sample_page("md-2", "Second Topic", vec!["tag2"])).unwrap();

        let filter = ExportFilter { topics: None, status: None, tags: None, since: None };

        let output = ExportEngine::export(&store, &privacy, &ctx, &filter, ExportFormat::Markdown).unwrap();
        match output {
            ExportOutput::Markdown(files) => {
                assert_eq!(files.len(), 2);
                // Files should be named by topic id
                let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
                assert!(paths.contains(&"md-1.md"));
                assert!(paths.contains(&"md-2.md"));
            }
            _ => panic!("Expected Markdown output"),
        }
    }
}
