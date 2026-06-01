//! ISS-204 manual DB-verify probe — confirm the OccurredOn event-time edge
//! flows end-to-end:  MemoryRecord (temporal=Day) -> resolve_for_backfill
//! -> GraphDelta.edges -> apply_graph_delta -> graph_edges SQL dump, with a
//! traversable `<subject> --occurred_on--> <ISO day>` literal edge carrying
//! non-NULL source_memory_id.
//!
//! The unit tests in stage_edge_extract.rs already prove Component 1 emits
//! the right *draft*. This probe closes the remaining gap: that the draft
//! survives resolution + persist into a real SQLite graph with provenance.
//!
//! Run:
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   cargo run -p engramai --example iss204_probe
//!
//! Deterministic: StubTriples (no LLM), default_embedder (identity), and a
//! fresh on-disk DB. No Ollama / Anthropic dependency.

use std::sync::{Arc, Mutex as StdMutex};

use chrono::{NaiveDate, TimeZone, Utc};

use engramai::dimensions::TemporalMark;
use engramai::entities::{EntityConfig, EntityExtractor};
use engramai::graph::edge::EdgeEnd;
use engramai::graph::schema::Predicate as GraphPredicate;
use engramai::graph::store::{GraphWrite, SqliteGraphStore};
use engramai::graph::{init_graph_tables, CanonicalPredicate};
use engramai::resolution::default_embedder;
use engramai::resolution::pipeline::{PipelineConfig, ResolutionPipeline};
use engramai::storage::Storage;
use engramai::triple::{Predicate, Triple};
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};

// ---- A no-op MemoryReader: resolve_for_backfill takes &MemoryRecord and
//      never hits the reader. ----
struct NullReader;
impl engramai::resolution::pipeline::MemoryReader for NullReader {
    fn fetch(
        &self,
        _id: &str,
    ) -> Result<Option<(MemoryRecord, String)>, engramai::resolution::pipeline::MemoryReadError>
    {
        Ok(None)
    }
}

// ---- A trivial triple extractor returning a fixed triple set. ----
struct StubTriples(Vec<Triple>);
impl engramai::triple_extractor::TripleExtractor for StubTriples {
    fn extract_triples(
        &self,
        _content: &str,
    ) -> Result<Vec<Triple>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.0.clone())
    }
}

const MUSEUM_MEM_ID: &str = "iss204-museum-mem";

fn museum_memory() -> MemoryRecord {
    let mut mem = MemoryRecord {
        id: MUSEUM_MEM_ID.into(),
        content: "Melanie took her kids to the museum (2023-07-05)".into(),
        memory_type: MemoryType::Episodic,
        layer: MemoryLayer::Working,
        created_at: Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 1.0,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss204_probe".into(),
        metadata: None,
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
    };
    // Canonical `dimensions.temporal` tagged-enum layout that
    // derived_temporal_mark() reads. Day(2023-07-05) -> day precision.
    let mark = TemporalMark::Day(NaiveDate::from_ymd_opt(2023, 7, 5).unwrap());
    let temporal = serde_json::to_value(&mark).unwrap();
    mem.metadata = Some(serde_json::json!({
        "dimensions": { "temporal": temporal }
    }));
    mem
}

fn main() {
    println!("=== ISS-204 probe: OccurredOn event-time edge end-to-end ===\n");

    // Fresh on-disk DB. Storage::with_unified_substrate runs all migrations
    // (incl. migrate_unified_nodes + graph tables) so the file has the full
    // schema. We then open our OWN raw connection to the same file and leak
    // it (mirrors production memory.rs `Box::leak`) to get a 'static store
    // the pipeline can hold in Arc<Mutex<_>>.
    let tmp = std::env::temp_dir().join(format!("iss204_probe_{}.db", std::process::id()));
    let db_path = tmp.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&tmp);
    {
        // Run migrations, then drop the Storage handle (file persists).
        let _storage =
            Storage::with_unified_substrate(&db_path, true).expect("init unified storage schema");
    }

    let mem = museum_memory();
    let triples = vec![Triple::new(
        "Melanie".into(),
        Predicate::RelatedTo,
        "museum".into(),
        0.9,
    )];

    // Owned 'static connection for the pipeline's graph store.
    let conn = {
        let c = rusqlite::Connection::open(&db_path).expect("open graph conn");
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        init_graph_tables(&c).expect("ensure graph tables"); // idempotent
        Box::leak(Box::new(c))
    };
    // Seed the museum memory's `nodes` row so the edge's source_memory_id FK
    // resolves (ISS-197/199: graph_edges.memory_id FK -> nodes(id)). Real
    // ingest always projects the memory into nodes before edge persist.
    seed_node_row(conn, MUSEUM_MEM_ID, &mem.content);

    let store: SqliteGraphStore<'static> = SqliteGraphStore::new(conn).with_namespace("default");
    let store_arc = Arc::new(StdMutex::new(store));

    let pipeline = ResolutionPipeline::new(
        Arc::new(NullReader),
        Arc::new(EntityExtractor::new(&EntityConfig::default())),
        Arc::new(StubTriples(triples)),
        Arc::clone(&store_arc),
        default_embedder(),
        PipelineConfig::default(),
    );

    // --- Leg 1: resolve_for_backfill, inspect GraphDelta.edges ---
    let delta = pipeline
        .resolve_for_backfill(&mem, "default")
        .expect("resolve_for_backfill ok");

    println!(
        "--- Leg 1: GraphDelta.edges ({} total) ---",
        delta.edges.len()
    );
    let mut occurred_on = Vec::new();
    for e in &delta.edges {
        let obj = match &e.object {
            EdgeEnd::Entity { id } => format!("Entity({id})"),
            EdgeEnd::Literal { value } => format!("Literal({value})"),
        };
        println!(
            "  predicate={:?} subject_id={} object={} memory_id={:?} valid_from={:?}",
            e.predicate, e.subject_id, obj, e.memory_id, e.valid_from
        );
        if matches!(
            &e.predicate,
            GraphPredicate::Canonical(CanonicalPredicate::OccurredOn)
        ) {
            occurred_on.push(e.clone());
        }
    }

    assert_eq!(
        occurred_on.len(),
        1,
        "expected exactly one OccurredOn edge, got {}",
        occurred_on.len()
    );
    let oo = &occurred_on[0];
    match &oo.object {
        EdgeEnd::Literal { value } => {
            assert_eq!(value.as_str(), Some("2023-07-05"), "literal day mismatch");
        }
        other => panic!("OccurredOn object must be Literal, got {other:?}"),
    }
    assert_eq!(
        oo.memory_id.as_deref(),
        Some(MUSEUM_MEM_ID),
        "OccurredOn edge memory_id must be the museum memory"
    );
    assert_eq!(
        oo.valid_from, None,
        "valid_from must be None (event-time != fact-validity write-clock)"
    );
    println!(
        "\n  OK: exactly 1 OccurredOn edge, object=Literal(\"2023-07-05\"), \
         memory_id={MUSEUM_MEM_ID}, valid_from=None\n"
    );

    // --- Leg 2: apply the delta, SQL-dump graph_edges ---
    {
        let mut store = store_arc.lock().unwrap();
        let report = store
            .apply_graph_delta(&delta)
            .expect("apply_graph_delta ok");
        println!(
            "--- Leg 2: applied delta (entities_upserted={}, edges_inserted={}) ---",
            report.entities_upserted, report.edges_inserted
        );
    }

    // Fresh read connection to SQL-dump the persisted edge.
    let read = rusqlite::Connection::open(&db_path).expect("open read conn");
    dump_occurred_on_edges(&read);

    let _ = std::fs::remove_file(&tmp);
}

/// Insert a minimal `nodes` row so the graph_edges.memory_id FK resolves.
fn seed_node_row(conn: &rusqlite::Connection, id: &str, content: &str) {
    let now = Utc::now().timestamp() as f64;
    conn.execute(
        "INSERT OR IGNORE INTO nodes (id, node_kind, content, created_at, updated_at) \
         VALUES (?1, 'memory', ?2, ?3, ?3)",
        rusqlite::params![id, content, now],
    )
    .expect("seed nodes row");
}

/// SQL-dump every OccurredOn edge to confirm the traversable path with
/// non-NULL source_memory_id (provenance).
fn dump_occurred_on_edges(conn: &rusqlite::Connection) {
    println!("\n--- graph_edges OccurredOn rows ---");
    let mut stmt = conn
        .prepare(
            "SELECT e.predicate_label, ge.canonical_name, e.object_literal, e.memory_id \
             FROM graph_edges e \
             LEFT JOIN graph_entities ge ON ge.id = e.subject_id \
             WHERE e.predicate_label LIKE '%ccurred%'",
        )
        .expect("prepare dump");
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .expect("query dump");
    let mut count = 0;
    for r in rows {
        let (pred, subj, obj, mem_id) = r.expect("row");
        println!(
            "  predicate_label={pred} subject={subj:?} object_literal={obj:?} memory_id={mem_id:?}"
        );
        assert!(
            mem_id.is_some(),
            "source provenance (memory_id) must be non-NULL for OccurredOn edge"
        );
        count += 1;
    }
    assert!(count >= 1, "expected >=1 OccurredOn edge in graph_edges");
    println!("\n=== PASS: {count} OccurredOn edge(s) persisted with non-NULL provenance ===");
}
