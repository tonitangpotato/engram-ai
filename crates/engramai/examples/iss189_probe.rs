//! ISS-189 empirical probe — run the Factual plan against the leaked
//! conv-44 graph.db with full plan logging, and report whether the answer
//! episode (a8b823f4) makes it into the candidate pool.
//!
//! Run:
//!   RUST_LOG=engramai::retrieval::factual=trace \
//!   cargo run --example iss189_probe --features <none needed>
//!
//! This is a throwaway diagnostic, not a test. It exists to replace
//! read-the-code guessing with ground truth: what anchors resolve, which
//! direction Stage 2 traverses, and whether the seed/scan pulls the
//! answer in.

use chrono::Utc;
use engramai::graph::store::{GraphRead, SqliteGraphStore};
use engramai::retrieval::adapters::graph_entity_resolver::GraphEntityResolver;
use engramai::retrieval::budget::{BudgetController, CostCaps, StageBudget};
use engramai::retrieval::plans::factual::{FactualPlan, FactualPlanInputs};

const DB: &str = "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpAZKa5X/graph.db";
const QUERY: &str = "Which year did Audrey adopt the first three of her dogs?";
const ANSWER: &str = "a8b823f4";

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let mut conn = rusqlite::Connection::open(DB).expect("open leaked graph.db");
    let store = SqliteGraphStore::new(&mut conn).with_namespace("default");

    let resolver = GraphEntityResolver::new(&store as &dyn GraphRead);

    let inputs = FactualPlanInputs {
        query: QUERY,
        query_time: Utc::now(),
        as_of: None,
        include_superseded: false,
        min_confidence: None,
        max_anchors: 8,
        predicate_filter: None,
        memory_limit_per_entity: 100,
        requested_k: 10,
        entity_filter: None,
    };

    let plan = FactualPlan::new();
    let mut budget = BudgetController::new(None, StageBudget::default(), CostCaps::default());

    let result = plan
        .execute(&inputs, &resolver, &store, &mut budget)
        .expect("factual plan ran");

    println!("\n========== ISS-189 PROBE RESULT ==========");
    println!("query  : {QUERY:?}");
    println!("outcome: {:?}", result.outcome);
    println!("anchors: {}", result.anchors.len());
    for a in &result.anchors {
        println!(
            "  - {} {:?} ms={:.3}",
            a.entity_id, a.canonical_name, a.match_strength
        );
    }
    println!("edges traversed (outgoing): {}", result.edges.len());
    println!("candidate pool size       : {}", result.memories.len());

    let in_pool = result.memories.iter().any(|m| m.memory_id == ANSWER);
    println!(
        "\nANSWER episode {ANSWER} in candidate pool? {}",
        if in_pool { "YES ✅" } else { "NO ❌" }
    );
    if !in_pool {
        println!("→ confirms the gap is BEFORE fusion: the Factual plan never");
        println!("  surfaces the answer. (Expected with the current outgoing-only");
        println!("  traversal — the answer lives on an INCOMING part_of edge.)");
    }
    println!("==========================================\n");
}
