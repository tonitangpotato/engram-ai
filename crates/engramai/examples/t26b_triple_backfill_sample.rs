//! T26b — sample triple-backfill dry-run with quality report.
//!
//! Runs the T26a `backfill_triples_from_memories` driver against a
//! bounded sample of production memories (default: 100), then emits a
//! markdown quality report so potato can decide whether to proceed
//! with the full T26c production run (~24k rows, ~$25).
//!
//! # Design intent (v04-unified-substrate §8.4 T26b)
//!
//! - **Non-destructive**: by default writes to a temp DB clone of the
//!   source. Pass `--in-place` to write back to the source DB (only
//!   use this if you're already inside a planned cutover window).
//! - **Bounded cost**: `--sample N` caps the extractor calls; with
//!   default 100 and Haiku at ~$0.001/call the dry-run is ~$0.10.
//! - **Real extractor**: wires `AnthropicTripleExtractor` (Haiku) so
//!   the report reflects real LLM quality, not a mock. Tests for the
//!   driver itself live in `tests/v04_phase_c_triple_backfill.rs`;
//!   this example is operational tooling, not a unit test.
//! - **Quality report**: prints per-memory triple counts, predicate
//!   distribution, sample of extracted triples, BackfillRun counters,
//!   wall-clock, extrapolated full-run cost & time.
//!
//! # Usage
//!
//! ```bash
//! ANTHROPIC_AUTH_TOKEN=sk-ant-... \
//!   cargo run --release --example t26b_triple_backfill_sample -- \
//!     --source /path/to/engram-memory.db \
//!     --sample 100 \
//!     --rps 5.0 \
//!     --out /tmp/t26b-report.md
//! ```
//!
//! # Exit codes
//! - 0: dry-run completed (report printed; quality is human-judged)
//! - 1: backfill driver returned an error
//! - 2: setup error (missing env var, source DB not found, etc.)

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use engramai::storage::Storage;
use engramai::substrate::triple_backfill::{
    backfill_triples_from_memories, TripleBackfillOpts,
};
use engramai::triple_extractor::AnthropicTripleExtractor;
use rusqlite::Connection;

const HAIKU_COST_PER_CALL_USD: f64 = 0.001; // Haiku 4.5 @ ~1k in / ~200 out tokens per memory
const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";

struct Args {
    source: PathBuf,
    sample: u64,
    namespace: Option<String>,
    rps: f64,
    out: Option<PathBuf>,
    in_place: bool,
    model: String,
}

fn parse_args() -> Result<Args, String> {
    let argv: Vec<String> = env::args().collect();
    let mut source: Option<PathBuf> = None;
    let mut sample: u64 = 100;
    let mut namespace: Option<String> = None;
    let mut rps: f64 = 5.0;
    let mut out: Option<PathBuf> = None;
    let mut in_place = false;
    let mut model = HAIKU_MODEL.to_string();

    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "--source" => {
                i += 1;
                source = Some(PathBuf::from(argv.get(i).ok_or("--source needs a value")?));
            }
            "--sample" => {
                i += 1;
                sample = argv
                    .get(i)
                    .ok_or("--sample needs a value")?
                    .parse()
                    .map_err(|e| format!("--sample parse: {e}"))?;
            }
            "--namespace" => {
                i += 1;
                namespace = Some(argv.get(i).ok_or("--namespace needs a value")?.clone());
            }
            "--rps" => {
                i += 1;
                rps = argv
                    .get(i)
                    .ok_or("--rps needs a value")?
                    .parse()
                    .map_err(|e| format!("--rps parse: {e}"))?;
            }
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(argv.get(i).ok_or("--out needs a value")?));
            }
            "--in-place" => {
                in_place = true;
            }
            "--model" => {
                i += 1;
                model = argv.get(i).ok_or("--model needs a value")?.clone();
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
        i += 1;
    }

    Ok(Args {
        source: source.ok_or("--source is required")?,
        sample,
        namespace,
        rps,
        out,
        in_place,
        model,
    })
}

fn print_help() {
    println!(
        "t26b_triple_backfill_sample — dry-run triple extraction on a bounded sample

USAGE:
  ANTHROPIC_AUTH_TOKEN=... cargo run --release --example t26b_triple_backfill_sample -- [OPTIONS]

OPTIONS:
  --source PATH       Source engram-memory.db (required)
  --sample N          Sample size (default: 100)
  --namespace NS      Restrict to one namespace (default: all)
  --rps F             Rate limit, calls/sec (default: 5.0)
  --out PATH          Write markdown report to file (also prints to stdout)
  --in-place          Write triples back to --source DB (default: clone to temp)
  --model NAME        Override model (default: {HAIKU_MODEL})
"
    );
}

fn clone_db_to_temp(source: &Path) -> Result<PathBuf, String> {
    let dir = env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let target = dir.join(format!("t26b-sample-{stamp}.db"));
    fs::copy(source, &target).map_err(|e| format!("clone failed: {e}"))?;
    // sqlite WAL sidecars — copy if present so the clone is consistent
    for sidecar in &["-wal", "-shm"] {
        let s = source.with_extension(format!(
            "{}{}",
            source.extension().and_then(|s| s.to_str()).unwrap_or("db"),
            sidecar
        ));
        if s.exists() {
            let t = target.with_extension(format!("db{sidecar}"));
            let _ = fs::copy(&s, &t); // best-effort; non-WAL is fine
        }
    }
    Ok(target)
}

#[derive(Default)]
struct SampleStats {
    triples_total: u64,
    memories_with_triples: u64,
    predicate_hist: BTreeMap<String, u64>,
    sample_triples: Vec<(String, String, String, String, f64)>, // (mem_id, subj, pred, obj, conf)
    confidence_sum: f64,
    confidence_n: u64,
}

fn collect_stats(db_path: &Path, run_id: &str) -> Result<SampleStats, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("open clone: {e}"))?;
    let mut stats = SampleStats::default();

    // All triples written by this run_id — use backfill_runs.started_at as
    // the lower-bound filter on triple rows (triples table has no run_id col
    // since the table predates backfill_runs).
    let started_at: Option<f64> = conn
        .query_row(
            "SELECT started_at FROM backfill_runs WHERE run_id = ?",
            [run_id],
            |row| row.get(0),
        )
        .ok();
    let lower = started_at.unwrap_or(0.0);

    let mut stmt = conn
        .prepare(
            "SELECT memory_id, subject, predicate, object, confidence
             FROM triples
             WHERE created_at >= ?
             ORDER BY memory_id, subject",
        )
        .map_err(|e| format!("prepare triples: {e}"))?;
    let rows = stmt
        .query_map([lower], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })
        .map_err(|e| format!("query triples: {e}"))?;

    let mut current_mem: Option<String> = None;
    let mut current_count: u64 = 0;
    for row in rows {
        let (mem_id, subj, pred, obj, conf) = row.map_err(|e| format!("row: {e}"))?;
        stats.triples_total += 1;
        *stats.predicate_hist.entry(pred.clone()).or_insert(0) += 1;
        stats.confidence_sum += conf;
        stats.confidence_n += 1;
        if stats.sample_triples.len() < 30 {
            stats
                .sample_triples
                .push((mem_id.clone(), subj.clone(), pred.clone(), obj.clone(), conf));
        }
        match &current_mem {
            Some(prev) if prev == &mem_id => current_count += 1,
            _ => {
                if current_mem.is_some() && current_count > 0 {
                    stats.memories_with_triples += 1;
                }
                current_mem = Some(mem_id);
                current_count = 1;
            }
        }
    }
    if current_mem.is_some() && current_count > 0 {
        stats.memories_with_triples += 1;
    }
    Ok(stats)
}

fn render_report(
    args: &Args,
    target_db: &Path,
    run: &engramai::substrate::backfill::BackfillRun,
    elapsed_secs: f64,
    stats: &SampleStats,
    total_memories_in_source: u64,
) -> String {
    let mut out = String::new();
    let push = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };

    push(&mut out, "# T26b — Triple Backfill Sample Report");
    push(&mut out, "");
    push(&mut out, &format!("- **Source DB**: `{}`", args.source.display()));
    push(&mut out, &format!("- **Target DB**: `{}` ({})", target_db.display(), if args.in_place { "in-place" } else { "temp clone" }));
    push(&mut out, &format!("- **Sample**: {} memories", args.sample));
    push(&mut out, &format!("- **Namespace filter**: {}", args.namespace.as_deref().unwrap_or("(all)")));
    push(&mut out, &format!("- **Model**: `{}`", args.model));
    push(&mut out, &format!("- **Rate limit**: {} req/sec", args.rps));
    push(&mut out, &format!("- **Run ID**: `{}`", run.run_id));
    push(&mut out, "");

    push(&mut out, "## Driver counters (BackfillRun)");
    push(&mut out, "");
    push(&mut out, &format!("- Memories read:           **{}**", run.rows_read));
    push(&mut out, &format!("- Triples inserted:        **{}**", run.rows_inserted));
    push(&mut out, &format!("- Memories skipped (already had triples): {}", run.rows_skipped_existing));
    push(&mut out, &format!("- Memories failed:         {}", run.rows_failed));
    push(&mut out, &format!("- Counter invariant:       rows_read = inserted + skipped + failed → {} = {} + {} + {} {}",
        run.rows_read, run.rows_inserted, run.rows_skipped_existing, run.rows_failed,
        if run.rows_inserted + run.rows_skipped_existing + run.rows_failed == run.rows_read { "✅" } else { "❌" }
    ));
    push(&mut out, "");

    push(&mut out, "## Wall-clock & cost");
    push(&mut out, "");
    push(&mut out, &format!("- Wall-clock:              **{:.1}s** ({:.2}s/memory avg)", elapsed_secs, elapsed_secs / (run.rows_read.max(1)) as f64));
    let sample_cost = (run.rows_read as f64) * HAIKU_COST_PER_CALL_USD;
    push(&mut out, &format!("- Sample cost (est):       **${:.3}** at ${:.4}/call", sample_cost, HAIKU_COST_PER_CALL_USD));
    push(&mut out, "");

    if total_memories_in_source > 0 && run.rows_read > 0 {
        let scale = total_memories_in_source as f64 / run.rows_read as f64;
        let full_cost = sample_cost * scale;
        let full_secs = elapsed_secs * scale;
        push(&mut out, &format!(
            "## Extrapolation to full run ({} memories in source)",
            total_memories_in_source
        ));
        push(&mut out, "");
        push(&mut out, &format!("- Estimated cost:          **${:.2}** (scale {:.1}x)", full_cost, scale));
        push(&mut out, &format!("- Estimated wall-clock:    **{:.0}s** ({:.1} min)", full_secs, full_secs / 60.0));
        push(&mut out, &format!("- Triples expected:        ~{}", (stats.triples_total as f64 * scale) as u64));
        push(&mut out, "");
    }

    push(&mut out, "## Triple statistics");
    push(&mut out, "");
    push(&mut out, &format!("- Total triples written:   **{}**", stats.triples_total));
    push(&mut out, &format!("- Triples per memory:      {:.2} avg", stats.triples_total as f64 / (run.rows_read.max(1)) as f64));
    push(&mut out, &format!("- Memories with ≥1 triple: {}/{}", stats.memories_with_triples, run.rows_read));
    if stats.confidence_n > 0 {
        push(&mut out, &format!("- Mean confidence:         {:.2}", stats.confidence_sum / stats.confidence_n as f64));
    }
    push(&mut out, "");

    push(&mut out, "## Predicate distribution (top 20)");
    push(&mut out, "");
    let mut hist: Vec<(&String, &u64)> = stats.predicate_hist.iter().collect();
    hist.sort_by(|a, b| b.1.cmp(a.1));
    push(&mut out, "| predicate | count |");
    push(&mut out, "|-----------|-------|");
    for (p, c) in hist.iter().take(20) {
        push(&mut out, &format!("| `{}` | {} |", p, c));
    }
    push(&mut out, "");

    push(&mut out, "## Sample triples (first 30)");
    push(&mut out, "");
    push(&mut out, "| memory_id (short) | subject | predicate | object | conf |");
    push(&mut out, "|-------------------|---------|-----------|--------|------|");
    for (mid, s, p, o, c) in &stats.sample_triples {
        let mid_short = mid.chars().take(8).collect::<String>();
        let s = truncate(s, 32);
        let o = truncate(o, 32);
        push(&mut out, &format!("| `{mid_short}` | {s} | `{p}` | {o} | {c:.2} |"));
    }
    push(&mut out, "");

    push(&mut out, "## Human judgement checklist");
    push(&mut out, "");
    push(&mut out, "- [ ] Predicate vocabulary feels meaningful (no LLM gibberish)");
    push(&mut out, "- [ ] Subject/object spans are concrete, not whole-sentence");
    push(&mut out, "- [ ] Confidence distribution makes sense (most >0.7, some <0.5)");
    push(&mut out, "- [ ] No PII or sensitive content surfaced in unexpected ways");
    push(&mut out, "- [ ] Extrapolated cost & wall-clock acceptable for full run");
    push(&mut out, "");
    push(&mut out, "**If all checks pass → proceed to T26c (full ~24k run on `--source` in-place).**");
    out
}

fn truncate(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    // Escape pipes for markdown tables
    out.replace('|', "\\|").replace('\n', " ")
}

fn count_memories_in_source(db: &Path, ns: Option<&str>) -> Result<u64, String> {
    let conn = Connection::open(db).map_err(|e| format!("open source for count: {e}"))?;
    let (sql, has_param) = match ns {
        Some(_) => (
            "SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL AND namespace = ?",
            true,
        ),
        None => ("SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL", false),
    };
    let count: u64 = if has_param {
        conn.query_row(sql, [ns.unwrap()], |row| row.get(0))
    } else {
        conn.query_row(sql, [], |row| row.get(0))
    }
    .map_err(|e| format!("count memories: {e}"))?;
    Ok(count)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("arg error: {e}\n");
            print_help();
            return ExitCode::from(2);
        }
    };

    if !args.source.exists() {
        eprintln!("source DB not found: {}", args.source.display());
        return ExitCode::from(2);
    }
    let token = match env::var("ANTHROPIC_AUTH_TOKEN").or_else(|_| env::var("ANTHROPIC_API_KEY")) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("ANTHROPIC_AUTH_TOKEN (or ANTHROPIC_API_KEY) not set");
            return ExitCode::from(2);
        }
    };
    let is_oauth = token.starts_with("sk-ant-oat"); // OAuth-issued tokens

    let target_db = if args.in_place {
        args.source.clone()
    } else {
        match clone_db_to_temp(&args.source) {
            Ok(p) => {
                eprintln!("cloned source to: {}", p.display());
                p
            }
            Err(e) => {
                eprintln!("clone error: {e}");
                return ExitCode::from(2);
            }
        }
    };

    let total_in_source = count_memories_in_source(&args.source, args.namespace.as_deref())
        .unwrap_or_else(|e| {
            eprintln!("warn: could not count source memories: {e}");
            0
        });
    eprintln!(
        "source contains {} non-deleted memories{}",
        total_in_source,
        args.namespace.as_deref().map(|n| format!(" in ns '{n}'")).unwrap_or_default()
    );

    let storage = match Storage::new(&target_db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("open storage: {e}");
            return ExitCode::from(2);
        }
    };
    let extractor = AnthropicTripleExtractor::with_model(&token, is_oauth, &args.model);
    let opts = TripleBackfillOpts {
        batch_size: 64,
        rate_limit_per_sec: args.rps,
        max_retries: 2,
        retry_backoff_ms: 500,
        namespace_filter: args.namespace.clone(),
        max_memories: Some(args.sample),
    };

    eprintln!(
        "running backfill_triples_from_memories(sample={}, rps={}) …",
        args.sample, args.rps
    );
    let t0 = Instant::now();
    let run = match backfill_triples_from_memories(&storage, &extractor, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("backfill error: {e}");
            return ExitCode::from(1);
        }
    };
    let elapsed = t0.elapsed().as_secs_f64();
    eprintln!(
        "done in {:.1}s — read={} inserted={} skipped={} failed={}",
        elapsed, run.rows_read, run.rows_inserted, run.rows_skipped_existing, run.rows_failed
    );

    let stats = match collect_stats(&target_db, &run.run_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stats error: {e}");
            return ExitCode::from(1);
        }
    };

    let report = render_report(&args, &target_db, &run, elapsed, &stats, total_in_source);
    println!("{report}");
    if let Some(out) = &args.out {
        if let Err(e) = fs::write(out, &report) {
            eprintln!("warn: failed to write --out {}: {e}", out.display());
        } else {
            eprintln!("report written to {}", out.display());
        }
    }

    ExitCode::SUCCESS
}
