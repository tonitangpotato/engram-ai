//! ISS-019 Step 9 — 5KB smoke rebuild pilot.
//!
//! Reads a sample of agent-session memories from a source DB, re-runs
//! the full `store_raw` write path (extractor → dimensions → merge →
//! persist) against a fresh target DB, then asserts the three
//! §9 coverage thresholds.
//!
//! # Usage
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-ant-... cargo run --example iss019_smoke_pilot --release -- \
//!   --source /path/to/source.db \
//!   --target /path/to/target.db \
//!   --sample 100
//! ```
//!
//! # Exit codes
//! - 0: all assertions passed
//! - 1: one or more assertions failed (details printed)
//! - 2: setup error (DB open, API key, etc.)
//!
//! # Why not full 1,088 rows?
//! The design calls this a *smoke* pilot. 100 random rows from the
//! last 24h exercise every code path (factual/episodic/emotional/
//! relational memory types, short/long content, unicode) at ~$0.10
//! Haiku cost vs ~$1 for full day. Step 10 does the 58MB full
//! rebuild once this pilot passes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use engramai::store_api::{RawStoreOutcome, StorageMeta};
use engramai::{AnthropicExtractor, Memory, MemoryType};
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// Read from an engram memories table (legacy path — content is
    /// already-consolidated summaries, hence the §9 threshold mismatch).
    Engram,
    /// Read from a RustClaw sessions.db. Each session row contains a
    /// `messages` JSON array; we extract raw user-turn text (with
    /// `[TELEGRAM ...]` prefix preserved) as the test signal. This
    /// mirrors the real "new message enters agent" ingestion path.
    Sessions,
}

struct Args {
    source: PathBuf,
    target: PathBuf,
    sample: usize,
    source_kind: SourceKind,
    api_key: String,
    is_oauth: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut source = None;
    let mut target = None;
    let mut sample = 100_usize;
    let mut source_kind = SourceKind::Engram;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => source = args.next().map(PathBuf::from),
            "--target" => target = args.next().map(PathBuf::from),
            "--sample" => {
                sample = args
                    .next()
                    .ok_or("--sample requires a number")?
                    .parse()
                    .map_err(|e| format!("--sample: {e}"))?;
            }
            "--source-kind" => {
                let kind = args.next().ok_or("--source-kind requires a value")?;
                source_kind = match kind.as_str() {
                    "engram" => SourceKind::Engram,
                    "sessions" => SourceKind::Sessions,
                    other => return Err(format!("--source-kind: unknown '{other}' (expected engram|sessions)")),
                };
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: iss019_smoke_pilot --source <src.db> --target <tgt.db> [--sample N] [--source-kind engram|sessions]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }

    let source = source.ok_or("--source required")?;
    let target = target.ok_or("--target required")?;
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .or_else(|_| std::env::var("ANTHROPIC_OAUTH_TOKEN"))
        .map_err(|_| "ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN env var required".to_string())?;
    let is_oauth = std::env::var("ANTHROPIC_OAUTH_TOKEN").is_ok();

    Ok(Args {
        source,
        target,
        sample,
        source_kind,
        api_key,
        is_oauth,
    })
}

/// Row read from source — minimum needed to replay through store_raw.
#[derive(Debug)]
struct SourceRow {
    id: String,
    content: String,
    memory_type: String,
    importance: f64,
    created_at: f64,
    namespace: String,
}

fn load_sample(source: &PathBuf, sample_size: usize) -> Result<Vec<SourceRow>, String> {
    let conn = Connection::open(source).map_err(|e| format!("open source: {e}"))?;

    // Prefer recent (last 24h) rows with non-trivial content.
    // `ORDER BY RANDOM()` on a 18k-row table takes ~50ms — fine for pilot.
    let mut stmt = conn
        .prepare(
            "SELECT id, content, memory_type, importance, created_at, namespace
             FROM memories
             WHERE created_at >= strftime('%s', 'now', '-1 day')
               AND deleted_at IS NULL
               AND length(content) >= 20
             ORDER BY RANDOM()
             LIMIT ?1",
        )
        .map_err(|e| format!("prepare: {e}"))?;

    let rows: Vec<SourceRow> = stmt
        .query_map([sample_size as i64], |row| {
            Ok(SourceRow {
                id: row.get(0)?,
                content: row.get(1)?,
                memory_type: row.get(2)?,
                importance: row.get(3)?,
                created_at: row.get(4)?,
                namespace: row.get(5)?,
            })
        })
        .map_err(|e| format!("query: {e}"))?
        .filter_map(Result::ok)
        .collect();

    if rows.is_empty() {
        return Err("no rows matched — source DB has no recent memories".into());
    }
    Ok(rows)
}

/// Load sample from a RustClaw sessions.db.
///
/// Extracts raw user-turn messages from the `messages` JSON array of each
/// session. This gives the extractor the same signal it sees during real
/// ingestion: `[TELEGRAM ... timestamp]` prefix, conversational context,
/// unmodified raw content.
///
/// Sampling strategy (80/20 by session key prefix):
/// - 80%: `telegram:*` (clean TELEGRAM prefix with sender+timestamp)
/// - 20%: other (`agent:*`, `heartbeat:*`) for diversity — tests how
///   extractor handles sparse-signal inputs
fn load_sample_from_sessions(
    source: &PathBuf,
    sample_size: usize,
) -> Result<Vec<SourceRow>, String> {
    use serde_json::Value;

    let conn = Connection::open(source).map_err(|e| format!("open sessions: {e}"))?;

    // Pull all sessions, bucket by key prefix.
    let mut stmt = conn
        .prepare("SELECT key, messages FROM sessions")
        .map_err(|e| format!("prepare sessions: {e}"))?;

    let mut tg_sessions: Vec<(String, String)> = Vec::new();
    let mut other_sessions: Vec<(String, String)> = Vec::new();

    let rows = stmt
        .query_map([], |row| {
            let key: String = row.get(0)?;
            let msgs: String = row.get(1)?;
            Ok((key, msgs))
        })
        .map_err(|e| format!("query sessions: {e}"))?;

    for r in rows {
        let (key, msgs_json) = r.map_err(|e| format!("row: {e}"))?;
        if key.starts_with("telegram:") {
            tg_sessions.push((key, msgs_json));
        } else {
            other_sessions.push((key, msgs_json));
        }
    }

    // Extract user-turn messages from each session.
    // Returns (session_key, msg_idx, content) tuples.
    fn extract_user_messages(key: &str, msgs_json: &str) -> Vec<(String, usize, String)> {
        let mut out = Vec::new();
        let Ok(arr) = serde_json::from_str::<Value>(msgs_json) else {
            return out;
        };
        let Some(msgs) = arr.as_array() else {
            return out;
        };
        for (i, m) in msgs.iter().enumerate() {
            if m.get("role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }
            let Some(content) = m.get("content") else { continue };
            // Content can be a string or an array of content blocks.
            let text = match content {
                Value::String(s) => s.clone(),
                Value::Array(blocks) => {
                    let mut parts = Vec::new();
                    for b in blocks {
                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                            if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                                parts.push(t.to_string());
                            }
                        }
                    }
                    parts.join("\n")
                }
                _ => continue,
            };
            // Skip if too short or if it's a tool_result / system placeholder
            let trimmed = text.trim();
            if trimmed.len() < 20 {
                continue;
            }
            // Skip "[Previous conversation summary]" compacted meta-turns
            if trimmed.starts_with("[Previous conversation summary]") {
                continue;
            }
            // Skip if content is very large (>8KB) — these are usually
            // system-injected blobs (memory dumps, tool output replay)
            if trimmed.len() > 8000 {
                continue;
            }
            out.push((key.to_string(), i, text));
        }
        out
    }

    let mut tg_msgs: Vec<(String, usize, String)> = Vec::new();
    for (key, j) in &tg_sessions {
        tg_msgs.extend(extract_user_messages(key, j));
    }
    let mut other_msgs: Vec<(String, usize, String)> = Vec::new();
    for (key, j) in &other_sessions {
        other_msgs.extend(extract_user_messages(key, j));
    }

    println!(
        "   session buckets: {} telegram sessions ({} user msgs), {} other sessions ({} user msgs)",
        tg_sessions.len(),
        tg_msgs.len(),
        other_sessions.len(),
        other_msgs.len()
    );

    // 80/20 split, with quota rebalancing when one pool is short.
    //
    // BUG FIX (2026-04-22): The original implementation had two defects:
    //   1. If one pool had fewer items than its target quota, the shortfall
    //      was NOT transferred to the other pool — sample was silently
    //      truncated. E.g., requesting 100 with 20 tg msgs + 388 other msgs
    //      yielded only 40 rows (20 + 20) instead of 100.
    //   2. `stride = pool.len() / n` with integer division + `idx += stride`
    //      could miss items near the end when pool size wasn't a multiple
    //      of n, resulting in picked.len() < n on exit.
    //
    // Fix:
    //   - Compute desired quotas, then rebalance: move unmet tg demand
    //     to `other`, and vice versa, capped by each pool's capacity.
    //   - Use `idx = i * pool.len() / n` (floor-of-fraction) which always
    //     produces exactly n distinct indices in [0, pool.len()).
    let desired_tg = (sample_size as f64 * 0.80).round() as usize;
    let desired_other = sample_size.saturating_sub(desired_tg);

    // First pass: take min(desired, available) from each pool.
    let tg_avail = tg_msgs.len();
    let other_avail = other_msgs.len();
    let take_tg_initial = desired_tg.min(tg_avail);
    let take_other_initial = desired_other.min(other_avail);

    // Rebalance: give the shortfall to the other pool, capped by remaining capacity.
    let tg_shortfall = desired_tg.saturating_sub(take_tg_initial);
    let other_shortfall = desired_other.saturating_sub(take_other_initial);
    let take_tg = take_tg_initial + other_shortfall.min(tg_avail - take_tg_initial);
    let take_other = take_other_initial + tg_shortfall.min(other_avail - take_other_initial);

    // Shuffle via RANDOM ordering is hard without rand crate; use
    // a deterministic-ish permutation from current time mod len.
    // This is a smoke pilot, not a statistical test — good enough.
    fn pseudo_sample<T: Clone>(mut pool: Vec<T>, n: usize) -> Vec<T> {
        if pool.len() <= n {
            return pool;
        }
        // Rotate by a time-based offset so repeated runs hit different items.
        let offset = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as usize)
            .unwrap_or(0))
            % pool.len();
        pool.rotate_left(offset);
        // Uniform-stride pick: idx = i * pool.len() / n guarantees exactly
        // n distinct indices in [0, pool.len()) when pool.len() > n.
        let plen = pool.len();
        let mut picked = Vec::with_capacity(n);
        for i in 0..n {
            let idx = i * plen / n;
            picked.push(pool[idx].clone());
        }
        picked
    }

    let picked_tg = pseudo_sample(tg_msgs, take_tg);
    let picked_other = pseudo_sample(other_msgs, take_other);

    println!(
        "   quota plan: desired={}+{}, took={}+{} (tg_shortfall={}, other_shortfall={})",
        desired_tg, desired_other, take_tg, take_other, tg_shortfall, other_shortfall,
    );

    let mut rows: Vec<SourceRow> = Vec::with_capacity(picked_tg.len() + picked_other.len());

    // namespace/memory_type defaults — we don't know these for raw user
    // messages, so give the extractor freedom by picking reasonable defaults.
    // Importance hint at 0.5 (neutral) so extractor decides.
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    for (key, i, text) in picked_tg {
        rows.push(SourceRow {
            id: format!("session:{}:{}", key, i),
            content: text,
            memory_type: "episodic".to_string(), // user-said-something is episodic by default
            importance: 0.5,
            created_at: now_secs,
            namespace: key, // session key IS the namespace (channel)
        });
    }
    for (key, i, text) in picked_other {
        rows.push(SourceRow {
            id: format!("session:{}:{}", key, i),
            content: text,
            memory_type: "episodic".to_string(),
            importance: 0.5,
            created_at: now_secs,
            namespace: key,
        });
    }

    if rows.is_empty() {
        return Err("no user messages extracted from sessions.db".into());
    }
    Ok(rows)
}

fn parse_memory_type(s: &str) -> MemoryType {
    match s {
        "factual" => MemoryType::Factual,
        "episodic" => MemoryType::Episodic,
        "emotional" => MemoryType::Emotional,
        "procedural" => MemoryType::Procedural,
        "relational" => MemoryType::Relational,
        "opinion" => MemoryType::Opinion,
        "causal" => MemoryType::Causal,
        _ => MemoryType::Factual,
    }
}

/// Walks target DB, counts dimensional field coverage on stored rows.
#[derive(Debug, Default)]
struct DimensionalCoverage {
    total_rows: u64,
    rows_with_dimensions_block: u64,
    rows_with_participants: u64,
    rows_with_temporal: u64,
    rows_with_causation: u64,
}

impl DimensionalCoverage {
    fn collect(target: &PathBuf) -> Result<Self, String> {
        let conn = Connection::open(target).map_err(|e| format!("open target: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT metadata FROM memories WHERE deleted_at IS NULL")
            .map_err(|e| format!("prepare: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                let m: Option<String> = row.get(0)?;
                Ok(m)
            })
            .map_err(|e| format!("query: {e}"))?;

        let mut cov = Self::default();
        for row in rows {
            let meta_str = row.map_err(|e| format!("row: {e}"))?;
            cov.total_rows += 1;

            let Some(raw) = meta_str else { continue };
            let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };

            // Look for `engram.dimensions` block anywhere in the
            // metadata tree. v2 layout puts it at top-level under
            // `engram.dimensions`; v1 may have it under `dimensions`.
            let dims = val
                .get("engram")
                .and_then(|e| e.get("dimensions"))
                .or_else(|| val.get("dimensions"));

            let Some(dims) = dims else { continue };
            cov.rows_with_dimensions_block += 1;

            // `participants` is a comma-separated String per design §2
            // (DimensionalMetadata::participants: Option<String>), not an
            // array. Accept either: non-empty string, or non-empty array
            // (defensive — in case future extractors emit arrays).
            let has_participants = match dims.get("participants") {
                Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
                Some(serde_json::Value::Array(a)) => !a.is_empty(),
                _ => false,
            };
            if has_participants {
                cov.rows_with_participants += 1;
            }
            // `temporal` is a TemporalMark struct per design §2.2 (object
            // with `kind` + `value`), NOT a plain string. Check for
            // non-null, non-empty object OR non-empty string (defensive).
            let has_temporal = match dims.get("temporal") {
                Some(serde_json::Value::Null) | None => false,
                Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
                Some(serde_json::Value::Object(o)) => !o.is_empty(),
                Some(_) => true,
            };
            if has_temporal {
                cov.rows_with_temporal += 1;
            }
            // `causation` is a free-form Option<String>.
            let has_causation = match dims.get("causation") {
                Some(serde_json::Value::Null) | None => false,
                Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
                Some(_) => true, // other non-null shapes count as present
            };
            if has_causation {
                cov.rows_with_causation += 1;
            }
        }
        Ok(cov)
    }

    fn pct(&self, numerator: u64) -> f64 {
        if self.total_rows == 0 {
            0.0
        } else {
            numerator as f64 / self.total_rows as f64
        }
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    println!("═══════════════════════════════════════════════════════");
    println!("  ISS-019 Step 9 — Smoke Rebuild Pilot");
    println!("═══════════════════════════════════════════════════════");
    println!("  source:       {}", args.source.display());
    println!("  source-kind:  {:?}", args.source_kind);
    println!("  target:       {}", args.target.display());
    println!("  sample:       {} rows", args.sample);
    println!();

    // Fresh target — delete if exists so we start clean.
    if args.target.exists() {
        if let Err(e) = std::fs::remove_file(&args.target) {
            eprintln!("failed to clear target: {e}");
            return ExitCode::from(2);
        }
    }

    // ── Load sample ───────────────────────────────────────────
    let sample = match args.source_kind {
        SourceKind::Engram => load_sample(&args.source, args.sample),
        SourceKind::Sessions => load_sample_from_sessions(&args.source, args.sample),
    };
    let sample = match sample {
        Ok(s) => s,
        Err(e) => {
            eprintln!("load_sample: {e}");
            return ExitCode::from(2);
        }
    };
    println!("📥 Loaded {} rows from source", sample.len());

    // Histogram by memory_type for transparency
    let mut type_hist: HashMap<String, u64> = HashMap::new();
    for r in &sample {
        *type_hist.entry(r.memory_type.clone()).or_insert(0) += 1;
    }
    print!("   by type: ");
    for (t, n) in &type_hist {
        print!("{t}={n} ");
    }
    println!("\n");

    // ── Build target Memory with Anthropic extractor ──────────
    let target_path = args.target.to_string_lossy().to_string();
    let mut mem = match Memory::new(&target_path, None) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Memory::new: {e}");
            return ExitCode::from(2);
        }
    };

    let extractor = AnthropicExtractor::new(&args.api_key, args.is_oauth);
    mem.set_extractor(Box::new(extractor));
    println!("🧠 Target Memory initialized with AnthropicExtractor (oauth={})", args.is_oauth);
    println!();

    // ── Replay ────────────────────────────────────────────────
    let t_start = std::time::Instant::now();
    let mut replayed = 0_u64;
    let mut errs = 0_u64;
    for (i, row) in sample.iter().enumerate() {
        let meta = StorageMeta {
            memory_type_hint: Some(parse_memory_type(&row.memory_type)),
            importance_hint: Some(row.importance),
            namespace: Some(row.namespace.clone()),
            source: Some(format!("iss019-pilot:src={}", row.id)),
            ..Default::default()
        };
        // Note: created_at from source is not propagated — target
        // rows get "now". This is fine for coverage assertions which
        // look at metadata content, not timestamps.
        let _ = row.created_at; // silence unused

        match mem.store_raw(&row.content, meta) {
            Ok(RawStoreOutcome::Stored(_))
            | Ok(RawStoreOutcome::Skipped { .. })
            | Ok(RawStoreOutcome::Quarantined { .. }) => {
                replayed += 1;
            }
            Err(e) => {
                errs += 1;
                eprintln!("  [{}] store_raw error: {e}", i);
            }
        }

        if (i + 1) % 20 == 0 {
            println!("   progress: {}/{}", i + 1, sample.len());
        }
    }
    let elapsed = t_start.elapsed();
    println!();
    println!("⏱  Replay done: {} rows in {:.1}s ({} errors)", replayed, elapsed.as_secs_f64(), errs);

    // ── Snapshot stats ────────────────────────────────────────
    let stats = match mem.write_stats() {
        Some(s) => s,
        None => {
            eprintln!("write_stats unavailable — Memory not built with a CountingSink");
            return ExitCode::from(2);
        }
    };

    println!();
    println!("📊 WriteStats:");
    println!("   stored_count      = {}", stats.stored_count);
    println!("   stored_fact_count = {}", stats.stored_fact_count);
    println!("   merged_count      = {}", stats.merged_count);
    println!("   skipped_count     = {}", stats.skipped_count);
    println!("   quarantined_count = {}", stats.quarantined_count);
    println!("   total_calls       = {}", stats.total_calls());
    println!("   coverage          = {:.2}%", stats.coverage() * 100.0);
    if !stats.skipped_by_reason.is_empty() {
        println!("   skip reasons:");
        for (r, n) in &stats.skipped_by_reason {
            println!("     {:?} = {}", r, n);
        }
    }
    println!();

    // Drop mem to close DB cleanly before we re-open for coverage scan.
    drop(mem);

    // ── Dimensional coverage scan on target DB ────────────────
    let cov = match DimensionalCoverage::collect(&args.target) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("coverage collect: {e}");
            return ExitCode::from(2);
        }
    };
    println!("🎯 DimensionalCoverage (target DB):");
    println!("   total_rows             = {}", cov.total_rows);
    println!("   with engram.dimensions = {} ({:.1}%)",
        cov.rows_with_dimensions_block, cov.pct(cov.rows_with_dimensions_block) * 100.0);
    println!("   participants present   = {} ({:.1}%)",
        cov.rows_with_participants, cov.pct(cov.rows_with_participants) * 100.0);
    println!("   temporal present       = {} ({:.1}%)",
        cov.rows_with_temporal, cov.pct(cov.rows_with_temporal) * 100.0);
    println!("   causation present      = {} ({:.1}%)",
        cov.rows_with_causation, cov.pct(cov.rows_with_causation) * 100.0);
    println!();

    // ── Assertions ────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Assertions (§9)");
    println!("═══════════════════════════════════════════════════════");
    let mut failed = 0;

    let a1 = stats.coverage() > 0.95;
    println!("  [{}] stored/(total) > 0.95   — actual {:.2}%",
        if a1 { "✅" } else { "❌" }, stats.coverage() * 100.0);
    if !a1 { failed += 1; }

    let part_pct = cov.pct(cov.rows_with_participants);
    let a2 = part_pct > 0.60;
    println!("  [{}] participants > 60%      — actual {:.1}%",
        if a2 { "✅" } else { "❌" }, part_pct * 100.0);
    if !a2 { failed += 1; }

    let temp_pct = cov.pct(cov.rows_with_temporal);
    let a3 = temp_pct > 0.40;
    println!("  [{}] temporal > 40%          — actual {:.1}%",
        if a3 { "✅" } else { "❌" }, temp_pct * 100.0);
    if !a3 { failed += 1; }

    let cause_pct = cov.pct(cov.rows_with_causation);
    let a4 = cause_pct > 0.30;
    println!("  [{}] causation > 30%         — actual {:.1}%",
        if a4 { "✅" } else { "❌" }, cause_pct * 100.0);
    if !a4 { failed += 1; }

    let missing = cov.total_rows - cov.rows_with_dimensions_block;
    let a5 = missing == 0;
    println!("  [{}] zero rows missing engram.dimensions — actual {} missing",
        if a5 { "✅" } else { "❌" }, missing);
    if !a5 { failed += 1; }

    println!();
    if failed == 0 {
        println!("🟢 ALL ASSERTIONS PASSED — Step 9 green. Cleared for Step 10 (58MB full).");
        ExitCode::SUCCESS
    } else {
        println!("🔴 {} assertion(s) FAILED — do not proceed to Step 10.", failed);
        ExitCode::from(1)
    }
}
