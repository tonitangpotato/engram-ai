#![allow(clippy::too_many_lines)]
//! LoCoMo conv-26 sessions 1-3 retrieval — **v0.2 baseline** driver.
//!
//! Twin of `locomo_conv26_retrieval.rs` (which uses the v0.3
//! `graph_query_locked` orchestrator). This driver instead calls the
//! pre-v0.3 `recall_from_namespace` path — FTS + embedding + ACT-R
//! hybrid, no graph orchestrator, no plan classifier, no abstract /
//! affective / hyperedge layer.
//!
//! Purpose: produce an apples-to-apples comparison so we can answer
//! "did v0.3 retrieval improve hit@5 over v0.2 on the same data?"
//!
//! Same dataset, same questions, same scoring (gold dia_id ∈ top-K
//! returned `record.source`). Only the retrieval API differs.
//!
//! # Usage
//! ```bash
//! cargo run --release --example locomo_conv26_baseline_v02 -- \
//!   --db .gid/issues/_smoke-locomo-2026-04-27/locomo-conv26-s1-3-postd991715.db \
//!   --dataset /Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json \
//!   --max-session 3 \
//!   --limit 5 \
//!   --ns locomo-conv26-postd991715
//! ```

use engramai::Memory;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    db: PathBuf,
    dataset: PathBuf,
    max_session: u32,
    limit: usize,
    namespace: Option<String>,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut db = None;
    let mut dataset = None;
    let mut max_session = 3u32;
    let mut limit = 5usize;
    let mut namespace: Option<String> = None;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--db" => db = Some(PathBuf::from(iter.next().ok_or("--db needs value")?)),
            "--dataset" => {
                dataset = Some(PathBuf::from(iter.next().ok_or("--dataset needs value")?))
            }
            "--max-session" => {
                max_session = iter.next().ok_or("--max-session needs value")?.parse()?
            }
            "--limit" => limit = iter.next().ok_or("--limit needs value")?.parse()?,
            "--ns" | "--namespace" => namespace = Some(iter.next().ok_or("--ns needs value")?),
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }
    Ok(Args {
        db: db.ok_or("--db required")?,
        dataset: dataset.ok_or("--dataset required")?,
        max_session,
        limit,
        namespace,
    })
}

fn evidence_session(dia_id: &str) -> Option<u32> {
    // Format: "D{n}:{m}" → n
    let rest = dia_id.strip_prefix('D')?;
    let n_str = rest.split(':').next()?;
    n_str.parse().ok()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;

    println!("=== LoCoMo conv-26 retrieval — v0.2 baseline ===");
    println!("  DB:        {}", args.db.display());
    println!("  Dataset:   {}", args.dataset.display());
    println!("  Sessions:  1..={}", args.max_session);
    println!("  Limit:     {}", args.limit);
    println!("  Namespace: {:?}", args.namespace);
    println!("  Path:      Memory::recall_from_namespace (FTS + embedding + ACT-R)");
    println!();

    // 1. Load dataset, extract conv-26 + filter QAs whose evidence falls
    //    in sessions 1..=max_session (same logic as v0.3 driver).
    let raw = std::fs::read_to_string(&args.dataset)?;
    let v: Value = serde_json::from_str(&raw)?;
    let conv = v
        .as_array()
        .ok_or("dataset not array")?
        .iter()
        .find(|c| c["sample_id"].as_str() == Some("conv-26"))
        .ok_or("conv-26 not found")?;

    // Match v0.3 driver semantics exactly: keep only QAs whose evidence
    // is *entirely* within sessions 1..=max_session. Using `.all()` (not
    // `.any()`) ensures the question is answerable from ingested data
    // alone — otherwise we'd score the system on questions that need
    // unseen sessions, inflating the "miss" count unfairly.
    let qa_all = conv["qa"].as_array().ok_or("no qa array")?;
    let qas: Vec<&Value> = qa_all
        .iter()
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

    println!(
        "Loaded {} QAs (out of {}) with evidence in sessions 1..={}",
        qas.len(),
        qa_all.len(),
        args.max_session
    );

    // 2. Open Memory (no graph store needed for v0.2 path)
    let mut mem = Memory::new(args.db.to_str().unwrap(), None)?;
    println!("Opened memory (v0.2 recall path — no graph layer)");
    println!();

    let mut hits = 0usize;
    let mut empty_results = 0usize;
    let mut total = 0usize;

    use std::collections::BTreeMap;
    let mut per_category_total: BTreeMap<u64, usize> = BTreeMap::new();
    let mut per_category_hits: BTreeMap<u64, usize> = BTreeMap::new();

    for (idx, q) in qas.iter().enumerate() {
        let question = q["question"].as_str().unwrap_or("");
        let evidence: HashSet<String> = q["evidence"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let category = q["category"].as_u64().unwrap_or(0);

        let ns_ref = args.namespace.as_deref();
        let results = mem.recall_from_namespace(question, args.limit, None, None, ns_ref)?;
        total += 1;

        if results.is_empty() {
            empty_results += 1;
        }

        // Score: any returned record whose `source` is
        // "locomo/conv-26/D{x}:{y}" matches a gold evidence dia_id?
        let mut hit = false;
        let mut top_sources: Vec<String> = Vec::new();
        for r in &results {
            let dia_id = r.record.source.rsplit('/').next().unwrap_or("");
            top_sources.push(dia_id.to_string());
            if evidence.contains(dia_id) {
                hit = true;
            }
        }
        if hit {
            hits += 1;
        }

        *per_category_total.entry(category).or_insert(0) += 1;
        if hit {
            *per_category_hits.entry(category).or_insert(0) += 1;
        }

        let q_short: String = question.chars().take(70).collect();
        let mark = if hit {
            "✓"
        } else if results.is_empty() {
            "∅"
        } else {
            "✗"
        };
        println!(
            "[{:>2}/{}] {} cat={} got={} | gold={:?} top={:?}",
            idx + 1,
            qas.len(),
            mark,
            category,
            results.len(),
            evidence.iter().collect::<Vec<_>>(),
            top_sources,
        );
        let _ = q_short; // (reserved for verbose mode)
    }

    let pct = (hits as f64) / (total as f64) * 100.0;
    println!();
    println!("=== Results (v0.2 baseline) ===");
    println!("  Total queries: {}", total);
    println!("  Hits@{}:       {} ({:.1}%)", args.limit, hits, pct);
    println!("  Empty:         {}", empty_results);

    println!();
    println!("=== Per-category breakdown ===");
    for (cat, n) in &per_category_total {
        let h = per_category_hits.get(cat).copied().unwrap_or(0);
        let p = (h as f64) / (*n as f64) * 100.0;
        println!("  cat={}  n={:<3} hits={:<3} ({:>5.1}%)", cat, n, h, p);
    }

    Ok(())
}
