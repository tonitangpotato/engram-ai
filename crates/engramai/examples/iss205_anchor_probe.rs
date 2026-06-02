//! ISS-205 anchor-resolution probe — diagnose why `GraphEntityResolver`
//! does NOT surface the query SUBJECT entity ('Caroline') into the
//! top-5 anchors for conv-26 q0 ("When did Caroline go to the LGBTQ
//! support group?").
//!
//! probe5 trace showed the 5 anchors were Go/group/support/LGBTQ support
//! group/support group — all occurred_on_edges=0, Caroline absent. Yet the
//! 'caroline' alias exists and points to the Caroline canonical node which
//! owns 31 OccurredOn edges. This probe runs the REAL resolver against the
//! REAL DB and dumps EVERY anchor it returns with match_strength, so we can
//! see whether Caroline is (a) never produced, or (b) produced but ranked
//! below max_anchors=5.
//!
//! Read-only, deterministic, no LLM.
//!
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   ISS205_DB_PATH=/var/folders/.../T/.tmpK8lZyN/substrate.db \
//!     cargo run -p engramai --example iss205_anchor_probe

use engramai::graph::store::{GraphRead, SqliteGraphStore};
use engramai::retrieval::adapters::graph_entity_resolver::GraphEntityResolver;
use engramai::retrieval::plans::factual::EntityResolver;

const QUERY: &str = "When did Caroline go to the LGBTQ support group?";

fn default_db() -> String {
    std::env::var("ISS205_DB_PATH").unwrap_or_else(|_| {
        "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpK8lZyN/substrate.db".into()
    })
}

fn main() {
    println!("=== ISS-205 anchor-resolution probe ===\n");
    let db_path = default_db();
    println!("DB: {db_path}");
    println!("Query: {QUERY:?}\n");

    let mut conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open DB read-only");

    let store: SqliteGraphStore<'_> = SqliteGraphStore::new(&mut conn)
        .with_namespace("default")
        .with_unified_substrate(true);

    // Sanity: which namespaces does the store see?
    let namespaces = store.list_namespaces().expect("list_namespaces");
    println!("namespaces: {namespaces:?}\n");

    let resolver = GraphEntityResolver::new(&store);
    let anchors = resolver.resolve(QUERY);

    println!("resolver returned {} anchor(s):", anchors.len());
    let mut caroline_found = false;
    for (i, a) in anchors.iter().enumerate() {
        let is_caroline = a.canonical_name.eq_ignore_ascii_case("caroline");
        if is_caroline {
            caroline_found = true;
        }
        let marker = if is_caroline { "  <== CAROLINE" } else { "" };
        let in_top5 = if i < 5 { "" } else { "  (BELOW max_anchors=5)" };
        println!(
            "  [{i:>2}] strength={:.4}  {}  ({}){marker}{in_top5}",
            a.match_strength,
            a.canonical_name,
            &a.entity_id.to_string()[..8],
        );
    }

    println!(
        "\nCaroline present in resolver output: {caroline_found}"
    );
    if caroline_found {
        println!(
            "=> resolver DOES produce Caroline; bug is RANK/TRUNCATION (max_anchors=5)."
        );
    } else {
        println!(
            "=> resolver does NOT produce Caroline; bug is MENTION-EXTRACTION or SEARCH_CANDIDATES."
        );
    }
}
