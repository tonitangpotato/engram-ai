//! # Fused-pool diagnostic dump (ISS-175)
//!
//! `task:retr-iss175-probe` — env-gated, off by default. When enabled,
//! every call to [`crate::retrieval::fusion::fuse_and_rank`] writes its
//! complete post-fusion / post-sort / pre-truncation candidate pool to
//! disk as JSONL. This exists to diagnose ISS-175 (Factual fusion
//! reweighting): Factual queries surface 150-263 candidates and we
//! need to know where the gold answer sits in that pool — top-K dumps
//! (engram-bench `ENGRAM_BENCH_DUMP_CANDIDATES=1`) cannot see rank 50+.
//!
//! ## Contract
//!
//! - **Default behaviour**: no env vars set → zero work, zero
//!   allocation, byte-identical output to pre-ISS-175 callers. The
//!   hot path is a single thread-local read + an env var read cached
//!   in a `OnceCell`.
//! - **Activation**: caller (the benchmark driver, typically) sets
//!   `ENGRAM_DUMP_FUSED_POOL_DIR=/some/dir` (the directory must
//!   already exist — we don't `mkdir -p` because that hides typos)
//!   AND attaches a query-id label via
//!   [`set_dump_label`] before invoking retrieval.
//! - **Filename**: `<dir>/<label>-<intent>.jsonl`. Label is the
//!   caller-provided opaque string (driver's qid, typically); intent
//!   is the lowercase `Intent::Debug`. Existing files are appended
//!   to, not overwritten — multiple calls for the same (label,
//!   intent) accumulate so plan-internal `fuse_and_rank` invocations
//!   (e.g. inside Hybrid sub-plans) are not lost.
//! - **Optional whitelist**: `ENGRAM_DUMP_FUSED_POOL_QIDS=q40,q43,q71,q75`
//!   filters by label so a full bench run produces dumps for only
//!   the queries under investigation (Factual SF qids). Unset =
//!   dump all labelled queries.
//! - **Determinism**: this module is pure I/O on the side. It MUST
//!   NOT mutate `candidates` or short-circuit any later stage.
//!   Failures (disk full, permission denied) are logged via
//!   `eprintln!` and swallowed — diagnostic instrumentation must
//!   never break the production path.
//!
//! ## Output format
//!
//! One JSON object per line, in fused-and-sorted order:
//!
//! ```json
//! {
//!   "label": "q40",
//!   "intent": "factual",
//!   "rank": 1,
//!   "memory_id": "uuid…",
//!   "score": 0.81,
//!   "graph_score": 0.75,
//!   "bm25_score": 0.0,
//!   "vector_score": 0.42,
//!   "recency_score": null,
//!   "actr_score": null,
//!   "affect_similarity": null,
//!   "kind": "memory",
//!   "content_head": "Caroline went to the support group on…"
//! }
//! ```
//!
//! Topic candidates use `"kind": "topic"` and put the topic title in
//! `content_head`; the six subscore fields are all `null` (topics
//! never carry signal-level breakdown — Abstract plan scores them
//! directly).
//!
//! ## Why a thread-local for the label
//!
//! The label is conceptually per-query, but `fuse_and_rank` is a
//! pure function several layers below the API surface. Threading a
//! `query_id: Option<&str>` through `execute_plan` → plan adapter →
//! `fuse_and_rank` would pollute four call signatures and force
//! every test fixture to pick a name. Thread-locals are the standard
//! out-of-band channel for this in Rust diagnostic infrastructure
//! (see `tracing`'s span machinery). The benchmark driver
//! [`set_dump_label`] before each query and [`clear_dump_label`]
//! after; if the driver forgets, the dump still works but lands
//! under `unlabelled` and the operator can grep `unlabelled` to
//! catch the leak.

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;

use crate::retrieval::api::ScoredResult;
use crate::retrieval::classifier::Intent;

thread_local! {
    /// Per-thread label attached to the next `fuse_and_rank` dump.
    /// `None` = no label; the dump (if env-enabled) writes under
    /// `unlabelled` so leaks surface to operator inspection. Cleared
    /// by [`clear_dump_label`] after each query.
    static DUMP_LABEL: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the dump label for subsequent `fuse_and_rank` calls on this
/// thread. Idempotent on the same value; overwrites if different.
/// Pair with [`clear_dump_label`] after retrieval completes.
pub fn set_dump_label<S: Into<String>>(label: S) {
    DUMP_LABEL.with(|cell| {
        *cell.borrow_mut() = Some(label.into());
    });
}

/// Clear the dump label. Safe to call when no label is set (no-op).
pub fn clear_dump_label() {
    DUMP_LABEL.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

#[cfg(test)]
fn peek_dump_label() -> Option<String> {
    DUMP_LABEL.with(|cell| cell.borrow().clone())
}

/// Cached env state — read once per process to keep the hot path
/// branch-light. `None` variants mean the relevant env var was unset
/// at the time of first access; subsequent changes are ignored on
/// purpose (eval runs MUST have a single fixed dump configuration).
#[derive(Debug)]
struct DumpEnv {
    dir: Option<PathBuf>,
    qid_whitelist: Option<Vec<String>>,
}

static DUMP_ENV: OnceLock<DumpEnv> = OnceLock::new();

fn dump_env() -> &'static DumpEnv {
    DUMP_ENV.get_or_init(|| {
        let dir = std::env::var("ENGRAM_DUMP_FUSED_POOL_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let qid_whitelist = std::env::var("ENGRAM_DUMP_FUSED_POOL_QIDS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<String>| !v.is_empty());
        DumpEnv { dir, qid_whitelist }
    })
}

/// Hot-path entry point — called from
/// [`crate::retrieval::fusion::combiner::fuse_and_rank`] right after
/// sort, right before return. The fast path (dump disabled) is a
/// single pointer comparison: `dump_env().dir.is_none()`.
///
/// This function NEVER panics. All I/O failures are logged and
/// swallowed.
pub(crate) fn maybe_dump_fused_pool(intent: Intent, candidates: &[ScoredResult]) {
    let env = dump_env();
    let Some(dir) = env.dir.as_ref() else {
        return; // fast path — diagnostic off
    };

    let label = DUMP_LABEL.with(|cell| cell.borrow().clone());
    let label_str = label.unwrap_or_else(|| "unlabelled".to_string());

    // Whitelist filter — when set, skip dumps for labels not on the
    // list. "unlabelled" is never on the whitelist (intentional —
    // the operator gave a specific qid list and "unlabelled" means
    // the driver forgot to set one, which is a bug to surface, not
    // a query to keep).
    if let Some(allow) = env.qid_whitelist.as_ref() {
        if !allow.iter().any(|x| x == &label_str) {
            return;
        }
    }

    if let Err(e) = write_dump(dir, &label_str, intent, candidates) {
        eprintln!(
            "[engram::fusion::dump] failed to write fused pool dump \
             label={label_str} intent={intent:?}: {e}"
        );
    }
}

fn write_dump(
    dir: &Path,
    label: &str,
    intent: Intent,
    candidates: &[ScoredResult],
) -> std::io::Result<()> {
    let intent_str = intent_to_str(intent);
    let path = dir.join(format!("{label}-{intent_str}.jsonl"));

    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    for (i, c) in candidates.iter().enumerate() {
        let row = project_row(label, intent_str, i + 1, c);
        let line = serde_json::to_string(&row).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
    }
    f.flush()?;
    Ok(())
}

fn intent_to_str(intent: Intent) -> &'static str {
    intent.as_str()
}

const CONTENT_HEAD_CHARS: usize = 200;

#[derive(Debug, Serialize)]
struct DumpRow<'a> {
    label: &'a str,
    intent: &'a str,
    rank: usize,
    memory_id: String,
    score: f64,
    graph_score: Option<f64>,
    bm25_score: Option<f64>,
    vector_score: Option<f64>,
    recency_score: Option<f64>,
    actr_score: Option<f64>,
    affect_similarity: Option<f64>,
    kind: &'static str,
    content_head: String,
}

fn project_row<'a>(
    label: &'a str,
    intent_str: &'a str,
    rank: usize,
    c: &ScoredResult,
) -> DumpRow<'a> {
    match c {
        ScoredResult::Memory { record, score, sub_scores, .. } => DumpRow {
            label,
            intent: intent_str,
            rank,
            memory_id: record.id.clone(),
            score: *score,
            graph_score: sub_scores.graph_score,
            bm25_score: sub_scores.bm25_score,
            vector_score: sub_scores.vector_score,
            recency_score: sub_scores.recency_score,
            actr_score: sub_scores.actr_score,
            affect_similarity: sub_scores.affect_similarity,
            kind: "memory",
            content_head: truncate_chars(&record.content, CONTENT_HEAD_CHARS),
        },
        ScoredResult::Topic { topic, score, .. } => DumpRow {
            label,
            intent: intent_str,
            rank,
            memory_id: topic.topic_id.to_string(),
            score: *score,
            graph_score: None,
            bm25_score: None,
            vector_score: None,
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
            kind: "topic",
            content_head: truncate_chars(&topic.title, CONTENT_HEAD_CHARS),
        },
    }
}

/// UTF-8-safe char-bounded truncation. Mirrors
/// `engram-bench::drivers::locomo::truncate_chars` but kept local to
/// avoid a cross-crate dep for diagnostic-only code.
fn truncate_chars(s: &str, max: usize) -> String {
    let mut end = s.len();
    let mut count = 0;
    for (idx, _) in s.char_indices() {
        if count == max {
            end = idx;
            break;
        }
        count += 1;
    }
    if end == s.len() {
        s.to_string()
    } else {
        let mut out = s[..end].to_string();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::api::{ScoredResult, SubScores};
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};

    fn mk_record(id: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            content: format!("content of {id}"),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
            occurred_at: None,
            access_times: vec![],
            working_strength: 0.0,
            core_strength: 0.0,
            importance: 0.0,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    fn mem(id: &str, score: f64, sub: SubScores) -> ScoredResult {
        ScoredResult::Memory {
            record: mk_record(id),
            score,
            sub_scores: sub,
            embedding: None,
        }
    }

    #[test]
    fn label_set_clear_roundtrip() {
        assert_eq!(peek_dump_label(), None);
        set_dump_label("q40");
        assert_eq!(peek_dump_label(), Some("q40".to_string()));
        clear_dump_label();
        assert_eq!(peek_dump_label(), None);
    }

    #[test]
    fn truncate_is_utf8_safe() {
        // multi-byte char must not be split
        let s = "你好世界Aあいうえお";
        let out = truncate_chars(s, 5);
        assert!(out.ends_with('…'));
        // First 5 chars: 你好世界A
        assert!(out.starts_with("你好世界A"));
        // No truncation when within limit
        let short = "abc";
        assert_eq!(truncate_chars(short, 10), "abc");
    }

    #[test]
    fn project_row_memory_carries_all_subscores() {
        let sub = SubScores {
            vector_score: Some(0.42),
            bm25_score: Some(0.0),
            graph_score: Some(0.75),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let c = mem("mem-1", 0.81, sub);
        let row = project_row("q40", "factual", 1, &c);
        assert_eq!(row.label, "q40");
        assert_eq!(row.intent, "factual");
        assert_eq!(row.rank, 1);
        assert_eq!(row.memory_id, "mem-1");
        assert!((row.score - 0.81).abs() < 1e-9);
        assert_eq!(row.graph_score, Some(0.75));
        assert_eq!(row.bm25_score, Some(0.0));
        assert_eq!(row.vector_score, Some(0.42));
        assert_eq!(row.recency_score, None);
        assert_eq!(row.kind, "memory");
        assert!(row.content_head.contains("mem-1"));
    }

    #[test]
    fn write_dump_appends_jsonl_lines() {
        // Use a unique tempdir per test so concurrent tests on the
        // same OnceLock'd dump_env don't fight. We bypass dump_env
        // here and drive write_dump directly — that's the unit
        // under test.
        let dir = tempfile::tempdir().expect("tempdir");
        let sub_a = SubScores {
            vector_score: Some(0.4),
            bm25_score: Some(0.1),
            graph_score: Some(0.7),
            ..Default::default()
        };
        let sub_b = SubScores {
            vector_score: Some(0.3),
            bm25_score: Some(0.0),
            graph_score: Some(0.5),
            ..Default::default()
        };
        let cands = vec![mem("a", 0.6, sub_a), mem("b", 0.45, sub_b)];

        write_dump(dir.path(), "q40", Intent::Factual, &cands).expect("write");

        let path = dir.path().join("q40-factual.jsonl");
        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);

        let row0: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(row0["label"], "q40");
        assert_eq!(row0["intent"], "factual");
        assert_eq!(row0["rank"], 1);
        assert_eq!(row0["memory_id"], "a");
        assert!((row0["score"].as_f64().unwrap() - 0.6).abs() < 1e-9);
        assert!((row0["graph_score"].as_f64().unwrap() - 0.7).abs() < 1e-9);
        assert!((row0["bm25_score"].as_f64().unwrap() - 0.1).abs() < 1e-9);
        assert!((row0["vector_score"].as_f64().unwrap() - 0.4).abs() < 1e-9);
        assert!(row0["recency_score"].is_null());
        assert_eq!(row0["kind"], "memory");

        let row1: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(row1["rank"], 2);
        assert_eq!(row1["memory_id"], "b");

        // Second call appends (does not truncate).
        write_dump(dir.path(), "q40", Intent::Factual, &cands[..1]).expect("append");
        let body2 = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body2.lines().count(), 3, "append must not truncate");
    }

    #[test]
    fn maybe_dump_fused_pool_no_op_when_env_unset() {
        // dump_env() is process-cached via OnceLock — if some other
        // test set ENGRAM_DUMP_FUSED_POOL_DIR before this test ran,
        // env.dir will be Some. Skip the assertion in that case
        // (the no-op path is exercised by 99.9% of the test suite).
        if dump_env().dir.is_some() {
            return;
        }
        let cands: Vec<ScoredResult> = vec![];
        // Must not panic, must not allocate, must not write anywhere.
        maybe_dump_fused_pool(Intent::Factual, &cands);
    }

    #[test]
    fn intent_to_str_covers_known_variants() {
        // Pin runtime string contract — analysis scripts depend on
        // these lowercased names. Delegates to Intent::as_str() so
        // any new variant is automatically picked up.
        assert_eq!(intent_to_str(Intent::Factual), "factual");
        assert_eq!(intent_to_str(Intent::Episodic), "episodic");
        assert_eq!(intent_to_str(Intent::Abstract), "abstract");
        assert_eq!(intent_to_str(Intent::Affective), "affective");
        assert_eq!(intent_to_str(Intent::Hybrid), "hybrid");
    }
}
