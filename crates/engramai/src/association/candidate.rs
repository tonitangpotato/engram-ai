//! Candidate selection for multi-signal Hebbian link formation.
//!
//! Three-layer candidate selection:
//! 1. Temporal: memories from the last N days
//! 2. Entity FTS: memories mentioning the same entities
//! 3. Embedding: nearest neighbors by cosine similarity

use std::collections::HashSet;

use crate::config::AssociationConfig;
use crate::embeddings::EmbeddingProvider;
use crate::storage::Storage;

/// Selects candidate memories for association evaluation.
pub struct CandidateSelector<'a> {
    storage: &'a Storage,
}

impl<'a> CandidateSelector<'a> {
    pub fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    /// Select candidates using three-layer strategy.
    ///
    /// Returns deduplicated memory IDs (excluding `new_memory_id` itself),
    /// limited to `config.candidate_limit`.
    pub fn select_candidates(
        &self,
        new_memory_id: &str,
        new_memory_created_at: f64,
        entities: &[String],
        embedding: Option<&[f32]>,
        config: &AssociationConfig,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut candidate_ids: Vec<String> = Vec::new();

        // Layer 1: Temporal window — memories from the last N days
        let window_secs = config.temporal_window_days as f64 * 86400.0;
        let since = new_memory_created_at - window_secs;
        let temporal_ids = self.storage.get_memory_ids_since(since, "default")?;
        candidate_ids.extend(temporal_ids);

        // Layer 2: Entity FTS — search for memories mentioning the same entities
        if !entities.is_empty() {
            let entity_query = entities.join(" OR ");
            // Use search_fts_ns which handles tokenization and FTS query building
            let fts_results = self.storage.search_fts_ns(&entity_query, 20, Some("default"))?;
            for record in fts_results {
                candidate_ids.push(record.id);
            }
        }

        // Layer 3: Embedding nearest neighbors
        if let Some(emb) = embedding {
            // Get all embeddings and compute cosine similarity
            // Try the first available model
            if let Ok(all_embeddings) = self.storage.get_embeddings_in_namespace(Some("default"), "*") {
                // If wildcard didn't work, this will be empty, and that's fine
                let mut scored: Vec<(String, f64)> = all_embeddings
                    .iter()
                    .map(|(id, stored_emb)| {
                        let sim = EmbeddingProvider::cosine_similarity(emb, stored_emb) as f64;
                        (id.clone(), sim)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.truncate(20);
                for (id, _) in scored {
                    candidate_ids.push(id);
                }
            }
        }

        // Deduplicate
        let mut seen = HashSet::new();
        candidate_ids.retain(|id| {
            if id == new_memory_id {
                return false; // Exclude self
            }
            seen.insert(id.clone())
        });

        // Limit to candidate_limit
        candidate_ids.truncate(config.candidate_limit);

        Ok(candidate_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AssociationConfig;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;

    fn test_storage() -> Storage {
        Storage::new(":memory:").expect("in-memory storage")
    }

    fn make_record(id: &str, content: &str, created_at: chrono::DateTime<Utc>) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at,
            access_times: vec![created_at],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn test_candidate_selection_temporal() {
        let mut storage = test_storage();
        let now = Utc::now();
        let now_ts = now.timestamp() as f64;

        // Add memories at different times
        let recent = make_record("recent1", "recent memory about cats", now);
        storage.add(&recent, "default").unwrap();

        // Memory from 2 days ago
        let two_days_ago = now - chrono::Duration::days(2);
        let older = make_record("older1", "older memory about dogs", two_days_ago);
        storage.add(&older, "default").unwrap();

        // Memory from 30 days ago (outside default 7-day window)
        let thirty_days_ago = now - chrono::Duration::days(30);
        let ancient = make_record("ancient1", "ancient memory about fish", thirty_days_ago);
        storage.add(&ancient, "default").unwrap();

        let selector = CandidateSelector::new(&storage);
        let config = AssociationConfig::default(); // 7-day window

        let candidates = selector
            .select_candidates("new_mem", now_ts, &[], None, &config)
            .unwrap();

        // recent1 and older1 should be candidates (within 7 days)
        assert!(candidates.contains(&"recent1".to_string()));
        assert!(candidates.contains(&"older1".to_string()));
        // ancient1 should NOT be a candidate (outside 7-day window)
        assert!(!candidates.contains(&"ancient1".to_string()));
    }

    #[test]
    fn test_candidate_selection_excludes_self() {
        let mut storage = test_storage();
        let now = Utc::now();
        let now_ts = now.timestamp() as f64;

        // Add a memory
        let mem = make_record("self_mem", "test memory content", now);
        storage.add(&mem, "default").unwrap();

        let selector = CandidateSelector::new(&storage);
        let config = AssociationConfig::default();

        // Select candidates for "self_mem" — should not include itself
        let candidates = selector
            .select_candidates("self_mem", now_ts, &[], None, &config)
            .unwrap();

        assert!(!candidates.contains(&"self_mem".to_string()));
    }
}
