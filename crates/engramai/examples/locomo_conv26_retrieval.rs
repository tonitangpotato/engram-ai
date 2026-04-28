#![allow(clippy::too_many_lines)]
//! LoCoMo conv-26 sessions 1-3 retrieval smoke driver.
//!
//! Post commit `d991715` (retrieval cognitive state readback + T9
//! PipelineRecordProcessor). First end-to-end test of the v0.3
//! `graph_query_locked` orchestrator on real LoCoMo data.
//!
//! # Pipeline
//! 1. Open existing v0.2 ingested DB (from `01_ingest.py`).
//! 2. Install graph store via `with_graph_store` (post-migration path).
//! 3. Load gold QAs from `locomo10.json` whose evidence falls in
//!    sessions 1-3 (27 questions per audit).
//! 4. For each Q: `graph_query_locked(GraphQuery::new(question).with_limit(5))`.
//! 5. Score: did any returned memory's `source` match a gold evidence
//!    `dia_id`? (Session-level recall@5, since LoCoMo evidence is
//!    diaglogue-turn-level and we ingested per-turn.)
//!
//! # Usage
//! ```bash
//! cargo run --example locomo_conv26_retrieval --release -- \
//!   --db /Users/potato/clawd/projects/engram/.gid/issues/_smoke-locomo-2026-04-27/locomo-conv26-s1-3-postd991715.db \
//!   --graph-db /Users/potato/clawd/projects/engram/.gid/issues/_smoke-locomo-2026-04-27/locomo-conv26-s1-3-postd991715.graph.db \
//!   --dataset /Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json \
//!   --max-session 3 \
//!   --limit 5
//! ```

use engramai::Memory;
use engramai::retrieval::api::{GraphQuery, ScoredResult};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    db: PathBuf,
    graph_db: PathBuf,
    dataset: PathBuf,
    max_session: u32,
    limit: usize,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut db = None;
    let mut graph_db = None;
    let mut dataset = None;
    let mut max_session = 3u32;
    let mut limit = 5usize;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--db" => db = Some(PathBuf::from(iter.next().ok_or("--db needs value")?)),
            "--graph-db" => {
                graph_db = Some(PathBuf::from(iter.next().ok_or("--graph-db needs value")?))
            }
            "--dataset" => dataset = Some(PathBuf::from(iter.next().ok_or("--dataset needs value")?)),
            "--max-session" => {
                max_session = iter.next().ok_or("--max-session needs value")?.parse()?
            }
            "--limit" => limit = iter.next().ok_or("--limit needs value")?.parse()?,
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }
    Ok(Args {
        db: db.ok_or("--db required")?,
        graph_db: graph_db.ok_or("--graph-db required")?,
        dataset: dataset.ok_or("--dataset required")?,
        max_session,
        limit,
    })
}

/// Parse `D{n}:{m}` → session number `n`. Returns None on malformed input.
fn evidence_session(dia_id: &str) -> Option<u32> {
    // "D2:5" → "2"
    let stripped = dia_id.strip_prefix('D')?;
    let (sess, _turn) = stripped.split_once(':')?;
    sess.parse().ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let args = parse_args()?;

    println!("=== LoCoMo conv-26 retrieval smoke ===");
    println!("  DB:        {}", args.db.display());
    println!("  Graph DB:  {} (will be created)", args.graph_db.display());
    println!("  Dataset:   {}", args.dataset.display());
    println!("  Sessions:  1..={}", args.max_session);
    println!("  Top-K:     {}", args.limit);
    println!();

    // 1. Load dataset, extract conv-26 + filter QAs
    let raw = std::fs::read_to_string(&args.dataset)?;
    let data: Value = serde_json::from_str(&raw)?;
    let conv = data.as_array().ok_or("dataset not an array")?
        .iter()
        .find(|c| c["sample_id"].as_str() == Some("conv-26"))
        .ok_or("conv-26 not found")?;

    let qa_all = conv["qa"].as_array().ok_or("qa missing")?;
    let qas: Vec<&Value> = qa_all.iter()
        .filter(|q| {
            let evidence = q["evidence"].as_array();
            match evidence {
                Some(ev) if !ev.is_empty() => ev.iter().all(|e| {
                    e.as_str()
                        .and_then(evidence_session)
                        .map(|s| s <= args.max_session)
                        .unwrap_or(false)
                }),
                _ => false,
            }
        })
        .collect();
    println!("Loaded {} QAs (out of {}) with evidence in sessions 1..={}",
             qas.len(), qa_all.len(), args.max_session);

    // 2. Open Memory + install graph store (creates fresh graph layer if missing)
    let mem = Memory::new(args.db.to_str().unwrap(), None)?
        .with_graph_store(&args.graph_db)?;
    println!("Opened memory + installed graph store");
    println!();

    // 3. Run each query (synchronous busy-poll matches engramai test pattern —
    // graph_query futures don't await IO today, they return Ready on first poll)
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

    let mut hits = 0usize;
    let mut empty_results = 0usize;
    let mut total = 0usize;

    for (idx, q) in qas.iter().enumerate() {
        let question = q["question"].as_str().unwrap_or("");
        let gold_answer = q["answer"].as_str().unwrap_or("");
        let evidence: HashSet<String> = q["evidence"].as_array()
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let category = q["category"].as_u64().unwrap_or(0);

        let query = GraphQuery::new(question).with_limit(args.limit);
        let resp = block_on(mem.graph_query_locked(query))?;
        total += 1;

        if resp.results.is_empty() {
            empty_results += 1;
        }

        // Score: any returned record whose `source` is "locomo/conv-26/D{x}:{y}"
        // matches a gold evidence dia_id?
        let mut hit = false;
        let mut top_sources: Vec<String> = Vec::new();
        for r in &resp.results {
            if let ScoredResult::Memory { record, .. } = r {
                // record.source format: "locomo/conv-26/D{x}:{y}"
                let dia_id = record.source.rsplit('/').next().unwrap_or("");
                top_sources.push(dia_id.to_string());
                if evidence.contains(dia_id) {
                    hit = true;
                }
            }
        }
        if hit {
            hits += 1;
        }

        // Print compact result row
        let q_short: String = question.chars().take(70).collect();
        let mark = if hit { "✓" } else if resp.results.is_empty() { "∅" } else { "✗" };
        println!(
            "[{:>2}/{}] {} cat={} hit={} plan={:?} got={} | gold={:?} top={:?}",
            idx + 1, qas.len(), mark, category, hit, resp.plan_used, resp.results.len(),
            evidence.iter().take(2).collect::<Vec<_>>(), top_sources.iter().take(2).collect::<Vec<_>>(),
        );
        let _ = gold_answer;
        let _ = q_short;
    }

    println!();
    println!("=== Summary ===");
    println!("  Total queries:    {}", total);
    println!("  Hits @ {}:         {} ({:.1}%)", args.limit, hits, 100.0 * hits as f64 / total.max(1) as f64);
    println!("  Empty results:    {} ({:.1}%)", empty_results, 100.0 * empty_results as f64 / total.max(1) as f64);

    Ok(())
}
