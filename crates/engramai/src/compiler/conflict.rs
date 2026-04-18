//! Conflict detection and resolution for compiled topic pages.
//!
//! Finds contradictions between overlapping topics and identifies
//! near-duplicate topics that should be merged.

use std::collections::HashSet;

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

/// Compute Jaccard similarity of source_memory_ids for two topics.
fn source_overlap(a: &TopicPage, b: &TopicPage) -> f64 {
    let set_a: HashSet<&str> = a.metadata.source_memory_ids.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = b.metadata.source_memory_ids.iter().map(|s| s.as_str()).collect();
    jaccard_similarity(&set_a, &set_b)
}

/// Compute word-set Jaccard similarity of content strings.
fn content_similarity(a: &TopicPage, b: &TopicPage) -> f64 {
    let words_a: HashSet<&str> = a.content.split_whitespace().collect();
    let words_b: HashSet<&str> = b.content.split_whitespace().collect();
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
    /// - `similarity_threshold`: 0.1 (minimum Jaccard for candidate pairs)
    /// - `duplicate_threshold`: 0.6 (source overlap above which topics are duplicates)
    pub fn new() -> Self {
        Self {
            similarity_threshold: 0.1,
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
                match self.heuristic_classify(overlap) {
                    ct => ct,
                }
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
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Helper to create a topic page with specified source memory IDs.
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
        assert_eq!(detector.similarity_threshold, 0.1);
        assert_eq!(detector.duplicate_threshold, 0.6);
    }
}
