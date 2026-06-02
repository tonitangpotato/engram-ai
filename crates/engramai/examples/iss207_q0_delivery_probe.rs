//! ISS-205→208 end-to-end DELIVERY probe for conv-26 q0.
//!
//! All five issues in the q0 temporal-retrieval chain are resolved in
//! code:
//!   - ISS-204: occurred_on edges materialised on the entity node.
//!   - ISS-205: date-asking temporal reservation admits the gold dated
//!     episode into the Factual seed pool (privilege band).
//!   - ISS-206: the resolved date `2023-05-07` surfaces on the gold
//!     generator line (ISS-190/191 surfacing path, verify-only).
//!   - ISS-207: hybrid factual sub-plan orders by (privilege-tier,
//!     breadth-desc, id) so the reserved gold row keeps its top slot
//!     through fusion instead of being collapsed to memory_id order.
//!   - ISS-208: edges_of was never undercounting (closed not-a-bug).
//!
//! The isolated probes proved each piece. THIS probe proves they
//! COMPOSE: it drives the REAL retrieval orchestrator (the exact
//! `Memory::graph_query_locked` path the bench uses) for the q0 query
//! string against the forensic DB, under the locked ISS-190 envelope
//! with temporal reservation ON, and reports whether the gold dated
//! episode lands in the final top-10 — and whether its surfaced
//! generator line carries `[2023-05-07]`.
//!
//! Gold (conv-26 q0): node `a838a102` "Caroline attended a LGBTQ
//! support group", temporal {kind:day, value:2023-05-07}. Its
//! occurred_on edge hangs off the UPPERCASE Caroline entity
//! `d7f9a67a` (owns all 31 occurred_on edges); the lowercase
//! mention-node `1d11ce4c` owns zero. Anchor resolution must land on
//! `d7f9a67a` for reservation to fire — the `iss205_anchor_probe`
//! confirms it now resolves at rank 0 strength 1.0.
//!
//! PASS  => gold a838a102 in top-10  => retrieval DELIVERY works; the
//!          residual q0 gate is generation, not retrieval. With the
//!          surfaced `[2023-05-07]` line in context, q0 should flip 0→1.
//! FAIL  => gold absent from top-10  => delivery still broken; the
//!          reservation/ordering fix does not compose end-to-end.
//!
//! Requires Ollama up (local nomic-embed-text, deterministic) to embed
//! the query text. Read-only on the DB. No answer-generation LLM call.
//!
//! Run:
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   cargo run -p engramai --example iss207_q0_delivery_probe
//!   # override DB: ISS207_DB_PATH=/path/to/substrate.db cargo run ...

use engramai::retrieval::api::{GraphQuery, ScoredResult};
use engramai::{Memory, MemoryConfig};

const QUERY: &str = "When did Caroline go to the LGBTQ support group?";
const GOLD_PREFIX: &str = "a838a102";
const GOLD_DATE: &str = "2023-05-07";
const TOP_K: usize = 10;
const RESERVATION: usize = 5;

fn default_db() -> String {
    std::env::var("ISS207_DB_PATH").unwrap_or_else(|_| {
        "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpK8lZyN/substrate.db".into()
    })
}

/// Mirror of engram-bench `derived_temporal_value`: reads
/// /engram/dimensions/temporal/value from the record metadata, so the
/// probe reports the SAME surfaced line the bench generator would build.
fn derived_temporal_value(meta: &Option<serde_json::Value>) -> Option<String> {
    let v = meta
        .as_ref()?
        .pointer("/engram/dimensions/temporal/value")
        .and_then(|v| v.as_str())?
        .trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn main() {
    println!("=== ISS-205→208 q0 end-to-end delivery probe ===\n");
    let db = default_db();
    println!("DB:    {db}");
    println!("Query: {QUERY:?}");
    println!("Envelope: top_k={TOP_K} temporal_reservation={RESERVATION} (locked ISS-190)\n");

    // Unified-substrate reads (the forensic DB was ingested single-file
    // with unified on — matches the bench `fresh_in_memory_db` default).
    let mut cfg = MemoryConfig::default();
    cfg.unified_substrate = true;

    let memory = Memory::new(&db, Some(cfg))
        .expect("Memory::new on forensic DB")
        // Co-locate the read-side graph store on the same file
        // (single-file mode) so entity/edge reads hit the unified
        // nodes/edges — exactly what the bench does post-ISS-195.
        .with_graph_store(&db)
        .expect("with_graph_store on forensic DB");

    // Locked envelope: the bench builds this same GraphQuery (limit +
    // temporal_reservation; mmr/k_seed/bm25/cross_encoder/entity_channel/
    // factual_reweight/populate all default-None → locked defaults).
    let gq = GraphQuery::new(QUERY)
        .with_limit(TOP_K)
        .with_temporal_reservation(Some(RESERVATION))
        .with_explain(true);

    // Synchronous busy-poll — matches the engramai test/example pattern
    // (graph_query futures don't await IO today; they return Ready on
    // first poll). Avoids pulling a runtime into the example.
    fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
        use std::pin::Pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};
        struct NoopWaker;
        impl Wake for NoopWaker {
            fn wake(self: Arc<Self>) {}
        }
        let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
        let waker = Waker::from(Arc::new(NoopWaker));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                return out;
            }
        }
    }

    let resp = block_on(memory.graph_query_locked(gq)).expect("graph_query_locked");

    println!("plan_used: {:?}", resp.plan_used);
    println!("outcome:   {:?}\n", resp.outcome);
    println!("--- top-{} results ---", resp.results.len());

    let mut gold_rank: Option<usize> = None;
    let mut gold_line: Option<String> = None;

    for (i, r) in resp.results.iter().enumerate() {
        match r {
            ScoredResult::Memory {
                record, score, ..
            } => {
                let id = record.id.to_string();
                let short = &id[..id.len().min(12)];
                let content: String =
                    record.content.chars().take(52).collect();
                let when = derived_temporal_value(&record.metadata);
                let line = match &when {
                    Some(w) => format!("[{w}] {content}"),
                    None => content.clone(),
                };
                let mark = if id.starts_with(GOLD_PREFIX) {
                    gold_rank = Some(i);
                    gold_line = Some(line.clone());
                    "  <== GOLD"
                } else {
                    ""
                };
                println!("  [{i:2}] score={score:.4}  {short}  {line}{mark}");
            }
            ScoredResult::Topic { topic, score, .. } => {
                println!("  [{i:2}] score={score:.4}  TOPIC  {}", topic.title);
            }
        }
    }

    println!("\n--- verdict ---");
    match gold_rank {
        Some(rank) => {
            println!("GOLD a838a102 in top-{TOP_K}: YES (rank {rank})");
            let line = gold_line.unwrap_or_default();
            let dated = line.contains(GOLD_DATE);
            println!("Surfaced generator line: {line:?}");
            println!(
                "Carries resolved date {GOLD_DATE}: {}",
                if dated { "YES" } else { "NO" }
            );
            if dated {
                println!(
                    "\nPASS — retrieval DELIVERS the dated gold episode into top-{TOP_K}.\n\
                     The generator will see {GOLD_DATE} in context; q0 should flip 0→1.\n\
                     Residual gate is generation only."
                );
            } else {
                println!(
                    "\nPARTIAL — gold delivered but date NOT surfaced on its line.\n\
                     ISS-206 surfacing did not fire on this read path — investigate."
                );
            }
        }
        None => {
            println!("GOLD a838a102 in top-{TOP_K}: NO");
            println!(
                "\nFAIL — gold absent from top-{TOP_K}. ISS-205 reservation +\n\
                 ISS-207 ordering do NOT compose end-to-end. Dump trace / pool\n\
                 to see where the reserved row is lost before truncation."
            );
        }
    }
}
