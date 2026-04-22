//! Link formation for multi-signal Hebbian association discovery.
//!
//! Combines signal scores, filters by threshold, enforces link budget,
//! and persists associations to the database.

use crate::association::signals::SignalComputer;
use crate::config::AssociationConfig;
use crate::storage::Storage;

/// A candidate link before persistence.
#[derive(Debug)]
struct ProtoLink {
    target_id: String,
    strength: f64,
    combined_score: f64,
    signal_source: String,
    signal_detail: String,
}

/// Orchestrates link formation for a newly stored memory.
pub struct LinkFormer<'a> {
    storage: &'a Storage,
}

impl<'a> LinkFormer<'a> {
    pub fn new(storage: &'a Storage) -> Self {
        Self { storage }
    }

    /// Discover associations for a newly stored memory.
    ///
    /// For each candidate memory ID:
    /// 1. Fetch candidate's entities, embedding, and timestamp from storage
    /// 2. Compute signal scores
    /// 3. Compute weighted combined score
    /// 4. Filter by `config.link_threshold`
    /// 5. Sort by combined score descending, take top `config.max_links_per_memory`
    /// 6. Persist each link via `storage.record_association()`
    ///
    /// Returns the number of newly created links.
    #[allow(clippy::too_many_arguments)]
    pub fn discover_associations(
        &self,
        new_memory_id: &str,
        candidates: Vec<String>,
        new_entities: &[String],
        new_embedding: Option<&[f32]>,
        new_timestamp: f64,
        config: &AssociationConfig,
        namespace: &str,
    ) -> Result<usize, rusqlite::Error> {
        let mut proto_links: Vec<ProtoLink> = Vec::new();

        for candidate_id in &candidates {
            // Fetch candidate data from storage
            let cand_entities = self.storage.get_entities_for_memory(candidate_id)?;
            let cand_embedding = self.storage.get_embedding_for_memory(candidate_id)?;
            let cand_timestamp = match self.storage.get_memory_timestamp(candidate_id)? {
                Some(ts) => ts,
                None => continue, // Memory doesn't exist, skip
            };

            // Compute all signals
            let scores = SignalComputer::compute_all(
                new_entities,
                &cand_entities,
                new_embedding,
                cand_embedding.as_deref(),
                new_timestamp,
                cand_timestamp,
            );

            // Compute weighted combined score
            let combined = scores.combined(config);

            // Filter by threshold
            if combined >= config.link_threshold {
                let signal_source = scores.signal_source(0.2).to_string();
                proto_links.push(ProtoLink {
                    target_id: candidate_id.clone(),
                    strength: config.initial_strength,
                    combined_score: combined,
                    signal_source,
                    signal_detail: scores.to_json(),
                });
            }
        }

        // Sort by combined score descending
        proto_links.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top-K
        proto_links.truncate(config.max_links_per_memory);

        // Persist links and count new creations
        let mut created = 0;
        for link in &proto_links {
            let is_new = self.storage.record_association(
                new_memory_id,
                &link.target_id,
                link.strength,
                &link.signal_source,
                &link.signal_detail,
                namespace,
            )?;
            if is_new {
                created += 1;
            }
        }

        Ok(created)
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

    fn store_memory_with_entities(
        storage: &mut Storage,
        id: &str,
        content: &str,
        entities: &[&str],
        timestamp: chrono::DateTime<Utc>,
    ) {
        let record = make_record(id, content, timestamp);
        storage.add(&record, "default").unwrap();

        // Store entities: create entity records and link them to the memory
        for entity_name in entities {
            // Use the storage's store_entities method if available,
            // or manually insert into entities + memory_entities tables
            let entity_id = format!("ent_{}", entity_name.to_lowercase().replace(' ', "_"));
            let now_ts = timestamp.timestamp() as f64;
            storage
                .connection()
                .execute(
                    "INSERT OR IGNORE INTO entities (id, name, entity_type, namespace, created_at, updated_at) \
                     VALUES (?1, ?2, 'concept', 'default', ?3, ?3)",
                    rusqlite::params![entity_id, entity_name, now_ts],
                )
                .unwrap();
            storage
                .connection()
                .execute(
                    "INSERT OR IGNORE INTO memory_entities (memory_id, entity_id, role) \
                     VALUES (?1, ?2, 'mention')",
                    rusqlite::params![id, entity_id],
                )
                .unwrap();
        }
    }

    #[allow(dead_code)]
    fn store_memory_with_embedding(
        storage: &mut Storage,
        id: &str,
        content: &str,
        embedding: &[f32],
        timestamp: chrono::DateTime<Utc>,
    ) {
        let record = make_record(id, content, timestamp);
        storage.add(&record, "default").unwrap();

        // Store embedding as BLOB
        let blob: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let dims = embedding.len() as i64;
        storage
            .connection()
            .execute(
                "INSERT INTO memory_embeddings (memory_id, model, embedding, dimensions, created_at) \
                 VALUES (?1, 'test/model', ?2, ?3, ?4)",
                rusqlite::params![id, blob, dims, chrono::Utc::now().to_rfc3339()],
            )
            .unwrap();
    }

    #[test]
    fn test_discover_no_candidates() {
        let storage = test_storage();
        let former = LinkFormer::new(&storage);
        let config = AssociationConfig::default();

        let created = former
            .discover_associations(
                "new_mem",
                vec![],
                &["cat".to_string()],
                None,
                1700000000.0,
                &config,
                "default",
            )
            .unwrap();

        assert_eq!(created, 0, "no candidates should produce 0 links");
    }

    #[test]
    fn test_discover_below_threshold() {
        let mut storage = test_storage();
        let now = Utc::now();
        let thirty_days_ago = now - chrono::Duration::days(30);

        // Create a candidate with no entity overlap, no embedding, distant time
        let record = make_record("cand1", "totally unrelated memory", thirty_days_ago);
        storage.add(&record, "default").unwrap();

        // Create the new memory record too (for FK constraints if needed later)
        let new_record = make_record("new_mem", "a new memory", now);
        storage.add(&new_record, "default").unwrap();

        let former = LinkFormer::new(&storage);
        let mut config = AssociationConfig::default();
        config.link_threshold = 0.4;

        let created = former
            .discover_associations(
                "new_mem",
                vec!["cand1".to_string()],
                &["cat".to_string(), "dog".to_string()],
                None,
                now.timestamp() as f64,
                &config,
                "default",
            )
            .unwrap();

        assert_eq!(created, 0, "candidates below threshold should produce 0 links");
    }

    #[test]
    fn test_discover_creates_links() {
        let mut storage = test_storage();
        let now = Utc::now();

        // Create new memory
        let new_record = make_record("new_mem", "memory about cats and dogs", now);
        storage.add(&new_record, "default").unwrap();

        // Create candidate with entity overlap and same timestamp
        store_memory_with_entities(
            &mut storage,
            "cand1",
            "another memory about cats",
            &["cat", "fish"],
            now,
        );

        // New memory's entities include "cat" — overlap with cand1
        let new_entities = vec!["cat".to_string(), "dog".to_string()];

        let former = LinkFormer::new(&storage);
        let mut config = AssociationConfig::default();
        // Lower threshold to make it easier to create links
        // temporal_proximity at same time = 1.0, w_temporal = 0.2 → contributes 0.2
        // entity jaccard = 1/3 ≈ 0.333, w_entity = 0.3 → contributes 0.1
        // total ≈ 0.3, so set threshold below that
        config.link_threshold = 0.2;

        let created = former
            .discover_associations(
                "new_mem",
                vec!["cand1".to_string()],
                &new_entities,
                None,
                now.timestamp() as f64,
                &config,
                "default",
            )
            .unwrap();

        assert!(created >= 1, "should create at least 1 link, got {}", created);

        // Verify link exists in DB
        let count: i64 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM hebbian_links WHERE \
                 (source_id = 'new_mem' AND target_id = 'cand1') OR \
                 (source_id = 'cand1' AND target_id = 'new_mem')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "exactly one link should exist");
    }

    #[test]
    fn test_discover_respects_max_links() {
        let mut storage = test_storage();
        let now = Utc::now();

        // Create new memory
        let new_record = make_record("new_mem", "memory about animals", now);
        storage.add(&new_record, "default").unwrap();

        // Create 10 candidates, all with entity overlap
        for i in 0..10 {
            let id = format!("cand{}", i);
            store_memory_with_entities(
                &mut storage,
                &id,
                &format!("candidate {} about animals", i),
                &["animal"],
                now,
            );
        }

        let new_entities = vec!["animal".to_string()];
        let candidates: Vec<String> = (0..10).map(|i| format!("cand{}", i)).collect();

        let former = LinkFormer::new(&storage);
        let mut config = AssociationConfig::default();
        config.link_threshold = 0.1; // low threshold so all pass
        config.max_links_per_memory = 3; // but only keep top 3

        let created = former
            .discover_associations(
                "new_mem",
                candidates,
                &new_entities,
                None,
                now.timestamp() as f64,
                &config,
                "default",
            )
            .unwrap();

        assert_eq!(created, 3, "should create exactly max_links_per_memory links");

        // Verify count in DB
        let count: i64 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM hebbian_links WHERE source_id = 'new_mem'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_discover_link_metadata() {
        let mut storage = test_storage();
        let now = Utc::now();

        // Create new memory
        let new_record = make_record("new_mem", "memory about cats", now);
        storage.add(&new_record, "default").unwrap();

        // Create candidate with strong entity overlap and same timestamp
        store_memory_with_entities(
            &mut storage,
            "cand1",
            "another memory about cats",
            &["cat"],
            now,
        );

        let new_entities = vec!["cat".to_string()];

        let former = LinkFormer::new(&storage);
        let mut config = AssociationConfig::default();
        config.link_threshold = 0.1;

        let created = former
            .discover_associations(
                "new_mem",
                vec!["cand1".to_string()],
                &new_entities,
                None,
                now.timestamp() as f64,
                &config,
                "default",
            )
            .unwrap();

        assert_eq!(created, 1);

        // Verify signal_source and signal_detail are stored
        let (signal_source, signal_detail): (String, String) = storage
            .connection()
            .query_row(
                "SELECT signal_source, signal_detail FROM hebbian_links \
                 WHERE source_id = 'new_mem' AND target_id = 'cand1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        // signal_source should be a valid value
        assert!(
            ["entity", "embedding", "temporal", "multi"].contains(&signal_source.as_str()),
            "signal_source should be valid, got: {}",
            signal_source
        );

        // signal_detail should be valid JSON with all three fields
        let detail: serde_json::Value = serde_json::from_str(&signal_detail)
            .expect("signal_detail should be valid JSON");
        assert!(detail["entity_overlap"].is_number(), "should have entity_overlap");
        assert!(detail["embedding_cosine"].is_number(), "should have embedding_cosine");
        assert!(detail["temporal_proximity"].is_number(), "should have temporal_proximity");

        // Entity overlap should be 1.0 (identical entity set: both have only "cat")
        let entity_overlap = detail["entity_overlap"].as_f64().unwrap();
        assert!(
            (entity_overlap - 1.0).abs() < 1e-6,
            "entity_overlap should be 1.0 for identical entities, got {}",
            entity_overlap
        );
    }
}
