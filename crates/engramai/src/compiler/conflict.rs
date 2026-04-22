//! Conflict detection and resolution for compiled topic pages.
//!
//! Finds contradictions between overlapping topics and identifies
//! near-duplicate topics that should be merged.

use std::collections::HashSet;
use std::sync::OnceLock;

use chrono::Utc;

use super::llm::LlmProvider;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Jaccard similarity of two sets: |intersection| / |union|.
fn jaccard_similarity(a: &HashSet<&str>, b: &HashSet<&str>) -> f64 {
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// Returns the set of stop words to exclude from content similarity computation.
///
/// Includes: template/markdown vocabulary, memory type labels, common English
/// stop words, and common Chinese stop words.
fn stop_words() -> &'static HashSet<&'static str> {
    static STOP_WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    STOP_WORDS.get_or_init(|| {
        [
            // Markdown headers / formatting
            "#", "##", "###", "---", "|", "*", "**",
            // Template labels
            "Topic", "Summary", "Compiled", "compiled", "from", "memories",
            "Key", "Points", "Details", "Memory", "Memory:", "Type", "Type:",
            "Importance", "Importance:", "Date", "Date:",
            // Memory type labels
            "factual", "procedural", "episodic", "relational", "emotional",
            "opinion", "causal",
            // Common English stop words
            "the", "a", "an", "is", "are", "was", "were", "be", "been",
            "being", "have", "has", "had", "do", "does", "did", "will",
            "would", "could", "should", "may", "might", "shall", "can",
            "need", "must", "to", "of", "in", "for", "on", "with", "at",
            "by", "as", "into", "through", "during", "before", "after",
            "above", "below", "between", "and", "but", "or", "nor", "not",
            "no", "so", "if", "then", "than", "that", "this", "these",
            "those", "it", "its", "they", "them", "their", "we", "our",
            "you", "your", "he", "she", "his", "her",
            // Common Chinese stop words
            "的", "了", "在", "是", "和", "有", "我", "不", "也", "就",
            "人", "都", "一", "这", "中", "上", "大", "会", "到", "来",
            "用", "要", "可以", "他", "她", "它", "个", "把", "被", "让",
            "给", "从", "对", "向", "与", "而", "但", "还", "又", "或",
            "如", "很", "更", "最", "那", "着", "过", "能", "时", "等",
            "所", "之", "以",
        ]
        .into_iter()
        .collect()
    })
}

/// Returns true if a token should be excluded from content similarity.
///
/// Filters out: stop words, tokens <= 2 chars, and pure-digit tokens.
fn is_noise_token(token: &str) -> bool {
    // Short tokens (catches date fragments, formatting artifacts, most single-char words)
    if token.len() <= 2 {
        return true;
    }
    // Pure digits (dates, numbers)
    if token.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Stop words (template vocab, common English/Chinese)
    stop_words().contains(token)
}

/// Compute Jaccard similarity of source_memory_ids for two topics.
fn source_overlap(a: &TopicPage, b: &TopicPage) -> f64 {
    let set_a: HashSet<&str> = a.metadata.source_memory_ids.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = b.metadata.source_memory_ids.iter().map(|s| s.as_str()).collect();
    jaccard_similarity(&set_a, &set_b)
}

/// Compute word-set Jaccard similarity of content strings.
///
/// Filters out non-discriminative vocabulary (template structure, stop words,
/// short tokens, pure digits) before computing Jaccard similarity. This prevents
/// false-positive conflicts caused by shared template boilerplate.
fn content_similarity(a: &TopicPage, b: &TopicPage) -> f64 {
    let words_a: HashSet<&str> = a
        .content
        .split_whitespace()
        .filter(|w| !is_noise_token(w))
        .collect();
    let words_b: HashSet<&str> = b
        .content
        .split_whitespace()
        .filter(|w| !is_noise_token(w))
        .collect();
    jaccard_similarity(&words_a, &words_b)
}

/// Determine severity based on source overlap level.
fn severity_from_overlap(overlap: f64) -> ConflictSeverity {
    if overlap >= 0.8 {
        ConflictSeverity::Critical
    } else if overlap >= 0.6 {
        ConflictSeverity::High
    } else if overlap >= 0.3 {
        ConflictSeverity::Medium
    } else {
        ConflictSeverity::Low
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CONFLICT DETECTOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Detects conflicts and duplicates across compiled topic pages.
pub struct ConflictDetector {
    similarity_threshold: f64,
    duplicate_threshold: f64,
}

impl ConflictDetector {
    /// Create a new detector with default thresholds.
    ///
    /// - `similarity_threshold`: 0.3 (minimum Jaccard for candidate pairs, after stop-word filtering)
    /// - `duplicate_threshold`: 0.6 (source overlap above which topics are duplicates)
    pub fn new() -> Self {
        Self {
            similarity_threshold: 0.3,
            duplicate_threshold: 0.6,
        }
    }

    /// Detect conflicts between topics.
    ///
    /// Two-phase approach:
    /// 1. Candidate selection — find topic pairs with overlapping source memories
    ///    (shared source_memory_ids via Jaccard > 0.1) or high summary similarity
    /// 2. For each pair, classify as contradiction, near-duplicate, or no conflict
    ///
    /// Without LLM: uses heuristics only (overlap + content similarity)
    /// With LLM: sends pairs to LLM for contradiction analysis
    pub fn detect_conflicts(
        &self,
        topics: &[TopicPage],
        scope: &ConflictScope,
        llm: Option<&dyn LlmProvider>,
    ) -> Result<Vec<ConflictRecord>, KcError> {
        let pairs = self.select_pairs(topics, scope);
        let mut results = Vec::new();

        for (i, j) in pairs {
            let topic_a = &topics[i];
            let topic_b = &topics[j];

            let overlap = source_overlap(topic_a, topic_b);
            let content_sim = content_similarity(topic_a, topic_b);

            // Phase 1: candidate selection — skip if below threshold
            if overlap <= self.similarity_threshold && content_sim <= self.similarity_threshold {
                continue;
            }

            // Phase 2: classification
            let conflict_type = if overlap > self.duplicate_threshold {
                ConflictType::Redundant
            } else if let Some(llm_provider) = llm {
                // Use LLM for contradiction check
                match self.llm_contradiction_check(topic_a, topic_b, llm_provider) {
                    Ok(true) => ConflictType::Contradiction,
                    Ok(false) => continue, // No conflict detected by LLM
                    Err(_) => {
                        // LLM failed, fall back to heuristic
                        self.heuristic_classify(overlap)
                    }
                }
            } else {
                // No LLM: heuristic classification
                self.heuristic_classify(overlap)
            };

            let severity = severity_from_overlap(overlap);
            let conflict_id = ConflictId(format!("conflict-{}-{}", topic_a.id, topic_b.id));

            let description = match &conflict_type {
                ConflictType::Redundant => format!(
                    "Topics '{}' and '{}' are near-duplicates (source overlap: {:.0}%)",
                    topic_a.title, topic_b.title, overlap * 100.0
                ),
                ConflictType::Contradiction => format!(
                    "Topics '{}' and '{}' may contain contradictory information (source overlap: {:.0}%)",
                    topic_a.title, topic_b.title, overlap * 100.0
                ),
                ConflictType::Outdated => format!(
                    "Topics '{}' and '{}' may have temporal conflict (source overlap: {:.0}%)",
                    topic_a.title, topic_b.title, overlap * 100.0
                ),
            };

            let evidence = vec![
                format!("Source memory overlap: {:.1}%", overlap * 100.0),
                format!("Content similarity: {:.1}%", content_sim * 100.0),
            ];

            let conflict = Conflict {
                id: conflict_id,
                conflict_type,
                scope: ConflictScope::BetweenTopics(topic_a.id.clone(), topic_b.id.clone()),
                description,
                status: ConflictStatus::Detected,
                detected_at: Utc::now(),
                resolved_at: None,
            };

            results.push(ConflictRecord {
                conflict,
                severity,
                evidence,
            });
        }

        Ok(results)
    }

    /// Find near-duplicate topic groups.
    ///
    /// Groups topics where source memory overlap > duplicate_threshold (default: 0.6)
    pub fn detect_duplicates(&self, topics: &[TopicPage]) -> Vec<DuplicateGroup> {
        let n = topics.len();
        // Union-Find for grouping
        let mut parent: Vec<usize> = (0..n).collect();

        fn find(parent: &mut [usize], x: usize) -> usize {
            if parent[x] != x {
                parent[x] = find(parent, parent[x]);
            }
            parent[x]
        }

        fn union(parent: &mut [usize], a: usize, b: usize) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra != rb {
                parent[rb] = ra;
            }
        }

        // Track max overlap within each eventual group
        let mut max_overlap: std::collections::HashMap<(usize, usize), f64> =
            std::collections::HashMap::new();

        for i in 0..n {
            for j in (i + 1)..n {
                let overlap = source_overlap(&topics[i], &topics[j]);
                if overlap > self.duplicate_threshold {
                    union(&mut parent, i, j);
                    let key = (i.min(j), i.max(j));
                    let entry = max_overlap.entry(key).or_insert(0.0);
                    if overlap > *entry {
                        *entry = overlap;
                    }
                }
            }
        }

        // Collect groups
        let mut groups: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            let root = find(&mut parent, i);
            groups.entry(root).or_default().push(i);
        }

        groups
            .into_values()
            .filter(|members| members.len() > 1)
            .map(|members| {
                // The topic with the most sources is canonical
                let canonical_idx = *members
                    .iter()
                    .max_by_key(|&&idx| topics[idx].metadata.source_memory_ids.len())
                    .unwrap();

                // Compute average similarity for the group
                let mut total_sim = 0.0;
                let mut pair_count = 0;
                for a in &members {
                    for b in &members {
                        if a < b {
                            let key = (*a.min(b), *a.max(b));
                            if let Some(&sim) = max_overlap.get(&key) {
                                total_sim += sim;
                                pair_count += 1;
                            }
                        }
                    }
                }
                let avg_sim = if pair_count > 0 {
                    total_sim / pair_count as f64
                } else {
                    0.0
                };

                let duplicates: Vec<TopicId> = members
                    .iter()
                    .filter(|&&idx| idx != canonical_idx)
                    .map(|&idx| topics[idx].id.clone())
                    .collect();

                DuplicateGroup {
                    canonical: topics[canonical_idx].id.clone(),
                    duplicates,
                    similarity: avg_sim,
                }
            })
            .collect()
    }

    /// Generate resolution suggestions for a conflict.
    pub fn suggest_resolutions(&self, conflict: &ConflictRecord) -> Vec<String> {
        match &conflict.conflict.conflict_type {
            ConflictType::Contradiction => vec![
                "Merge topics keeping newer content".to_string(),
                "Review both and manually resolve".to_string(),
                "Archive older topic".to_string(),
            ],
            ConflictType::Redundant => vec![
                "Merge into single topic".to_string(),
                "Archive the smaller topic".to_string(),
            ],
            ConflictType::Outdated => vec![
                "Recompile from latest sources".to_string(),
                "Archive outdated version".to_string(),
            ],
        }
    }

    // ── Private helpers ──────────────────────────────────────────────────

    /// Determine which topic index pairs to check based on the scope.
    fn select_pairs(&self, topics: &[TopicPage], scope: &ConflictScope) -> Vec<(usize, usize)> {
        let n = topics.len();
        match scope {
            ConflictScope::WithinTopic(target_id) => {
                // Find the target topic index
                if let Some(target_idx) = topics.iter().position(|t| t.id == *target_id) {
                    (0..n)
                        .filter(|&i| i != target_idx)
                        .map(|i| {
                            if target_idx < i {
                                (target_idx, i)
                            } else {
                                (i, target_idx)
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            ConflictScope::BetweenTopics(id_a, id_b) => {
                let pos_a = topics.iter().position(|t| t.id == *id_a);
                let pos_b = topics.iter().position(|t| t.id == *id_b);
                match (pos_a, pos_b) {
                    (Some(a), Some(b)) if a != b => vec![(a.min(b), a.max(b))],
                    _ => Vec::new(),
                }
            }
        }
    }

    /// Heuristic classification without LLM.
    ///
    /// - overlap > duplicate_threshold → Redundant
    /// - overlap > 0.3 → Outdated (assume temporal conflict)
    /// - otherwise not reached (filtered by candidate selection)
    fn heuristic_classify(&self, overlap: f64) -> ConflictType {
        if overlap > self.duplicate_threshold {
            ConflictType::Redundant
        } else if overlap > 0.3 {
            ConflictType::Outdated
        } else {
            // Low overlap candidate — still flag as outdated (heuristic best-guess)
            ConflictType::Outdated
        }
    }

    /// Use LLM to check if two topic summaries contradict each other.
    fn llm_contradiction_check(
        &self,
        topic_a: &TopicPage,
        topic_b: &TopicPage,
        llm: &dyn LlmProvider,
    ) -> Result<bool, KcError> {
        let prompt = format!(
            "Do these two topic summaries contradict each other? \
             Topic A: {}. Topic B: {}. \
             Answer YES or NO, then briefly explain.",
            topic_a.summary, topic_b.summary
        );

        let request = LlmRequest {
            task: LlmTask::DetectConflict,
            prompt,
            max_tokens: Some(256),
            temperature: Some(0.0),
        };

        let response = llm
            .complete(&request)
            .map_err(|e| KcError::ConflictDetection(format!("LLM error: {e}")))?;

        // Parse the response — look for YES at the beginning
        let content = response.content.trim().to_uppercase();
        Ok(content.starts_with("YES"))
    }
}

impl Default for ConflictDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ISS-020 P0.5 — Dimensional Conflict Detection (memory-level, pre-synthesis)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Operates on `MemorySnapshot` pairs BEFORE topic-page synthesis. Catches
// structural contradictions (same actors/domain + opposing stance) that the
// lexical Jaccard on post-compiled TopicPages can't see.
//
// Philosophy (investigation.md §4.4 + §6 Q6):
// - We flag *candidates*, not verdicts. The synthesis LLM gets the final say
//   on "contradiction vs evolution". This ships P0 without the false-positive
//   risk of a hard-reject rule.
// - Temporal succession guard: if dates parse on both sides and one clearly
//   post-dates the other → tag as `EvolutionCandidate`, lower severity.
//   When `temporal` is unparseable, fall back to `created_at` ordering.
//   **Never** string-compare free-form `temporal` phrases.

use crate::compiler::compilation::MemorySnapshot;

/// Kind of memory-level conflict candidate flagged for synthesis-time review.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DimensionalConflictKind {
    /// Same domain + participants overlap + non-equal stance — likely contradiction.
    /// Synthesis LLM decides if this is a real contradiction or context shift.
    StanceContradiction,
    /// Same pair as above, but temporal or created_at ordering suggests one
    /// memory supersedes the other — probably stance evolution, not contradiction.
    /// Emitted with lower priority so synthesis can downweight the older one.
    EvolutionCandidate,
}

/// A flagged dimensional conflict candidate between two memories.
/// NOT a verdict — synthesis LLM resolves ambiguity.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DimensionalConflictCandidate {
    pub memory_a: String,
    pub memory_b: String,
    pub kind: DimensionalConflictKind,
    /// Domain both memories share (non-empty by construction).
    pub domain: String,
    /// One-line rationale for humans / LLM context.
    pub reason: String,
}

/// Tokenize a participants string into case-insensitive entity tokens.
/// Very conservative: whitespace split + lowercase + strip trailing punctuation.
fn tokenize_participants(s: &str) -> HashSet<String> {
    s.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '/')
        .filter_map(|t| {
            let t = t.trim_matches(|c: char| !c.is_alphanumeric());
            if t.is_empty() {
                None
            } else {
                Some(t.to_ascii_lowercase())
            }
        })
        .collect()
}

/// True when both participants strings share at least one token.
/// Returns false if either side is None.
fn participants_overlap(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => {
            let ax = tokenize_participants(x);
            let bx = tokenize_participants(y);
            !ax.is_disjoint(&bx)
        }
        _ => false,
    }
}

/// Minimal stance-opposition heuristic (P0.5 crude v1).
///
/// True when both sides have stance AND the stance strings are not
/// (case-insensitively) equal. Upgradable later to an LLM / opposition model.
/// Deliberately crude — synthesis LLM is the final arbiter per §6 Q6.
fn stances_oppose(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x.trim().eq_ignore_ascii_case(y.trim()).not(),
        _ => false,
    }
}

// Stable `.not()` on bool came with Rust 1.73; provide a shim trait if older.
// Use builtin `!` instead to avoid dependency on any specific version.
trait BoolExt {
    fn not(self) -> bool;
}
impl BoolExt for bool {
    #[inline]
    fn not(self) -> bool {
        !self
    }
}

/// Temporal-succession guard.
///
/// Returns `Some(true)`  if memory B clearly supersedes A (B is newer).
/// Returns `Some(false)` if A supersedes B.
/// Returns `None`        if ordering cannot be established.
///
/// Strategy per investigation §6 Q6:
/// 1. Prefer parseable `temporal` strings — but parsing free-form phrases
///    like "Q1 2025" or "after the refactor" is hard; we only accept
///    ISO-8601 dates here (`YYYY-MM-DD`). Anything else falls through.
/// 2. Fallback: compare `created_at` timestamps — weaker but always available.
fn temporal_succession(a: &MemorySnapshot, b: &MemorySnapshot) -> Option<bool> {
    // Try parseable temporal dates first.
    fn parse_iso(s: &str) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
    }
    let ta = a
        .dimensions
        .as_ref()
        .and_then(|d| d.temporal.as_deref())
        .and_then(parse_iso);
    let tb = b
        .dimensions
        .as_ref()
        .and_then(|d| d.temporal.as_deref())
        .and_then(parse_iso);
    if let (Some(da), Some(db)) = (ta, tb) {
        if da != db {
            return Some(db > da);
        }
    }
    // Fallback: created_at.
    if a.created_at != b.created_at {
        Some(b.created_at > a.created_at)
    } else {
        None
    }
}

/// Detect dimensional conflict candidates between a pair of memories.
///
/// Returns `None` when:
/// - Either memory has no dimensions (legacy — graceful degradation §7.2)
/// - Domains absent or mismatched
/// - No participant overlap
/// - Stances equal or either side missing
///
/// When a StanceContradiction candidate is detected AND temporal succession
/// is clearly resolvable, the result is downgraded to `EvolutionCandidate`.
pub fn detect_pair_dimensional_conflict(
    a: &MemorySnapshot,
    b: &MemorySnapshot,
) -> Option<DimensionalConflictCandidate> {
    let da = a.dimensions.as_ref()?;
    let db = b.dimensions.as_ref()?;

    // Same domain — MUST be set on both and equal (case-sensitive match OK;
    // extractor produces a fixed vocabulary).
    let domain_a = da.domain.as_deref()?;
    let domain_b = db.domain.as_deref()?;
    if domain_a != domain_b {
        return None;
    }

    // Participants must overlap.
    if !participants_overlap(da.participants.as_deref(), db.participants.as_deref()) {
        return None;
    }

    // Stances must be divergent (both present + non-equal).
    if !stances_oppose(da.stance.as_deref(), db.stance.as_deref()) {
        return None;
    }

    // Classify: contradiction vs evolution.
    let kind = match temporal_succession(a, b) {
        Some(_) => DimensionalConflictKind::EvolutionCandidate,
        None => DimensionalConflictKind::StanceContradiction,
    };

    let reason = format!(
        "domain={domain_a}; stance A={:?}; stance B={:?}",
        da.stance, db.stance
    );
    Some(DimensionalConflictCandidate {
        memory_a: a.id.clone(),
        memory_b: b.id.clone(),
        kind,
        domain: domain_a.to_string(),
        reason,
    })
}

/// Scan all memory pairs in a candidate set for dimensional conflicts.
///
/// O(N²) — acceptable at KC scale (typically <500 memories per topic).
/// For larger inputs, consider pre-grouping by `dimensions.domain` first
/// (investigation §4.5 clustering pre-filter, P1 work).
pub fn detect_dimensional_conflicts(
    memories: &[MemorySnapshot],
) -> Vec<DimensionalConflictCandidate> {
    let mut out = Vec::new();
    for i in 0..memories.len() {
        for j in (i + 1)..memories.len() {
            if let Some(c) = detect_pair_dimensional_conflict(&memories[i], &memories[j]) {
                out.push(c);
            }
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;


    fn make_topic(id: &str, title: &str, sources: &[&str]) -> TopicPage {
        TopicPage {
            id: TopicId(id.to_string()),
            title: title.to_string(),
            content: format!("Content about {title}"),
            sections: Vec::new(),
            summary: format!("Summary of {title}"),
            metadata: TopicMetadata {
                created_at: Utc::now(),
                updated_at: Utc::now(),
                compilation_count: 1,
                source_memory_ids: sources.iter().map(|s| s.to_string()).collect(),
                tags: vec![],
                quality_score: Some(0.8),
            },
            status: TopicStatus::Active,
            version: 1,
        }
    }

    /// Helper to create a topic with custom content and sources.
    fn make_topic_with_content(
        id: &str,
        title: &str,
        content: &str,
        sources: &[&str],
    ) -> TopicPage {
        TopicPage {
            id: TopicId(id.to_string()),
            title: title.to_string(),
            content: content.to_string(),
            sections: Vec::new(),
            summary: format!("Summary of {title}"),
            metadata: TopicMetadata {
                created_at: Utc::now(),
                updated_at: Utc::now(),
                compilation_count: 1,
                source_memory_ids: sources.iter().map(|s| s.to_string()).collect(),
                tags: vec![],
                quality_score: Some(0.8),
            },
            status: TopicStatus::Active,
            version: 1,
        }
    }

    // ── test_detect_conflict_overlapping ──────────────────────────────────

    #[test]
    fn test_detect_conflict_overlapping() {
        // Two topics with 50% source overlap (5 shared out of 10 union)
        // Topic A: m1..m7, Topic B: m3..m10 → shared m3..m7 = 5, union m1..m10 = 10
        let topic_a = make_topic("a", "Topic A", &["m1", "m2", "m3", "m4", "m5", "m6", "m7"]);
        let topic_b = make_topic("b", "Topic B", &["m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"]);
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        assert_eq!(conflicts.len(), 1);
        // 50% overlap: > 0.3 but < 0.6 → Outdated (heuristic)
        assert_eq!(conflicts[0].conflict.conflict_type, ConflictType::Outdated);
    }

    // ── test_detect_no_conflict ──────────────────────────────────────────

    #[test]
    fn test_detect_no_conflict() {
        // Disjoint topics — no shared sources, distinct content
        let topic_a = make_topic_with_content(
            "a",
            "Rust Programming",
            "systems language focusing on safety and performance",
            &["m1", "m2", "m3"],
        );
        let topic_b = make_topic_with_content(
            "b",
            "French Cooking",
            "culinary arts and gourmet recipes from Paris",
            &["m10", "m11", "m12"],
        );
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        assert!(conflicts.is_empty());
    }

    // ── test_detect_duplicates_basic ─────────────────────────────────────

    #[test]
    fn test_detect_duplicates_basic() {
        // Two topics with 80% overlap: 8 shared out of 10 union
        let topic_a = make_topic("a", "Topic A", &["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9"]);
        let topic_b = make_topic("b", "Topic B", &["m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"]);
        // shared: m2..m9 = 8, union: m1..m10 = 10 → jaccard = 0.8
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let groups = detector.detect_duplicates(&topics);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].duplicates.len(), 1);
        assert!(groups[0].similarity > 0.7);
    }

    // ── test_detect_duplicates_none ──────────────────────────────────────

    #[test]
    fn test_detect_duplicates_none() {
        // Completely disjoint topics
        let topic_a = make_topic_with_content(
            "a",
            "Rust Programming",
            "systems language focusing on safety and performance",
            &["m1", "m2", "m3"],
        );
        let topic_b = make_topic_with_content(
            "b",
            "French Cooking",
            "culinary arts and gourmet recipes from Paris",
            &["m10", "m11", "m12"],
        );
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let groups = detector.detect_duplicates(&topics);

        assert!(groups.is_empty());
    }

    // ── test_scope_within_topic ──────────────────────────────────────────

    #[test]
    fn test_scope_within_topic() {
        // Three topics: A overlaps B, A overlaps C, B does not overlap C
        let topic_a = make_topic("a", "Topic A", &["m1", "m2", "m3", "m4", "m5"]);
        let topic_b = make_topic("b", "Topic B", &["m3", "m4", "m5", "m6", "m7"]);
        let topic_c = make_topic("c", "Topic C", &["m1", "m2", "m3", "m8", "m9"]);
        let topics = vec![topic_a, topic_b, topic_c];

        let detector = ConflictDetector::new();
        // Only check conflicts involving topic B
        let scope = ConflictScope::WithinTopic(TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        // B overlaps A (shared m3,m4,m5 out of union m1..m7 = 7 → ~0.43)
        // B overlaps C (shared m3 out of union m1..m9 minus gaps = {m1,m2,m3,m4,m5,m6,m7,m8,m9} = 9 → ~0.11)
        // All conflicts should involve topic B
        for c in &conflicts {
            match &c.conflict.scope {
                ConflictScope::BetweenTopics(a, b) => {
                    assert!(
                        a.0 == "b" || b.0 == "b",
                        "Expected conflict to involve topic 'b', got ({}, {})",
                        a, b
                    );
                }
                _ => panic!("Expected BetweenTopics scope"),
            }
        }
        // At least one conflict (A-B)
        assert!(!conflicts.is_empty());
    }

    // ── test_scope_between_topics ────────────────────────────────────────

    #[test]
    fn test_scope_between_topics() {
        let topic_a = make_topic("a", "Topic A", &["m1", "m2", "m3", "m4", "m5"]);
        let topic_b = make_topic("b", "Topic B", &["m3", "m4", "m5", "m6", "m7"]);
        let topic_c = make_topic("c", "Topic C", &["m1", "m2", "m3", "m8", "m9"]);
        let topics = vec![topic_a, topic_b, topic_c];

        let detector = ConflictDetector::new();
        // Only check A vs C
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("c".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        // Verify only A-C pair is present (if they overlap enough)
        for c in &conflicts {
            match &c.conflict.scope {
                ConflictScope::BetweenTopics(a, b) => {
                    let pair = (&a.0 as &str, &b.0 as &str);
                    assert!(
                        pair == ("a", "c") || pair == ("c", "a"),
                        "Expected conflict between 'a' and 'c', got ({}, {})",
                        a, b
                    );
                }
                _ => panic!("Expected BetweenTopics scope"),
            }
        }
    }

    // ── test_suggest_resolutions ─────────────────────────────────────────

    #[test]
    fn test_suggest_resolutions() {
        let detector = ConflictDetector::new();

        // Contradiction
        let contradiction_record = ConflictRecord {
            conflict: Conflict {
                id: ConflictId("c1".into()),
                conflict_type: ConflictType::Contradiction,
                scope: ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into())),
                description: "test".into(),
                status: ConflictStatus::Detected,
                detected_at: Utc::now(),
                resolved_at: None,
            },
            severity: ConflictSeverity::High,
            evidence: vec![],
        };
        let suggestions = detector.suggest_resolutions(&contradiction_record);
        assert_eq!(suggestions.len(), 3);
        assert!(suggestions[0].contains("newer content"));

        // Redundant
        let redundant_record = ConflictRecord {
            conflict: Conflict {
                id: ConflictId("c2".into()),
                conflict_type: ConflictType::Redundant,
                scope: ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into())),
                description: "test".into(),
                status: ConflictStatus::Detected,
                detected_at: Utc::now(),
                resolved_at: None,
            },
            severity: ConflictSeverity::Medium,
            evidence: vec![],
        };
        let suggestions = detector.suggest_resolutions(&redundant_record);
        assert_eq!(suggestions.len(), 2);
        assert!(suggestions[0].contains("Merge"));

        // Outdated
        let outdated_record = ConflictRecord {
            conflict: Conflict {
                id: ConflictId("c3".into()),
                conflict_type: ConflictType::Outdated,
                scope: ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into())),
                description: "test".into(),
                status: ConflictStatus::Detected,
                detected_at: Utc::now(),
                resolved_at: None,
            },
            severity: ConflictSeverity::Low,
            evidence: vec![],
        };
        let suggestions = detector.suggest_resolutions(&outdated_record);
        assert_eq!(suggestions.len(), 2);
        assert!(suggestions[0].contains("Recompile"));
    }

    // ── test_conflict_severity ───────────────────────────────────────────

    #[test]
    fn test_conflict_severity() {
        // Verify severity is assigned based on overlap level
        assert_eq!(severity_from_overlap(0.05), ConflictSeverity::Low);
        assert_eq!(severity_from_overlap(0.15), ConflictSeverity::Low);
        assert_eq!(severity_from_overlap(0.35), ConflictSeverity::Medium);
        assert_eq!(severity_from_overlap(0.5), ConflictSeverity::Medium);
        assert_eq!(severity_from_overlap(0.65), ConflictSeverity::High);
        assert_eq!(severity_from_overlap(0.75), ConflictSeverity::High);
        assert_eq!(severity_from_overlap(0.85), ConflictSeverity::Critical);
        assert_eq!(severity_from_overlap(0.95), ConflictSeverity::Critical);

        // Also test via actual conflict detection to ensure severity is assigned
        // Topic overlap of ~43% → Medium severity
        let topic_a = make_topic("a", "Topic A", &["m1", "m2", "m3", "m4", "m5"]);
        let topic_b = make_topic("b", "Topic B", &["m3", "m4", "m5", "m6", "m7"]);
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].severity, ConflictSeverity::Medium);
    }

    // ── Additional edge-case tests ───────────────────────────────────────

    #[test]
    fn test_empty_topics() {
        let detector = ConflictDetector::new();
        let topics: Vec<TopicPage> = vec![];
        let scope = ConflictScope::WithinTopic(TopicId("x".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();
        assert!(conflicts.is_empty());

        let groups = detector.detect_duplicates(&topics);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_single_topic() {
        let detector = ConflictDetector::new();
        let topics = vec![make_topic("a", "Topic A", &["m1", "m2"])];
        let scope = ConflictScope::WithinTopic(TopicId("a".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_redundant_conflict_high_overlap() {
        // 90% overlap → Redundant
        let topic_a = make_topic(
            "a",
            "Topic A",
            &["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );
        let topic_b = make_topic(
            "b",
            "Topic B",
            &["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m11"],
        );
        // shared: m1..m9 = 9, union: m1..m11 = 11 → jaccard ≈ 0.818
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict.conflict_type, ConflictType::Redundant);
    }

    #[test]
    fn test_content_similarity_candidate() {
        // Topics with no source overlap but high content similarity
        let topic_a = make_topic_with_content(
            "a",
            "Topic A",
            "the quick brown fox jumps over the lazy dog every day",
            &["m1", "m2"],
        );
        let topic_b = make_topic_with_content(
            "b",
            "Topic B",
            "the quick brown fox jumps over the lazy dog every night",
            &["m3", "m4"],
        );
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let scope = ConflictScope::BetweenTopics(TopicId("a".into()), TopicId("b".into()));
        let conflicts = detector.detect_conflicts(&topics, &scope, None).unwrap();

        // High content similarity should produce a candidate
        // Content words: 9 shared out of 11 union → ~0.82 > 0.1 threshold
        assert!(!conflicts.is_empty());
    }

    #[test]
    fn test_duplicate_canonical_has_most_sources() {
        // The topic with more sources should be canonical
        let topic_a = make_topic("a", "Smaller Topic", &["m1", "m2", "m3", "m4", "m5"]);
        let topic_b = make_topic(
            "b",
            "Bigger Topic",
            &["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8"],
        );
        // shared: 5, union: 8 → jaccard = 0.625 > 0.6
        let topics = vec![topic_a, topic_b];

        let detector = ConflictDetector::new();
        let groups = detector.detect_duplicates(&topics);

        assert_eq!(groups.len(), 1);
        // b has more sources → canonical
        assert_eq!(groups[0].canonical.0, "b");
        assert_eq!(groups[0].duplicates.len(), 1);
        assert_eq!(groups[0].duplicates[0].0, "a");
    }

    #[test]
    fn test_default_impl() {
        let detector = ConflictDetector::default();
        assert_eq!(detector.similarity_threshold, 0.3);
        assert_eq!(detector.duplicate_threshold, 0.6);
    }

    // ── Stop-word filtering tests ────────────────────────────────────────

    #[test]
    fn test_content_similarity_template_only_returns_zero() {
        // Two topics that share ONLY template vocabulary — should return 0.0
        let topic_a = make_topic_with_content(
            "a",
            "Topic A",
            "# Topic\n## Summary\nCompiled from 3 memories\n## Key Points\n## Details\n### Memory:\nType: factual\nImportance: 0.8\nDate: 2024-01-15\n---",
            &["m1", "m2"],
        );
        let topic_b = make_topic_with_content(
            "b",
            "Topic B",
            "# Topic\n## Summary\nCompiled from 5 memories\n## Key Points\n## Details\n### Memory:\nType: procedural\nImportance: 0.6\nDate: 2024-03-20\n---",
            &["m3", "m4"],
        );

        let sim = content_similarity(&topic_a, &topic_b);
        assert_eq!(sim, 0.0, "Topics sharing only template vocabulary should have 0.0 similarity");
    }

    #[test]
    fn test_content_similarity_same_subject_different_templates() {
        // Two topics about the same subject but with different template decorations
        let topic_a = make_topic_with_content(
            "a",
            "Rust Ownership",
            "# Topic\n## Summary\nCompiled from 4 memories\nRust ownership borrowing lifetimes stack heap allocation move semantics\nType: factual\nImportance: 0.9",
            &["m1", "m2"],
        );
        let topic_b = make_topic_with_content(
            "b",
            "Rust Memory",
            "## Details\n### Memory:\nRust ownership borrowing lifetimes stack heap allocation drop trait\nDate: 2024-06-01\nType: procedural",
            &["m3", "m4"],
        );

        let sim = content_similarity(&topic_a, &topic_b);
        assert!(
            sim > 0.4,
            "Topics about the same subject should have high similarity after filtering, got {sim}"
        );
    }

    #[test]
    fn test_stop_word_set_includes_template_vocabulary() {
        let sw = stop_words();
        // Template markdown
        assert!(sw.contains("#"));
        assert!(sw.contains("##"));
        assert!(sw.contains("###"));
        assert!(sw.contains("---"));
        // Template labels
        assert!(sw.contains("Topic"));
        assert!(sw.contains("Summary"));
        assert!(sw.contains("Compiled"));
        assert!(sw.contains("compiled"));
        assert!(sw.contains("memories"));
        assert!(sw.contains("Key"));
        assert!(sw.contains("Points"));
        assert!(sw.contains("Details"));
        assert!(sw.contains("Memory:"));
        assert!(sw.contains("Type:"));
        assert!(sw.contains("Importance:"));
        assert!(sw.contains("Date:"));
        // Memory types
        assert!(sw.contains("factual"));
        assert!(sw.contains("procedural"));
        assert!(sw.contains("episodic"));
        assert!(sw.contains("relational"));
        assert!(sw.contains("emotional"));
        assert!(sw.contains("opinion"));
        assert!(sw.contains("causal"));
    }

    #[test]
    fn test_noise_token_filter() {
        // Short tokens filtered
        assert!(is_noise_token("a"));
        assert!(is_noise_token("is"));
        assert!(is_noise_token(""));
        // Pure digits filtered
        assert!(is_noise_token("2024"));
        assert!(is_noise_token("04"));
        assert!(is_noise_token("15"));
        // Stop words filtered
        assert!(is_noise_token("the"));
        assert!(is_noise_token("and"));
        assert!(is_noise_token("的"));
        assert!(is_noise_token("factual"));
        // Real content words NOT filtered
        assert!(!is_noise_token("Rust"));
        assert!(!is_noise_token("ownership"));
        assert!(!is_noise_token("borrowing"));
        assert!(!is_noise_token("programming"));
    }

    // ─── ISS-020 P0.5 dimensional conflict detection ──────────────────────────

    fn mk_dim(
        stance: Option<&str>,
        participants: Option<&str>,
        domain: Option<&str>,
        temporal: Option<&str>,
    ) -> Dimensions {
        Dimensions {
            stance: stance.map(String::from),
            participants: participants.map(String::from),
            domain: domain.map(String::from),
            temporal: temporal.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn iss020_p0_5_same_domain_same_participants_opposing_stance_flags_conflict() {
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "Rust is the best",
            mk_dim(Some("prefers Rust"), Some("potato"), Some("coding"), None),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "Go is better",
            mk_dim(Some("prefers Go"), Some("potato"), Some("coding"), None),
        );
        let conflict = detect_pair_dimensional_conflict(&a, &b);
        let c = conflict.expect("conflict must be flagged");
        // Without temporal ordering AND same created_at (new test), it's a pure contradiction.
        // But MemorySnapshot::test uses Utc::now() so created_at will differ slightly;
        // accept either classification — the important thing is that SOMETHING was flagged.
        assert!(matches!(
            c.kind,
            DimensionalConflictKind::StanceContradiction | DimensionalConflictKind::EvolutionCandidate
        ));
        assert_eq!(c.domain, "coding");
    }

    #[test]
    fn iss020_p0_5_different_domain_no_conflict() {
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "",
            mk_dim(Some("prefers X"), Some("potato"), Some("coding"), None),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "",
            mk_dim(Some("prefers Y"), Some("potato"), Some("trading"), None),
        );
        assert!(detect_pair_dimensional_conflict(&a, &b).is_none());
    }

    #[test]
    fn iss020_p0_5_no_participants_overlap_no_conflict() {
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "",
            mk_dim(Some("prefers X"), Some("alice"), Some("coding"), None),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "",
            mk_dim(Some("prefers Y"), Some("bob"), Some("coding"), None),
        );
        assert!(detect_pair_dimensional_conflict(&a, &b).is_none());
    }

    #[test]
    fn iss020_p0_5_equal_stance_no_conflict() {
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "",
            mk_dim(Some("prefers Rust"), Some("potato"), Some("coding"), None),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "",
            mk_dim(Some("PREFERS RUST"), Some("potato"), Some("coding"), None),
        );
        assert!(
            detect_pair_dimensional_conflict(&a, &b).is_none(),
            "case-insensitive equal stance must not flag"
        );
    }

    #[test]
    fn iss020_p0_5_legacy_memory_no_conflict() {
        // dimensions=None → graceful degradation, falls through.
        let a = MemorySnapshot::test("a", "legacy");
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "new",
            mk_dim(Some("prefers X"), Some("potato"), Some("coding"), None),
        );
        assert!(detect_pair_dimensional_conflict(&a, &b).is_none());
    }

    #[test]
    fn iss020_p0_5_temporal_succession_downgrades_to_evolution() {
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "Rust then",
            mk_dim(
                Some("prefers Rust"),
                Some("potato"),
                Some("coding"),
                Some("2025-01-01"),
            ),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "Go now",
            mk_dim(
                Some("prefers Go"),
                Some("potato"),
                Some("coding"),
                Some("2026-01-01"),
            ),
        );
        let c = detect_pair_dimensional_conflict(&a, &b).expect("conflict candidate");
        assert_eq!(c.kind, DimensionalConflictKind::EvolutionCandidate);
    }

    #[test]
    fn iss020_p0_5_participants_tokenization_case_insensitive() {
        // Multi-token participants with different casing still overlap.
        let a = MemorySnapshot::test_with_dimensions(
            "a",
            "",
            mk_dim(Some("s1"), Some("Alice, Bob"), Some("coding"), None),
        );
        let b = MemorySnapshot::test_with_dimensions(
            "b",
            "",
            mk_dim(Some("s2"), Some("bob and charlie"), Some("coding"), None),
        );
        // Both have different stances + share "bob" (case-insensitive) + same domain.
        let c = detect_pair_dimensional_conflict(&a, &b);
        assert!(c.is_some(), "bob/Bob must overlap across case");
    }

    #[test]
    fn iss020_p0_5_scan_multiple_memories() {
        let c_rust_2025 = mk_dim(
            Some("prefers Rust"),
            Some("potato"),
            Some("coding"),
            Some("2025-01-01"),
        );
        let c_go_2026 = mk_dim(
            Some("prefers Go"),
            Some("potato"),
            Some("coding"),
            Some("2026-01-01"),
        );
        let trading = mk_dim(
            Some("bullish ETH"),
            Some("potato"),
            Some("trading"),
            None,
        );
        let mems = vec![
            MemorySnapshot::test_with_dimensions("m1", "c1", c_rust_2025),
            MemorySnapshot::test_with_dimensions("m2", "c2", c_go_2026),
            MemorySnapshot::test_with_dimensions("m3", "c3", trading),
        ];
        let conflicts = detect_dimensional_conflicts(&mems);
        assert_eq!(conflicts.len(), 1, "only m1↔m2 (coding) should flag");
        assert_eq!(conflicts[0].memory_a, "m1");
        assert_eq!(conflicts[0].memory_b, "m2");
        assert_eq!(conflicts[0].kind, DimensionalConflictKind::EvolutionCandidate);
    }
}
