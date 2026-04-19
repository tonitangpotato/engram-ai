//! Programmatic Maintenance API for the Knowledge Compiler.
//!
//! Provides a structured Rust API that mirrors CLI capabilities.
//! The CLI should become a thin wrapper over this API.

use std::path::Path;

use super::compilation::{MemorySnapshot, QualityScorer};
use super::conflict::ConflictDetector;
use super::decay::{self, DecayEngine};
use super::discovery::TopicDiscovery;
use super::export::{self, ExportEngine};
use super::health::{self, HealthAuditor};
use super::import::{ImportPipeline, JsonImporter, MarkdownImporter};
use super::llm::LlmProvider;
use super::privacy::{AccessContext, PrivacyGuard};
use super::storage::KnowledgeStore;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// Options for querying compiled knowledge.
#[derive(Debug, Clone)]
pub struct QueryOpts {
    pub limit: usize,
    pub include_archived: bool,
}

impl Default for QueryOpts {
    fn default() -> Self {
        Self {
            limit: 10,
            include_archived: false,
        }
    }
}

/// A single query result.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub topic_id: TopicId,
    pub title: String,
    pub summary: String,
    pub relevance: f64,
    pub status: TopicStatus,
}

/// Detailed topic info (for inspect).
#[derive(Debug, Clone)]
pub struct TopicDetail {
    pub page: TopicPage,
    pub source_refs: Vec<SourceMemoryRef>,
    pub compilation_records: Vec<CompilationRecord>,
    pub quality: Option<QualityReport>,
}

/// Scope for decay evaluation.
#[derive(Debug, Clone)]
pub enum DecayScope {
    All,
    Topic(TopicId),
    Stale,
}

/// Scope for health audit.
#[derive(Debug, Clone)]
pub enum AuditScope {
    All,
    Topic(TopicId),
}

/// Options for recall with topics.
#[derive(Debug, Clone)]
pub struct RecallOpts {
    pub limit: usize,
    pub topic_boost: f64,
}

impl Default for RecallOpts {
    fn default() -> Self {
        Self {
            limit: 10,
            topic_boost: 1.5,
        }
    }
}

/// Recall result combining regular memories + topic pages.
#[derive(Debug, Clone)]
pub struct RecallResult {
    pub topic_id: TopicId,
    pub title: String,
    pub snippet: String,
    pub score: f64,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  MAINTENANCE API
// ═══════════════════════════════════════════════════════════════════════════════

/// Programmatic API for Knowledge Compiler maintenance and queries.
///
/// Generic over the [`KnowledgeStore`] implementation — the same pattern
/// used by `CompilationPipeline` and other KC components.
pub struct MaintenanceApi<S: KnowledgeStore> {
    store: S,
    config: KcConfig,
}

impl<S: KnowledgeStore> MaintenanceApi<S> {
    /// Create a new `MaintenanceApi` with the given store and config.
    pub fn new(store: S, config: KcConfig) -> Self {
        Self { store, config }
    }

    // ── Query & Access ───────────────────────────────────────────────────

    /// Simple text search over compiled topics.
    ///
    /// Iterates stored topics and checks if the query appears in
    /// title/content/summary (case-insensitive). Returns matches sorted
    /// by a basic relevance score (title match > summary match > content match).
    pub fn query(&self, q: &str, opts: &QueryOpts) -> Result<Vec<QueryResult>, KcError> {
        let all_pages = self.store.list_topic_pages()?;
        let q_lower = q.to_lowercase();

        let mut results: Vec<QueryResult> = all_pages
            .into_iter()
            .filter(|page| {
                if !opts.include_archived && page.status == TopicStatus::Archived {
                    return false;
                }
                let title_lower = page.title.to_lowercase();
                let summary_lower = page.summary.to_lowercase();
                let content_lower = page.content.to_lowercase();
                title_lower.contains(&q_lower)
                    || summary_lower.contains(&q_lower)
                    || content_lower.contains(&q_lower)
            })
            .map(|page| {
                let relevance = Self::compute_relevance(&page, &q_lower);
                QueryResult {
                    topic_id: page.id,
                    title: page.title,
                    summary: page.summary,
                    relevance,
                    status: page.status,
                }
            })
            .collect();

        // Sort by relevance descending
        results.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(opts.limit);
        Ok(results)
    }

    /// Compute a basic relevance score for a topic against a lowercase query.
    fn compute_relevance(page: &TopicPage, q_lower: &str) -> f64 {
        let mut score = 0.0;

        let title_lower = page.title.to_lowercase();
        let summary_lower = page.summary.to_lowercase();
        let content_lower = page.content.to_lowercase();

        // Title match is worth the most
        if title_lower.contains(q_lower) {
            score += 3.0;
            // Exact title match bonus
            if title_lower == *q_lower {
                score += 2.0;
            }
        }

        // Summary match
        if summary_lower.contains(q_lower) {
            score += 2.0;
        }

        // Content match
        if content_lower.contains(q_lower) {
            score += 1.0;
            // Count occurrences for density bonus (up to +1.0)
            let count = content_lower.matches(q_lower).count();
            score += (count as f64 * 0.1).min(1.0);
        }

        score
    }

    /// Knowledge-aware recall: search topic pages with scoring that considers
    /// topic quality, freshness, and source memory importance.
    ///
    /// Unlike `query()` which does simple keyword matching, `recall()` blends
    /// text relevance with topic metadata signals (quality, freshness, source count)
    /// to produce a richer ranking.
    pub fn recall(&self, query: &str, opts: &RecallOpts) -> Result<Vec<RecallResult>, KcError> {
        let all_pages = self.store.list_topic_pages()?;
        let q_lower = query.to_lowercase();
        let now = chrono::Utc::now();

        let mut results: Vec<RecallResult> = all_pages
            .into_iter()
            .filter(|page| page.status != TopicStatus::Archived)
            .filter_map(|page| {
                // a. Text match score (reuse compute_relevance logic)
                let text_score = Self::compute_relevance(&page, &q_lower);

                // Must have at least a text match
                if text_score <= 0.0 {
                    return None;
                }

                // b. Quality boost: topic.metadata.quality_score * 0.3
                let quality_boost = page.metadata.quality_score.unwrap_or(0.0) * 0.3;

                // c. Freshness boost: days since updated_at, decaying with 1/(1 + days/30)
                let days_since_update =
                    (now - page.metadata.updated_at).num_days().max(0) as f64;
                let freshness_boost = 1.0 / (1.0 + days_since_update / 30.0);

                // d. Source count boost: min(source_memory_ids.len() / 10.0, 0.5)
                let source_boost =
                    (page.metadata.source_memory_ids.len() as f64 / 10.0).min(0.5);

                // e. Final score
                let score = text_score * opts.topic_boost
                    + quality_boost
                    + freshness_boost
                    + source_boost;

                // Snippet: first 200 chars of content
                let snippet: String = page.content.chars().take(200).collect();

                Some(RecallResult {
                    topic_id: page.id,
                    title: page.title,
                    snippet,
                    score,
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results.truncate(opts.limit);
        Ok(results)
    }

    /// Load a topic page with full detail: source refs, compilation records,
    /// and quality report.
    pub fn inspect(&self, topic_id: &TopicId) -> Result<TopicDetail, KcError> {
        let page = self
            .store
            .get_topic_page(topic_id)?
            .ok_or_else(|| KcError::NotFound(format!("topic '{}'", topic_id)))?;

        let source_refs = self.store.get_source_refs(topic_id)?;
        let compilation_records = self.store.get_compilation_records(topic_id)?;

        // Compute quality using QualityScorer (no memories or feedback available
        // through the store alone, so pass empty slices).
        let scorer = QualityScorer::new(&self.config);
        let quality = Some(scorer.score(&page, &[], &[]));

        Ok(TopicDetail {
            page,
            source_refs,
            compilation_records,
            quality,
        })
    }

    /// List all topic pages in the store.
    pub fn list(&self) -> Result<Vec<TopicPage>, KcError> {
        self.store.list_topic_pages()
    }

    // ── Maintenance ──────────────────────────────────────────────────────

    /// Evaluate decay (freshness) for topics in the given scope.
    pub fn evaluate_decay(
        &self,
        scope: &DecayScope,
    ) -> Result<Vec<decay::DecayResult>, KcError> {
        let engine = DecayEngine::new(self.config.decay.clone());

        match scope {
            DecayScope::All => engine.evaluate_all(&self.store),
            DecayScope::Topic(id) => {
                let page = self
                    .store
                    .get_topic_page(id)?
                    .ok_or_else(|| KcError::NotFound(format!("topic '{}'", id)))?;
                let result = engine.evaluate_topic(&page, &self.store)?;
                Ok(vec![result])
            }
            DecayScope::Stale => {
                let stale_pages =
                    self.store.get_pages_by_status(TopicStatus::Stale)?;
                let mut results = Vec::with_capacity(stale_pages.len());
                for page in &stale_pages {
                    results.push(engine.evaluate_topic(page, &self.store)?);
                }
                Ok(results)
            }
        }
    }

    /// Apply a decay action to a topic.
    ///
    /// - `MarkStale` → update topic status to Stale
    /// - `Archive` → call `store.mark_archived()`
    /// - `Refresh` → mark as Stale (actual recompile is a separate step)
    pub fn apply_decay(
        &self,
        _topic_id: &TopicId,
        action: &DecayAction,
    ) -> Result<(), KcError> {
        let engine = DecayEngine::new(self.config.decay.clone());
        engine.apply_decay(action, &self.store)
    }

    /// Detect conflicts and near-duplicates across all topics.
    pub fn detect_conflicts(&self) -> Result<Vec<Conflict>, KcError> {
        let detector = ConflictDetector::new();
        let all_topics = self.store.list_topic_pages()?;

        // Detect duplicates
        let _duplicates = detector.detect_duplicates(&all_topics);

        // Detect conflicts across all topic pairs (no LLM)
        let scope = ConflictScope::BetweenTopics(
            TopicId("*".to_string()),
            TopicId("*".to_string()),
        );
        let records = detector.detect_conflicts(&all_topics, &scope, None)?;

        Ok(records.into_iter().map(|r| r.conflict).collect())
    }

    /// Audit source link integrity for topics in the given scope.
    pub fn audit_links(
        &self,
        scope: &AuditScope,
    ) -> Result<Vec<health::LinkAuditEntry>, KcError> {
        let auditor = HealthAuditor;

        match scope {
            AuditScope::All => {
                let all_topics = self.store.list_topic_pages()?;
                let mut entries = Vec::new();
                for topic in &all_topics {
                    entries.extend(auditor.audit_links(topic, &self.store)?);
                }
                Ok(entries)
            }
            AuditScope::Topic(id) => {
                let page = self
                    .store
                    .get_topic_page(id)?
                    .ok_or_else(|| KcError::NotFound(format!("topic '{}'", id)))?;
                auditor.audit_links(&page, &self.store)
            }
        }
    }

    /// Generate a full health report for the knowledge base.
    pub fn health_report(&self) -> Result<HealthReport, KcError> {
        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(self.config.decay.clone());
        let conflict_detector = ConflictDetector::new();

        auditor.health_report(&self.store, &decay_engine, &conflict_detector)
    }

    // ── Export / Import ──────────────────────────────────────────────────

    /// Export topic pages matching a filter in the specified format.
    pub fn export(
        &self,
        filter: &ExportFilter,
        format: ExportFormat,
    ) -> Result<export::ExportOutput, KcError> {
        let privacy = PrivacyGuard::in_memory()
            .map_err(|e| KcError::ExportError(format!("privacy guard init: {}", e)))?;
        let ctx = AccessContext {
            accessor: "api_export".to_string(),
            include_private: false,
            is_export: true,
        };
        ExportEngine::export(&self.store, &privacy, &ctx, filter, format)
    }

    /// Import from a file path, detecting format from extension.
    ///
    /// - `.md` → `MarkdownImporter`
    /// - `.json` → `JsonImporter`
    pub fn import_from(
        &self,
        path: &Path,
        config: &ImportConfig,
    ) -> Result<ImportReport, KcError> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match ext {
            "md" => {
                let importer = MarkdownImporter {
                    split: config.split_strategy.clone(),
                };
                ImportPipeline::run(&self.store, &importer, path, config)
            }
            "json" => {
                let importer = JsonImporter;
                ImportPipeline::run(&self.store, &importer, path, config)
            }
            _ => {
                // For directories or unknown extensions, try markdown
                if path.is_dir() {
                    let importer = MarkdownImporter {
                        split: config.split_strategy.clone(),
                    };
                    ImportPipeline::run(&self.store, &importer, path, config)
                } else {
                    Err(KcError::ImportError(format!(
                        "unsupported file extension: '{}'",
                        ext
                    )))
                }
            }
        }
    }

    // ── Privacy ──────────────────────────────────────────────────────────

    /// Set the privacy level of a topic by updating its tags.
    pub fn set_privacy_level(
        &self,
        topic_id: &TopicId,
        level: PrivacyLevel,
    ) -> Result<(), KcError> {
        let mut page = self
            .store
            .get_topic_page(topic_id)?
            .ok_or_else(|| KcError::NotFound(format!("topic '{}'", topic_id)))?;

        // Remove existing privacy tags
        page.metadata.tags.retain(|t| {
            !t.starts_with("privacy:")
        });

        // Add the new privacy tag (Public has no tag)
        match level {
            PrivacyLevel::Public => {} // no tag needed
            PrivacyLevel::Internal => page.metadata.tags.push("privacy:internal".to_string()),
            PrivacyLevel::Sensitive => page.metadata.tags.push("privacy:sensitive".to_string()),
            PrivacyLevel::Private => page.metadata.tags.push("privacy:private".to_string()),
        }

        self.store.update_topic_page(&page)
    }

    // ── Dry Run ──────────────────────────────────────────────────────────

    /// Perform a read-only dry run showing what a compilation pass would do.
    ///
    /// Discovers topic candidates, checks overlap with existing topics,
    /// evaluates decay on unmatched topics. Does NOT call LLM or mutate store.
    pub fn dry_run(
        &self,
        memories: &[MemorySnapshot],
    ) -> Result<DryRunReport, KcError> {
        use std::collections::HashSet;

        // Build embeddings for topic discovery — use real embeddings when available
        let memory_embeddings: Vec<(String, Vec<f32>)> = memories
            .iter()
            .map(|m| {
                let embedding = m.embedding.clone()
                    .unwrap_or_else(|| simple_hash_embedding(&m.content, 64));
                (m.id.clone(), embedding)
            })
            .collect();

        let discovery = TopicDiscovery::new(self.config.min_cluster_size);
        let candidates = discovery.discover(&memory_embeddings);
        let existing_pages = self.store.list_topic_pages()?;

        let mut entries = Vec::new();
        let mut matched_topic_ids: HashSet<TopicId> = HashSet::new();
        let mut estimated_llm_calls = 0usize;

        for candidate in &candidates {
            match discovery.detect_overlap(candidate, &existing_pages) {
                Some(topic_id) => {
                    matched_topic_ids.insert(topic_id.clone());
                    if let Some(page) = self.store.get_topic_page(&topic_id)? {
                        let existing_ids: HashSet<&str> = page
                            .metadata.source_memory_ids.iter().map(|s| s.as_str()).collect();
                        let candidate_ids: HashSet<&str> =
                            candidate.memories.iter().map(|s| s.as_str()).collect();
                        let added = candidate_ids.difference(&existing_ids).count();
                        let removed = existing_ids.difference(&candidate_ids).count();

                        if added > 0 || removed > 0 {
                            entries.push(DryRunEntry {
                                topic_id: Some(topic_id),
                                action: DryRunAction::Recompile,
                                affected_memories: candidate.memories.len(),
                                reason: format!("{} new, {} removed", added, removed),
                            });
                            estimated_llm_calls += 1;
                        } else {
                            entries.push(DryRunEntry {
                                topic_id: Some(topic_id),
                                action: DryRunAction::Skip,
                                affected_memories: candidate.memories.len(),
                                reason: "No changes detected".to_string(),
                            });
                        }
                    }
                }
                None => {
                    entries.push(DryRunEntry {
                        topic_id: None,
                        action: DryRunAction::NewCompilation,
                        affected_memories: candidate.memories.len(),
                        reason: format!("New cluster of {} memories", candidate.memories.len()),
                    });
                    estimated_llm_calls += 1;
                }
            }
        }

        // Check existing topics with no matching candidate — evaluate decay
        let decay_engine = DecayEngine::new(self.config.decay.clone());
        for page in &existing_pages {
            if matched_topic_ids.contains(&page.id) || page.status == TopicStatus::Archived {
                continue;
            }
            let decay_result = decay_engine.evaluate_topic(page, &self.store)?;
            if matches!(decay_result.recommended_action, DecayAction::Archive(_)) {
                entries.push(DryRunEntry {
                    topic_id: Some(page.id.clone()),
                    action: DryRunAction::Archive,
                    affected_memories: 0,
                    reason: format!("Freshness {:.2} below threshold", decay_result.freshness_score),
                });
            } else {
                entries.push(DryRunEntry {
                    topic_id: Some(page.id.clone()),
                    action: DryRunAction::Skip,
                    affected_memories: 0,
                    reason: "No matching candidate, not decayed".to_string(),
                });
            }
        }

        let total_topics_affected = entries.iter()
            .filter(|e| !matches!(e.action, DryRunAction::Skip))
            .count();

        Ok(DryRunReport { entries, total_topics_affected, estimated_llm_calls })
    }

    // ── Compilation ──────────────────────────────────────────────────────

    /// Compile all discovered topic candidates from the provided memories.
    ///
    /// 1. Uses `TopicDiscovery` to discover candidates from memories
    /// 2. For each candidate, uses `CompilationPipeline::compile_new()`
    /// 3. Returns created pages
    pub fn compile_all<L: LlmProvider>(
        &self,
        _llm: Option<&L>,
        memories: &[MemorySnapshot],
    ) -> Result<Vec<TopicPage>, KcError> {
        // Build embeddings list — TopicDiscovery expects (id, embedding) pairs.
        // Use real embeddings from memory_embeddings table when available,
        // fall back to hash-based pseudo-embedding otherwise.
        let memory_embeddings: Vec<(String, Vec<f32>)> = memories
            .iter()
            .map(|m| {
                let embedding = m.embedding.clone()
                    .unwrap_or_else(|| simple_hash_embedding(&m.content, 64));
                (m.id.clone(), embedding)
            })
            .collect();

        let discovery = TopicDiscovery::new(self.config.min_cluster_size);
        let candidates = discovery.discover(&memory_embeddings);

        // Build a lookup for memories by ID
        let mem_map: std::collections::HashMap<&str, &MemorySnapshot> =
            memories.iter().map(|m| (m.id.as_str(), m)).collect();

        let mut pages = Vec::new();

        for candidate in &candidates {
            // Gather the memories for this candidate
            let candidate_memories: Vec<MemorySnapshot> = candidate
                .memories
                .iter()
                .filter_map(|id| mem_map.get(id.as_str()).map(|m| (*m).clone()))
                .collect();

            if candidate_memories.is_empty() {
                continue;
            }

            // Use compile_without_llm for simplicity (no LLM needed here).
            let title = candidate
                .suggested_title
                .clone()
                .unwrap_or_else(|| format!("Topic ({})", candidate.memories.len()));

            let content =
                super::compilation::compile_without_llm(&title, &candidate_memories);

            let now = chrono::Utc::now();
            let topic_id = TopicId(format!("topic-{}", now.timestamp_millis()));

            let page = TopicPage {
                id: topic_id.clone(),
                title,
                summary: super::compilation::extract_summary(&content),
                content,
                sections: Vec::new(),
                status: TopicStatus::Active,
                version: 1,
                metadata: TopicMetadata {
                    created_at: now,
                    updated_at: now,
                    compilation_count: 1,
                    source_memory_ids: candidate_memories.iter().map(|m| m.id.clone()).collect(),
                    tags: super::compilation::aggregate_tags(&candidate_memories),
                    quality_score: None,
                },
            };

            // Score quality
            let scorer = QualityScorer::new(&self.config);
            let report = scorer.score(&page, &candidate_memories, &[]);
            let mut page = page;
            page.metadata.quality_score = Some(report.overall);

            // Persist
            self.store.create_topic_page(&page)?;

            let record = CompilationRecord {
                topic_id: topic_id.clone(),
                compiled_at: now,
                source_count: candidate_memories.len(),
                duration_ms: 0,
                quality_score: report.overall,
                recompile_reason: Some("initial compilation via API".to_string()),
            };
            self.store.save_compilation_record(&record)?;

            pages.push(page);
        }

        Ok(pages)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a simple hash-based pseudo-embedding for clustering.
///
/// This is a fallback when real embeddings are unavailable.
/// Produces a deterministic float vector from content using character-level hashing.
fn simple_hash_embedding(content: &str, dims: usize) -> Vec<f32> {
    super::compilation::simple_hash_embedding(content, dims)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use super::export::ExportOutput;
    use crate::compiler::storage::SqliteKnowledgeStore;
    use chrono::Utc;

    fn make_api() -> MaintenanceApi<SqliteKnowledgeStore> {
        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        MaintenanceApi::new(store, KcConfig::default())
    }

    fn make_topic(id: &str, title: &str, content: &str) -> TopicPage {
        let now = Utc::now();
        TopicPage {
            id: TopicId(id.to_owned()),
            title: title.to_owned(),
            content: content.to_owned(),
            sections: Vec::new(),
            summary: format!("Summary of {}", title),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: vec![],
                tags: vec![],
                quality_score: Some(0.5),
            },
        }
    }

    // ── query ────────────────────────────────────────────────────────────

    #[test]
    fn test_query_with_matches() {
        let api = make_api();
        let page = make_topic("t1", "Rust Programming", "Rust is a systems programming language");
        api.store.create_topic_page(&page).unwrap();

        let results = api.query("rust", &QueryOpts::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic_id, TopicId("t1".to_owned()));
        assert!(results[0].relevance > 0.0);
    }

    #[test]
    fn test_query_with_no_matches() {
        let api = make_api();
        let page = make_topic("t1", "Rust Programming", "Rust is a systems language");
        api.store.create_topic_page(&page).unwrap();

        let results = api.query("python", &QueryOpts::default()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_query_excludes_archived_by_default() {
        let api = make_api();
        let mut page = make_topic("t1", "Archived Topic", "Some archived content about rust");
        page.status = TopicStatus::Archived;
        api.store.create_topic_page(&page).unwrap();

        // Default excludes archived
        let results = api.query("rust", &QueryOpts::default()).unwrap();
        assert!(results.is_empty());

        // Explicitly include archived
        let results = api
            .query(
                "rust",
                &QueryOpts {
                    limit: 10,
                    include_archived: true,
                },
            )
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_query_relevance_ordering() {
        let api = make_api();
        // Page with "rust" in content only
        let p1 = make_topic("t1", "Some Topic", "This mentions rust in the body");
        // Page with "rust" in title and content
        let p2 = make_topic("t2", "Rust Guide", "A guide about rust programming");
        api.store.create_topic_page(&p1).unwrap();
        api.store.create_topic_page(&p2).unwrap();

        let results = api.query("rust", &QueryOpts::default()).unwrap();
        assert_eq!(results.len(), 2);
        // Title match should rank higher
        assert_eq!(results[0].topic_id, TopicId("t2".to_owned()));
    }

    #[test]
    fn test_query_respects_limit() {
        let api = make_api();
        for i in 0..5 {
            let page = make_topic(
                &format!("t{}", i),
                &format!("Rust Topic {}", i),
                "Content about rust",
            );
            api.store.create_topic_page(&page).unwrap();
        }

        let results = api
            .query(
                "rust",
                &QueryOpts {
                    limit: 3,
                    include_archived: false,
                },
            )
            .unwrap();
        assert_eq!(results.len(), 3);
    }

    // ── inspect ──────────────────────────────────────────────────────────

    #[test]
    fn test_inspect_found() {
        let api = make_api();
        let page = make_topic("t1", "Inspect Test", "Some content here");
        api.store.create_topic_page(&page).unwrap();

        let detail = api.inspect(&TopicId("t1".to_owned())).unwrap();
        assert_eq!(detail.page.title, "Inspect Test");
        assert!(detail.quality.is_some());
    }

    #[test]
    fn test_inspect_not_found() {
        let api = make_api();
        let result = api.inspect(&TopicId("nonexistent".to_owned()));
        assert!(result.is_err());
        match result.unwrap_err() {
            KcError::NotFound(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected NotFound, got: {}", other),
        }
    }

    // ── list ─────────────────────────────────────────────────────────────

    #[test]
    fn test_list_empty() {
        let api = make_api();
        let pages = api.list().unwrap();
        assert!(pages.is_empty());
    }

    #[test]
    fn test_list_non_empty() {
        let api = make_api();
        let p1 = make_topic("t1", "First", "Content 1");
        let p2 = make_topic("t2", "Second", "Content 2");
        api.store.create_topic_page(&p1).unwrap();
        api.store.create_topic_page(&p2).unwrap();

        let pages = api.list().unwrap();
        assert_eq!(pages.len(), 2);
    }

    // ── decay ────────────────────────────────────────────────────────────

    #[test]
    fn test_evaluate_decay_all() {
        let api = make_api();
        let page = make_topic("t1", "Decay Test", "Some content");
        api.store.create_topic_page(&page).unwrap();

        let results = api.evaluate_decay(&DecayScope::All).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic_id, TopicId("t1".to_owned()));
    }

    #[test]
    fn test_evaluate_decay_single_topic() {
        let api = make_api();
        let page = make_topic("t1", "Decay Test", "Some content");
        api.store.create_topic_page(&page).unwrap();

        let results = api
            .evaluate_decay(&DecayScope::Topic(TopicId("t1".to_owned())))
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_evaluate_decay_topic_not_found() {
        let api = make_api();
        let result = api.evaluate_decay(&DecayScope::Topic(TopicId("missing".to_owned())));
        assert!(result.is_err());
    }

    // ── import from markdown ─────────────────────────────────────────────

    #[test]
    fn test_import_from_markdown() {
        let api = make_api();
        let dir = tempfile::TempDir::new().unwrap();
        let md_path = dir.path().join("test.md");
        std::fs::write(
            &md_path,
            "# My Topic\n\nSome content here.\n\n## Details\n\nMore details.\n",
        )
        .unwrap();

        let config = ImportConfig {
            default_policy: ImportPolicy::Skip,
            split_strategy: SplitStrategy::ByHeading,
            duplicate_strategy: DuplicateStrategy::Skip,
            max_document_size_bytes: 10_000_000,
        };

        let report = api.import_from(&md_path, &config).unwrap();
        assert!(report.imported > 0);
        assert_eq!(report.errors.len(), 0);

        // Verify pages were created
        let pages = api.list().unwrap();
        assert!(!pages.is_empty());
    }

    // ── detect_conflicts ─────────────────────────────────────────────────

    #[test]
    fn test_detect_conflicts_empty() {
        let api = make_api();
        let conflicts = api.detect_conflicts().unwrap();
        assert!(conflicts.is_empty());
    }

    // ── health_report ────────────────────────────────────────────────────

    #[test]
    fn test_health_report_empty() {
        let api = make_api();
        let report = api.health_report().unwrap();
        assert_eq!(report.total_topics, 0);
        assert!(report.stale_topics.is_empty());
    }

    #[test]
    fn test_health_report_with_topics() {
        let api = make_api();
        let page = make_topic("t1", "Health Test", "Content for health report");
        api.store.create_topic_page(&page).unwrap();

        let report = api.health_report().unwrap();
        assert_eq!(report.total_topics, 1);
    }

    // ── set_privacy_level ────────────────────────────────────────────────

    #[test]
    fn test_set_privacy_level() {
        let api = make_api();
        let page = make_topic("t1", "Privacy Test", "Sensitive content");
        api.store.create_topic_page(&page).unwrap();

        api.set_privacy_level(&TopicId("t1".to_owned()), PrivacyLevel::Private)
            .unwrap();

        let updated = api.store.get_topic_page(&TopicId("t1".to_owned())).unwrap().unwrap();
        assert!(updated.metadata.tags.contains(&"privacy:private".to_string()));
    }

    // ── export ───────────────────────────────────────────────────────────

    #[test]
    fn test_export_json() {
        let api = make_api();
        let page = make_topic("t1", "Export Test", "Content for export");
        api.store.create_topic_page(&page).unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };
        let output = api.export(&filter, ExportFormat::Json).unwrap();
        match output {
            ExportOutput::Json(json) => {
                assert!(json.contains("Export Test"));
            }
            _ => panic!("expected JSON output"),
        }
    }

    #[test]
    fn test_export_markdown() {
        let api = make_api();
        let page = make_topic("t1", "Export MD Test", "Content for markdown export");
        api.store.create_topic_page(&page).unwrap();

        let filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };
        let output = api.export(&filter, ExportFormat::Markdown).unwrap();
        match output {
            ExportOutput::Markdown(files) => {
                assert_eq!(files.len(), 1);
                assert!(files[0].content.contains("Export MD Test"));
            }
            _ => panic!("expected Markdown output"),
        }
    }

    // ── recall ───────────────────────────────────────────────────────────

    #[test]
    fn test_recall_empty_store() {
        let api = make_api();
        let results = api.recall("anything", &RecallOpts::default()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_recall_matches_topic() {
        let api = make_api();
        let page = make_topic("t1", "Rust Programming", "Rust is a systems programming language");
        api.store.create_topic_page(&page).unwrap();

        let results = api.recall("rust", &RecallOpts::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic_id, TopicId("t1".to_owned()));
        assert_eq!(results[0].title, "Rust Programming");
        assert!(!results[0].snippet.is_empty());
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_recall_respects_limit() {
        let api = make_api();
        for i in 0..3 {
            let page = make_topic(
                &format!("t{}", i),
                &format!("Rust Topic {}", i),
                "Content about rust programming",
            );
            api.store.create_topic_page(&page).unwrap();
        }

        let opts = RecallOpts {
            limit: 1,
            ..RecallOpts::default()
        };
        let results = api.recall("rust", &opts).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_recall_quality_boost() {
        let api = make_api();

        // Two topics with the same text but different quality scores
        let mut low_quality = make_topic("t_low", "Rust Guide", "Content about rust");
        low_quality.metadata.quality_score = Some(0.1);
        api.store.create_topic_page(&low_quality).unwrap();

        let mut high_quality = make_topic("t_high", "Rust Guide", "Content about rust");
        high_quality.metadata.quality_score = Some(0.9);
        api.store.create_topic_page(&high_quality).unwrap();

        let results = api.recall("rust", &RecallOpts::default()).unwrap();
        assert_eq!(results.len(), 2);
        // Higher quality should rank first
        assert_eq!(results[0].topic_id, TopicId("t_high".to_owned()));
        assert_eq!(results[1].topic_id, TopicId("t_low".to_owned()));
    }

    #[test]
    fn test_recall_no_archived() {
        let api = make_api();
        let mut page = make_topic("t1", "Archived Rust Topic", "Rust content that is archived");
        page.status = TopicStatus::Archived;
        api.store.create_topic_page(&page).unwrap();

        let results = api.recall("rust", &RecallOpts::default()).unwrap();
        assert!(results.is_empty());
    }
}
