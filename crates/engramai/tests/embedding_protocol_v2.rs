//! Tests for Engram Embedding Protocol v2.
//!
//! Validates:
//! - BLOB serialization round-trip
//! - Multi-model storage (composite PK: memory_id, model)
//! - Model-scoped queries
//! - Validation (NaN, Inf, empty rejection)
//! - Delete by model
//! - Missing embedding detection
//! - Migration awareness
//! - Namespace + model filtering

use engramai::storage::Storage;
use engramai::types::{MemoryRecord, MemoryType, MemoryLayer};
use engramai::embeddings::EmbeddingConfig;
use chrono::Utc;

/// Helper: create a test memory record.
fn make_memory(id: &str, content: &str) -> MemoryRecord {
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: Utc::now(),
        access_times: vec![Utc::now()],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: String::new(),
        contradicts: None,
        contradicted_by: None,
        metadata: None,
    }
}

/// Helper: create a simple test embedding of given dimensions.
fn make_embedding(dims: usize, seed: f32) -> Vec<f32> {
    (0..dims).map(|i| (i as f32 + seed) * 0.01).collect()
}

#[test]
fn test_embedding_round_trip() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("rt-1", "round trip test");
    storage.add(&mem, "default").unwrap();

    let embedding = make_embedding(768, 1.0);
    let model = "ollama/nomic-embed-text";

    // Store
    storage.store_embedding("rt-1", &embedding, model, 768).unwrap();

    // Read back
    let retrieved = storage.get_embedding("rt-1", model).unwrap();
    assert!(retrieved.is_some(), "Embedding should exist");

    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.len(), 768, "Dimension mismatch");

    // Verify exact values match (bit-perfect round-trip)
    for (i, (orig, ret)) in embedding.iter().zip(retrieved.iter()).enumerate() {
        assert_eq!(
            orig.to_bits(),
            ret.to_bits(),
            "Value mismatch at index {}: {} vs {}",
            i,
            orig,
            ret
        );
    }
}

#[test]
fn test_multi_model_storage() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("mm-1", "multi model test");
    storage.add(&mem, "default").unwrap();

    let emb_a = make_embedding(768, 1.0);
    let emb_b = make_embedding(1536, 2.0);
    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    // Store two different models for the same memory
    storage.store_embedding("mm-1", &emb_a, model_a, 768).unwrap();
    storage.store_embedding("mm-1", &emb_b, model_b, 1536).unwrap();

    // Both should exist
    let ret_a = storage.get_embedding("mm-1", model_a).unwrap();
    let ret_b = storage.get_embedding("mm-1", model_b).unwrap();

    assert!(ret_a.is_some(), "Model A embedding should exist");
    assert!(ret_b.is_some(), "Model B embedding should exist");
    assert_eq!(ret_a.unwrap().len(), 768);
    assert_eq!(ret_b.unwrap().len(), 1536);
}

#[test]
fn test_model_scoped_query() {
    let mut storage = Storage::new(":memory:").unwrap();

    // Create two memories
    let mem1 = make_memory("ms-1", "memory one");
    let mem2 = make_memory("ms-2", "memory two");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "default").unwrap();

    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    // Store: mem1 has both models, mem2 has only model_a
    storage.store_embedding("ms-1", &make_embedding(768, 1.0), model_a, 768).unwrap();
    storage.store_embedding("ms-1", &make_embedding(1536, 2.0), model_b, 1536).unwrap();
    storage.store_embedding("ms-2", &make_embedding(768, 3.0), model_a, 768).unwrap();

    // Query model_a: should return 2 results
    let all_a = storage.get_all_embeddings(model_a).unwrap();
    assert_eq!(all_a.len(), 2, "Model A should have 2 embeddings");

    // Query model_b: should return 1 result
    let all_b = storage.get_all_embeddings(model_b).unwrap();
    assert_eq!(all_b.len(), 1, "Model B should have 1 embedding");
    assert_eq!(all_b[0].0, "ms-1", "Model B's only embedding should be ms-1");
}

#[test]
fn test_blob_validation_nan() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("nan-1", "nan test");
    storage.add(&mem, "default").unwrap();

    let mut embedding = make_embedding(768, 1.0);
    embedding[100] = f32::NAN;

    let result = storage.store_embedding("nan-1", &embedding, "test/model", 768);
    assert!(result.is_err(), "Should reject embedding with NaN");
}

#[test]
fn test_blob_validation_inf() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("inf-1", "inf test");
    storage.add(&mem, "default").unwrap();

    let mut embedding = make_embedding(768, 1.0);
    embedding[50] = f32::INFINITY;

    let result = storage.store_embedding("inf-1", &embedding, "test/model", 768);
    assert!(result.is_err(), "Should reject embedding with Inf");

    // Also test negative infinity
    embedding[50] = f32::NEG_INFINITY;
    let result = storage.store_embedding("inf-1", &embedding, "test/model", 768);
    assert!(result.is_err(), "Should reject embedding with -Inf");
}

#[test]
fn test_blob_validation_empty() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("empty-1", "empty test");
    storage.add(&mem, "default").unwrap();

    let embedding: Vec<f32> = vec![];
    let result = storage.store_embedding("empty-1", &embedding, "test/model", 0);
    assert!(result.is_err(), "Should reject empty embedding");
}

#[test]
fn test_delete_embedding_by_model() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("del-1", "delete test");
    storage.add(&mem, "default").unwrap();

    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    storage.store_embedding("del-1", &make_embedding(768, 1.0), model_a, 768).unwrap();
    storage.store_embedding("del-1", &make_embedding(1536, 2.0), model_b, 1536).unwrap();

    // Delete only model_a
    storage.delete_embedding("del-1", model_a).unwrap();

    // model_a should be gone
    assert!(storage.get_embedding("del-1", model_a).unwrap().is_none(),
        "Model A should be deleted");

    // model_b should still exist
    assert!(storage.get_embedding("del-1", model_b).unwrap().is_some(),
        "Model B should still exist");
}

#[test]
fn test_delete_all_embeddings() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("delall-1", "delete all test");
    storage.add(&mem, "default").unwrap();

    storage.store_embedding("delall-1", &make_embedding(768, 1.0), "model/a", 768).unwrap();
    storage.store_embedding("delall-1", &make_embedding(768, 2.0), "model/b", 768).unwrap();

    storage.delete_all_embeddings("delall-1").unwrap();

    assert!(storage.get_embedding("delall-1", "model/a").unwrap().is_none());
    assert!(storage.get_embedding("delall-1", "model/b").unwrap().is_none());
}

#[test]
fn test_get_memories_without_embeddings() {
    let mut storage = Storage::new(":memory:").unwrap();

    let mem1 = make_memory("miss-1", "has embedding");
    let mem2 = make_memory("miss-2", "no embedding");
    let mem3 = make_memory("miss-3", "also no embedding");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "default").unwrap();
    storage.add(&mem3, "default").unwrap();

    let model = "ollama/nomic-embed-text";
    storage.store_embedding("miss-1", &make_embedding(768, 1.0), model, 768).unwrap();

    let missing = storage.get_memories_without_embeddings(model).unwrap();
    assert_eq!(missing.len(), 2, "Should find 2 memories without embeddings");
    assert!(missing.contains(&"miss-2".to_string()));
    assert!(missing.contains(&"miss-3".to_string()));
    assert!(!missing.contains(&"miss-1".to_string()));
}

#[test]
fn test_get_memories_without_embeddings_model_specific() {
    let mut storage = Storage::new(":memory:").unwrap();

    let mem1 = make_memory("mms-1", "has both models");
    let mem2 = make_memory("mms-2", "has only model a");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "default").unwrap();

    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    storage.store_embedding("mms-1", &make_embedding(768, 1.0), model_a, 768).unwrap();
    storage.store_embedding("mms-1", &make_embedding(1536, 2.0), model_b, 1536).unwrap();
    storage.store_embedding("mms-2", &make_embedding(768, 3.0), model_a, 768).unwrap();

    // model_a: both have it, so 0 missing
    let missing_a = storage.get_memories_without_embeddings(model_a).unwrap();
    assert_eq!(missing_a.len(), 0, "All memories have model A embeddings");

    // model_b: only mms-1 has it, mms-2 is missing
    let missing_b = storage.get_memories_without_embeddings(model_b).unwrap();
    assert_eq!(missing_b.len(), 1, "mms-2 should be missing model B");
    assert_eq!(missing_b[0], "mms-2");
}

#[test]
fn test_model_id_format() {
    // Default (Ollama)
    let config = EmbeddingConfig::default();
    assert_eq!(config.model_id(), "ollama/nomic-embed-text");

    // OpenAI
    let config = EmbeddingConfig::openai(None);
    assert_eq!(config.model_id(), "openai/text-embedding-3-small");

    // OpenAI ada
    let config = EmbeddingConfig::openai_ada(None);
    assert_eq!(config.model_id(), "openai/text-embedding-ada-002");

    // Custom Ollama
    let config = EmbeddingConfig::ollama("mxbai-embed-large", 1024);
    assert_eq!(config.model_id(), "ollama/mxbai-embed-large");
}

#[test]
fn test_fresh_install_v2_schema() {
    let storage = Storage::new(":memory:").unwrap();

    // Check protocol version is set to "2"
    let conn = storage.connection();
    let version: String = conn
        .query_row(
            "SELECT value FROM engram_meta WHERE key = 'embedding_protocol_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "2", "Fresh DB should have protocol version 2");

    // Check table has correct columns
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(memory_embeddings)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(cols.contains(&"memory_id".to_string()));
    assert!(cols.contains(&"model".to_string()));
    assert!(cols.contains(&"embedding".to_string()));
    assert!(cols.contains(&"dimensions".to_string()));
    assert!(cols.contains(&"created_at".to_string()));
}

#[test]
fn test_embedding_dimensions_stored_correctly() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("dim-1", "dimensions test");
    storage.add(&mem, "default").unwrap();

    let embedding = make_embedding(384, 1.0);
    let model = "local/minilm-l6-v2";
    storage.store_embedding("dim-1", &embedding, model, 384).unwrap();

    // Check stored dimensions via raw SQL
    let conn = storage.connection();
    let (stored_dims, blob_len): (i64, usize) = conn
        .query_row(
            "SELECT dimensions, LENGTH(embedding) FROM memory_embeddings WHERE memory_id = ? AND model = ?",
            rusqlite::params!["dim-1", model],
            |row| Ok((row.get(0)?, row.get::<_, usize>(1)?)),
        )
        .unwrap();

    assert_eq!(stored_dims, 384, "Stored dimensions should be 384");
    assert_eq!(blob_len, 384 * 4, "Blob length should be dims * 4");
    assert_eq!(blob_len as i64 / 4, stored_dims, "Blob size / 4 must equal dimensions");
}

#[test]
fn test_embeddings_in_namespace() {
    let mut storage = Storage::new(":memory:").unwrap();

    // Create memories in different namespaces
    let mem1 = make_memory("ns-1", "namespace default");
    let mem2 = make_memory("ns-2", "namespace other");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "other").unwrap();

    let model = "ollama/nomic-embed-text";
    storage.store_embedding("ns-1", &make_embedding(768, 1.0), model, 768).unwrap();
    storage.store_embedding("ns-2", &make_embedding(768, 2.0), model, 768).unwrap();

    // Query default namespace
    let default_embs = storage.get_embeddings_in_namespace(Some("default"), model).unwrap();
    assert_eq!(default_embs.len(), 1, "Default namespace should have 1 embedding");
    assert_eq!(default_embs[0].0, "ns-1");

    // Query other namespace
    let other_embs = storage.get_embeddings_in_namespace(Some("other"), model).unwrap();
    assert_eq!(other_embs.len(), 1, "Other namespace should have 1 embedding");
    assert_eq!(other_embs[0].0, "ns-2");

    // Query wildcard namespace (all)
    let all_embs = storage.get_embeddings_in_namespace(Some("*"), model).unwrap();
    assert_eq!(all_embs.len(), 2, "Wildcard should return all embeddings");
}

#[test]
fn test_embeddings_namespace_model_intersection() {
    let mut storage = Storage::new(":memory:").unwrap();

    let mem1 = make_memory("nmi-1", "ns default model a");
    let mem2 = make_memory("nmi-2", "ns default model b");
    let mem3 = make_memory("nmi-3", "ns other model a");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "default").unwrap();
    storage.add(&mem3, "other").unwrap();

    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    storage.store_embedding("nmi-1", &make_embedding(768, 1.0), model_a, 768).unwrap();
    storage.store_embedding("nmi-2", &make_embedding(1536, 2.0), model_b, 1536).unwrap();
    storage.store_embedding("nmi-3", &make_embedding(768, 3.0), model_a, 768).unwrap();

    // default + model_a: only nmi-1
    let results = storage.get_embeddings_in_namespace(Some("default"), model_a).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "nmi-1");

    // default + model_b: only nmi-2
    let results = storage.get_embeddings_in_namespace(Some("default"), model_b).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "nmi-2");

    // other + model_a: only nmi-3
    let results = storage.get_embeddings_in_namespace(Some("other"), model_a).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "nmi-3");

    // other + model_b: none
    let results = storage.get_embeddings_in_namespace(Some("other"), model_b).unwrap();
    assert_eq!(results.len(), 0);
}

#[test]
fn test_embedding_overwrite_same_model() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("ow-1", "overwrite test");
    storage.add(&mem, "default").unwrap();

    let model = "ollama/nomic-embed-text";
    let emb_v1 = make_embedding(768, 1.0);
    let emb_v2 = make_embedding(768, 99.0);

    // Store v1
    storage.store_embedding("ow-1", &emb_v1, model, 768).unwrap();
    let ret1 = storage.get_embedding("ow-1", model).unwrap().unwrap();
    assert_eq!(ret1[0].to_bits(), emb_v1[0].to_bits());

    // Overwrite with v2 (INSERT OR REPLACE)
    storage.store_embedding("ow-1", &emb_v2, model, 768).unwrap();
    let ret2 = storage.get_embedding("ow-1", model).unwrap().unwrap();
    assert_eq!(ret2[0].to_bits(), emb_v2[0].to_bits(), "Should be overwritten with v2");
    assert_ne!(ret2[0].to_bits(), emb_v1[0].to_bits(), "Should NOT be v1 anymore");

    // Should still be only 1 row
    let all = storage.get_all_embeddings(model).unwrap();
    assert_eq!(all.len(), 1, "Should have exactly 1 embedding after overwrite");
}

#[test]
fn test_embedding_stats() {
    let mut storage = Storage::new(":memory:").unwrap();

    let mem1 = make_memory("st-1", "stats test 1");
    let mem2 = make_memory("st-2", "stats test 2");
    let mem3 = make_memory("st-3", "stats test 3");
    storage.add(&mem1, "default").unwrap();
    storage.add(&mem2, "default").unwrap();
    storage.add(&mem3, "default").unwrap();

    let model = "ollama/nomic-embed-text";
    storage.store_embedding("st-1", &make_embedding(768, 1.0), model, 768).unwrap();
    storage.store_embedding("st-2", &make_embedding(768, 2.0), model, 768).unwrap();

    let stats = storage.embedding_stats().unwrap();
    assert_eq!(stats.total_memories, 3);
    assert_eq!(stats.embedded_count, 2);
}

#[test]
fn test_cascade_delete_memory_removes_embeddings() {
    let mut storage = Storage::new(":memory:").unwrap();
    let mem = make_memory("cas-1", "cascade test");
    storage.add(&mem, "default").unwrap();

    storage.store_embedding("cas-1", &make_embedding(768, 1.0), "model/a", 768).unwrap();
    storage.store_embedding("cas-1", &make_embedding(768, 2.0), "model/b", 768).unwrap();

    // Delete the memory itself
    storage.delete("cas-1").unwrap();

    // Embeddings should be cascade-deleted (FK ON DELETE CASCADE)
    assert!(storage.get_embedding("cas-1", "model/a").unwrap().is_none());
    assert!(storage.get_embedding("cas-1", "model/b").unwrap().is_none());
}

#[test]
fn test_nonexistent_embedding_returns_none() {
    let storage = Storage::new(":memory:").unwrap();

    let result = storage.get_embedding("does-not-exist", "ollama/nomic-embed-text").unwrap();
    assert!(result.is_none(), "Should return None for nonexistent memory");
}

#[test]
fn test_cross_model_isolation() {
    // Verify that getting embeddings for one model never returns another model's data
    let mut storage = Storage::new(":memory:").unwrap();

    let mem = make_memory("iso-1", "isolation test");
    storage.add(&mem, "default").unwrap();

    let model_a = "ollama/nomic-embed-text";
    let model_b = "openai/text-embedding-3-small";

    // Distinctly different values
    let emb_a: Vec<f32> = vec![1.0; 768];
    let emb_b: Vec<f32> = vec![2.0; 1536];

    storage.store_embedding("iso-1", &emb_a, model_a, 768).unwrap();
    storage.store_embedding("iso-1", &emb_b, model_b, 1536).unwrap();

    // Model A returns 768-dim with all 1.0
    let ret_a = storage.get_embedding("iso-1", model_a).unwrap().unwrap();
    assert_eq!(ret_a.len(), 768);
    assert!((ret_a[0] - 1.0).abs() < 0.001);

    // Model B returns 1536-dim with all 2.0
    let ret_b = storage.get_embedding("iso-1", model_b).unwrap().unwrap();
    assert_eq!(ret_b.len(), 1536);
    assert!((ret_b[0] - 2.0).abs() < 0.001);
}
