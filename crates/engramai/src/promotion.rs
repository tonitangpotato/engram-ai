//! Knowledge Promotion: detect high-frequency memory patterns and suggest
//! promoting them to persistent documents (SOUL.md, MEMORY.md, etc.)
//!
//! ISS-008

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::config::PromotionConfig;
use crate::storage::Storage;

/// A cluster of memories that are candidates for promotion to persistent docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionCandidate {
    /// Unique candidate ID (hash of member IDs)
    pub id: String,
    /// Memory IDs in this cluster
    pub member_ids: Vec<String>,
    /// Summarized content snippets (first 100 chars of each)
    pub snippets: Vec<String>,
    /// Average core_strength across members
    pub avg_core_strength: f64,
    /// Average importance across members
    pub avg_importance: f64,
    /// Time span in days (earliest to latest)
    pub time_span_days: f64,
    /// Number of Hebbian links within the cluster
    pub internal_link_count: usize,
    /// Suggested target document (e.g., "SOUL.md", "MEMORY.md")
    pub suggested_target: String,
    /// LLM-generated summary (None until summarized)
    pub summary: Option<String>,
    /// Created timestamp
    pub created_at: DateTime<Utc>,
    /// Status: pending, approved, dismissed
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "pending".to_string()
}

/// Determine the suggested target document based on memory content.
fn suggest_target(snippets: &[String]) -> String {
    let combined = snippets.join(" ").to_lowercase();

    let soul_keywords = ["principle", "always", "never", "rule", "belief", "value", "identity"];
    let procedural_keywords = ["how to", "step", "procedure", "workflow", "process", "method"];

    let soul_score: usize = soul_keywords.iter().filter(|k| combined.contains(*k)).count();
    let proc_score: usize = procedural_keywords.iter().filter(|k| combined.contains(*k)).count();

    if soul_score >= 2 {
        "SOUL.md".to_string()
    } else if proc_score >= 2 {
        "AGENTS.md".to_string()
    } else {
        "MEMORY.md".to_string()
    }
}

/// Generate a deterministic candidate ID from sorted member IDs.
fn candidate_id(member_ids: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut sorted = member_ids.to_vec();
    sorted.sort();
    let mut hasher = DefaultHasher::new();
    sorted.hash(&mut hasher);
    format!("promo-{:016x}", hasher.finish())
}

/// Find connected components via BFS in an adjacency list.
fn connected_components(adj: &HashMap<String, HashSet<String>>, nodes: &HashSet<String>) -> Vec<Vec<String>> {
    let mut visited = HashSet::new();
    let mut components = Vec::new();

    for node in nodes {
        if visited.contains(node) {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(node.clone());
        visited.insert(node.clone());

        while let Some(current) = queue.pop_front() {
            component.push(current.clone());
            if let Some(neighbors) = adj.get(&current) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) && nodes.contains(neighbor) {
                        visited.insert(neighbor.clone());
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        components.push(component);
    }

    components
}

/// Detect promotable clusters from storage.
pub fn detect_promotable_clusters(
    storage: &Storage,
    config: &PromotionConfig,
) -> Result<Vec<PromotionCandidate>, Box<dyn std::error::Error>> {
    // 1. Query all Core-layer memories with core_strength > min_core_strength
    let all_memories = storage.all()?;
    let core_memories: Vec<_> = all_memories
        .iter()
        .filter(|m| m.core_strength > config.min_core_strength)
        .collect();

    if core_memories.is_empty() {
        return Ok(Vec::new());
    }

    let core_ids: HashSet<String> = core_memories.iter().map(|m| m.id.clone()).collect();

    // 2. Build adjacency from Hebbian links (weight > min_hebbian_weight)
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    let mut link_pairs: HashSet<(String, String)> = HashSet::new();

    for mem in &core_memories {
        if let Ok(links) = storage.get_hebbian_links_weighted(&mem.id) {
            for (target_id, weight) in links {
                if weight > config.min_hebbian_weight && core_ids.contains(&target_id) {
                    adj.entry(mem.id.clone()).or_default().insert(target_id.clone());
                    adj.entry(target_id.clone()).or_default().insert(mem.id.clone());

                    let pair = if mem.id < target_id {
                        (mem.id.clone(), target_id.clone())
                    } else {
                        (target_id.clone(), mem.id.clone())
                    };
                    link_pairs.insert(pair);
                }
            }
        }
    }

    // 3. Find connected components
    let components = connected_components(&adj, &core_ids);

    // 4. Filter and build candidates
    let mut candidates = Vec::new();

    // Build a lookup map for quick access
    let mem_map: HashMap<&str, &crate::types::MemoryRecord> =
        core_memories.iter().map(|m| (m.id.as_str(), *m)).collect();

    for component in components {
        if component.len() < config.min_cluster_size {
            continue;
        }

        let members: Vec<_> = component
            .iter()
            .filter_map(|id| mem_map.get(id.as_str()).copied())
            .collect();

        if members.is_empty() {
            continue;
        }

        let avg_core_strength: f64 =
            members.iter().map(|m| m.core_strength).sum::<f64>() / members.len() as f64;
        let avg_importance: f64 =
            members.iter().map(|m| m.importance).sum::<f64>() / members.len() as f64;

        if avg_importance < config.min_avg_importance {
            continue;
        }

        // Time span
        let min_t = members.iter().map(|m| m.created_at).min().unwrap();
        let max_t = members.iter().map(|m| m.created_at).max().unwrap();
        let time_span_days = (max_t - min_t).num_seconds() as f64 / 86400.0;

        if time_span_days < config.min_time_span_days {
            continue;
        }

        // Count internal links
        let component_set: HashSet<&str> = component.iter().map(|s| s.as_str()).collect();
        let internal_link_count = link_pairs
            .iter()
            .filter(|(a, b)| component_set.contains(a.as_str()) && component_set.contains(b.as_str()))
            .count();

        let snippets: Vec<String> = members
            .iter()
            .map(|m| {
                if m.content.len() > 100 {
                    format!("{}...", &m.content[..100])
                } else {
                    m.content.clone()
                }
            })
            .collect();

        let member_ids: Vec<String> = component.clone();
        let suggested_target = suggest_target(&snippets);
        let id = candidate_id(&member_ids);

        // Check if already promoted
        match storage.is_cluster_already_promoted(&member_ids) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                log::warn!("Failed to check promotion overlap: {e}");
                continue;
            }
        }

        candidates.push(PromotionCandidate {
            id,
            member_ids,
            snippets,
            avg_core_strength,
            avg_importance,
            time_span_days,
            internal_link_count,
            suggested_target,
            summary: None,
            created_at: Utc::now(),
            status: "pending".to_string(),
        });
    }

    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::{Duration, Utc};

    fn make_record(id: &str, content: &str, core_strength: f64, importance: f64, days_ago: i64) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Core,
            created_at: Utc::now() - Duration::days(days_ago),
            access_times: vec![Utc::now() - Duration::days(days_ago)],
            working_strength: 0.0,
            core_strength,
            importance,
            pinned: false,
            consolidation_count: 5,
            last_consolidated: Some(Utc::now()),
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    fn add_hebbian_link(storage: &Storage, src: &str, tgt: &str, strength: f64) {
        storage
            .record_association(src, tgt, strength, "test", "{}", "default")
            .unwrap();
    }

    #[test]
    fn test_detect_empty() {
        let storage = Storage::new(":memory:").unwrap();
        let config = PromotionConfig::default();

        let candidates = detect_promotable_clusters(&storage, &config).unwrap();
        assert!(candidates.is_empty(), "No memories → no candidates");
    }

    #[test]
    fn test_detect_cluster() {
        let mut storage = Storage::new(":memory:").unwrap();
        let config = PromotionConfig {
            enabled: true,
            min_core_strength: 0.6,
            min_hebbian_weight: 0.3,
            min_cluster_size: 3,
            min_time_span_days: 2.0,
            min_avg_importance: 0.4,
        };

        // Create 4 memories with high core_strength spanning 3 days
        let records = vec![
            make_record("m1", "principle: always be kind to users", 0.8, 0.7, 3),
            make_record("m2", "rule: never ignore user feedback", 0.75, 0.6, 2),
            make_record("m3", "principle: always validate inputs", 0.9, 0.8, 1),
            make_record("m4", "rule: never skip error handling", 0.7, 0.5, 0),
        ];

        for r in &records {
            storage.add(r, "default").unwrap();
        }

        // Create Hebbian links between them
        add_hebbian_link(&storage, "m1", "m2", 0.5);
        add_hebbian_link(&storage, "m2", "m3", 0.6);
        add_hebbian_link(&storage, "m3", "m4", 0.4);
        add_hebbian_link(&storage, "m1", "m4", 0.35);

        let candidates = detect_promotable_clusters(&storage, &config).unwrap();
        assert_eq!(candidates.len(), 1, "Should find 1 cluster");

        let c = &candidates[0];
        assert_eq!(c.member_ids.len(), 4);
        assert!(c.avg_core_strength > 0.6);
        assert!(c.time_span_days >= 2.0);
        assert!(c.internal_link_count >= 3);
        // Content has "principle" and "rule" → SOUL.md
        assert_eq!(c.suggested_target, "SOUL.md");
        assert_eq!(c.status, "pending");
    }

    #[test]
    fn test_dedup_already_promoted() {
        let mut storage = Storage::new(":memory:").unwrap();
        let config = PromotionConfig {
            enabled: true,
            min_core_strength: 0.6,
            min_hebbian_weight: 0.3,
            min_cluster_size: 3,
            min_time_span_days: 2.0,
            min_avg_importance: 0.4,
        };

        let records = vec![
            make_record("m1", "principle: always be kind", 0.8, 0.7, 3),
            make_record("m2", "rule: never ignore feedback", 0.75, 0.6, 2),
            make_record("m3", "principle: always validate", 0.9, 0.8, 1),
        ];

        for r in &records {
            storage.add(r, "default").unwrap();
        }

        add_hebbian_link(&storage, "m1", "m2", 0.5);
        add_hebbian_link(&storage, "m2", "m3", 0.6);
        add_hebbian_link(&storage, "m1", "m3", 0.4);

        // First detection should find 1 candidate
        let candidates = detect_promotable_clusters(&storage, &config).unwrap();
        assert_eq!(candidates.len(), 1);

        // Store and resolve as approved
        let c = &candidates[0];
        storage.store_promotion_candidate(c).unwrap();
        storage.resolve_promotion(&c.id, "approved").unwrap();

        // Second detection should find 0 (already promoted)
        let candidates2 = detect_promotable_clusters(&storage, &config).unwrap();
        assert!(candidates2.is_empty(), "Same cluster should not be detected again");
    }

    #[test]
    fn test_suggest_target_soul() {
        let snippets = vec![
            "principle: always be honest".to_string(),
            "rule: never lie to users".to_string(),
            "always validate input data".to_string(),
        ];
        assert_eq!(suggest_target(&snippets), "SOUL.md");
    }

    #[test]
    fn test_suggest_target_agents() {
        let snippets = vec![
            "how to deploy the application step by step".to_string(),
            "procedure for database migration".to_string(),
            "workflow for code review process".to_string(),
        ];
        assert_eq!(suggest_target(&snippets), "AGENTS.md");
    }

    #[test]
    fn test_suggest_target_default() {
        let snippets = vec![
            "the database uses PostgreSQL".to_string(),
            "we deployed version 2.0 yesterday".to_string(),
        ];
        assert_eq!(suggest_target(&snippets), "MEMORY.md");
    }

    #[test]
    fn test_candidate_id_deterministic() {
        let ids1 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let ids2 = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(candidate_id(&ids1), candidate_id(&ids2), "Order should not matter");
    }

    #[test]
    fn test_pending_promotions() {
        let storage = Storage::new(":memory:").unwrap();

        let candidate = PromotionCandidate {
            id: "test-001".to_string(),
            member_ids: vec!["m1".to_string(), "m2".to_string(), "m3".to_string()],
            snippets: vec!["snippet1".to_string(), "snippet2".to_string()],
            avg_core_strength: 0.8,
            avg_importance: 0.7,
            time_span_days: 3.0,
            internal_link_count: 4,
            suggested_target: "MEMORY.md".to_string(),
            summary: None,
            created_at: Utc::now(),
            status: "pending".to_string(),
        };

        storage.store_promotion_candidate(&candidate).unwrap();

        let pending = storage.get_pending_promotions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "test-001");
        assert_eq!(pending[0].status, "pending");

        storage.resolve_promotion("test-001", "approved").unwrap();
        let pending2 = storage.get_pending_promotions().unwrap();
        assert!(pending2.is_empty());
    }
}
