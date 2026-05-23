//! T30 — Phase D probe-parity driver.
//!
//! Runs a 50-query probe set against a backfilled snapshot DB twice:
//!   - arm A: `unified_substrate=false` (legacy read paths)
//!   - arm B: `unified_substrate=true`  (unified-substrate read paths)
//!
//! For each query, computes Jaccard@10 over the top-K memory IDs from
//! the two arms. The driver passes iff
//!
//!     #(queries with Jaccard ≥ 0.95) / 50 ≥ 0.95
//!
//! per design v04-unified-substrate §5.4 acceptance ("Recall@10 ≥ 95%
//! of legacy").
//!
//! # Prereqs
//!
//! 1. Snapshot a production DB:        `cp engram-memory.db /tmp/t30-probe.db`
//! 2. Run Phase C backfill on it:      `t30_phase_d_backfill_runner --source /tmp/t30-probe.db`
//! 3. Run this driver:                 `t30_probe_parity --source /tmp/t30-probe.db`
//!
//! Without (2) the unified arm reads from empty nodes/edges tables and
//! parity collapses to 0%, which would be a misleading fail.
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example t30_probe_parity -- \
//!   --source /tmp/t30-probe.db \
//!   [--top-k 10] \
//!   [--jaccard-threshold 0.95] \
//!   [--parity-threshold 0.95] \
//!   [--out /tmp/t30-probe-parity.json]
//! ```
//!
//! # Exit codes
//! - 0: parity_ratio ≥ parity_threshold
//! - 1: parity_ratio < parity_threshold (Phase D not ready to flip)
//! - 2: setup error
//! - 3: retrieval errors prevented a meaningful comparison

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use engramai::retrieval::{GraphQuery, GraphQueryResponse, RetrievalError, ScoredResult};
use engramai::{Memory, MemoryConfig};
use serde::Serialize;

/// 50-query probe set against rustclaw's production engram DB.
///
/// Composition rationale:
/// - 20 broad-topic queries reused from migration_integrity (rust /
///   python / memory / etc.) for cross-bench comparability.
/// - 30 production-data-specific queries pulled from the entity table
///   (high-frequency mentions: ISS-NNN issue references, project
///   names, source-file names, key concepts) so the test exercises
///   real retrieval shape, not synthetic noise.
const PROBE_QUERIES: &[&str] = &[
    // -- broad topics (mirror migration_integrity.rs PARITY_QUERY_SET) --
    "rust programming language",
    "memory safety",
    "python data science",
    "dynamic typing",
    "graph database",
    "entity relationship",
    "memory consolidation",
    "long-term storage",
    "retrieval augmented generation",
    "private knowledge",
    "embeddings dense vectors",
    "semantic meaning",
    "rust",
    "python",
    "graph",
    "memory",
    "retrieval",
    "embedding",
    "knowledge",
    "vectors",
    // -- production-data-specific (entity-mention frequency, top 30) --
    "gid-rs graph database",
    "sqlite storage backend",
    "rustclaw daemon",
    "engram knowledge compiler",
    "knowledge compiler clustering",
    "infomap algorithm",
    "hebbian links",
    "namespace isolation",
    "soft delete tombstone",
    "memory dual write",
    "phase b dual write",
    "phase c backfill",
    "unified substrate",
    "nodes and edges schema",
    "FTS5 full-text search",
    "anthropic api oauth",
    "embedding provider ollama",
    "session compaction",
    "loop detector",
    "tool execution batch",
    "specialist sub-agent",
    "design document review",
    "review findings apply workflow",
    "ritual phase gating",
    "task dependency graph",
    "GOAL satisfies edge",
    "agentctl orchestrator",
    "interview prep code",
    "autoalpha trading",
    "xinfluencer scrape",
];

#[derive(Debug, Serialize)]
struct PerQueryReport {
    query: String,
    legacy_ids: Vec<String>,
    unified_ids: Vec<String>,
    legacy_error: Option<String>,
    unified_error: Option<String>,
    jaccard: f64,
    passes_jaccard_threshold: bool,
}

#[derive(Debug, Serialize)]
struct ProbeParityReport {
    source: String,
    top_k: usize,
    jaccard_threshold: f64,
    parity_threshold: f64,
    queries_total: usize,
    queries_passing_jaccard: usize,
    queries_with_legacy_error: usize,
    queries_with_unified_error: usize,
    parity_ratio: f64,
    passed: bool,
    wall_clock_secs: f64,
    per_query: Vec<PerQueryReport>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let cfg = match parse_args(&args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("usage: t30_probe_parity --source <path> [--top-k 10] [--jaccard-threshold 0.95] [--parity-threshold 0.95] [--out path.json]");
            return ExitCode::from(2);
        }
    };

    if !cfg.source.exists() {
        eprintln!("source DB not found: {}", cfg.source.display());
        return ExitCode::from(2);
    }

    let path_str = match cfg.source.to_str() {
        Some(s) => s,
        None => {
            eprintln!("source path is not UTF-8");
            return ExitCode::from(2);
        }
    };

    println!("T30 probe-parity driver");
    println!("source:             {}", cfg.source.display());
    println!("top-k:              {}", cfg.top_k);
    println!("jaccard threshold:  {:.2}", cfg.jaccard_threshold);
    println!("parity threshold:   {:.2}", cfg.parity_threshold);
    println!("queries:            {}", PROBE_QUERIES.len());
    println!();

    let t0 = Instant::now();

    // -- arm A: legacy --
    println!("=== arm A: unified_substrate=false (legacy) ===");
    let legacy_results = match run_arm(path_str, false, cfg.top_k) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("legacy arm failed to start: {e}");
            return ExitCode::from(3);
        }
    };

    // -- arm B: unified --
    println!("=== arm B: unified_substrate=true (unified) ===");
    let unified_results = match run_arm(path_str, true, cfg.top_k) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("unified arm failed to start: {e}");
            return ExitCode::from(3);
        }
    };

    // -- compare per query --
    let mut per_query = Vec::with_capacity(PROBE_QUERIES.len());
    let mut passing = 0usize;
    let mut legacy_err = 0usize;
    let mut unified_err = 0usize;

    for (i, q) in PROBE_QUERIES.iter().enumerate() {
        let (l_ids, l_err) = &legacy_results[i];
        let (u_ids, u_err) = &unified_results[i];
        if l_err.is_some() {
            legacy_err += 1;
        }
        if u_err.is_some() {
            unified_err += 1;
        }
        // Errors on either arm count as parity failure for this query.
        // Without this guard, jaccard(∅, ∅) = 1.0 would silently pass
        // a fully-broken retrieval run.
        let jac = if l_err.is_some() || u_err.is_some() {
            0.0
        } else {
            jaccard(l_ids, u_ids)
        };
        let pass = jac >= cfg.jaccard_threshold;
        if pass {
            passing += 1;
        }
        per_query.push(PerQueryReport {
            query: (*q).to_string(),
            legacy_ids: l_ids.clone(),
            unified_ids: u_ids.clone(),
            legacy_error: l_err.clone(),
            unified_error: u_err.clone(),
            jaccard: jac,
            passes_jaccard_threshold: pass,
        });
    }

    let total = PROBE_QUERIES.len();
    let parity_ratio = passing as f64 / total as f64;
    let passed = parity_ratio >= cfg.parity_threshold;

    let report = ProbeParityReport {
        source: cfg.source.display().to_string(),
        top_k: cfg.top_k,
        jaccard_threshold: cfg.jaccard_threshold,
        parity_threshold: cfg.parity_threshold,
        queries_total: total,
        queries_passing_jaccard: passing,
        queries_with_legacy_error: legacy_err,
        queries_with_unified_error: unified_err,
        parity_ratio,
        passed,
        wall_clock_secs: t0.elapsed().as_secs_f64(),
        per_query,
    };

    print_summary(&report);

    if let Some(out) = &cfg.out {
        let body = serde_json::to_string_pretty(&report).expect("serialize report");
        if let Err(e) = fs::write(out, body) {
            eprintln!("warning: failed to write {}: {e}", out.display());
        } else {
            println!();
            println!("wrote report: {}", out.display());
        }
    }

    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_arm(
    path: &str,
    unified: bool,
    top_k: usize,
) -> Result<Vec<(Vec<String>, Option<String>)>, String> {
    let mut config = MemoryConfig::default();
    config.unified_substrate = unified;
    let mem = Memory::new(path, Some(config))
        .map_err(|e| format!("Memory::new: {e}"))?
        .with_graph_store(path)
        .map_err(|e| format!("with_graph_store: {e}"))?;

    // Busy-poll block_on — matches the pattern in
    // examples/locomo_conv26_retrieval.rs. `graph_query` futures don't
    // await IO today (they return Ready on first poll), so this is
    // safe and avoids pulling tokio into example dependencies.
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

    let mut out = Vec::with_capacity(PROBE_QUERIES.len());
    let t0 = Instant::now();
    for q in PROBE_QUERIES {
        let query = GraphQuery::new(*q).with_limit(top_k);
        let result = block_on(mem.graph_query(query));
        match result {
            Ok(resp) => out.push((top_ids(&resp), None)),
            Err(e) => out.push((Vec::new(), Some(format!("{e}")))),
        }
    }
    println!(
        "  ran {} queries in {:.2}s (unified={})",
        PROBE_QUERIES.len(),
        t0.elapsed().as_secs_f64(),
        unified
    );
    Ok(out)
}

fn top_ids(resp: &GraphQueryResponse) -> Vec<String> {
    resp.results
        .iter()
        .map(|sr| match sr {
            ScoredResult::Memory { record, .. } => record.id.clone(),
            ScoredResult::Topic { topic, .. } => format!("topic:{}", topic.topic_id),
        })
        .collect()
}

fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let sa: HashSet<&String> = a.iter().collect();
    let sb: HashSet<&String> = b.iter().collect();
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn print_summary(r: &ProbeParityReport) {
    println!();
    println!("=== Per-query Jaccard ===");
    for q in &r.per_query {
        let marker = if q.passes_jaccard_threshold { "✓" } else { "✗" };
        let err_tag = match (&q.legacy_error, &q.unified_error) {
            (Some(_), Some(_)) => " [both-err]",
            (Some(_), None) => " [legacy-err]",
            (None, Some(_)) => " [unified-err]",
            (None, None) => "",
        };
        println!(
            "  {} {:.3}  {}{}",
            marker,
            q.jaccard,
            truncate(&q.query, 50),
            err_tag
        );
    }
    println!();
    println!("=== Aggregate ===");
    println!("  queries_total:            {}", r.queries_total);
    println!("  queries_passing_jaccard:  {}", r.queries_passing_jaccard);
    println!("  queries_with_legacy_err:  {}", r.queries_with_legacy_error);
    println!("  queries_with_unified_err: {}", r.queries_with_unified_error);
    println!(
        "  parity_ratio:             {:.4}  (threshold {:.2})",
        r.parity_ratio, r.parity_threshold
    );
    println!("  wall-clock:               {:.2}s", r.wall_clock_secs);
    println!();
    if r.passed {
        println!("PASS — Phase D parity gate cleared on probe set");
    } else {
        println!("FAIL — parity_ratio below threshold; do NOT flip unified default");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

struct Cfg {
    source: PathBuf,
    top_k: usize,
    jaccard_threshold: f64,
    parity_threshold: f64,
    out: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Cfg, String> {
    let mut source: Option<PathBuf> = None;
    let mut top_k: usize = 10;
    let mut jaccard_threshold: f64 = 0.95;
    let mut parity_threshold: f64 = 0.95;
    let mut out: Option<PathBuf> = None;

    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--source" => {
                let v = it.next().ok_or("--source requires a value")?;
                source = Some(Path::new(v).to_path_buf());
            }
            "--top-k" => {
                let v = it.next().ok_or("--top-k requires a value")?;
                top_k = v.parse().map_err(|_| "--top-k must be an integer")?;
            }
            "--jaccard-threshold" => {
                let v = it.next().ok_or("--jaccard-threshold requires a value")?;
                jaccard_threshold = v
                    .parse()
                    .map_err(|_| "--jaccard-threshold must be a float")?;
            }
            "--parity-threshold" => {
                let v = it.next().ok_or("--parity-threshold requires a value")?;
                parity_threshold = v
                    .parse()
                    .map_err(|_| "--parity-threshold must be a float")?;
            }
            "--out" => {
                let v = it.next().ok_or("--out requires a value")?;
                out = Some(Path::new(v).to_path_buf());
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }

    let source = source.ok_or("--source is required")?;
    Ok(Cfg {
        source,
        top_k,
        jaccard_threshold,
        parity_threshold,
        out,
    })
}

// suppress unused-import warnings for the conditional `RetrievalError`
#[allow(dead_code)]
fn _force_use_retrieval_error(_e: RetrievalError) {}
