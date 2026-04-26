//! Test-only helpers shared across `graph::*` and `resolution::*` test
//! modules. Compiled out of release builds (`#[cfg(test)]` at the
//! `graph::mod` use site).
//!
//! Why this lives here, not duplicated per file:
//!  - `fresh_conn` + `insert_test_entity` were originally module-private
//!    inside `graph::store::tests`. The §3.4.1 candidate-retrieval driver
//!    in `resolution::candidate_retrieval` needs the same primitives to
//!    test driver↔store integration.
//!  - Duplicating them is an engineering-integrity smell (two sources of
//!    truth on schema setup → drift waiting to happen).
//!  - Promoting them to a `pub(crate)` helper module keeps a single source.

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::graph::entity::{Entity, EntityKind};
use crate::graph::storage_graph::init_graph_tables;
use crate::graph::store::{GraphStore, SqliteGraphStore};

/// Open an in-memory connection with foreign-keys ON, the v0.2 `memories`
/// stub table that some §4.1 tables reference, and the full graph schema.
///
/// Mirrors the function originally inlined in `graph::store::tests`. If you
/// touch the schema seed, update both the migration path and this fixture.
pub fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory");
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT NOT NULL);",
    )
    .unwrap();
    init_graph_tables(&conn).expect("init graph tables");
    conn
}

/// Insert an entity with a specified `last_seen` and optional embedding
/// into the test store. Returns the persisted entity (with the same `id`
/// it was written under, so callers can match it in query output).
pub fn insert_test_entity(
    store: &mut SqliteGraphStore<'_>,
    name: &str,
    kind: EntityKind,
    last_seen: DateTime<Utc>,
    embedding: Option<Vec<f32>>,
) -> Entity {
    let mut e = Entity::new(name.into(), kind, last_seen);
    e.last_seen = last_seen;
    e.embedding = embedding;
    store.insert_entity(&e).expect("insert ok");
    e
}
