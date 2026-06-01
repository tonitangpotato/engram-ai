//! T26a: resumable triple-extraction backfill driver.
//!
//! Iterates memories whose content has not yet been triple-extracted,
//! calls an injected `TripleExtractor` per memory, and persists the
//! results via `Storage::store_triples`. State is checkpointed to the
//! `triple_backfill_checkpoint` table so a crashed run resumes from the
//! last successful memory_id rather than restarting from scratch.
//!
//! # Design intent (see `.gid/features/v04-unified-substrate/design.md` §8.4 T26a)
//!
//! - **Infrastructure only.** No live API calls are made by this module;
//!   the extractor is a trait object the caller injects. Tests use
//!   `NoopTripleExtractor` and a counted mock; production wires
//!   `AnthropicTripleExtractor`.
//! - **Resumable.** A checkpoint row is upserted after every successful
//!   memory. On restart, the driver picks up at `memory_id >
//!   last_memory_id`.
//! - **Rate-limited.** A simple token-bucket (interval = 1/rps) is
//!   enforced between extractor calls to respect upstream API limits.
//! - **Retry.** Per-memory exponential backoff up to `max_retries`
//!   before counting that memory as `failed` and continuing.
//! - **Audit.** Emits a `BackfillRun` row on completion via
//!   `backfill_runs` (legacy_table = `"triples"`). Counter invariant
//!   matches the parent crate convention: `rows_read = rows_inserted +
//!   rows_skipped_existing + rows_failed`.
//!
//! # What "no live API calls" means in the test suite
//!
//! Every test in `tests/v04_phase_c_triple_backfill.rs` injects either
//! `NoopTripleExtractor` (returns empty Vec) or a local
//! `CountingMockExtractor` that returns canned `Triple` lists and can
//! be programmed to fail-then-succeed for retry-path coverage. Cargo
//! test never reaches a real Anthropic or Ollama endpoint.

use std::error::Error;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::params;
use serde_json::json;
use uuid::Uuid;

use crate::storage::Storage;
use crate::substrate::backfill::BackfillRun;
use crate::triple_extractor::TripleExtractor;

/// ISS-128: cap on failed memory_ids persisted in the audit notes
/// JSON. At 14k-memory runs we've seen up to ~5k failures (T26c);
/// 5000 IDs at ~40 chars each ≈ 200 KB, well within SQLite's TEXT
/// comfort zone. Beyond this we set `failed_ids_truncated: true`
/// and stop pushing — the operator gets a clear signal that the
/// list is incomplete rather than an unbounded JSON blob.
const FAILED_IDS_CAP: usize = 10_000;

/// Driver options for the triple backfill.
///
/// Defaults are tuned for the LoCoMo-scale dev DB (~5k memories); the
/// 24k production run reuses the same struct via
/// `TripleBackfillOpts::production()`.
#[derive(Debug, Clone)]
pub struct TripleBackfillOpts {
    /// Memories fetched per SQL page. SQLite handles 256 well; larger
    /// pages risk holding a long-lived statement on the read side.
    pub batch_size: usize,
    /// Maximum extractor calls per second. `f64::INFINITY` disables
    /// rate limiting (tests use this).
    pub rate_limit_per_sec: f64,
    /// How many times to retry a single memory before counting it as
    /// failed. `0` = try once, no retries.
    pub max_retries: u32,
    /// Backoff base (ms) for the first retry. Subsequent retries
    /// double (`backoff * 2^attempt`).
    pub retry_backoff_ms: u64,
    /// Restrict to a single namespace. `None` = all namespaces.
    pub namespace_filter: Option<String>,
    /// Hard cap on memories processed in this invocation. `None` =
    /// unlimited. Used by the dry-run (T26b) path to cap a sample.
    pub max_memories: Option<u64>,
}

impl Default for TripleBackfillOpts {
    fn default() -> Self {
        Self {
            batch_size: 64,
            rate_limit_per_sec: f64::INFINITY,
            max_retries: 0,
            retry_backoff_ms: 250,
            namespace_filter: None,
            max_memories: None,
        }
    }
}

impl TripleBackfillOpts {
    /// Conservative defaults for the 24k-row production run.
    pub fn production() -> Self {
        Self {
            batch_size: 128,
            rate_limit_per_sec: 5.0,
            max_retries: 3,
            retry_backoff_ms: 1_000,
            namespace_filter: None,
            max_memories: None,
        }
    }
}

/// Run the triple backfill, resuming from any prior in-progress run.
///
/// Returns a `BackfillRun` whose `rows_read` is the number of memories
/// inspected by this invocation and `rows_inserted` is the **total
/// triples** written. `rows_skipped_existing` counts memories that
/// already had at least one triple row (idempotent skip). `rows_failed`
/// counts memories that exhausted `max_retries`.
///
/// **Counter invariant.** `rows_read = rows_inserted_memories +
/// rows_skipped_existing + rows_failed` (where `rows_inserted_memories`
/// is memories that produced ≥1 triple). We surface `rows_inserted`
/// as the **triple count** for audit usefulness; the per-memory
/// invariant is recorded in `notes.memories_inserted`.
pub fn backfill_triples_from_memories(
    storage: &Storage,
    extractor: &dyn TripleExtractor,
    opts: &TripleBackfillOpts,
) -> Result<BackfillRun, Box<dyn Error + Send + Sync>> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    // Open audit + checkpoint rows immediately so a crash is detectable.
    let notes = json!({
        "driver": "backfill_triples_from_memories",
        "design_ref": "v04-unified-substrate §8.4 T26a",
        "rate_limit_per_sec": opts.rate_limit_per_sec,
        "max_retries": opts.max_retries,
        "namespace_filter": opts.namespace_filter,
    })
    .to_string();
    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'triples', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes],
    )?;
    storage.conn().execute(
        r#"
        INSERT INTO triple_backfill_checkpoint (
            run_id, last_memory_id, memories_processed, triples_inserted,
            memories_failed, status, started_at, updated_at,
            namespace_filter, notes
        ) VALUES (?, NULL, 0, 0, 0, 'in_progress', ?, ?, ?, '{}')
        "#,
        params![run_id, started_at, started_at, opts.namespace_filter],
    )?;

    // Resume cursor: if an earlier run exists in-progress for the same
    // namespace, pick up its `last_memory_id`. Latest-wins on started_at.
    let mut cursor: Option<String> = storage
        .conn()
        .query_row(
            r#"
        SELECT last_memory_id FROM triple_backfill_checkpoint
        WHERE status = 'in_progress' AND run_id != ?
              AND (namespace_filter IS ? OR namespace_filter = ?)
        ORDER BY started_at DESC LIMIT 1
        "#,
            params![run_id, opts.namespace_filter, opts.namespace_filter],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None);

    let mut rows_read: u64 = 0;
    let mut memories_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_failed: u64 = 0;
    let mut triples_inserted_total: u64 = 0;
    // ISS-128: persist failed memory_ids in the audit notes JSON so a
    // rerun (ISS-129) can target only the failures rather than
    // re-iterating the full corpus. Cap at FAILED_IDS_CAP to keep the
    // JSON blob bounded on pathological runs; the truncation flag tells
    // the operator that the list is incomplete.
    let mut failed_memory_ids: Vec<String> = Vec::new();
    let mut failed_ids_truncated: bool = false;
    let mut last_error_message: Option<String> = None;
    let min_interval = if opts.rate_limit_per_sec.is_finite() && opts.rate_limit_per_sec > 0.0 {
        Some(Duration::from_secs_f64(1.0 / opts.rate_limit_per_sec))
    } else {
        None
    };
    let mut last_call_at: Option<Instant> = None;

    'outer: loop {
        // Fetch next page of memory_ids strictly greater than cursor.
        // Soft-deleted rows excluded — matches the read-switch contract.
        let page = fetch_memory_page(storage, cursor.as_deref(), opts)?;
        if page.is_empty() {
            break;
        }
        for (memory_id, content) in page {
            if let Some(cap) = opts.max_memories {
                if rows_read >= cap {
                    break 'outer;
                }
            }
            rows_read += 1;

            // Idempotent skip: if any triple row already exists for
            // this memory, count as skipped without invoking extractor.
            let existing: i64 = storage.conn().query_row(
                "SELECT COUNT(1) FROM triples WHERE memory_id = ?",
                params![memory_id],
                |r| r.get(0),
            )?;
            if existing > 0 {
                rows_skipped_existing += 1;
                cursor = Some(memory_id.clone());
                update_checkpoint(
                    storage,
                    &run_id,
                    &memory_id,
                    rows_read - rows_failed,
                    triples_inserted_total,
                    rows_failed,
                )?;
                continue;
            }

            // Rate limit gate.
            if let (Some(iv), Some(prev)) = (min_interval, last_call_at) {
                let elapsed = prev.elapsed();
                if elapsed < iv {
                    thread::sleep(iv - elapsed);
                }
            }
            last_call_at = Some(Instant::now());

            // Try-with-retry.
            let mut attempt: u32 = 0;
            let result = loop {
                match extractor.extract_triples(&content) {
                    Ok(triples) => break Ok(triples),
                    Err(e) if attempt < opts.max_retries => {
                        let backoff = opts.retry_backoff_ms.saturating_mul(1u64 << attempt);
                        attempt += 1;
                        thread::sleep(Duration::from_millis(backoff));
                        // Reset the rate-limit clock so the next attempt
                        // doesn't double-sleep.
                        last_call_at = Some(Instant::now());
                        let _ = e;
                    }
                    Err(e) => break Err(e),
                }
            };

            match result {
                Ok(triples) => {
                    let inserted = storage.store_triples(&memory_id, &triples)?;
                    // Count every successful extraction as "processed",
                    // even when the extractor returned an empty Vec
                    // (Noop, or LLM judged content un-triple-able). The
                    // memory was inspected and the extractor succeeded
                    // — that's the meaningful bucket for invariant
                    // accounting. `triples_inserted_total` carries the
                    // distinct "how many rows did we write" signal.
                    memories_inserted += 1;
                    triples_inserted_total += inserted as u64;
                }
                Err(e) => {
                    rows_failed += 1;
                    // ISS-128: record the failing memory_id and the
                    // final error message so a follow-up rerun can
                    // target the failures specifically. Bounded at
                    // FAILED_IDS_CAP.
                    if failed_memory_ids.len() < FAILED_IDS_CAP {
                        failed_memory_ids.push(memory_id.clone());
                    } else {
                        failed_ids_truncated = true;
                    }
                    last_error_message = Some(e.to_string());
                }
            }
            cursor = Some(memory_id.clone());
            update_checkpoint(
                storage,
                &run_id,
                &memory_id,
                rows_read - rows_failed,
                triples_inserted_total,
                rows_failed,
            )?;
        }
    }

    // Final audit + checkpoint flip.
    let finished_at = utc_now_f64();
    let final_notes = json!({
        "driver": "backfill_triples_from_memories",
        "design_ref": "v04-unified-substrate §8.4 T26a",
        "memories_inserted": memories_inserted,
        "triples_inserted_total": triples_inserted_total,
        "rate_limit_per_sec": opts.rate_limit_per_sec,
        "max_retries": opts.max_retries,
        "namespace_filter": opts.namespace_filter,
        // ISS-128: failure forensics. `failed_memory_ids` is the list
        // of memory_ids that exhausted max_retries this run. Capped at
        // FAILED_IDS_CAP — if `failed_ids_truncated` is true, more
        // failures exist than are listed. `last_error_message` is the
        // most recent extractor Err.to_string() for quick visual
        // triage; full per-id error capture is a future enhancement.
        "failed_memory_ids": failed_memory_ids,
        "failed_ids_truncated": failed_ids_truncated,
        "last_error_message": last_error_message,
    })
    .to_string();
    storage.conn().execute(
        r#"
        UPDATE backfill_runs
           SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
               rows_failed = ?, finished_at = ?, notes = ?
         WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            triples_inserted_total as i64,
            rows_skipped_existing as i64,
            rows_failed as i64,
            finished_at,
            final_notes,
            run_id,
        ],
    )?;
    storage.conn().execute(
        r#"
        UPDATE triple_backfill_checkpoint
           SET status = 'completed', updated_at = ?,
               memories_processed = ?, triples_inserted = ?,
               memories_failed = ?
         WHERE run_id = ?
        "#,
        params![
            finished_at,
            (rows_read - rows_failed) as i64,
            triples_inserted_total as i64,
            rows_failed as i64,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "triples".to_string(),
        rows_read,
        rows_inserted: triples_inserted_total,
        rows_skipped_existing,
        rows_failed,
    };
    // Sanity: memories_inserted + skipped + failed = rows_read.
    debug_assert_eq!(
        memories_inserted + rows_skipped_existing + rows_failed,
        rows_read,
        "T26a per-memory counter invariant broken: \
         inserted({}) + skipped({}) + failed({}) != read({})",
        memories_inserted,
        rows_skipped_existing,
        rows_failed,
        rows_read
    );
    Ok(run)
}

/// Fetch the next page of `(memory_id, content)` strictly greater
/// than `cursor`, ordered by `id` (lexicographic — stable for resume).
/// Soft-deleted memories are skipped.
fn fetch_memory_page(
    storage: &Storage,
    cursor: Option<&str>,
    opts: &TripleBackfillOpts,
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let cursor_clause = if cursor.is_some() { "AND id > ?" } else { "" };
    let ns_clause = if opts.namespace_filter.is_some() {
        "AND namespace = ?"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id, content FROM memories \
         WHERE deleted_at IS NULL {} {} \
         ORDER BY id ASC LIMIT ?",
        cursor_clause, ns_clause
    );
    let mut stmt = storage.conn().prepare(&sql)?;

    // rusqlite needs typed params; build the list manually.
    let limit = opts.batch_size as i64;
    let rows: Vec<(String, String)> = match (cursor, opts.namespace_filter.as_deref()) {
        (Some(c), Some(ns)) => stmt
            .query_map(params![c, ns, limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?,
        (Some(c), None) => stmt
            .query_map(params![c, limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?,
        (None, Some(ns)) => stmt
            .query_map(params![ns, limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?,
        (None, None) => stmt
            .query_map(params![limit], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?,
    };
    Ok(rows)
}

fn update_checkpoint(
    storage: &Storage,
    run_id: &str,
    last_memory_id: &str,
    memories_processed: u64,
    triples_inserted: u64,
    memories_failed: u64,
) -> Result<(), rusqlite::Error> {
    storage.conn().execute(
        r#"
        UPDATE triple_backfill_checkpoint
           SET last_memory_id = ?, memories_processed = ?,
               triples_inserted = ?, memories_failed = ?,
               updated_at = ?
         WHERE run_id = ?
        "#,
        params![
            last_memory_id,
            memories_processed as i64,
            triples_inserted as i64,
            memories_failed as i64,
            utc_now_f64(),
            run_id,
        ],
    )?;
    Ok(())
}

fn utc_now_f64() -> f64 {
    let now = chrono::Utc::now();
    now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64 / 1e9)
}
