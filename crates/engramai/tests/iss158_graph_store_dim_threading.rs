//! ISS-158 regression: `Memory::with_graph_store` must thread
//! `config.embedding.dimensions` into the installed `SqliteGraphStore`
//! so non-default embedders (bge-large 1024d etc.) don't trip the
//! 768d default and surface `GraphError::Invariant("entity embedding
//! dim mismatch")` at runtime.

use chrono::Utc;
use engramai::embeddings::EmbeddingConfig;
use engramai::graph::entity::{Entity, EntityKind};
use engramai::graph::store::GraphWrite;
use engramai::{Memory, MemoryConfig};
use tempfile::TempDir;
use uuid::Uuid;

/// Build a `Memory` with a non-default 1024d embedding config + an
/// installed graph store. Insert a 1024d-embedded entity through the
/// `Memory::graph_mut` accessor. Pre-ISS-158 the graph store used
/// `default_embedding_dim()` (=768) and the insert would fail with
/// `GraphError::Invariant`. Post-fix the configured 1024 is honored
/// and the insert succeeds.
#[test]
fn with_graph_store_honors_configured_embedding_dim() {
    let dir = TempDir::new().expect("tempdir");
    let substrate = dir.path().join("sub.db");
    let graph = dir.path().join("graph.db");

    let mut cfg = MemoryConfig::default();
    cfg.embedding = EmbeddingConfig {
        provider: "ollama".into(),
        model: "bge-large".into(),
        host: "http://localhost:11434".into(),
        dimensions: 1024,
        timeout_secs: 30,
        api_key: None,
    };

    let mut mem = Memory::new(substrate.to_str().unwrap(), Some(cfg))
        .expect("Memory::new")
        .with_graph_store(&graph)
        .expect("with_graph_store");

    // Build a 1024d entity and try to write it through the configured
    // graph store. Pre-fix: store has embedding_dim=768, codec rejects
    // the 1024d blob. Post-fix: store has embedding_dim=1024, accepts.
    let id = Uuid::new_v4();
    let mut e = Entity::new(id, "BgeAlice".to_string(), EntityKind::Person, Utc::now());
    e.embedding = Some(vec![0.1f32; 1024]);

    let mut store = mem.graph_mut().with_namespace("default");
    let result = store.insert_entity(&e);
    assert!(
        result.is_ok(),
        "insert_entity with 1024d embedding should succeed when embedder \
         is configured for 1024d (post-ISS-158); got: {:?}",
        result
    );
}

/// Sanity-check: with the default 768d embedder a 768d entity still
/// writes cleanly (regression guard against accidentally wiring the
/// dim threading wrong).
#[test]
fn with_graph_store_default_768_still_works() {
    let dir = TempDir::new().expect("tempdir");
    let substrate = dir.path().join("sub.db");
    let graph = dir.path().join("graph.db");

    // Default config: nomic-embed-text 768d.
    let cfg = MemoryConfig::default();
    assert_eq!(cfg.embedding.dimensions, 768, "default config dim regressed");

    let mut mem = Memory::new(substrate.to_str().unwrap(), Some(cfg))
        .expect("Memory::new")
        .with_graph_store(&graph)
        .expect("with_graph_store");

    let id = Uuid::new_v4();
    let mut e = Entity::new(id, "NomicAlice".to_string(), EntityKind::Person, Utc::now());
    e.embedding = Some(vec![0.1f32; 768]);

    let mut store = mem.graph_mut().with_namespace("default");
    let result = store.insert_entity(&e);
    assert!(
        result.is_ok(),
        "default 768d path regressed: {:?}",
        result
    );
}

/// Mismatch direction: 1024d config + 768d entity blob must still
/// error (no silent acceptance). Confirms the dim check is still
/// active, just bound to the configured value.
#[test]
fn mismatch_against_configured_dim_still_errors() {
    let dir = TempDir::new().expect("tempdir");
    let substrate = dir.path().join("sub.db");
    let graph = dir.path().join("graph.db");

    let mut cfg = MemoryConfig::default();
    cfg.embedding.dimensions = 1024;
    cfg.embedding.model = "bge-large".into();

    let mut mem = Memory::new(substrate.to_str().unwrap(), Some(cfg))
        .expect("Memory::new")
        .with_graph_store(&graph)
        .expect("with_graph_store");

    let id = Uuid::new_v4();
    let mut e = Entity::new(id, "WrongDim".to_string(), EntityKind::Person, Utc::now());
    // Deliberately wrong: 768d blob against 1024d-configured store.
    e.embedding = Some(vec![0.1f32; 768]);

    let mut store = mem.graph_mut().with_namespace("default");
    let result = store.insert_entity(&e);
    assert!(
        result.is_err(),
        "insert_entity with mismatched dim should fail — dim check must \
         still be active, just bound to the configured value"
    );
}
