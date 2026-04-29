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
//! 4. For each Q: `graph_query_locked(GraphQuery::new(question).with_limit(5).with_namespace(ns))`.
//!    (ISS-056: namespace must be threaded through or every query hits `"default"`.)
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
//!   --limit 5 \
//!   --ns conv26
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
    /// Namespace to scope retrieval against (ISS-056). Defaults to
    /// `"default"` if unset, matching pre-ISS-056 single-tenant
    /// behavior. For conv-26 data ingested under `--ns conv26`, you
    /// MUST pass `--ns conv26` here too — otherwise every query
    /// hits `default` and returns 0 hits.
    namespace: Option<String>,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut db = None;
    let mut graph_db = None;
    let mut dataset = None;
    let mut max_session = 3u32;
    let mut limit = 5usize;
    let mut namespace: Option<String> = None;

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
            "--ns" | "--namespace" => {
                namespace = Some(iter.next().ok_or("--ns needs value")?)
            }
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }
    Ok(Args {
        db: db.ok_or("--db required")?,
        graph_db: graph_db.ok_or("--graph-db required")?,
        dataset: dataset.ok_or("--dataset required")?,
        max_session,
        limit,
        namespace,
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

    // ISS-049-followup: per-plan stats so we can attribute hits/empties
    // to specific plan paths. Distinguishes "Abstract was never selected"
    // from "Abstract was selected but produced 0 candidates" — the
    // server-side `engramai::retrieval` log lines complement this with
    // the actual plan_kind dispatched (which may differ from `plan_used`
    // after a downgrade).
    use std::collections::BTreeMap;
    let mut per_plan_total: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_plan_hits: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_plan_empty: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_outcome: BTreeMap<String, usize> = BTreeMap::new();

    // Phase-2 (2026-04-29): per-category breakdown.
    // Categories (verified against locomo10.json + gemini_utils.py prompts):
    //   1=Multi-hop, 2=Temporal, 3=Open-ended, 4=Single-hop, 5=Adversarial.
    // Cat=5 hits are surfaced separately because "hit" semantics differ:
    // for cat 1-4 a hit means the gold dialog was retrieved (= correctness signal);
    // for cat=5 the gold answer is "unanswerable" — surfacing the dialog is a
    // necessary precondition for answering correctly but NOT sufficient,
    // so cat=5 hit-rate must not be averaged into the headline number.
    let mut per_cat_total: BTreeMap<u64, usize> = BTreeMap::new();
    let mut per_cat_hits: BTreeMap<u64, usize> = BTreeMap::new();

    for (idx, q) in qas.iter().enumerate() {
        let question = q["question"].as_str().unwrap_or("");
        let gold_answer = q["answer"].as_str().unwrap_or("");
        let evidence: HashSet<String> = q["evidence"].as_array()
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let category = q["category"].as_u64().unwrap_or(0);

        let mut query = GraphQuery::new(question).with_limit(args.limit);
        if let Some(ns) = &args.namespace {
            query = query.with_namespace(ns.clone());
        }
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
        let plan_label = format!("{:?}", resp.plan_used);
        let outcome_label = resp.outcome.slug().to_string();

        *per_plan_total.entry(plan_label.clone()).or_insert(0) += 1;
        if hit {
            *per_plan_hits.entry(plan_label.clone()).or_insert(0) += 1;
        }
        if resp.results.is_empty() {
            *per_plan_empty.entry(plan_label.clone()).or_insert(0) += 1;
        }
        *per_outcome.entry(outcome_label.clone()).or_insert(0) += 1;

        *per_cat_total.entry(category).or_insert(0) += 1;
        if hit {
            *per_cat_hits.entry(category).or_insert(0) += 1;
        }

        println!(
            "[{:>2}/{}] {} cat={} hit={} plan={} outcome={} got={} | gold={:?} top={:?}",
            idx + 1, qas.len(), mark, category, hit, plan_label, outcome_label, resp.results.len(),
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

    // Phase-2 per-category breakdown. Cat=5 (Adversarial) is reported separately
    // because retrieval-hit ≠ correctness for adversarial questions: the "right"
    // answer is "unanswerable", so surfacing the gold dialog is a necessary
    // precondition but the LLM must still recognize the question is unanswerable.
    println!();
    println!("=== Per-category breakdown ===");
    let cat_label = |c: u64| -> &'static str {
        match c {
            1 => "Multi-hop",
            2 => "Temporal",
            3 => "Open-ended",
            4 => "Single-hop",
            5 => "Adversarial",
            _ => "?",
        }
    };
    let mut headline_total = 0usize;
    let mut headline_hits = 0usize;
    for (cat, n) in &per_cat_total {
        let h = per_cat_hits.get(cat).copied().unwrap_or(0);
        let pct = 100.0 * h as f64 / (*n).max(1) as f64;
        let note = if *cat == 5 {
            "  ← retrieval hit ≠ correctness (gold answer is 'unanswerable')"
        } else {
            ""
        };
        println!("  cat={} {:<12} n={:<3} hits={:<3} ({:>5.1}%){}",
                 cat, cat_label(*cat), n, h, pct, note);
        if *cat != 5 {
            headline_total += n;
            headline_hits += h;
        }
    }
    if per_cat_total.contains_key(&5) {
        let pct = 100.0 * headline_hits as f64 / headline_total.max(1) as f64;
        println!();
        println!("  Headline hit@{} (cat 1-4 only): {}/{} = {:.1}%",
                 args.limit, headline_hits, headline_total, pct);
    }

    println!();
    println!("=== Per-plan breakdown ===");
    println!("  (plan_used = the intent surfaced post-execution; may differ from dispatched plan_kind after downgrade)");
    for (plan, n) in &per_plan_total {
        let h = per_plan_hits.get(plan).copied().unwrap_or(0);
        let e = per_plan_empty.get(plan).copied().unwrap_or(0);
        let pct = if *n > 0 { 100.0 * h as f64 / *n as f64 } else { 0.0 };
        println!(
            "  {:<14} n={:<3} hits={:<3} ({:>5.1}%) empty={}",
            plan, n, h, pct, e,
        );
    }

    println!();
    println!("=== Per-outcome breakdown ===");
    println!("  (RetrievalOutcome slug; non-Ok values indicate plan downgrades or missing substrate)");
    for (oc, n) in &per_outcome {
        println!("  {:<28} n={}", oc, n);
    }
    println!();
    println!("Tip: rerun with `RUST_LOG=engramai::retrieval=info` (or =debug for classifier scores) to see per-query plan_kind dispatch + sub-plan fan-out.");
    println!("     This distinguishes (a) plan never dispatched, (b) dispatched + 0 candidates, (c) dispatched + downgraded.");

    Ok(())
}
