//! ISS-205 manual real-DB probe — confirm the date-asking temporal
//! reservation admits the gold dated episode into the Factual seed pool.
//!
//! Scenario (real conv-26 q0): "When did Caroline go to the LGBTQ support
//! group?" carries no range (`extract_time_range` → None) but asks for a
//! date (`asks_for_date` → true). The gold answer is Caroline's EARLIEST
//! `OccurredOn` episode (2023-05-07 → mem 83cd73d8...), which the recency
//! scan evicts on a dense anchor (Caroline carries 31 OccurredOn edges).
//!
//! This probe opens the live post-ISS-204 DB, pulls Caroline's OccurredOn
//! edges through the SAME `edges_of` path the reservation uses, applies the
//! EXACT date-ascending take-R admission logic from
//! `factual.rs` (date-asking branch), and asserts the gold memory id is in
//! the admitted set for R = 1..=5. It also cross-checks the parsed dates
//! against `Edge::object_literal_date()`.
//!
//! Run (DB path defaults to the live .tmpcYbhzb snapshot; override with
//! ISS205_DB_PATH):
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   cargo run -p engramai --example iss205_probe
//!
//! Deterministic, read-only: opens the DB read-only, no LLM, no mutation.

use engramai::graph::edge::Edge;
use engramai::graph::store::{GraphRead, SqliteGraphStore};
use engramai::graph::{CanonicalPredicate, Predicate};
use uuid::Uuid;

/// Caroline's entity node in the live conv-26 DB (carries all 31
/// OccurredOn edges, per the manual DB probe).
const CAROLINE_NODE: &str = "09d75777-fd5"; // prefix; full UUID resolved below
/// The gold answer memory for q0 (LGBTQ support group, 2023-05-07).
const GOLD_MEM_PREFIX: &str = "83cd73d8";
const GOLD_DATE: &str = "2023-05-07";

fn default_db() -> String {
    std::env::var("ISS205_DB_PATH").unwrap_or_else(|_| {
        "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpcYbhzb/substrate.db".into()
    })
}

fn main() {
    println!("=== ISS-205 probe: date-asking reservation admits gold episode ===\n");

    let db_path = default_db();
    println!("DB: {db_path}\n");

    // Read-only connection to the live DB.
    let mut conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open DB read-only");

    // Resolve Caroline's full UUID from the prefix (the node carrying the
    // 31 OccurredOn edges).
    let caroline_uuid: String = conn
        .query_row(
            "SELECT DISTINCT source_id FROM edges \
             WHERE predicate='occurred_on' AND source_id LIKE ?1 \
             ORDER BY source_id LIMIT 1",
            rusqlite::params![format!("{CAROLINE_NODE}%")],
            |r| r.get(0),
        )
        .expect("resolve Caroline node uuid");
    println!("Caroline node = {caroline_uuid}");
    let caroline = Uuid::parse_str(&caroline_uuid).expect("parse Caroline uuid");

    // Pull OccurredOn edges through the SAME path the reservation uses:
    // GraphRead::edges_of(anchor, Some(OccurredOn), include_superseded=false).
    let store: SqliteGraphStore<'_> = SqliteGraphStore::new(&mut conn)
        .with_namespace("default")
        .with_unified_substrate(true);
    let dated_edges: Vec<Edge> = store
        .edges_of(
            caroline,
            Some(&Predicate::Canonical(CanonicalPredicate::OccurredOn)),
            false,
        )
        .expect("edges_of OccurredOn");
    println!("OccurredOn edges via edges_of(): {}\n", dated_edges.len());
    assert!(
        dated_edges.len() >= 31,
        "expected >=31 OccurredOn edges for Caroline, got {}",
        dated_edges.len()
    );

    // Replicate the date-asking admission: parse (date, memory_id) using the
    // production Edge::object_literal_date(), sort date-ASC ties by mem id.
    let mut dated: Vec<(chrono::NaiveDate, String)> = dated_edges
        .iter()
        .filter_map(|e| {
            let mid = e.memory_id.as_ref()?;
            let date = e.object_literal_date()?;
            Some((date, mid.clone()))
        })
        .collect();
    println!(
        "parsed (date, memory_id) pairs: {} (edges with NULL memory_id or unparseable date dropped)",
        dated.len()
    );
    assert_eq!(
        dated.len(),
        dated_edges.len(),
        "every OccurredOn edge must carry a parseable date + non-NULL memory_id"
    );

    dated.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    println!("\n--- earliest 6 dated episodes (date ASC) ---");
    for (d, mid) in dated.iter().take(6) {
        let marker = if mid.starts_with(GOLD_MEM_PREFIX) { "  <== GOLD" } else { "" };
        println!("  {d}  {}{marker}", &mid[..mid.len().min(12)]);
    }

    // The earliest episode MUST be the gold (2023-05-07 / 83cd73d8...).
    let (first_date, first_mid) = &dated[0];
    assert_eq!(
        first_date.to_string(),
        GOLD_DATE,
        "earliest dated episode must be the gold date {GOLD_DATE}"
    );
    assert!(
        first_mid.starts_with(GOLD_MEM_PREFIX),
        "earliest episode memory id must be the gold mem {GOLD_MEM_PREFIX}*, got {first_mid}"
    );

    // Admission check for R = 1..=5: gold must be in the admitted set for
    // every R (it is the earliest, so date-ASC take-R always includes it).
    println!("\n--- admission by R (date-asking, date-ASC take-R) ---");
    for r in 1..=5usize {
        let admitted: Vec<&String> = dated.iter().take(r).map(|(_, m)| m).collect();
        let gold_in = admitted.iter().any(|m| m.starts_with(GOLD_MEM_PREFIX));
        println!("  R={r}: admitted {} episode(s), gold_in_pool={gold_in}", admitted.len());
        assert!(
            gold_in,
            "gold episode must be admitted for R={r} (it is the earliest)"
        );
    }

    println!(
        "\n=== PASS: date-asking reservation admits gold mem {GOLD_MEM_PREFIX}* \
         ({GOLD_DATE}) into the Factual seed pool for all R>=1 ==="
    );
}
