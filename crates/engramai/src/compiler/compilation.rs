//! Compilation pipeline — change detection, trigger evaluation, quality scoring,
//! prompt construction, and the main `CompilationPipeline` orchestrator.

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::time::Instant;

use crate::compiler::llm::LlmProvider;
use crate::compiler::storage::KnowledgeStore;
use crate::compiler::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  Memory Snapshot
// ═══════════════════════════════════════════════════════════════════════════════

/// Lightweight snapshot of a memory used during compilation.
#[derive(Clone, Debug)]
pub struct MemorySnapshot {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub importance: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
    /// Pre-computed embedding from engram's memory_embeddings table.
    /// When Some, used directly for clustering. When None, falls back to hash-based pseudo-embedding.
    pub embedding: Option<Vec<f32>>,
}

impl MemorySnapshot {
    #[cfg(test)]
    pub fn test(id: &str, content: &str) -> Self {
        Self {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: "factual".to_string(),
            importance: 0.5,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            tags: vec![],
            embedding: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Change Detection
// ═══════════════════════════════════════════════════════════════════════════════

/// Detects what changed between the current memory set and the last compilation.
pub struct ChangeDetector;

impl ChangeDetector {
    /// Compare current memories against the previous compilation state.
    pub fn detect(
        current_memories: &[MemorySnapshot],
        last_record: Option<&CompilationRecord>,
        previous_memory_ids: &[String],
    ) -> ChangeSet {
        let current_ids: HashSet<&str> = current_memories.iter().map(|m| m.id.as_str()).collect();
        let prev_ids: HashSet<&str> = previous_memory_ids.iter().map(|s| s.as_str()).collect();

        match last_record {
            None => ChangeSet {
                added: current_memories.iter().map(|m| m.id.clone()).collect(),
                modified: vec![],
                removed: vec![],
                last_compiled: None,
            },
            Some(record) => {
                let added: Vec<String> = current_ids
                    .difference(&prev_ids)
                    .map(|s| s.to_string())
                    .collect();

                let removed: Vec<String> = prev_ids
                    .difference(&current_ids)
                    .map(|s| s.to_string())
                    .collect();

                let modified: Vec<String> = current_memories
                    .iter()
                    .filter(|m| prev_ids.contains(m.id.as_str()) && m.updated_at > record.compiled_at)
                    .map(|m| m.id.clone())
                    .collect();

                ChangeSet {
                    added,
                    modified,
                    removed,
                    last_compiled: Some(record.compiled_at),
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Trigger Evaluation
// ═══════════════════════════════════════════════════════════════════════════════

/// Decides whether a topic needs recompilation based on detected changes.
pub struct TriggerEvaluator<'a> {
    config: &'a KcConfig,
}

impl<'a> TriggerEvaluator<'a> {
    pub fn new(config: &'a KcConfig) -> Self {
        Self { config }
    }

    /// Evaluate whether a topic should be recompiled given current memories,
    /// the last compilation record, previous content hashes, and the recompile strategy.
    ///
    /// Implements the design doc §3.3 IncrementalTrigger decision logic:
    /// 1. Build change set using ChangeDetector
    /// 2. Content hash dedup (handled by ChangeDetector via previous_hashes)
    /// 3. Strategy-based decision (Eager/Lazy/Manual)
    pub fn evaluate(
        &self,
        current_memories: &[MemorySnapshot],
        last_record: Option<&CompilationRecord>,
        previous_hashes: &[String],
        strategy: &RecompileStrategy,
    ) -> TriggerDecision {
        // 1. Build change set using ChangeDetector
        let change_set = ChangeDetector::detect(current_memories, last_record, previous_hashes);

        let total_changes =
            change_set.added.len() + change_set.modified.len() + change_set.removed.len();

        // 2. Content hash dedup: if a memory was "modified" but hash is identical, exclude it
        //    (This should already happen in ChangeDetector::detect via previous_hashes comparison)

        if total_changes == 0 {
            return TriggerDecision::Skip {
                reason: "No changes detected".into(),
            };
        }

        let total_sources = if let Some(rec) = last_record {
            rec.source_count.max(1)
        } else {
            current_memories.len().max(1)
        };

        let change_ratio = total_changes as f64 / total_sources as f64;

        // 3. Strategy-based decision
        match strategy {
            RecompileStrategy::Eager => {
                if change_ratio > 0.5 {
                    TriggerDecision::Full { change_set }
                } else {
                    TriggerDecision::Partial { change_set }
                }
            }
            RecompileStrategy::Lazy => {
                if change_ratio > 0.3 {
                    TriggerDecision::Full { change_set }
                } else {
                    TriggerDecision::Partial { change_set }
                }
            }
            RecompileStrategy::Manual => TriggerDecision::Skip {
                reason: "Manual strategy — recompile only on explicit request".into(),
            },
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Quality Scoring
// ═══════════════════════════════════════════════════════════════════════════════

/// Evaluates the quality of a compiled topic page.
pub struct QualityScorer<'a> {
    config: &'a KcConfig,
}

impl<'a> QualityScorer<'a> {
    pub fn new(config: &'a KcConfig) -> Self {
        Self { config }
    }

    /// Produce a quality report for a topic page given its source memories and feedback.
    pub fn score(&self, topic: &TopicPage, memories: &[MemorySnapshot], feedback: &[FeedbackEntry]) -> QualityReport {
        let coverage = self.score_coverage(topic, memories);
        let coherence = self.score_coherence(topic);
        let freshness = self.score_freshness(memories);
        let overall = coherence * 0.4 + coverage * 0.35 + freshness * 0.25;

        // Feedback penalty: each unresolved ThumbsDown reduces score by 0.05, cap at -0.2
        let unresolved_negatives = feedback.iter()
            .filter(|f| matches!(f.kind, FeedbackKind::ThumbsDown) && !f.resolved)
            .count();
        let penalty = (unresolved_negatives as f64 * 0.05).min(0.2);
        let overall = (overall - penalty).clamp(0.0, 1.0);

        let mut suggestions = Vec::new();
        if coverage < 0.7 {
            let uncited = memories.len() - (coverage * memories.len() as f64) as usize;
            suggestions.push(format!("{} source memories may be uncited — consider recompilation", uncited));
        }
        if coherence < 0.5 {
            suggestions.push(
                "Low coherence: content may be too brief or poorly structured".into(),
            );
        }
        if freshness < 0.3 {
            suggestions
                .push("Content may be stale: consider recompilation with recent memories".into());
        }

        let unresolved_count = feedback.iter().filter(|f| !f.resolved).count();
        if unresolved_count > 0 {
            suggestions.push(format!("{} user corrections pending — recompile to incorporate", unresolved_count));
        }

        QualityReport {
            topic_id: topic.id.clone(),
            coherence,
            coverage,
            freshness,
            overall,
            suggestions,
        }
    }

    /// What fraction of source memories are represented in the topic content?
    fn score_coverage(&self, topic: &TopicPage, memories: &[MemorySnapshot]) -> f64 {
        if memories.is_empty() {
            return 0.0;
        }
        let source_ids: HashSet<&str> = topic
            .metadata
            .source_memory_ids
            .iter()
            .map(|s| s.as_str())
            .collect();

        // ID match: is the memory listed in source_memory_ids?
        let id_matches = memories
            .iter()
            .filter(|m| source_ids.contains(m.id.as_str()))
            .count();
        let id_ratio = id_matches as f64 / memories.len() as f64;

        // Keyword match: does the topic content mention a keyword from each memory?
        let keyword_matches = memories
            .iter()
            .filter(|m| {
                m.content
                    .split_whitespace()
                    .find(|w| w.len() > 4)
                    .map(|kw| topic.content.contains(kw))
                    .unwrap_or(false)
            })
            .count();
        let kw_ratio = keyword_matches as f64 / memories.len() as f64;

        (id_ratio * 0.6 + kw_ratio * 0.4).clamp(0.0, 1.0)
    }

    /// Heuristic coherence based on content structure.
    fn score_coherence(&self, topic: &TopicPage) -> f64 {
        let len = topic.content.len();
        let mut score: f64 = if len < 100 {
            0.3
        } else if len < 300 {
            0.5
        } else {
            0.7
        };

        // Bonus for structured content (markdown headers)
        let has_headers = topic.content.lines().any(|l| l.starts_with('#'));
        if has_headers {
            score += 0.15;
        }

        // Bonus for paragraph breaks (multiple sections)
        let paragraph_count = topic.content.split("\n\n").count();
        if paragraph_count >= 3 {
            score += 0.1;
        }

        score.clamp(0.0, 1.0)
    }

    /// How recent are the source memories? Weighted by importance.
    fn score_freshness(&self, memories: &[MemorySnapshot]) -> f64 {
        if memories.is_empty() {
            return 0.0;
        }
        let now = Utc::now();
        let total_importance: f64 = memories.iter().map(|m| m.importance).sum();
        if total_importance <= 0.0 {
            return 0.0;
        }

        let weighted_sum: f64 = memories
            .iter()
            .map(|m| {
                let age_days = (now - m.created_at).num_days().max(0) as f64;
                let freshness = 1.0 / (1.0 + age_days / 30.0);
                freshness * m.importance
            })
            .sum();

        (weighted_sum / total_importance).clamp(0.0, 1.0)
    }

    /// Rank quality reports by overall score, worst first.
    pub fn rank_topics<'b>(&self, reports: &'b [QualityReport]) -> Vec<&'b QualityReport> {
        let mut sorted: Vec<&QualityReport> = reports.iter().collect();
        sorted.sort_by(|a, b| a.overall.partial_cmp(&b.overall).unwrap_or(std::cmp::Ordering::Equal));
        sorted
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Prompt Builders
// ═══════════════════════════════════════════════════════════════════════════════

/// Build an LLM prompt for full (from-scratch) compilation.
pub fn build_full_compile_prompt(
    title: &str,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
) -> String {
    let mut prompt = format!(
        "You are a knowledge compiler. Synthesize these memories into a coherent topic page.\n\n\
         Topic: {title}\n\n\
         Memories:\n"
    );

    for m in memories {
        let date = m.created_at.format("%Y-%m-%d");
        prompt.push_str(&format!("- [{}] ({date}): {}\n", m.memory_type, m.content));
    }

    if !user_edits.is_empty() {
        prompt.push_str("\nThe user has made manual edits. Preserve their intent:\n");
        for (original, replacement) in user_edits {
            prompt.push_str(&format!("- Original: \"{original}\" → Replacement: \"{replacement}\"\n"));
        }
    }

    prompt.push_str(
        "\nOutput a well-structured markdown document with:\n\
         1. A concise summary (2-3 sentences)\n\
         2. Key points organized by theme\n\
         3. Relevant details and context\n\
         4. Any contradictions or open questions\n",
    );

    prompt
}

/// Build an LLM prompt for incremental compilation (updating existing content).
pub fn build_incremental_compile_prompt(
    title: &str,
    existing_content: &str,
    changes: &ChangeSet,
    memories: &[MemorySnapshot],
    user_edits: &[(String, String)],
) -> String {
    let mem_index: std::collections::HashMap<&str, &MemorySnapshot> =
        memories.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut prompt = format!(
        "You are updating an existing knowledge page with new information.\n\n\
         Topic: {title}\n\n\
         Current content:\n{existing_content}\n\n\
         Changes since last compilation:\n"
    );

    if !changes.added.is_empty() {
        prompt.push_str("New memories:\n");
        for id in &changes.added {
            if let Some(m) = mem_index.get(id.as_str()) {
                let date = m.created_at.format("%Y-%m-%d");
                prompt.push_str(&format!("- [{}] ({date}): {}\n", m.memory_type, m.content));
            }
        }
    }

    if !changes.modified.is_empty() {
        prompt.push_str("Modified memories:\n");
        for id in &changes.modified {
            if let Some(m) = mem_index.get(id.as_str()) {
                let date = m.updated_at.format("%Y-%m-%d");
                prompt.push_str(&format!("- [{}] ({date}): {}\n", m.memory_type, m.content));
            }
        }
    }

    if !changes.removed.is_empty() {
        prompt.push_str(&format!("Removed memory IDs: {:?}\n", changes.removed));
    }

    if !user_edits.is_empty() {
        prompt.push_str("\nPreserve these user edits:\n");
        for (original, replacement) in user_edits {
            prompt.push_str(&format!("- \"{original}\" → \"{replacement}\"\n"));
        }
    }

    prompt.push_str(
        "\nUpdate the document to incorporate changes while maintaining structure. \
         Remove information from deleted memories.\n",
    );

    prompt
}

/// Fallback compilation when no LLM is available — concatenate memories.
pub fn compile_without_llm(title: &str, memories: &[MemorySnapshot]) -> String {
    let mut sorted: Vec<&MemorySnapshot> = memories.iter().collect();
    sorted.sort_by(|a, b| {
        b.importance
            .partial_cmp(&a.importance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    let mut out = format!("# {title}\n\n## Summary\n\nCompiled from {} memories.\n\n## Key Points\n\n", sorted.len());

    for m in &sorted {
        let date = m.created_at.format("%Y-%m-%d");
        let preview: String = m.content.chars().take(200).collect();
        out.push_str(&format!("- **{}** ({date}): {preview}\n", m.memory_type));
    }

    out.push_str("\n## Details\n\n");

    for m in &sorted {
        let date = m.created_at.format("%Y-%m-%d");
        out.push_str(&format!(
            "### Memory: {}\n{}\n\nType: {} | Importance: {:.2} | Date: {date}\n\n---\n\n",
            m.id, m.content, m.memory_type, m.importance,
        ));
    }

    out
}

/// Apply user edits to compiled content. If the original text is found, replace it;
/// otherwise append to a "User Notes" section.
pub fn preserve_user_edits(content: &str, edits: &[(String, String)]) -> String {
    let mut result = content.to_string();
    let mut unmatched = Vec::new();

    for (original, replacement) in edits {
        if result.contains(original.as_str()) {
            result = result.replacen(original, replacement, 1);
        } else {
            unmatched.push(replacement.as_str());
        }
    }

    if !unmatched.is_empty() {
        result.push_str("\n\n## User Notes\n\n");
        for note in unmatched {
            result.push_str(note);
            result.push('\n');
        }
    }

    result
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Compilation Pipeline
// ═══════════════════════════════════════════════════════════════════════════════

/// Orchestrates the full compilation cycle: detect changes → evaluate triggers →
/// compile (LLM or fallback) → score quality → persist results.
pub struct CompilationPipeline<S: KnowledgeStore, L: LlmProvider> {
    store: S,
    llm: Option<L>,
    config: KcConfig,
    verbose: bool,
}

impl<S: KnowledgeStore, L: LlmProvider> CompilationPipeline<S, L> {
    pub fn new(store: S, llm: Option<L>, config: KcConfig) -> Self {
        Self { store, llm, config, verbose: false }
    }

    /// Builder method to enable verbose mode (prints LLM prompts to stderr).
    pub fn with_verbose(mut self, v: bool) -> Self {
        self.verbose = v;
        self
    }

    /// Run the full pipeline for a single topic candidate: compile and persist.
    pub fn compile_new(
        &self,
        candidate: &TopicCandidate,
        memories: &[MemorySnapshot],
    ) -> Result<TopicPage, KcError> {
        let start = Instant::now();

        let title = candidate
            .suggested_title
            .clone()
            .unwrap_or_else(|| format!("Topic ({})", candidate.memories.len()));

        let topic_id = TopicId(format!(
            "topic-{}",
            Utc::now().timestamp_millis()
        ));

        // Compile content
        let content = self.compile_content(&title, memories, &[], None)?;

        // Build the topic page
        let now = Utc::now();
        let page = TopicPage {
            id: topic_id.clone(),
            title: title.clone(),
            summary: extract_summary(&content),
            content,
            sections: Vec::new(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: memories.iter().map(|m| m.id.clone()).collect(),
                tags: aggregate_tags(memories),
                quality_score: None, // scored below
            },
        };

        // Score quality
        let scorer = QualityScorer::new(&self.config);
        let report = scorer.score(&page, memories, &[]);

        // Update quality in page
        let mut page = page;
        page.metadata.quality_score = Some(report.overall);

        // Persist
        self.store.create_topic_page(&page)?;

        let record = CompilationRecord {
            topic_id: topic_id.clone(),
            compiled_at: now,
            source_count: memories.len(),
            duration_ms: start.elapsed().as_millis() as u64,
            quality_score: report.overall,
            recompile_reason: Some("initial compilation".to_string()),
        };
        self.store.save_compilation_record(&record)?;

        Ok(page)
    }

    /// Re-compile an existing topic with new/changed memories.
    pub fn recompile(
        &self,
        topic: &TopicPage,
        memories: &[MemorySnapshot],
        changes: &ChangeSet,
        user_edits: &[(String, String)],
    ) -> Result<TopicPage, KcError> {
        let start = Instant::now();

        // Decide strategy based on change magnitude
        let use_incremental = !changes.added.is_empty()
            && changes.removed.is_empty()
            && changes.added.len() + changes.modified.len() <= 3;

        let content = if use_incremental {
            self.compile_content(
                &topic.title,
                memories,
                user_edits,
                Some((&topic.content, changes)),
            )?
        } else {
            self.compile_content(&topic.title, memories, user_edits, None)?
        };

        // Apply user edits on top
        let content = if user_edits.is_empty() {
            content
        } else {
            preserve_user_edits(&content, user_edits)
        };

        let now = Utc::now();
        let mut updated = topic.clone();
        updated.content = content;
        updated.summary = extract_summary(&updated.content);
        updated.metadata.updated_at = now;
        updated.metadata.compilation_count += 1;
        updated.version += 1;
        updated.metadata.source_memory_ids = memories.iter().map(|m| m.id.clone()).collect();
        updated.metadata.tags = aggregate_tags(memories);

        // Score
        let scorer = QualityScorer::new(&self.config);
        let report = scorer.score(&updated, memories, &[]);
        updated.metadata.quality_score = Some(report.overall);

        // Persist
        self.store.update_topic_page(&updated)?;

        let record = CompilationRecord {
            topic_id: topic.id.clone(),
            compiled_at: now,
            source_count: memories.len(),
            duration_ms: start.elapsed().as_millis() as u64,
            quality_score: report.overall,
            recompile_reason: Some(format!(
                "recompile: {} added, {} modified, {} removed",
                changes.added.len(),
                changes.modified.len(),
                changes.removed.len()
            )),
        };
        self.store.save_compilation_record(&record)?;

        Ok(updated)
    }

    /// Perform a read-only dry run: show what a compilation pass *would* do
    /// without calling the LLM or mutating the store.
    ///
    /// For each `TopicCandidate` discovered from memories:
    /// - If no existing topic overlaps → `NewCompilation`
    /// - If an existing topic overlaps and has changes → `Recompile`
    /// - If an existing topic overlaps but nothing changed → `Skip`
    ///
    /// For existing topics with no matching candidate:
    /// - Evaluates decay → `Archive` if stale enough, else `Skip`
    pub fn dry_run(
        &self,
        memories: &[MemorySnapshot],
    ) -> Result<DryRunReport, KcError> {
        use crate::compiler::decay::DecayEngine;
        use crate::compiler::discovery::TopicDiscovery;

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
            // Check if this candidate overlaps an existing topic
            match discovery.detect_overlap(candidate, &existing_pages) {
                Some(topic_id) => {
                    matched_topic_ids.insert(topic_id.clone());

                    // Determine if there are changes worth recompiling
                    let page = self.store.get_topic_page(&topic_id)?;
                    if let Some(page) = page {
                        let existing_ids: HashSet<&str> = page
                            .metadata
                            .source_memory_ids
                            .iter()
                            .map(|s| s.as_str())
                            .collect();
                        let candidate_ids: HashSet<&str> =
                            candidate.memories.iter().map(|s| s.as_str()).collect();

                        let added = candidate_ids.difference(&existing_ids).count();
                        let removed = existing_ids.difference(&candidate_ids).count();

                        if added > 0 || removed > 0 {
                            entries.push(DryRunEntry {
                                topic_id: Some(topic_id),
                                action: DryRunAction::Recompile,
                                affected_memories: candidate.memories.len(),
                                reason: format!(
                                    "{} new memories, {} removed since last compile",
                                    added, removed
                                ),
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
                    // New topic
                    entries.push(DryRunEntry {
                        topic_id: None,
                        action: DryRunAction::NewCompilation,
                        affected_memories: candidate.memories.len(),
                        reason: format!(
                            "New cluster of {} memories",
                            candidate.memories.len()
                        ),
                    });
                    estimated_llm_calls += 1;
                }
            }
        }

        // Check existing topics that had no matching candidate — evaluate decay
        let decay_engine = DecayEngine::new(self.config.decay.clone());
        for page in &existing_pages {
            if matched_topic_ids.contains(&page.id) {
                continue;
            }
            if page.status == TopicStatus::Archived {
                continue;
            }

            let decay_result = decay_engine.evaluate_topic(page, &self.store)?;
            if matches!(decay_result.recommended_action, DecayAction::Archive(_)) {
                entries.push(DryRunEntry {
                    topic_id: Some(page.id.clone()),
                    action: DryRunAction::Archive,
                    affected_memories: 0,
                    reason: format!(
                        "Freshness score {:.2} below archive threshold",
                        decay_result.freshness_score
                    ),
                });
            } else {
                entries.push(DryRunEntry {
                    topic_id: Some(page.id.clone()),
                    action: DryRunAction::Skip,
                    affected_memories: 0,
                    reason: "No matching candidate and not decayed enough to archive".to_string(),
                });
            }
        }

        let total_topics_affected = entries
            .iter()
            .filter(|e| !matches!(e.action, DryRunAction::Skip))
            .count();

        Ok(DryRunReport {
            entries,
            total_topics_affected,
            estimated_llm_calls,
        })
    }

    /// Inner content compilation: tries LLM first, falls back to concatenation.
    fn compile_content(
        &self,
        title: &str,
        memories: &[MemorySnapshot],
        user_edits: &[(String, String)],
        incremental: Option<(&str, &ChangeSet)>,
    ) -> Result<String, KcError> {
        let prompt = match incremental {
            Some((existing, changes)) => {
                build_incremental_compile_prompt(title, existing, changes, memories, user_edits)
            }
            None => build_full_compile_prompt(title, memories, user_edits),
        };

        if self.verbose {
            eprintln!("[KC verbose] LLM prompt:\n{}", prompt);
        }

        match &self.llm {
            Some(provider) => {
                let request = LlmRequest {
                    task: LlmTask::Compile,
                    prompt,
                    max_tokens: Some(2048),
                    temperature: Some(0.3),
                };
                match provider.complete(&request) {
                    Ok(response) => Ok(response.content),
                    Err(e) => {
                        // Fallback to non-LLM compilation
                        eprintln!("LLM compilation failed ({e}), using fallback");
                        Ok(compile_without_llm(title, memories))
                    }
                }
            }
            None => Ok(compile_without_llm(title, memories)),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Generate a simple hash-based pseudo-embedding for clustering.
///
/// Fallback when real embeddings are unavailable.
/// Produces a deterministic float vector from content using character-level hashing.
pub fn simple_hash_embedding(content: &str, dims: usize) -> Vec<f32> {
    let mut embedding = vec![0.0f32; dims];
    for (i, byte) in content.bytes().enumerate() {
        let idx = i % dims;
        embedding[idx] += (byte as f32 - 128.0) / 128.0;
    }
    // Normalize
    let mag: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.0 {
        for v in &mut embedding {
            *v /= mag;
        }
    }
    embedding
}

/// Extract the first paragraph as a summary.
pub fn extract_summary(content: &str) -> String {
    // Skip the title line (# ...) then take the first non-empty paragraph
    let mut lines = content.lines().peekable();
    // Skip leading blank lines and title
    while let Some(line) = lines.peek() {
        if line.starts_with('#') || line.trim().is_empty() {
            lines.next();
        } else {
            break;
        }
    }
    let summary: Vec<&str> = lines
        .take_while(|l| !l.trim().is_empty())
        .collect();
    if summary.is_empty() {
        content.chars().take(200).collect()
    } else {
        summary.join(" ")
    }
}

/// Collect unique tags from all memories.
pub fn aggregate_tags(memories: &[MemorySnapshot]) -> Vec<String> {
    let mut tags: HashSet<String> = HashSet::new();
    for m in memories {
        for t in &m.tags {
            tags.insert(t.clone());
        }
    }
    let mut sorted: Vec<String> = tags.into_iter().collect();
    sorted.sort();
    sorted
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn make_config() -> KcConfig {
        KcConfig {
            min_cluster_size: 3,
            quality_threshold: 0.4,
            recompile_strategy: RecompileStrategy::Eager,
            decay: DecayConfig::default(),
            llm: LlmConfig::default(),
            import: ImportConfig::default(),
            intake: IntakeConfig::default(),
            lifecycle: LifecycleConfig::default(),
        }
    }

    fn make_topic(id: &str, compilation_count: u32, quality: Option<f64>) -> TopicPage {
        TopicPage {
            id: TopicId(id.to_string()),
            title: format!("Topic {id}"),
            summary: "A test topic".to_string(),
            content: "# Topic\n\nSome content about things.\n\nMore details here.".to_string(),
            sections: Vec::new(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: Utc::now() - Duration::days(7),
                updated_at: Utc::now(),
                compilation_count,
                source_memory_ids: vec!["m1".into(), "m2".into()],
                tags: vec!["test".into()],
                quality_score: quality,
            },
        }
    }

    // ── Change Detection ────────────────────────────────────────────────────

    #[test]
    fn test_detect_first_compilation() {
        let mems = vec![
            MemorySnapshot::test("m1", "first"),
            MemorySnapshot::test("m2", "second"),
        ];
        let cs = ChangeDetector::detect(&mems, None, &[]);

        assert_eq!(cs.added.len(), 2);
        assert!(cs.modified.is_empty());
        assert!(cs.removed.is_empty());
        assert!(cs.last_compiled.is_none());
    }

    #[test]
    fn test_detect_with_changes() {
        let now = Utc::now();
        let compiled_at = now - Duration::hours(2);

        let mut m_modified = MemorySnapshot::test("m1", "updated content");
        m_modified.updated_at = now; // after compiled_at
        let mut m_unchanged = MemorySnapshot::test("m2", "unchanged");
        m_unchanged.updated_at = compiled_at - Duration::hours(1); // before compiled_at
        let m_new = MemorySnapshot::test("m3", "brand new");

        let record = CompilationRecord {
            topic_id: TopicId("t1".into()),
            compiled_at,
            source_count: 3,
            duration_ms: 100,
            quality_score: 0.8,
            recompile_reason: None,
        };

        let cs = ChangeDetector::detect(
            &[m_modified, m_unchanged, m_new],
            Some(&record),
            &["m1".into(), "m2".into(), "m_old".into()],
        );

        assert!(cs.added.contains(&"m3".to_string()));
        assert!(cs.modified.contains(&"m1".to_string()));
        assert!(cs.removed.contains(&"m_old".to_string()));
        assert!(!cs.added.contains(&"m2".to_string()));
        assert!(!cs.modified.contains(&"m2".to_string()));
        assert_eq!(cs.last_compiled, Some(compiled_at));
    }

    // ── Trigger Evaluation ──────────────────────────────────────────────────

    #[test]
    fn test_trigger_skip_no_changes() {
        let config = make_config();
        let evaluator = TriggerEvaluator::new(&config);
        // Current memories are the same as previous — no changes
        let mems = vec![
            MemorySnapshot::test("m1", "first"),
            MemorySnapshot::test("m2", "second"),
        ];
        let record = CompilationRecord {
            topic_id: TopicId("t1".into()),
            compiled_at: Utc::now() + Duration::hours(1), // compiled after memory updated_at
            source_count: 2,
            duration_ms: 100,
            quality_score: 0.8,
            recompile_reason: None,
        };
        let prev_ids = vec!["m1".into(), "m2".into()];

        match evaluator.evaluate(&mems, Some(&record), &prev_ids, &RecompileStrategy::Eager) {
            TriggerDecision::Skip { reason } => {
                assert!(reason.contains("No changes"));
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    #[test]
    fn test_trigger_initial_compilation() {
        let config = make_config();
        let evaluator = TriggerEvaluator::new(&config);
        let mems = vec![MemorySnapshot::test("m1", "first")];

        // No last record, no previous IDs → all memories are "added"
        match evaluator.evaluate(&mems, None, &[], &RecompileStrategy::Eager) {
            TriggerDecision::Full { change_set } | TriggerDecision::Partial { change_set } => {
                assert!(!change_set.added.is_empty(), "should have added memories");
            }
            other => panic!("expected Full or Partial for initial compilation, got {:?}", other),
        }
    }

    #[test]
    fn test_trigger_eager_full_recompile() {
        let config = make_config();
        let evaluator = TriggerEvaluator::new(&config);
        // 2 source memories previously, now all different → >50% change ratio → Full
        let mems = vec![
            MemorySnapshot::test("m3", "new one"),
            MemorySnapshot::test("m4", "another new"),
        ];
        let record = CompilationRecord {
            topic_id: TopicId("t1".into()),
            compiled_at: Utc::now() + Duration::hours(1),
            source_count: 2,
            duration_ms: 100,
            quality_score: 0.8,
            recompile_reason: None,
        };
        let prev_ids = vec!["m1".into(), "m2".into()];

        match evaluator.evaluate(&mems, Some(&record), &prev_ids, &RecompileStrategy::Eager) {
            TriggerDecision::Full { change_set } => {
                // m1, m2 removed; m3, m4 added = 4 changes / 2 sources = 2.0 > 0.5
                assert!(!change_set.added.is_empty() || !change_set.removed.is_empty());
            }
            other => panic!("expected Full, got {:?}", other),
        }
    }

    #[test]
    fn test_trigger_eager_partial_recompile() {
        let config = make_config();
        let evaluator = TriggerEvaluator::new(&config);
        // 10 source memories, 1 added → 1/10 = 0.1 < 0.5 → Partial
        let mut mems: Vec<MemorySnapshot> = (1..=10)
            .map(|i| MemorySnapshot::test(&format!("m{}", i), &format!("content {}", i)))
            .collect();
        mems.push(MemorySnapshot::test("m11", "brand new"));
        let record = CompilationRecord {
            topic_id: TopicId("t1".into()),
            compiled_at: Utc::now() + Duration::hours(1),
            source_count: 10,
            duration_ms: 100,
            quality_score: 0.8,
            recompile_reason: None,
        };
        let prev_ids: Vec<String> = (1..=10).map(|i| format!("m{}", i)).collect();

        match evaluator.evaluate(&mems, Some(&record), &prev_ids, &RecompileStrategy::Eager) {
            TriggerDecision::Partial { change_set } => {
                assert!(change_set.added.contains(&"m11".to_string()));
            }
            other => panic!("expected Partial, got {:?}", other),
        }
    }

    #[test]
    fn test_trigger_manual_always_skips() {
        let config = make_config();
        let evaluator = TriggerEvaluator::new(&config);
        let mems = vec![MemorySnapshot::test("m1", "first")];

        match evaluator.evaluate(&mems, None, &[], &RecompileStrategy::Manual) {
            TriggerDecision::Skip { reason } => {
                assert!(reason.contains("Manual"));
            }
            other => panic!("expected Skip for Manual strategy, got {:?}", other),
        }
    }

    // ── Quality Scoring ─────────────────────────────────────────────────────

    #[test]
    fn test_quality_scorer_good() {
        let config = make_config();
        let scorer = QualityScorer::new(&config);

        let mems = vec![
            MemorySnapshot::test("m1", "Some important knowledge about Rust programming"),
            MemorySnapshot::test("m2", "Details about compiler optimization techniques"),
        ];

        let mut topic = make_topic("t1", 1, Some(0.8));
        topic.metadata.source_memory_ids = vec!["m1".into(), "m2".into()];
        topic.content =
            "# Topic\n\nKnowledge about Rust programming and compiler optimization techniques.\n\nMore details."
                .to_string();

        let report = scorer.score(&topic, &mems, &[]);

        assert!(report.coverage > 0.5, "coverage = {}", report.coverage);
        assert!(report.coherence > 0.5, "coherence = {}", report.coherence);
        assert!(report.overall > 0.4, "overall = {}", report.overall);
    }

    #[test]
    fn test_quality_scorer_poor_coverage() {
        let config = make_config();
        let scorer = QualityScorer::new(&config);

        let mems = vec![
            MemorySnapshot::test("m10", "Completely unrelated xyz content"),
            MemorySnapshot::test("m11", "More unrelated abc stuff"),
        ];

        let topic = make_topic("t1", 1, Some(0.5)); // source_memory_ids = [m1, m2]

        let report = scorer.score(&topic, &mems, &[]);

        assert!(report.coverage < 0.5, "coverage should be low: {}", report.coverage);
    }

    #[test]
    fn test_quality_scorer_short_content() {
        let config = make_config();
        let scorer = QualityScorer::new(&config);

        let mems = vec![MemorySnapshot::test("m1", "test")];
        let mut topic = make_topic("t1", 1, Some(0.5));
        topic.content = "short".to_string();

        let report = scorer.score(&topic, &mems, &[]);

        assert!(report.coherence <= 0.5, "coherence = {}", report.coherence);
    }

    // ── Prompt Builders ─────────────────────────────────────────────────────

    #[test]
    fn test_compile_without_llm() {
        let mems = vec![
            MemorySnapshot::test("m1", "First memory content"),
            MemorySnapshot::test("m2", "Second memory content"),
        ];
        let result = compile_without_llm("Test Topic", &mems);

        assert!(result.contains("# Test Topic"));
        assert!(result.contains("First memory content"));
        assert!(result.contains("Second memory content"));
        assert!(result.contains("Compiled from 2 memories"));
    }

    #[test]
    fn test_preserve_user_edits_found() {
        let content = "The cat sat on the mat.";
        let edits = vec![("cat".to_string(), "dog".to_string())];
        let result = preserve_user_edits(content, &edits);
        assert_eq!(result, "The dog sat on the mat.");
    }

    #[test]
    fn test_preserve_user_edits_not_found() {
        let content = "The cat sat on the mat.";
        let edits = vec![("elephant".to_string(), "A note about elephants".to_string())];
        let result = preserve_user_edits(content, &edits);
        assert!(result.contains("## User Notes"));
        assert!(result.contains("A note about elephants"));
    }

    #[test]
    fn test_full_compile_prompt_structure() {
        let mems = vec![MemorySnapshot::test("m1", "Memory about AI")];
        let prompt = build_full_compile_prompt("AI Topic", &mems, &[]);
        assert!(prompt.contains("Topic: AI Topic"));
        assert!(prompt.contains("[factual]"));
        assert!(prompt.contains("Memory about AI"));
    }

    #[test]
    fn test_incremental_compile_prompt() {
        let mems = vec![MemorySnapshot::test("m3", "New memory")];
        let changes = ChangeSet {
            added: vec!["m3".into()],
            modified: vec![],
            removed: vec!["m_old".into()],
            last_compiled: Some(Utc::now()),
        };
        let prompt = build_incremental_compile_prompt(
            "Topic",
            "existing content",
            &changes,
            &mems,
            &[],
        );
        assert!(prompt.contains("existing content"));
        assert!(prompt.contains("New memory"));
        assert!(prompt.contains("m_old"));
    }

    // ── Dry Run Tests ───────────────────────────────────────────────────────

    #[test]
    fn test_dry_run_no_existing_topics_all_new() {
        use crate::compiler::llm::NoopProvider;
        use crate::compiler::storage::SqliteKnowledgeStore;

        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();

        // Use a small cluster size so our memories form candidates
        let mut config = make_config();
        config.min_cluster_size = 2;

        let pipeline = CompilationPipeline::<SqliteKnowledgeStore, NoopProvider>::new(
            store, None, config,
        );

        // Create memories with similar content so they cluster together
        let memories = vec![
            MemorySnapshot::test("m1", "Rust programming language features"),
            MemorySnapshot::test("m2", "Rust programming language performance"),
            MemorySnapshot::test("m3", "Rust programming language safety"),
        ];

        let report = pipeline.dry_run(&memories).unwrap();

        // All entries should be NewCompilation (no existing topics)
        for entry in &report.entries {
            assert!(
                matches!(entry.action, DryRunAction::NewCompilation),
                "Expected NewCompilation, got {:?}",
                entry.action
            );
            assert!(entry.topic_id.is_none());
        }
        assert_eq!(report.total_topics_affected, report.entries.len());
        assert_eq!(report.estimated_llm_calls, report.entries.len());
    }

    #[test]
    fn test_dry_run_existing_topics_no_changes_skip() {
        use crate::compiler::llm::NoopProvider;
        use crate::compiler::storage::SqliteKnowledgeStore;

        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();

        // Create an existing topic page
        let now = Utc::now();
        let page = TopicPage {
            id: TopicId("existing-topic".to_string()),
            title: "Existing Topic".to_string(),
            content: "Some existing content".to_string(),
            sections: vec![],
            summary: "summary".to_string(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: vec!["m1".into(), "m2".into()],
                tags: vec![],
                quality_score: Some(0.8),
            },
        };
        store.create_topic_page(&page).unwrap();

        // Add source refs so decay doesn't trigger archive
        let refs = vec![
            SourceMemoryRef { memory_id: "m1".into(), relevance_score: 0.9, added_at: now },
            SourceMemoryRef { memory_id: "m2".into(), relevance_score: 0.9, added_at: now },
        ];
        store.save_source_refs(&TopicId("existing-topic".into()), &refs).unwrap();

        let mut config = make_config();
        config.min_cluster_size = 2;

        let pipeline = CompilationPipeline::<SqliteKnowledgeStore, NoopProvider>::new(
            store, None, config,
        );

        // Pass no memories — the existing topic should get Skip (or Archive via decay)
        // since there are no candidates at all
        let report = pipeline.dry_run(&[]).unwrap();

        // The existing topic should appear as Skip or Archive (decay-based)
        assert!(!report.entries.is_empty(), "Should have entry for existing topic");
        for entry in &report.entries {
            assert!(
                matches!(entry.action, DryRunAction::Skip | DryRunAction::Archive),
                "Expected Skip or Archive for unmatched topic, got {:?}",
                entry.action
            );
        }
    }

    // ── Verbose Flag Test ───────────────────────────────────────────────────

    #[test]
    fn test_verbose_compilation_succeeds() {
        use crate::compiler::llm::NoopProvider;
        use crate::compiler::storage::SqliteKnowledgeStore;

        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        let config = make_config();

        // Build pipeline with verbose=true
        let pipeline = CompilationPipeline::<SqliteKnowledgeStore, NoopProvider>::new(
            store, None, config,
        ).with_verbose(true);

        let memories = vec![
            MemorySnapshot::test("m1", "Test memory one"),
            MemorySnapshot::test("m2", "Test memory two"),
        ];

        let candidate = TopicCandidate {
            memories: vec!["m1".into(), "m2".into()],
            centroid_embedding: vec![0.0; 64],
            cohesion_score: 0.9,
            suggested_title: Some("Verbose Test Topic".to_string()),
        };

        // compile_new should succeed even with verbose=true (prints to stderr)
        let result = pipeline.compile_new(&candidate, &memories);
        assert!(result.is_ok(), "Compilation with verbose=true should succeed: {:?}", result.err());
        let page = result.unwrap();
        assert_eq!(page.title, "Verbose Test Topic");
        assert!(page.content.contains("Test memory one"));
    }
}
