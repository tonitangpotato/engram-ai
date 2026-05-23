//! T30 follow-up — tail-rank divergence diagnostic.
//!
//! Runs the 3 worst probe queries with `explain=true` against both
//! arms (`unified_substrate=false` and `true`) and dumps the full
//! candidate-score breakdown so we can see where the ranker diverges.
//!
//! # Why
//!
//! T30 probe-parity (commit 1c73a2c) showed parity_ratio=0.40 at K=10,
//! with 24/50 queries hitting Jaccard=0.818 (top-K set-identical
//! except one). The diff is at the tail, not the top — top ranks are
//! often byte-identical in order. This driver dumps both arms' fused
//! scores + per-signal sub-scores for the worst queries so we can see
//! *which signal* is producing the rank-N candidate swap.
//!
//! # Usage
//!
//! ```bash
//! cargo run --release -p engramai --example t30_rank_diag -- \
//!   --source /tmp/t30-probe.db \
//!   [--out /tmp/t30-rank-diag.md]
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use engramai::retrieval::{GraphQuery, GraphQueryResponse, ScoredResult};
use engramai::{Memory, MemoryConfig};

const WORST_QUERIES: &[&str] = &[
    "embedding",          // T30 K=10 jaccard=0.333 (5 swaps)
    "graph",              // T30 K=10 jaccard=0.538 (3 swaps)
    "session compaction", // T30 K=10 jaccard=0.538 (3 swaps)
    "semantic meaning",   // T30 K=10 jaccard=0.667 (2 swaps)
    "memory safety",      // T30 K=10 jaccard=0.818 (1 swap each — canonical small-diff case)
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let mut source: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--source" => source = it.next().map(|v| Path::new(v).to_path_buf()),
            "--out" => out = it.next().map(|v| Path::new(v).to_path_buf()),
            other => {
                eprintln!("unknown arg: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(source) = source else {
        eprintln!("usage: t30_rank_diag --source <path> [--out report.md]");
        return ExitCode::from(2);
    };
    let path_str = match source.to_str() {
        Some(s) => s,
        None => return ExitCode::from(2),
    };

    let mut report = String::new();
    report.push_str("# T30 tail-rank divergence diagnostic\n\n");
    report.push_str(&format!("source: `{}`\n\n", source.display()));

    for query in WORST_QUERIES {
        report.push_str(&format!("## query: `{}`\n\n", query));

        let legacy = match run_query(path_str, false, query) {
            Ok(r) => r,
            Err(e) => {
                report.push_str(&format!("legacy arm error: {e}\n\n"));
                continue;
            }
        };
        let unified = match run_query(path_str, true, query) {
            Ok(r) => r,
            Err(e) => {
                report.push_str(&format!("unified arm error: {e}\n\n"));
                continue;
            }
        };

        report.push_str(&format!(
            "- legacy plan: `{:?}`, outcome: `{:?}`\n",
            legacy.plan_used, legacy.outcome
        ));
        report.push_str(&format!(
            "- unified plan: `{:?}`, outcome: `{:?}`\n\n",
            unified.plan_used, unified.outcome
        ));

        let legacy_ids: Vec<String> = legacy.results.iter().map(scored_id).collect();
        let unified_ids: Vec<String> = unified.results.iter().map(scored_id).collect();

        report.push_str("### top-10 IDs\n\n");
        report.push_str("| rank | legacy | unified |\n");
        report.push_str("|---:|---|---|\n");
        let max = legacy_ids.len().max(unified_ids.len());
        for i in 0..max {
            let l = legacy_ids.get(i).cloned().unwrap_or_default();
            let u = unified_ids.get(i).cloned().unwrap_or_default();
            let same = if l == u && !l.is_empty() { " ✓" } else { "" };
            report.push_str(&format!("| {} | `{}`{} | `{}` |\n", i + 1, l, same, u));
        }
        report.push('\n');

        // Set diff — IDs only in one arm
        let l_set: std::collections::HashSet<&String> = legacy_ids.iter().collect();
        let u_set: std::collections::HashSet<&String> = unified_ids.iter().collect();
        let only_legacy: Vec<_> = l_set.difference(&u_set).collect();
        let only_unified: Vec<_> = u_set.difference(&l_set).collect();
        report.push_str(&format!(
            "- only-legacy ({}): {:?}\n",
            only_legacy.len(),
            only_legacy.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        ));
        report.push_str(&format!(
            "- only-unified ({}): {:?}\n\n",
            only_unified.len(),
            only_unified.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        ));

        // Fusion candidate dump from PlanTrace.fusion.candidates
        for (label, resp) in [("legacy", &legacy), ("unified", &unified)] {
            report.push_str(&format!("### {} fusion candidates\n\n", label));
            let Some(trace) = &resp.trace else {
                report.push_str("(no trace — explain=true not honored?)\n\n");
                continue;
            };
            let candidates = &trace.fusion.candidates;
            if candidates.is_empty() {
                report.push_str("(empty candidates list)\n\n");
            } else {
                report.push_str("plan weights: ");
                for (k, w) in &trace.fusion.weights {
                    report.push_str(&format!("`{}`={:.3} ", k, w));
                }
                report.push_str(&format!(
                    "renorm_scale={:.3} renorm_applied={}\n\n",
                    trace.fusion.renorm_scale, trace.fusion.renorm_applied
                ));
                report.push_str("| rank | id | fused | vector | bm25 | graph | recency | actr | affect |\n");
                report.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---:|\n");
                for (i, c) in candidates.iter().take(12).enumerate() {
                    report.push_str(&format!(
                        "| {} | `{}` | {:.4} | {} | {} | {} | {} | {} | {} |\n",
                        i + 1,
                        c.id,
                        c.fused_score,
                        fmt_opt(c.sub_scores.vector_score),
                        fmt_opt(c.sub_scores.bm25_score),
                        fmt_opt(c.sub_scores.graph_score),
                        fmt_opt(c.sub_scores.recency_score),
                        fmt_opt(c.sub_scores.actr_score),
                        fmt_opt(c.sub_scores.affect_similarity),
                    ));
                }
                report.push('\n');
            }
        }

        // Cross-arm overlap on candidate IDs (not just top-K — full
        // candidate pool). Tells us if the divergence is in the recall
        // set vs. just rank ordering.
        if let (Some(lt), Some(ut)) = (&legacy.trace, &unified.trace) {
            let l_cands: std::collections::HashSet<&String> =
                lt.fusion.candidates.iter().map(|c| &c.id).collect();
            let u_cands: std::collections::HashSet<&String> =
                ut.fusion.candidates.iter().map(|c| &c.id).collect();
            let shared = l_cands.intersection(&u_cands).count();
            let only_l = l_cands.difference(&u_cands).count();
            let only_u = u_cands.difference(&l_cands).count();
            report.push_str(&format!(
                "**candidate-pool overlap**: shared={}, only-legacy={}, only-unified={}\n\n",
                shared, only_l, only_u
            ));
        }

        report.push_str("---\n\n");
    }

    if let Some(out_path) = out {
        fs::write(&out_path, &report).expect("write report");
        eprintln!("wrote: {}", out_path.display());
    } else {
        println!("{}", report);
    }

    ExitCode::SUCCESS
}

fn run_query(
    path: &str,
    unified: bool,
    text: &str,
) -> Result<GraphQueryResponse, String> {
    let mut cfg = MemoryConfig::default();
    cfg.unified_substrate = unified;
    let mem = Memory::new(path, Some(cfg))
        .map_err(|e| format!("Memory::new: {e}"))?
        .with_graph_store(path)
        .map_err(|e| format!("with_graph_store: {e}"))?;
    let q = GraphQuery::new(text).with_limit(10).with_explain(true);

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
    block_on(mem.graph_query(q)).map_err(|e| format!("{e}"))
}

fn scored_id(sr: &ScoredResult) -> String {
    match sr {
        ScoredResult::Memory { record, .. } => record.id.clone(),
        ScoredResult::Topic { topic, .. } => format!("topic:{}", topic.topic_id),
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{:.3}", x),
        None => "—".to_string(),
    }
}
