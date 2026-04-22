//! TopicLifecycle — manages topic merge, split, and cross-topic linking (§3.4).
//!
//! Traces: GOAL-comp.4 (merge), GOAL-comp.5 (split), GOAL-comp.6 (cross-topic links).

use std::collections::HashSet;

use crate::compiler::types::*;

pub struct TopicLifecycle {
    config: LifecycleConfig,
}

impl TopicLifecycle {
    pub fn new(config: LifecycleConfig) -> Self {
        Self { config }
    }

    /// Analyze all topics and return suggested lifecycle operations.
    /// Takes topics and their source memory ID sets.
    pub fn analyze(&self, topics: &[(TopicPage, Vec<String>)]) -> LifecycleAnalysis {
        let mut merges = Vec::new();
        let mut splits = Vec::new();
        let mut links = Vec::new();

        // Merge detection: check all pairs
        for i in 0..topics.len() {
            for j in (i + 1)..topics.len() {
                let (ref page_a, ref mems_a) = topics[i];
                let (ref page_b, ref mems_b) = topics[j];

                let overlap = Self::memory_overlap(mems_a, mems_b);

                if overlap >= self.config.merge_overlap_threshold {
                    merges.push(LifecycleOp::Merge {
                        sources: vec![page_a.id.clone(), page_b.id.clone()],
                        target_title: format!("{} + {}", page_a.title, page_b.title),
                    });
                } else if overlap >= self.config.link_min_strength {
                    // Below merge threshold but above link threshold → cross-topic link
                    let shared: Vec<String> = {
                        let set_a: HashSet<&str> = mems_a.iter().map(|s| s.as_str()).collect();
                        mems_b
                            .iter()
                            .filter(|m| set_a.contains(m.as_str()))
                            .cloned()
                            .collect()
                    };
                    links.push(CrossTopicLink {
                        source: page_a.id.clone(),
                        target: page_b.id.clone(),
                        link_type: LinkType::References,
                        strength: overlap,
                        shared_memory_ids: shared,
                    });
                }
            }

            // Split detection: check if topic has too many sources
            let (ref page, ref mems) = topics[i];
            if mems.len() > self.config.max_topic_points {
                // Suggest split into ceil(len / max_topic_points) sub-topics
                let num_splits = mems.len().div_ceil(self.config.max_topic_points);
                let sub_titles: Vec<String> = (1..=num_splits)
                    .map(|n| format!("{} (Part {})", page.title, n))
                    .collect();
                splits.push(LifecycleOp::Split {
                    source: page.id.clone(),
                    new_topics: sub_titles,
                });
            }
        }

        LifecycleAnalysis {
            merges,
            splits,
            links,
        }
    }

    /// Execute a merge: combine source memory IDs from multiple topics into one.
    /// Returns the merged TopicPage and updated originals (now Archived).
    pub fn execute_merge(
        &self,
        sources: &[TopicPage],
        source_memories: &[Vec<String>],
        target_title: &str,
    ) -> Result<MergeResult, KcError> {
        if sources.len() < 2 {
            return Err(KcError::InvalidInput(
                "Merge requires at least 2 topics".into(),
            ));
        }

        // Union all source memory IDs
        let mut all_memories: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for mems in source_memories {
            for m in mems {
                if seen.insert(m.clone()) {
                    all_memories.push(m.clone());
                }
            }
        }

        // Create merged page — content will be filled by CompilationPipeline later
        let now = chrono::Utc::now();
        let merged = TopicPage {
            id: TopicId(format!("merged-{}", now.timestamp_millis())),
            title: target_title.to_string(),
            content: String::new(), // Placeholder — needs LLM compilation
            sections: Vec::new(),
            summary: format!("Merged from {} topics", sources.len()),
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 0,
                source_memory_ids: all_memories.clone(),
                tags: {
                    let mut tags: Vec<String> = sources
                        .iter()
                        .flat_map(|s| s.metadata.tags.iter().cloned())
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect();
                    tags.sort();
                    tags
                },
                quality_score: None,
            },
            status: TopicStatus::Active,
            version: 1,
        };

        let archived_ids: Vec<TopicId> = sources.iter().map(|s| s.id.clone()).collect();

        Ok(MergeResult {
            merged_page: merged,
            archived_topic_ids: archived_ids,
            unified_memory_ids: all_memories,
        })
    }

    /// Execute a split: partition source memories into sub-groups.
    /// Returns new TopicPages (stubs — need LLM compilation) and the original to archive.
    pub fn execute_split(
        &self,
        source: &TopicPage,
        source_memories: &[String],
        sub_titles: &[String],
    ) -> Result<SplitResult, KcError> {
        if sub_titles.is_empty() {
            return Err(KcError::InvalidInput(
                "Split requires at least 1 sub-topic".into(),
            ));
        }
        if source_memories.is_empty() {
            return Err(KcError::InvalidInput(
                "No source memories to split".into(),
            ));
        }

        let now = chrono::Utc::now();
        let chunk_size = source_memories.len().div_ceil(sub_titles.len());
        let mut new_pages = Vec::new();

        for (i, title) in sub_titles.iter().enumerate() {
            let start = i * chunk_size;
            let end = ((i + 1) * chunk_size).min(source_memories.len());
            if start >= source_memories.len() {
                break;
            }
            let chunk_memories: Vec<String> = source_memories[start..end].to_vec();

            new_pages.push(TopicPage {
                id: TopicId(format!("split-{}-{}", now.timestamp_millis(), i)),
                title: title.clone(),
                content: String::new(),
                sections: Vec::new(),
                summary: format!("Split from '{}'", source.title),
                metadata: TopicMetadata {
                    created_at: now,
                    updated_at: now,
                    compilation_count: 0,
                    source_memory_ids: chunk_memories,
                    tags: source.metadata.tags.clone(),
                    quality_score: None,
                },
                status: TopicStatus::Active,
                version: 1,
            });
        }

        Ok(SplitResult {
            new_pages,
            original_topic_id: source.id.clone(),
        })
    }

    /// Compute memory overlap ratio between two sets: |A ∩ B| / min(|A|, |B|).
    /// Returns 0.0 if either set is empty.
    fn memory_overlap(a: &[String], b: &[String]) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let set_a: HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
        let set_b: HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
        let intersection = set_a.intersection(&set_b).count();
        let min_size = set_a.len().min(set_b.len());
        intersection as f64 / min_size as f64
    }
}

/// Result of analyzing all topics for lifecycle operations.
#[derive(Clone, Debug)]
pub struct LifecycleAnalysis {
    pub merges: Vec<LifecycleOp>,
    pub splits: Vec<LifecycleOp>,
    pub links: Vec<CrossTopicLink>,
}

/// Result of a merge operation.
#[derive(Clone, Debug)]
pub struct MergeResult {
    pub merged_page: TopicPage,
    pub archived_topic_ids: Vec<TopicId>,
    pub unified_memory_ids: Vec<String>,
}

/// Result of a split operation.
#[derive(Clone, Debug)]
pub struct SplitResult {
    pub new_pages: Vec<TopicPage>,
    pub original_topic_id: TopicId,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_topic(id: &str, title: &str, memory_ids: Vec<&str>) -> (TopicPage, Vec<String>) {
        let now = chrono::Utc::now();
        let mems: Vec<String> = memory_ids.iter().map(|s| s.to_string()).collect();
        let page = TopicPage {
            id: TopicId(id.to_string()),
            title: title.to_string(),
            content: String::new(),
            sections: Vec::new(),
            summary: String::new(),
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 0,
                source_memory_ids: mems.clone(),
                tags: vec![],
                quality_score: None,
            },
            status: TopicStatus::Active,
            version: 1,
        };
        (page, mems)
    }

    #[test]
    fn test_analyze_detects_merge() {
        // Two topics with 70% overlap → merges list has 1 entry
        // A: m1..m10, B: m1..m7 + m11..m13  → 7 shared out of min(10,10)=10 → 0.7
        let a = make_topic(
            "a",
            "Topic A",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );
        let b = make_topic(
            "b",
            "Topic B",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m11", "m12", "m13"],
        );

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a, b]);

        assert_eq!(result.merges.len(), 1);
        assert!(result.links.is_empty());
    }

    #[test]
    fn test_analyze_no_merge_below_threshold() {
        // Two topics with 50% overlap → no merges (threshold 0.6)
        // A: m1..m10, B: m1..m5 + m11..m15  → 5 shared out of min(10,10)=10 → 0.5
        let a = make_topic(
            "a",
            "Topic A",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );
        let b = make_topic(
            "b",
            "Topic B",
            vec![
                "m1", "m2", "m3", "m4", "m5", "m11", "m12", "m13", "m14", "m15",
            ],
        );

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a, b]);

        assert!(result.merges.is_empty());
        // 0.5 >= 0.3 so it should be a link
        assert_eq!(result.links.len(), 1);
    }

    #[test]
    fn test_analyze_detects_split() {
        // Topic with 20 source memories (threshold 15) → splits list has 1 entry
        let mems: Vec<&str> = (1..=20).map(|i| {
            // Leak strings for test convenience
            let s: &str = Box::leak(format!("m{i}").into_boxed_str());
            s
        }).collect();
        let a = make_topic("a", "Big Topic", mems);

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a]);

        assert_eq!(result.splits.len(), 1);
        match &result.splits[0] {
            LifecycleOp::Split { source, new_topics } => {
                assert_eq!(source.0, "a");
                assert_eq!(new_topics.len(), 2); // ceil(20/15) = 2
            }
            _ => panic!("Expected Split op"),
        }
    }

    #[test]
    fn test_analyze_no_split_below_threshold() {
        // Topic with 10 source memories → no splits
        let a = make_topic(
            "a",
            "Small Topic",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a]);

        assert!(result.splits.is_empty());
    }

    #[test]
    fn test_analyze_detects_cross_link() {
        // Two topics with 40% overlap → links list has 1 entry
        // A: m1..m10, B: m1..m4 + m11..m16 → 4 shared out of min(10,10)=10 → 0.4
        let a = make_topic(
            "a",
            "Topic A",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );
        let b = make_topic(
            "b",
            "Topic B",
            vec![
                "m1", "m2", "m3", "m4", "m11", "m12", "m13", "m14", "m15", "m16",
            ],
        );

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a, b]);

        assert!(result.merges.is_empty());
        assert_eq!(result.links.len(), 1);
        assert!((result.links[0].strength - 0.4).abs() < f64::EPSILON);
        assert_eq!(result.links[0].shared_memory_ids.len(), 4);
    }

    #[test]
    fn test_analyze_no_link_below_threshold() {
        // Two topics with 20% overlap → no links (threshold 0.3)
        // A: m1..m10, B: m1..m2 + m11..m18 → 2 shared out of min(10,10)=10 → 0.2
        let a = make_topic(
            "a",
            "Topic A",
            vec!["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10"],
        );
        let b = make_topic(
            "b",
            "Topic B",
            vec![
                "m1", "m2", "m11", "m12", "m13", "m14", "m15", "m16", "m17", "m18",
            ],
        );

        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let result = lifecycle.analyze(&[a, b]);

        assert!(result.merges.is_empty());
        assert!(result.links.is_empty());
    }

    #[test]
    fn test_execute_merge() {
        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());

        let (page_a, mems_a) = make_topic("a", "Topic A", vec!["m1", "m2", "m3"]);
        let (mut page_b, mems_b) = make_topic("b", "Topic B", vec!["m2", "m3", "m4"]);
        page_b.metadata.tags = vec!["rust".to_string()];

        // Set tags on page_a as well
        let mut page_a = page_a;
        page_a.metadata.tags = vec!["programming".to_string()];

        let result = lifecycle
            .execute_merge(&[page_a, page_b], &[mems_a, mems_b], "Merged Topic")
            .unwrap();

        // Unified memories = union → m1, m2, m3, m4
        assert_eq!(result.unified_memory_ids.len(), 4);
        assert!(result.unified_memory_ids.contains(&"m1".to_string()));
        assert!(result.unified_memory_ids.contains(&"m4".to_string()));

        // Archived IDs match
        assert_eq!(result.archived_topic_ids.len(), 2);
        assert_eq!(result.archived_topic_ids[0].0, "a");
        assert_eq!(result.archived_topic_ids[1].0, "b");

        // Tags merged (sorted)
        let tags = &result.merged_page.metadata.tags;
        assert!(tags.contains(&"programming".to_string()));
        assert!(tags.contains(&"rust".to_string()));

        // Title
        assert_eq!(result.merged_page.title, "Merged Topic");
        assert_eq!(result.merged_page.status, TopicStatus::Active);
    }

    #[test]
    fn test_execute_merge_requires_two() {
        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let (page, mems) = make_topic("a", "Topic A", vec!["m1"]);

        let result = lifecycle.execute_merge(&[page], &[mems], "Solo");
        assert!(result.is_err());
        match result.unwrap_err() {
            KcError::InvalidInput(msg) => assert!(msg.contains("at least 2")),
            other => panic!("Expected InvalidInput, got: {:?}", other),
        }
    }

    #[test]
    fn test_execute_split() {
        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());

        let mems: Vec<String> = (1..=20).map(|i| format!("m{i}")).collect();
        let (page, _) = make_topic("a", "Big Topic", vec![]);
        let sub_titles = vec![
            "Big Topic (Part 1)".to_string(),
            "Big Topic (Part 2)".to_string(),
        ];

        let result = lifecycle.execute_split(&page, &mems, &sub_titles).unwrap();

        assert_eq!(result.new_pages.len(), 2);
        assert_eq!(result.original_topic_id.0, "a");

        // Memories are distributed: 10 each
        let total: usize = result
            .new_pages
            .iter()
            .map(|p| p.metadata.source_memory_ids.len())
            .sum();
        assert_eq!(total, 20);

        // Each page has content
        assert_eq!(result.new_pages[0].title, "Big Topic (Part 1)");
        assert_eq!(result.new_pages[1].title, "Big Topic (Part 2)");
    }

    #[test]
    fn test_execute_split_empty_memories() {
        let lifecycle = TopicLifecycle::new(LifecycleConfig::default());
        let (page, _) = make_topic("a", "Empty", vec![]);

        let result = lifecycle.execute_split(&page, &[], &["Part 1".to_string()]);
        assert!(result.is_err());
        match result.unwrap_err() {
            KcError::InvalidInput(msg) => assert!(msg.contains("No source memories")),
            other => panic!("Expected InvalidInput, got: {:?}", other),
        }
    }

    #[test]
    fn test_memory_overlap_empty() {
        assert!((TopicLifecycle::memory_overlap(&[], &[]) - 0.0).abs() < f64::EPSILON);
        assert!(
            (TopicLifecycle::memory_overlap(&["a".to_string()], &[]) - 0.0).abs() < f64::EPSILON
        );
        assert!(
            (TopicLifecycle::memory_overlap(&[], &["a".to_string()]) - 0.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_memory_overlap_identical() {
        let set = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!((TopicLifecycle::memory_overlap(&set, &set) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_memory_overlap_partial() {
        // 3 shared out of min(5, 4) = 4 → 3/4 = 0.75
        let a: Vec<String> = vec!["1", "2", "3", "4", "5"]
            .into_iter()
            .map(String::from)
            .collect();
        let b: Vec<String> = vec!["1", "2", "3", "6"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!((TopicLifecycle::memory_overlap(&a, &b) - 0.75).abs() < f64::EPSILON);
    }
}
