//! Knowledge decay engine for evaluating and managing topic freshness.
//!
//! Evaluates freshness of compiled topics based on source memories'
//! recency-weighted scores and recommends maintenance actions.

use chrono::{DateTime, Utc};

use super::storage::KnowledgeStore;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of evaluating a single topic's decay.
#[derive(Debug, Clone)]
pub struct DecayResult {
    pub topic_id: TopicId,
    pub freshness_score: f64,
    pub recommended_action: DecayAction,
    pub source_count: usize,
    pub evaluated_at: DateTime<Utc>,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ENGINE
// ═══════════════════════════════════════════════════════════════════════════════

/// Evaluates freshness of compiled topics based on source memories'
/// recency-weighted scores.
pub struct DecayEngine {
    config: DecayConfig,
}

impl DecayEngine {
    pub fn new(config: DecayConfig) -> Self {
        Self { config }
    }

    /// Evaluate freshness of a single topic.
    ///
    /// Freshness = weighted mean of source memory recency scores.
    /// More recently added sources contribute more.
    pub fn evaluate_topic(
        &self,
        page: &TopicPage,
        store: &dyn KnowledgeStore,
    ) -> Result<DecayResult, KcError> {
        let sources = store.get_source_refs(&page.id)?;

        if sources.is_empty() {
            return Ok(DecayResult {
                topic_id: page.id.clone(),
                freshness_score: 0.0,
                recommended_action: DecayAction::Archive(page.id.clone()),
                source_count: 0,
                evaluated_at: Utc::now(),
            });
        }

        // Compute recency-weighted freshness score
        let now = Utc::now();
        let mut weighted_sum = 0.0_f64;
        let mut weight_total = 0.0_f64;

        for src in &sources {
            // Days since source was added
            let age_days = (now - src.added_at).num_seconds() as f64 / 86400.0;
            let age_days = age_days.max(0.0);

            // Recency weight: newer sources weight more.
            // weight = 1 / (1 + age_days)^1.5
            let weight = 1.0 / (1.0 + age_days).powf(1.5);

            // Source's own relevance score contributes to freshness
            let source_score = src.relevance_score;

            weighted_sum += source_score * weight;
            weight_total += weight;
        }

        let freshness = if weight_total > 0.0 {
            (weighted_sum / weight_total).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Determine action based on thresholds
        let action = self.classify_action(freshness, page);

        Ok(DecayResult {
            topic_id: page.id.clone(),
            freshness_score: freshness,
            recommended_action: action,
            source_count: sources.len(),
            evaluated_at: Utc::now(),
        })
    }

    /// Evaluate all active topics.
    pub fn evaluate_all(
        &self,
        store: &dyn KnowledgeStore,
    ) -> Result<Vec<DecayResult>, KcError> {
        let pages = store.get_pages_by_status(TopicStatus::Active)?;
        let mut results = Vec::with_capacity(pages.len());
        for page in &pages {
            results.push(self.evaluate_topic(page, store)?);
        }
        Ok(results)
    }

    /// Apply a decay action to a topic.
    pub fn apply_decay(
        &self,
        action: &DecayAction,
        store: &dyn KnowledgeStore,
    ) -> Result<(), KcError> {
        match action {
            DecayAction::MarkStale(topic_id) => {
                // Update quality_score to reflect staleness
                store.update_activity_score(topic_id, 0.0)?;
                Ok(())
            }
            DecayAction::Archive(topic_id) => {
                store.mark_archived(topic_id, "Decay: freshness below archive threshold")?;
                Ok(())
            }
            DecayAction::Refresh(topic_id) => {
                // Bump the quality score slightly to indicate refresh is desired
                let page = store.get_topic_page(topic_id)?;
                if let Some(page) = page {
                    let current = page.metadata.quality_score.unwrap_or(0.5);
                    let new_score = (current + 0.1).clamp(0.0, 1.0);
                    store.update_activity_score(topic_id, new_score)?;
                }
                Ok(())
            }
        }
    }

    fn classify_action(&self, freshness: f64, page: &TopicPage) -> DecayAction {
        // Map config day thresholds to approximate freshness scores.
        // Higher archive_threshold_days → more lenient (lower score threshold).
        // Default 90 days → ~0.1, default 30 days → ~0.3
        let archive_threshold = 1.0 / (1.0 + self.config.archive_threshold_days as f64).powf(0.5);
        let stale_threshold = 1.0 / (1.0 + self.config.stale_threshold_days as f64).powf(0.5);

        if page.status == TopicStatus::Archived {
            // Already archived — suggest refresh (re-evaluation)
            return DecayAction::Refresh(page.id.clone());
        }

        if freshness < archive_threshold {
            DecayAction::Archive(page.id.clone())
        } else if freshness < stale_threshold {
            DecayAction::MarkStale(page.id.clone())
        } else {
            DecayAction::Refresh(page.id.clone())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::storage::SqliteKnowledgeStore;
    use chrono::Duration;

    fn make_store() -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::in_memory().unwrap();
        store.init_schema().unwrap();
        store
    }

    fn default_decay_config() -> DecayConfig {
        DecayConfig {
            check_interval_hours: 24,
            stale_threshold_days: 30,
            archive_threshold_days: 90,
            min_access_count: 0,
        }
    }

    fn make_test_page(id: &str) -> TopicPage {
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
                compilation_count: 1,
                source_memory_ids: vec![],
                tags: vec![],
                quality_score: Some(0.5),
            },
        }
    }

    #[test]
    fn test_evaluate_empty_topic() {
        let store = make_store();
        let page = make_test_page("empty-1");
        store.create_topic_page(&page).unwrap();
        // No source refs saved — topic has zero sources

        let engine = DecayEngine::new(default_decay_config());
        let result = engine.evaluate_topic(&page, &store).unwrap();

        assert_eq!(result.topic_id.0, "empty-1");
        assert!((result.freshness_score - 0.0).abs() < 1e-9);
        assert_eq!(result.source_count, 0);
        assert!(matches!(result.recommended_action, DecayAction::Archive(_)));
    }

    #[test]
    fn test_evaluate_fresh_topic() {
        let store = make_store();
        let page = make_test_page("fresh-1");
        store.create_topic_page(&page).unwrap();

        // Add recently-added sources with high relevance
        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m-1".into(),
                relevance_score: 0.95,
                added_at: now - Duration::minutes(30),
            },
            SourceMemoryRef {
                memory_id: "m-2".into(),
                relevance_score: 0.90,
                added_at: now - Duration::hours(2),
            },
        ];
        store
            .save_source_refs(&TopicId("fresh-1".into()), &refs)
            .unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let result = engine.evaluate_topic(&page, &store).unwrap();

        assert_eq!(result.source_count, 2);
        // Very recent sources → high freshness
        assert!(
            result.freshness_score > 0.8,
            "Expected high freshness, got {}",
            result.freshness_score
        );
        // High freshness → Refresh (no action needed, just a refresh marker)
        assert!(matches!(result.recommended_action, DecayAction::Refresh(_)));
    }

    #[test]
    fn test_evaluate_stale_topic() {
        let store = make_store();
        let page = make_test_page("stale-1");
        store.create_topic_page(&page).unwrap();

        // Add sources from long ago with low relevance
        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "old-1".into(),
                relevance_score: 0.1,
                added_at: now - Duration::days(1000),
            },
            SourceMemoryRef {
                memory_id: "old-2".into(),
                relevance_score: 0.05,
                added_at: now - Duration::days(800),
            },
        ];
        store
            .save_source_refs(&TopicId("stale-1".into()), &refs)
            .unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let result = engine.evaluate_topic(&page, &store).unwrap();

        assert_eq!(result.source_count, 2);
        // Very old sources → low freshness
        assert!(
            result.freshness_score < 0.3,
            "Expected low freshness, got {}",
            result.freshness_score
        );
        // Low freshness → MarkStale or Archive
        assert!(matches!(
            result.recommended_action,
            DecayAction::MarkStale(_) | DecayAction::Archive(_)
        ));
    }

    #[test]
    fn test_apply_decay_archive() {
        let store = make_store();
        let page = make_test_page("archive-1");
        store.create_topic_page(&page).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let action = DecayAction::Archive(TopicId("archive-1".into()));
        engine.apply_decay(&action, &store).unwrap();

        // Verify the topic is now archived
        let updated = store
            .get_topic_page(&TopicId("archive-1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, TopicStatus::Archived);
    }

    #[test]
    fn test_apply_decay_mark_stale() {
        let store = make_store();
        let page = make_test_page("stale-apply");
        store.create_topic_page(&page).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let action = DecayAction::MarkStale(TopicId("stale-apply".into()));
        engine.apply_decay(&action, &store).unwrap();

        // MarkStale sets quality_score to 0.0
        let updated = store.get_topic_page(&TopicId("stale-apply".into())).unwrap().unwrap();
        assert!((updated.metadata.quality_score.unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_apply_decay_refresh_bumps_score() {
        let store = make_store();
        let page = make_test_page("refresh-1");
        store.create_topic_page(&page).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let action = DecayAction::Refresh(TopicId("refresh-1".into()));
        engine.apply_decay(&action, &store).unwrap();

        let updated = store.get_topic_page(&TopicId("refresh-1".into())).unwrap().unwrap();
        // Default quality_score was 0.5, refresh adds 0.1 → 0.6
        assert!((updated.metadata.quality_score.unwrap() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn test_apply_decay_refresh_clamps_to_one() {
        let store = make_store();
        let mut page = make_test_page("refresh-cap");
        page.metadata.quality_score = Some(0.95);
        store.create_topic_page(&page).unwrap();
        // Set quality to 0.95 so refresh (+0.1) would exceed 1.0
        store.update_activity_score(&TopicId("refresh-cap".into()), 0.95).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let action = DecayAction::Refresh(TopicId("refresh-cap".into()));
        engine.apply_decay(&action, &store).unwrap();

        let updated = store.get_topic_page(&TopicId("refresh-cap".into())).unwrap().unwrap();
        // Clamped to 1.0
        assert!((updated.metadata.quality_score.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_evaluate_all_multiple_topics() {
        let store = make_store();
        for i in 0..3 {
            let page = make_test_page(&format!("eval-all-{i}"));
            store.create_topic_page(&page).unwrap();
        }

        let engine = DecayEngine::new(default_decay_config());
        let results = engine.evaluate_all(&store).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_evaluate_all_skips_archived() {
        let store = make_store();
        store.create_topic_page(&make_test_page("active-1")).unwrap();

        let mut archived = make_test_page("archived-1");
        archived.status = TopicStatus::Archived;
        store.create_topic_page(&archived).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        // evaluate_all only gets Active pages
        let results = engine.evaluate_all(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].topic_id.0, "active-1");
    }

    #[test]
    fn test_classify_already_archived() {
        let store = make_store();
        let mut page = make_test_page("already-archived");
        page.status = TopicStatus::Archived;
        store.create_topic_page(&page).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let result = engine.evaluate_topic(&page, &store).unwrap();
        // Archived topic with no sources → action should still be Refresh (re-evaluation)
        // because classify_action returns Refresh for already-archived
        // Note: it first hits the empty-sources early return with Archive
        // But if it had sources, classify_action would return Refresh
        assert!(matches!(result.recommended_action, DecayAction::Archive(_)));
    }

    #[test]
    fn test_single_very_relevant_source() {
        let store = make_store();
        let page = make_test_page("single-src");
        store.create_topic_page(&page).unwrap();

        let refs = vec![SourceMemoryRef {
            memory_id: "m-perfect".into(),
            relevance_score: 1.0,
            added_at: Utc::now(),
        }];
        store.save_source_refs(&TopicId("single-src".into()), &refs).unwrap();

        let engine = DecayEngine::new(default_decay_config());
        let result = engine.evaluate_topic(&page, &store).unwrap();
        assert_eq!(result.source_count, 1);
        // Single source with perfect relevance, added just now → very high freshness
        assert!(result.freshness_score > 0.9, "got {}", result.freshness_score);
        assert!(matches!(result.recommended_action, DecayAction::Refresh(_)));
    }

    #[test]
    fn test_custom_thresholds() {
        let config = DecayConfig {
            check_interval_hours: 12,
            stale_threshold_days: 7,    // Very aggressive
            archive_threshold_days: 14, // Very aggressive
            min_access_count: 0,
        };
        let engine = DecayEngine::new(config);

        let store = make_store();
        let page = make_test_page("custom-threshold");
        store.create_topic_page(&page).unwrap();

        // Sources from 20 days ago with low relevance — should be stale/archived
        let refs = vec![SourceMemoryRef {
            memory_id: "m-old".into(),
            relevance_score: 0.2,
            added_at: Utc::now() - Duration::days(20),
        }];
        store.save_source_refs(&TopicId("custom-threshold".into()), &refs).unwrap();

        let result = engine.evaluate_topic(&page, &store).unwrap();
        // Low relevance source → freshness = 0.2, stale_threshold ≈ 0.354 → MarkStale or Archive
        assert!(matches!(
            result.recommended_action,
            DecayAction::MarkStale(_) | DecayAction::Archive(_)
        ));
    }
}
