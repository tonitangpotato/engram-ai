//! Smoke test: does a 384-dim entity embedding actually persist into a
//! storage with default 768-dim? Reproduces the RUN-0008 finding
//! (embeddings end up NULL in DB despite f95480b's "embedding propagation"
//! fix).

use chrono::Utc;
use engramai::graph::entity::{Entity, EntityKind};
use engramai::graph::store::{GraphWrite, SqliteGraphStore};
use engramai::graph::init_graph_tables;
use uuid::Uuid;

#[test]
fn entity_with_384_dim_embedding_into_default_768_store_smoke() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT NOT NULL);",
    )
    .unwrap();
    init_graph_tables(&conn).expect("init graph tables");
    let mut store = SqliteGraphStore::new(&mut conn).with_namespace("smoke");
    // Default embedding_dim = 768 (DEFAULT_ENTITY_EMBEDDING_DIM).
    // Resolution pipeline produces 384 (DEFAULT_EMBEDDING_DIM).

    let id = Uuid::new_v4();
    let mut e = Entity::new(id, "Alice".to_string(), EntityKind::Person, Utc::now());
    e.embedding = Some(vec![0.1f32; 384]);

    let result = store.insert_entity(&e);
    eprintln!("insert_entity (384-dim into 768-dim store) result: {:?}", result);

    if let Err(err) = result {
        // Expected by code reading. If we land here, RUN-0008 ingest
        // shouldn't have produced 113 entities — yet it did. Mystery to
        // dig deeper.
        eprintln!("HARD ERROR — pipeline would abort if this happened in production");
        eprintln!("  err = {:?}", err);
        // Don't fail the test — we want to learn what production does
        return;
    }

    // Read back what landed.
    drop(store);
    let row: (Option<i64>, String) = conn.query_row(
        "SELECT length(embedding), CASE WHEN embedding IS NULL THEN 'NULL' ELSE 'BLOB' END FROM graph_entities WHERE id = ?1",
        rusqlite::params![id.as_bytes().to_vec()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    eprintln!("landed: len={:?} kind={}", row.0, row.1);
}
