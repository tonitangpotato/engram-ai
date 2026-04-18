//! Health reporting and link integrity auditing for compiled topic pages.
//!
//! Combines decay evaluation, conflict detection, and link integrity
//! into unified health reports with actionable recommendations.

use chrono::Utc;

use super::conflict::ConflictDetector;
use super::decay::DecayEngine;
use super::storage::KnowledgeStore;
use super::types::*;

// ═══════════════════════════════════════════════════════════════════════════════
//  TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of auditing a single source link.
#[derive(Debug, Clone)]
pub struct LinkAuditEntry {
    pub memory_id: String,
    pub topic_id: TopicId,
    pub status: LinkStatus,
    pub details: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  HEALTH AUDITOR
// ═══════════════════════════════════════════════════════════════════════════════

/// Validates source-link integrity and produces health reports.
///
/// Combines decay evaluation, conflict detection, and link integrity
/// checking into a unified health picture for individual topics or the
/// entire knowledge base.
pub struct HealthAuditor;

impl HealthAuditor {
    /// Audit source link integrity for a specific topic.
    ///
    /// Checks each `source_memory_id` in the topic's metadata against
    /// the source refs persisted in the store. Returns per-link status:
    ///
    /// - [`LinkStatus::Valid`] — ref exists with relevance ≥ 0.1
    /// - [`LinkStatus::Stale`] — ref exists but relevance < 0.1
    /// - [`LinkStatus::Broken`] — ref not found in the store
    pub fn audit_links(
        &self,
        topic: &TopicPage,
        store: &dyn KnowledgeStore,
    ) -> Result<Vec<LinkAuditEntry>, KcError> {
        let source_refs = store.get_source_refs(&topic.id)?;

        let entries = topic
            .metadata
            .source_memory_ids
            .iter()
            .map(|memory_id| {
                let maybe_ref = source_refs.iter().find(|r| r.memory_id == *memory_id);

                match maybe_ref {
                    None => LinkAuditEntry {
                        memory_id: memory_id.clone(),
                        topic_id: topic.id.clone(),
                        status: LinkStatus::Broken,
                        details: format!(
                            "Source memory '{}' not found in store refs",
                            memory_id
                        ),
                    },
                    Some(r) if r.relevance_score < 0.1 => LinkAuditEntry {
                        memory_id: memory_id.clone(),
                        topic_id: topic.id.clone(),
                        status: LinkStatus::Stale,
                        details: format!(
                            "Source memory '{}' has low relevance ({:.3})",
                            memory_id, r.relevance_score
                        ),
                    },
                    Some(r) => LinkAuditEntry {
                        memory_id: memory_id.clone(),
                        topic_id: topic.id.clone(),
                        status: LinkStatus::Valid,
                        details: format!(
                            "Source memory '{}' is valid (relevance {:.3})",
                            memory_id, r.relevance_score
                        ),
                    },
                }
            })
            .collect();

        Ok(entries)
    }

    /// Compute a health score for a single topic.
    ///
    /// Combines freshness (from [`DecayEngine`]), link integrity, and
    /// conflict count into a multi-dimensional [`TopicHealthScore`].
    pub fn topic_score(
        &self,
        topic: &TopicPage,
        store: &dyn KnowledgeStore,
        decay_engine: &DecayEngine,
        conflicts: &[ConflictRecord],
    ) -> Result<TopicHealthScore, KcError> {
        // 1. Freshness from decay engine
        let decay_result = decay_engine.evaluate_topic(topic, store)?;
        let freshness = decay_result.freshness_score;

        // 2. Link integrity
        let audit_entries = self.audit_links(topic, store)?;
        let total_links = audit_entries.len();
        let valid_links = audit_entries
            .iter()
            .filter(|e| e.status == LinkStatus::Valid)
            .count();
        let link_health = if total_links > 0 {
            valid_links as f64 / total_links as f64
        } else {
            1.0 // No links → nothing broken
        };

        // 3. Coherence from conflict count
        let conflict_count = conflicts.len();
        let coherence = 1.0 - (conflict_count as f64 * 0.2).min(1.0);

        // 4. Access frequency: compilation_count / max(1, days_since_creation)
        let now = Utc::now();
        let days_since_creation =
            (now - topic.metadata.created_at).num_seconds() as f64 / 86400.0;
        let days_since_creation = days_since_creation.max(1.0);
        let access_frequency =
            (topic.metadata.compilation_count as f64 / days_since_creation).min(1.0);

        // 5. Overall weighted score
        let overall = 0.4 * freshness
            + 0.3 * link_health
            + 0.2 * coherence
            + 0.1 * access_frequency;

        Ok(TopicHealthScore {
            topic_id: topic.id.clone(),
            freshness,
            coherence,
            link_health,
            access_frequency,
            overall,
        })
    }

    /// Generate a full health report for the knowledge base.
    ///
    /// Aggregates per-topic scores, finds broken links, and lists
    /// actionable [`MaintenanceRecommendation`]s.
    pub fn health_report(
        &self,
        store: &dyn KnowledgeStore,
        decay_engine: &DecayEngine,
        conflict_detector: &ConflictDetector,
    ) -> Result<HealthReport, KcError> {
        let all_topics = store.list_topic_pages()?;
        let total_topics = all_topics.len();

        // Detect conflicts across all topics (pairwise, no LLM)
        let all_conflicts = if all_topics.len() >= 2 {
            // Use WithinTopic scope for each topic and collect unique conflicts,
            // or iterate all pairs. For simplicity, detect for each topic vs rest.
            let mut collected = Vec::new();
            for i in 0..all_topics.len() {
                let scope = ConflictScope::WithinTopic(all_topics[i].id.clone());
                let mut conflicts = conflict_detector
                    .detect_conflicts(&all_topics, &scope, None)?;
                collected.append(&mut conflicts);
            }
            // Deduplicate by conflict id
            collected.sort_by(|a, b| a.conflict.id.0.cmp(&b.conflict.id.0));
            collected.dedup_by(|a, b| a.conflict.id.0 == b.conflict.id.0);
            collected
        } else {
            Vec::new()
        };

        let mut stale_topics = Vec::new();
        let mut broken_links = Vec::new();
        let mut recommendations = Vec::new();

        for topic in &all_topics {
            // Conflicts involving this topic
            let topic_conflicts: Vec<&ConflictRecord> = all_conflicts
                .iter()
                .filter(|c| match &c.conflict.scope {
                    ConflictScope::WithinTopic(id) => *id == topic.id,
                    ConflictScope::BetweenTopics(a, b) => *a == topic.id || *b == topic.id,
                })
                .collect();

            let topic_conflict_records: Vec<ConflictRecord> =
                topic_conflicts.iter().map(|c| (*c).clone()).collect();

            // Compute health score
            let score = self.topic_score(
                topic,
                store,
                decay_engine,
                &topic_conflict_records,
            )?;

            // Collect stale topics
            if score.freshness < 0.3 {
                stale_topics.push(topic.id.clone());
            }

            // Audit links and collect broken ones
            let audit_entries = self.audit_links(topic, store)?;
            let total_links = audit_entries.len();
            let broken_count = audit_entries
                .iter()
                .filter(|e| e.status == LinkStatus::Broken)
                .count();

            for entry in &audit_entries {
                if entry.status == LinkStatus::Broken || entry.status == LinkStatus::Stale {
                    broken_links.push(BrokenLink {
                        source_topic: topic.id.clone(),
                        target_topic: TopicId(entry.memory_id.clone()),
                        link_type: LinkType::DerivedFrom,
                        status: entry.status.clone(),
                        detected_at: Utc::now(),
                    });
                }
            }

            // Generate recommendations
            if score.freshness < 0.1 {
                recommendations.push(MaintenanceRecommendation {
                    topic_id: topic.id.clone(),
                    action: "Consider archiving".to_string(),
                    priority: 1,
                    reason: format!(
                        "Freshness score is very low ({:.3})",
                        score.freshness
                    ),
                });
            }

            if total_links > 0 && broken_count as f64 / total_links as f64 > 0.5 {
                recommendations.push(MaintenanceRecommendation {
                    topic_id: topic.id.clone(),
                    action: "Recompile from remaining valid sources".to_string(),
                    priority: 2,
                    reason: format!(
                        "More than 50% of links are broken ({}/{})",
                        broken_count, total_links
                    ),
                });
            }

            if topic_conflicts.len() > 2 {
                recommendations.push(MaintenanceRecommendation {
                    topic_id: topic.id.clone(),
                    action: "Review and resolve conflicts".to_string(),
                    priority: 3,
                    reason: format!(
                        "Topic has {} unresolved conflicts",
                        topic_conflicts.len()
                    ),
                });
            }
        }

        Ok(HealthReport {
            generated_at: Utc::now(),
            total_topics,
            stale_topics,
            conflicts: all_conflicts,
            broken_links,
            recommendations,
        })
    }

    /// Suggest a repair action for a broken or stale link.
    ///
    /// - Broken link with other valid sources → [`LinkRepairAction::Remove`]
    /// - Broken link as the last source → [`LinkRepairAction::MarkStale`]
    /// - Stale link → [`LinkRepairAction::MarkStale`]
    /// - Valid link → [`LinkRepairAction::MarkStale`] (should not happen)
    pub fn suggest_repair(
        &self,
        entry: &LinkAuditEntry,
        topic: &TopicPage,
    ) -> LinkRepairAction {
        match entry.status {
            LinkStatus::Broken => {
                // Count how many other source memory ids the topic has
                // (excluding the broken one)
                let valid_count = topic
                    .metadata
                    .source_memory_ids
                    .iter()
                    .filter(|id| *id != &entry.memory_id)
                    .count();

                if valid_count > 1 {
                    LinkRepairAction::Remove
                } else {
                    // Can't remove the last source
                    LinkRepairAction::MarkStale
                }
            }
            LinkStatus::Stale => LinkRepairAction::MarkStale,
            LinkStatus::Valid => LinkRepairAction::MarkStale, // shouldn't happen
        }
    }

    /// Execute a link repair action on a topic page.
    ///
    /// Applies the given [`LinkRepairAction`] to the topic, persists
    /// the changes via the store, and returns a [`RepairResult`]
    /// describing what was done.
    pub fn repair_link(
        &self,
        topic: &mut TopicPage,
        entry: &LinkAuditEntry,
        action: &LinkRepairAction,
        store: &dyn KnowledgeStore,
    ) -> Result<RepairResult, KcError> {
        match action {
            LinkRepairAction::Remove => {
                // Remove the memory_id from source_memory_ids
                topic
                    .metadata
                    .source_memory_ids
                    .retain(|id| id != &entry.memory_id);

                // Update source refs in store (save remaining refs without the removed one)
                let mut refs = store.get_source_refs(&topic.id)?;
                refs.retain(|r| r.memory_id != entry.memory_id);
                store.save_source_refs(&topic.id, &refs)?;

                // Update topic page in store
                store.update_topic_page(topic)?;

                Ok(RepairResult {
                    memory_id: entry.memory_id.clone(),
                    topic_id: topic.id.clone(),
                    action_taken: LinkRepairAction::Remove,
                    success: true,
                    details: format!(
                        "Removed broken source '{}' from topic '{}'",
                        entry.memory_id, topic.id
                    ),
                })
            }
            LinkRepairAction::MarkStale => {
                // Update topic status to Stale
                topic.status = TopicStatus::Stale;

                // Persist the change
                store.update_topic_page(topic)?;

                Ok(RepairResult {
                    memory_id: entry.memory_id.clone(),
                    topic_id: topic.id.clone(),
                    action_taken: LinkRepairAction::MarkStale,
                    success: true,
                    details: format!(
                        "Marked topic '{}' as stale due to link '{}'",
                        topic.id, entry.memory_id
                    ),
                })
            }
            LinkRepairAction::UpdateTarget(new_id) => {
                // Replace old memory_id with new_id in source_memory_ids
                for id in &mut topic.metadata.source_memory_ids {
                    if *id == entry.memory_id {
                        *id = new_id.0.clone();
                    }
                }

                // Update source refs: replace old ref with new one
                let mut refs = store.get_source_refs(&topic.id)?;
                for r in &mut refs {
                    if r.memory_id == entry.memory_id {
                        r.memory_id = new_id.0.clone();
                    }
                }
                store.save_source_refs(&topic.id, &refs)?;

                // Update topic page in store
                store.update_topic_page(topic)?;

                Ok(RepairResult {
                    memory_id: entry.memory_id.clone(),
                    topic_id: topic.id.clone(),
                    action_taken: LinkRepairAction::UpdateTarget(new_id.clone()),
                    success: true,
                    details: format!(
                        "Updated source '{}' → '{}' in topic '{}'",
                        entry.memory_id, new_id, topic.id
                    ),
                })
            }
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

    fn make_test_page(id: &str, source_ids: &[&str]) -> TopicPage {
        let now = Utc::now();
        TopicPage {
            id: TopicId(id.to_owned()),
            title: format!("Topic {id}"),
            content: format!("Content about {id}"),
            sections: Vec::new(),
            summary: format!("Summary of {id}"),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: source_ids.iter().map(|s| s.to_string()).collect(),
                tags: vec![],
                quality_score: Some(0.5),
            },
        }
    }

    // ── test_audit_links_all_valid ────────────────────────────────────────

    #[test]
    fn test_audit_links_all_valid() {
        let store = make_store();
        let page = make_test_page("t1", &["m1", "m2", "m3"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.9,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.8,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m3".into(),
                relevance_score: 0.7,
                added_at: now,
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let auditor = HealthAuditor;
        let entries = auditor.audit_links(&page, &store).unwrap();

        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.status, LinkStatus::Valid, "Expected Valid for {}", entry.memory_id);
        }
    }

    // ── test_audit_links_broken ──────────────────────────────────────────

    #[test]
    fn test_audit_links_broken() {
        let store = make_store();
        // Page references m1, m2, m3 but only m1 is in the store
        let page = make_test_page("t2", &["m1", "m2", "m3"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![SourceMemoryRef {
            memory_id: "m1".into(),
            relevance_score: 0.9,
            added_at: now,
        }];
        store.save_source_refs(&page.id, &refs).unwrap();

        let auditor = HealthAuditor;
        let entries = auditor.audit_links(&page, &store).unwrap();

        assert_eq!(entries.len(), 3);

        let valid_count = entries.iter().filter(|e| e.status == LinkStatus::Valid).count();
        let broken_count = entries.iter().filter(|e| e.status == LinkStatus::Broken).count();

        assert_eq!(valid_count, 1, "Expected 1 valid link");
        assert_eq!(broken_count, 2, "Expected 2 broken links");
    }

    // ── test_audit_links_stale ───────────────────────────────────────────

    #[test]
    fn test_audit_links_stale() {
        let store = make_store();
        let page = make_test_page("t3", &["m1", "m2"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.8,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.05, // Below 0.1 threshold → Stale
                added_at: now,
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let auditor = HealthAuditor;
        let entries = auditor.audit_links(&page, &store).unwrap();

        assert_eq!(entries.len(), 2);

        let m1 = entries.iter().find(|e| e.memory_id == "m1").unwrap();
        assert_eq!(m1.status, LinkStatus::Valid);

        let m2 = entries.iter().find(|e| e.memory_id == "m2").unwrap();
        assert_eq!(m2.status, LinkStatus::Stale);
    }

    // ── test_topic_score_healthy ─────────────────────────────────────────

    #[test]
    fn test_topic_score_healthy() {
        let store = make_store();
        let page = make_test_page("healthy-1", &["m1", "m2"]);
        store.create_topic_page(&page).unwrap();

        // Recent, high-relevance sources
        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.95,
                added_at: now - Duration::minutes(10),
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.90,
                added_at: now - Duration::minutes(20),
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(default_decay_config());
        let conflicts: Vec<ConflictRecord> = vec![];

        let score = auditor
            .topic_score(&page, &store, &decay_engine, &conflicts)
            .unwrap();

        assert_eq!(score.topic_id, page.id);
        assert!(score.freshness > 0.8, "Expected high freshness, got {}", score.freshness);
        assert!(
            (score.link_health - 1.0).abs() < 1e-9,
            "Expected perfect link health, got {}",
            score.link_health
        );
        assert!(
            (score.coherence - 1.0).abs() < 1e-9,
            "Expected perfect coherence (no conflicts), got {}",
            score.coherence
        );
        assert!(score.overall > 0.7, "Expected high overall score, got {}", score.overall);
    }

    // ── test_topic_score_degraded ────────────────────────────────────────

    #[test]
    fn test_topic_score_degraded() {
        let store = make_store();
        // Page references m1, m2, m3 but only m1 is stored (and very old)
        let page = make_test_page("degraded-1", &["m1", "m2", "m3"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![SourceMemoryRef {
            memory_id: "m1".into(),
            relevance_score: 0.1,
            added_at: now - Duration::days(500),
        }];
        store.save_source_refs(&page.id, &refs).unwrap();

        // Create some fake conflicts
        let conflicts = vec![
            ConflictRecord {
                conflict: Conflict {
                    id: ConflictId("c1".into()),
                    conflict_type: ConflictType::Contradiction,
                    scope: ConflictScope::WithinTopic(TopicId("degraded-1".into())),
                    description: "test conflict 1".into(),
                    status: ConflictStatus::Detected,
                    detected_at: Utc::now(),
                    resolved_at: None,
                },
                severity: ConflictSeverity::High,
                evidence: vec![],
            },
            ConflictRecord {
                conflict: Conflict {
                    id: ConflictId("c2".into()),
                    conflict_type: ConflictType::Outdated,
                    scope: ConflictScope::WithinTopic(TopicId("degraded-1".into())),
                    description: "test conflict 2".into(),
                    status: ConflictStatus::Detected,
                    detected_at: Utc::now(),
                    resolved_at: None,
                },
                severity: ConflictSeverity::Medium,
                evidence: vec![],
            },
        ];

        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(default_decay_config());

        let score = auditor
            .topic_score(&page, &store, &decay_engine, &conflicts)
            .unwrap();

        // Old source → low freshness
        assert!(score.freshness < 0.3, "Expected low freshness, got {}", score.freshness);
        // 1 valid out of 3 → link_health ≈ 0.333
        assert!(
            score.link_health < 0.5,
            "Expected low link health, got {}",
            score.link_health
        );
        // 2 conflicts → coherence = 1.0 - 0.4 = 0.6
        assert!(
            (score.coherence - 0.6).abs() < 1e-9,
            "Expected coherence 0.6, got {}",
            score.coherence
        );
        assert!(score.overall < 0.5, "Expected low overall score, got {}", score.overall);
    }

    // ── test_health_report_empty_store ────────────────────────────────────

    #[test]
    fn test_health_report_empty_store() {
        let store = make_store();

        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(default_decay_config());
        let conflict_detector = ConflictDetector::new();

        let report = auditor
            .health_report(&store, &decay_engine, &conflict_detector)
            .unwrap();

        assert_eq!(report.total_topics, 0);
        assert!(report.stale_topics.is_empty());
        assert!(report.conflicts.is_empty());
        assert!(report.broken_links.is_empty());
        assert!(report.recommendations.is_empty());
    }

    // ── test_health_report_mixed ─────────────────────────────────────────

    #[test]
    fn test_health_report_mixed() {
        let store = make_store();

        // Healthy topic with fresh sources
        let healthy = make_test_page("healthy", &["m1", "m2"]);
        store.create_topic_page(&healthy).unwrap();
        let now = Utc::now();
        store
            .save_source_refs(
                &healthy.id,
                &[
                    SourceMemoryRef {
                        memory_id: "m1".into(),
                        relevance_score: 0.9,
                        added_at: now - Duration::minutes(5),
                    },
                    SourceMemoryRef {
                        memory_id: "m2".into(),
                        relevance_score: 0.85,
                        added_at: now - Duration::minutes(10),
                    },
                ],
            )
            .unwrap();

        // Unhealthy topic: old source, broken links
        let unhealthy = make_test_page("unhealthy", &["m10", "m11", "m12"]);
        store.create_topic_page(&unhealthy).unwrap();
        // Only save m10, so m11 and m12 are broken
        store
            .save_source_refs(
                &unhealthy.id,
                &[SourceMemoryRef {
                    memory_id: "m10".into(),
                    relevance_score: 0.05, // Stale
                    added_at: now - Duration::days(500),
                }],
            )
            .unwrap();

        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(default_decay_config());
        let conflict_detector = ConflictDetector::new();

        let report = auditor
            .health_report(&store, &decay_engine, &conflict_detector)
            .unwrap();

        assert_eq!(report.total_topics, 2);

        // Unhealthy topic should appear as stale
        assert!(
            !report.stale_topics.is_empty(),
            "Expected at least one stale topic"
        );
        assert!(
            report.stale_topics.iter().any(|id| id.0 == "unhealthy"),
            "Expected 'unhealthy' in stale topics"
        );

        // Broken links from the unhealthy topic
        assert!(
            !report.broken_links.is_empty(),
            "Expected broken links from unhealthy topic"
        );
    }

    // ── test_suggest_repair_broken ───────────────────────────────────────

    #[test]
    fn test_suggest_repair_broken() {
        let auditor = HealthAuditor;

        // Topic with 3 source memories → broken one can be removed
        let topic = make_test_page("repair-1", &["m1", "m2", "m3"]);
        let broken_entry = LinkAuditEntry {
            memory_id: "m2".into(),
            topic_id: topic.id.clone(),
            status: LinkStatus::Broken,
            details: "not found".into(),
        };

        let action = auditor.suggest_repair(&broken_entry, &topic);
        assert!(
            matches!(action, LinkRepairAction::Remove),
            "Expected Remove when other valid sources exist, got {:?}",
            action
        );

        // Topic with only 2 source memories, broken one can't be removed
        // (only 1 remaining, which is ≤ 1)
        let topic_small = make_test_page("repair-2", &["m1", "m2"]);
        let broken_small = LinkAuditEntry {
            memory_id: "m1".into(),
            topic_id: topic_small.id.clone(),
            status: LinkStatus::Broken,
            details: "not found".into(),
        };

        let action_small = auditor.suggest_repair(&broken_small, &topic_small);
        assert!(
            matches!(action_small, LinkRepairAction::MarkStale),
            "Expected MarkStale when only 1 remaining source, got {:?}",
            action_small
        );
    }

    // ── test_suggest_repair_stale ────────────────────────────────────────

    #[test]
    fn test_suggest_repair_stale() {
        let auditor = HealthAuditor;

        let topic = make_test_page("repair-3", &["m1"]);
        let stale_entry = LinkAuditEntry {
            memory_id: "m1".into(),
            topic_id: topic.id.clone(),
            status: LinkStatus::Stale,
            details: "low relevance".into(),
        };

        let action = auditor.suggest_repair(&stale_entry, &topic);
        assert!(
            matches!(action, LinkRepairAction::MarkStale),
            "Expected MarkStale for stale link, got {:?}",
            action
        );
    }

    // ── test_recommendations_generated ───────────────────────────────────

    #[test]
    fn test_recommendations_generated() {
        let store = make_store();

        // Create a topic with very old source (freshness < 0.1) and all broken links
        let page = make_test_page("doomed", &["m1", "m2", "m3", "m4"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        // Only save m1 with an extremely old timestamp and low relevance
        store
            .save_source_refs(
                &page.id,
                &[SourceMemoryRef {
                    memory_id: "m1".into(),
                    relevance_score: 0.01,
                    added_at: now - Duration::days(2000),
                }],
            )
            .unwrap();

        let auditor = HealthAuditor;
        let decay_engine = DecayEngine::new(default_decay_config());
        let conflict_detector = ConflictDetector::new();

        let report = auditor
            .health_report(&store, &decay_engine, &conflict_detector)
            .unwrap();

        // Should have recommendations:
        // - "Consider archiving" (freshness < 0.1)
        // - "Recompile from remaining valid sources" (>50% broken links: 3/4 = 75%)
        assert!(
            !report.recommendations.is_empty(),
            "Expected at least one recommendation"
        );

        let archive_rec = report
            .recommendations
            .iter()
            .find(|r| r.action.contains("archiving"));
        assert!(
            archive_rec.is_some(),
            "Expected 'Consider archiving' recommendation, got: {:?}",
            report.recommendations
        );

        let recompile_rec = report
            .recommendations
            .iter()
            .find(|r| r.action.contains("Recompile"));
        assert!(
            recompile_rec.is_some(),
            "Expected 'Recompile' recommendation, got: {:?}",
            report.recommendations
        );
    }

    // ── test_repair_link_remove ──────────────────────────────────────────

    #[test]
    fn test_repair_link_remove() {
        let store = make_store();
        let mut page = make_test_page("repair-rm", &["m1", "m2", "m3"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.9,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.8,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m3".into(),
                relevance_score: 0.7,
                added_at: now,
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let entry = LinkAuditEntry {
            memory_id: "m2".into(),
            topic_id: page.id.clone(),
            status: LinkStatus::Broken,
            details: "not found".into(),
        };

        let auditor = HealthAuditor;
        let result = auditor
            .repair_link(&mut page, &entry, &LinkRepairAction::Remove, &store)
            .unwrap();

        assert!(result.success);
        assert_eq!(result.memory_id, "m2");
        assert!(matches!(result.action_taken, LinkRepairAction::Remove));

        // Verify source_memory_ids no longer contains m2
        assert!(!page.metadata.source_memory_ids.contains(&"m2".to_string()));
        assert_eq!(page.metadata.source_memory_ids.len(), 2);

        // Verify store source refs no longer contain m2
        let stored_refs = store.get_source_refs(&page.id).unwrap();
        assert_eq!(stored_refs.len(), 2);
        assert!(!stored_refs.iter().any(|r| r.memory_id == "m2"));

        // Verify the topic page in store is updated
        let stored_page = store.get_topic_page(&page.id).unwrap().unwrap();
        assert!(!stored_page.metadata.source_memory_ids.contains(&"m2".to_string()));
    }

    // ── test_repair_link_mark_stale ──────────────────────────────────────

    #[test]
    fn test_repair_link_mark_stale() {
        let store = make_store();
        let mut page = make_test_page("repair-stale", &["m1", "m2"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.9,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.05,
                added_at: now,
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let entry = LinkAuditEntry {
            memory_id: "m2".into(),
            topic_id: page.id.clone(),
            status: LinkStatus::Stale,
            details: "low relevance".into(),
        };

        let auditor = HealthAuditor;
        let result = auditor
            .repair_link(&mut page, &entry, &LinkRepairAction::MarkStale, &store)
            .unwrap();

        assert!(result.success);
        assert!(matches!(result.action_taken, LinkRepairAction::MarkStale));

        // Verify topic status changed to Stale
        assert_eq!(page.status, TopicStatus::Stale);

        // Verify store reflects the status change
        let stored_page = store.get_topic_page(&page.id).unwrap().unwrap();
        assert_eq!(stored_page.status, TopicStatus::Stale);
    }

    // ── test_repair_link_update_target ───────────────────────────────────

    #[test]
    fn test_repair_link_update_target() {
        let store = make_store();
        let mut page = make_test_page("repair-update", &["m1", "m2", "m3"]);
        store.create_topic_page(&page).unwrap();

        let now = Utc::now();
        let refs = vec![
            SourceMemoryRef {
                memory_id: "m1".into(),
                relevance_score: 0.9,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m2".into(),
                relevance_score: 0.8,
                added_at: now,
            },
            SourceMemoryRef {
                memory_id: "m3".into(),
                relevance_score: 0.7,
                added_at: now,
            },
        ];
        store.save_source_refs(&page.id, &refs).unwrap();

        let entry = LinkAuditEntry {
            memory_id: "m2".into(),
            topic_id: page.id.clone(),
            status: LinkStatus::Broken,
            details: "not found".into(),
        };

        let new_target = TopicId("m2-replacement".into());
        let auditor = HealthAuditor;
        let result = auditor
            .repair_link(
                &mut page,
                &entry,
                &LinkRepairAction::UpdateTarget(new_target.clone()),
                &store,
            )
            .unwrap();

        assert!(result.success);
        assert!(matches!(result.action_taken, LinkRepairAction::UpdateTarget(_)));

        // Verify source_memory_ids has new id, not old
        assert!(!page.metadata.source_memory_ids.contains(&"m2".to_string()));
        assert!(page.metadata.source_memory_ids.contains(&"m2-replacement".to_string()));
        assert_eq!(page.metadata.source_memory_ids.len(), 3);

        // Verify store source refs have the new id
        let stored_refs = store.get_source_refs(&page.id).unwrap();
        assert_eq!(stored_refs.len(), 3);
        assert!(stored_refs.iter().any(|r| r.memory_id == "m2-replacement"));
        assert!(!stored_refs.iter().any(|r| r.memory_id == "m2"));

        // Verify the topic page in store is updated
        let stored_page = store.get_topic_page(&page.id).unwrap().unwrap();
        assert!(stored_page.metadata.source_memory_ids.contains(&"m2-replacement".to_string()));
    }
}
