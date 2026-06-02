//! ISS-208 decisive probe — does `edges_of(entity, OccurredOn, false)`
//! return the full dated-edge count through the REAL `SqliteGraphStore`
//! handle, or the undercount-of-1 that probe5 logged?
//!
//! Raw sqlite3 ground truth on the forensic DB (.tmpK8lZyN):
//!   Caroline d7f9a67a-... → 31 OccurredOn edges (structural/default, all live)
//!   Melanie  074d5075-... → 17 OccurredOn edges (structural/default, all live)
//!
//! probe5's reverted eprintln in factual.rs logged `occurred_on_edges=1`
//! uniformly for every dense anchor. This probe calls the SAME
//! `GraphRead::edges_of` the reservation uses, with the SAME store config
//! (namespace=default, unified_substrate=true), and prints the count.
//!
//!   len == 31/17  => probe5 '1' was a measurement artifact (close ISS-208)
//!   len == 1      => real runtime edges_of defect (proceed to Step-1 split)
//!
//! Run:
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   cargo run -p engramai --example iss208_edges_of_probe
//!
//! Read-only, deterministic, no LLM.

use engramai::graph::edge::Edge;
use engramai::graph::store::{GraphRead, SqliteGraphStore};
use engramai::graph::{CanonicalPredicate, Predicate};
use uuid::Uuid;

const CAROLINE: &str = "d7f9a67a-5194-480f-85b2-8a3e09069b15";
const MELANIE: &str = "074d5075-516b-4168-9088-5ce9f84ae624";

fn default_db() -> String {
    std::env::var("ISS208_DB_PATH").unwrap_or_else(|_| {
        "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpK8lZyN/substrate.db".into()
    })
}

fn probe_one(conn: &mut rusqlite::Connection, label: &str, id: &str, expected: usize) {
    let node = Uuid::parse_str(id).expect("parse uuid");
    let store: SqliteGraphStore<'_> = SqliteGraphStore::new(conn)
        .with_namespace("default")
        .with_unified_substrate(true);
    let edges: Vec<Edge> = store
        .edges_of(
            node,
            Some(&Predicate::Canonical(CanonicalPredicate::OccurredOn)),
            false,
        )
        .expect("edges_of OccurredOn");
    let got = edges.len();
    let verdict = if got == expected {
        "MATCH (full count — probe5 '1' was a measurement artifact)"
    } else if got == 1 {
        "UNDERCOUNT=1 (real runtime edges_of defect)"
    } else {
        "PARTIAL (neither full nor 1 — investigate)"
    };
    println!(
        "{label}: edges_of(OccurredOn) returned {got} (raw-sql expects {expected}) => {verdict}"
    );
    // Dump the first few so we can see what edges_of actually returns.
    for (i, e) in edges.iter().take(5).enumerate() {
        println!(
            "    [{i}] date={:?} mid={:?} recorded_at={:?}",
            e.object_literal_date(),
            e.memory_id.as_ref().map(|m| &m[..m.len().min(8)]),
            e.recorded_at,
        );
    }
}

fn main() {
    let db_path = default_db();
    println!("=== ISS-208 edges_of probe ===");
    println!("DB: {db_path}\n");

    let mut conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open DB read-only");

    probe_one(&mut conn, "Caroline", CAROLINE, 31);
    probe_one(&mut conn, "Melanie", MELANIE, 17);
}
