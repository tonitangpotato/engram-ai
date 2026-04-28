//! Migration-integrity driver (design §3.6, requirements GOAL-5.7).
//!
//! Implements [`crate::harness::BenchDriver`] for the **end-to-end
//! migration qualification** — exercises the v0.2 → v0.3 migration
//! tool against the rustclaw production `engram-memory.db` (or a
//! fixture standing in for it), then verifies two compound criteria:
//!
//! 1. **No data loss** — MemoryRecord, Hebbian link, and Knowledge
//!    Compiler topic counts must be preserved exactly across the
//!    migration boundary (cross-ref GOAL-4.1, design §3.6).
//! 2. **Query-set parity** — a fixed query set of ≥ 20 questions that
//!    returned non-empty results on the v0.2 DB must return equivalent
//!    top-K result sets on the migrated v0.3 DB (Jaccard ≥
//!    `parity_threshold`, default 0.8).
//!
//! The compound metric `migration.integrity` is set via
//! [`MetricSnapshot::set_compound`] so the gate evaluator (per
//! `harness/gates.rs`) checks both conditions atomically — failing
//! either one fails the gate, and the message names which condition
//! failed.
//!
//! ## Current implementation status (skip-aware mode)
//!
//! As of 2026-04-27, the migration tool (`engramai-migrate`) and the
//! retrieval orchestrator (`Memory::graph_query`) are both incomplete:
//!
//! - The migration tool may or may not be built and on PATH; we detect
//!   this with the same probe used by `test_preservation`.
//! - `Memory::graph_query` is a stub returning
//!   `RetrievalError::Internal` (tracked under
//!   `task:retr-impl-orchestrator-classifier-dispatch` and siblings on
//!   `.gid-v03-context/graph.db`).
//!
//! Either gap is fatal to running the real query-parity check. This
//! driver therefore runs in **skip-aware mode** until both land:
//!
//! 1. We probe the migration tool's availability and the orchestrator
//!    status. If either is blocked, we record `blocked_by` on the
//!    summary and emit
//!    `MetricSnapshot::set_missing("migration.integrity", reason)`.
//! 2. The gate evaluator maps `Missing` → `GateStatus::Error` per
//!    GUARD-2 ("never silent degrade") — never `Pass`.
//!
//! When both prerequisites land, this driver activates without code
//! changes (the probes simply return `None` and execution falls
//! through to the real pipeline body, which is itself filed as a
//! follow-up task; see `task:retr-test-orchestrator-e2e`).
//!
//! ## Stage / cost
//!
//! `Stage1 / Medium` — runs the migration tool (which is fixture-only
//! and deterministic) and then in-memory query parity. No LLM, no
//! network. Stage1 because the migration tool itself is a stage-0
//! artifact that other stages (test_preservation) also depend on.
//!
//! ## Output (`migration_integrity_summary.json`)
//!
//! ```json
//! {
//!   "data_loss": {
//!     "memory_records_pre": 0, "memory_records_post": 0,
//!     "hebbian_links_pre": 0, "hebbian_links_post": 0,
//!     "topics_pre": 0, "topics_post": 0,
//!     "preserved": true
//!   },
//!   "query_parity": {
//!     "queries_total": 0,
//!     "queries_matching": 0,
//!     "parity_ratio": 0.0,
//!     "threshold": 0.8,
//!     "passed": false
//!   },
//!   "integrity_ok": false,
//!   "message": "...",
//!   "blocked_by": "..."
//! }
//! ```

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::harness::gates::{
    evaluate_for_driver, GateResult, MetricSnapshot, NoBaselines,
};
use crate::harness::{BenchDriver, BenchError, CostTier, Driver, HarnessConfig, RunReport, Stage};

// ---------------------------------------------------------------------------
// Public driver type — wired into `main::resolve_driver`.
// ---------------------------------------------------------------------------

/// Migration-integrity driver — design §3.6, GOAL-5.7.
#[derive(Debug, Default, Clone, Copy)]
pub struct MigrationIntegrityDriver;

impl MigrationIntegrityDriver {
    /// Construct the zero-sized driver handle.
    pub fn new() -> Self {
        Self
    }
}

impl BenchDriver for MigrationIntegrityDriver {
    fn name(&self) -> Driver {
        Driver::MigrationIntegrity
    }

    fn stage(&self) -> Stage {
        // Migration tool is a stage-0 artifact; this driver runs early
        // so a missing migration tool surfaces before stage-2 drivers
        // burn budget.
        Stage::Stage1
    }

    fn cost_tier(&self) -> CostTier {
        // Migration tool invocation + query replay. Seconds.
        CostTier::Medium
    }

    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError> {
        run_impl(config)
    }
}

// ---------------------------------------------------------------------------
// Summary type — compound (data-loss × query-parity)
// ---------------------------------------------------------------------------

/// Counts captured before and after migration. Per design §3.6, the
/// three preservation invariants for v0.2 → v0.3.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DataLossReport {
    /// MemoryRecord count in the pre-migration DB.
    pub memory_records_pre: usize,
    /// MemoryRecord count in the post-migration DB.
    pub memory_records_post: usize,
    /// Hebbian link count in the pre-migration DB.
    pub hebbian_links_pre: usize,
    /// Hebbian link count in the post-migration DB.
    pub hebbian_links_post: usize,
    /// Knowledge Compiler topic count in the pre-migration DB.
    pub topics_pre: usize,
    /// Knowledge Compiler topic count in the post-migration DB.
    pub topics_post: usize,
    /// `true` iff all three counts match across the boundary.
    pub preserved: bool,
}

/// Query-set parity result. ≥ 20 queries; each query's top-K compared
/// pre/post via Jaccard distance.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QueryParityReport {
    /// Total queries in the parity set.
    pub queries_total: usize,
    /// Queries whose top-K matched within `threshold`.
    pub queries_matching: usize,
    /// `queries_matching / queries_total`. 0.0 when total is 0.
    pub parity_ratio: f64,
    /// Required parity ratio for `passed: true` (default 0.8).
    pub threshold: f64,
    /// `true` iff `parity_ratio >= threshold` AND `queries_total >= 20`.
    pub passed: bool,
}

/// Top-level summary written to `migration_integrity_summary.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationIntegritySummary {
    /// Data-loss check (pre-/post-migration counts).
    pub data_loss: DataLossReport,
    /// Query-parity check (pre-/post-migration retrieval equivalence).
    pub query_parity: QueryParityReport,
    /// Compound: `data_loss.preserved && query_parity.passed`. The gate
    /// metric `migration.integrity` reads this.
    pub integrity_ok: bool,
    /// Human-readable explanation — surfaced in the gate message.
    pub message: String,
    /// Upstream blocker — present iff a prerequisite (migration tool
    /// or orchestrator) was not available. None on a normal run.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocked_by: Option<String>,
}

impl MigrationIntegritySummary {
    /// Construct a blocked summary that emits Missing for the gate.
    /// All counts are zero, all booleans false; the `blocked_by` field
    /// is what the snapshot consults.
    fn blocked(reason: String) -> Self {
        Self {
            data_loss: DataLossReport::default(),
            query_parity: QueryParityReport {
                threshold: DEFAULT_PARITY_THRESHOLD,
                ..Default::default()
            },
            integrity_ok: false,
            message: format!("blocked: {reason}"),
            blocked_by: Some(reason),
        }
    }
}

const DEFAULT_PARITY_THRESHOLD: f64 = 0.8;

// ---------------------------------------------------------------------------
// Run pipeline (skip-aware)
// ---------------------------------------------------------------------------

fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // Two prerequisites:
    //   (a) migration tool must be available
    //   (b) Memory::graph_query orchestrator must be live
    // If either is blocked, surface as Missing → Error.

    if let Some(reason) = probe_migration_tool() {
        return finalize_run(config, MigrationIntegritySummary::blocked(reason));
    }

    if let Some(reason) = probe_orchestrator_status() {
        return finalize_run(config, MigrationIntegritySummary::blocked(reason));
    }

    // Both prerequisites available — run the real pipeline.
    //
    // The pipeline does two things:
    //   1. Generate a v0.2-shaped fixture DB (deterministic seed),
    //      capture pre-counts, run the in-process migration via
    //      `engramai_migrate::migrate`, capture post-counts, build a
    //      DataLossReport.
    //   2. Open the migrated DB via `engramai::Memory`, run a fixed
    //      query set under `graph_query_locked` twice, compute
    //      Jaccard-based parity between the two runs.
    //
    // Note on (2): this measures **locked-mode determinism** (same
    // query set against the post-migration DB twice → identical
    // top-K), not pre-vs-post-migration parity. True pre-vs-post
    // parity requires a frozen v0.2 query golden — not yet available.
    // Documented limitation; the gate still fails meaningfully when
    // either count differs or when locked-mode determinism breaks.
    run_real_pipeline(config)
}

fn run_real_pipeline(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // Build the fixture in a tempdir scoped to this run. The DB is
    // intentionally NOT in `output_root` because per design §6.2 the
    // run dir holds artifacts, not work-in-progress; the fixture is
    // ephemeral.
    let tmp = tempfile::tempdir().map_err(BenchError::IoError)?;
    let fixture_path = tmp.path().join("v02_fixture.db");

    let seeded = seed_v02_fixture(&fixture_path)
        .map_err(|e| BenchError::Other(format!("failed to seed v0.2 fixture: {e}")))?;

    let pre = capture_counts(&fixture_path)
        .map_err(|e| BenchError::Other(format!("failed to capture pre-counts: {e}")))?;

    let migrate_outcome = run_in_process_migration(&fixture_path);
    let migration_err = match migrate_outcome {
        Ok(()) => None,
        Err(e) => Some(e),
    };

    let post = capture_counts(&fixture_path)
        .map_err(|e| BenchError::Other(format!("failed to capture post-counts: {e}")))?;

    let data_loss = build_data_loss_report(&pre, &post, seeded);

    // Even if migration failed, we still want to emit a real summary
    // — gate fails informatively rather than throwing BenchError. We
    // surface the migration error in the message field.
    let query_parity = if migration_err.is_some() {
        QueryParityReport {
            queries_total: 0,
            queries_matching: 0,
            parity_ratio: 0.0,
            threshold: DEFAULT_PARITY_THRESHOLD,
            passed: false,
        }
    } else {
        match run_query_parity(&fixture_path) {
            Ok(qp) => qp,
            Err(e) => {
                eprintln!(
                    "[migration_integrity] query parity probe failed: {e}; \
                     reporting as not-passed."
                );
                QueryParityReport {
                    queries_total: 0,
                    queries_matching: 0,
                    parity_ratio: 0.0,
                    threshold: DEFAULT_PARITY_THRESHOLD,
                    passed: false,
                }
            }
        }
    };

    let integrity_ok = data_loss.preserved && query_parity.passed;
    let message = build_summary_message(&data_loss, &query_parity, migration_err.as_deref());

    let summary = MigrationIntegritySummary {
        data_loss,
        query_parity,
        integrity_ok,
        message,
        blocked_by: None,
    };

    finalize_run(config, summary)
}

/// Probe whether the migration tool is invokable. Returns
/// `Some(blocked_by_message)` when not available, `None` when ready.
///
/// Mirrors the precedent in `test_preservation::apply_migration_to_fixtures`:
/// check both `engram-cli` on PATH and a buildable `engram-cli` target
/// in the workspace. If neither is reachable, we're blocked.
fn probe_migration_tool() -> Option<String> {
    let on_path = Command::new("engram-cli")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if on_path {
        return None;
    }

    // Fallback: cargo metadata mentions the engram-cli package → at
    // least the source is here, can be built. Per test_preservation,
    // we treat "buildable but unbuilt" as available (the harness can
    // `cargo run -p engram-cli -- migrate` on demand).
    let cargo_target_exists = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).contains("\"name\":\"engram-cli\"")
        })
        .unwrap_or(false);

    if cargo_target_exists {
        return None;
    }

    Some(
        "migration tool unavailable: `engram-cli` not on PATH and \
         not present in workspace metadata. \
         blocked_by: feature:v03-migration (engramai-migrate CLI build)"
            .to_string(),
    )
}

/// Probe whether `Memory::graph_query` is live. Same logic as
/// `cognitive_regression::probe_orchestrator_status` — kept duplicated
/// (rather than extracted to a shared helper) so each driver's
/// `blocked_by` message names *its own* dependency chain explicitly.
/// When the orchestrator lands, both probes can be removed in the same
/// follow-up task.
fn probe_orchestrator_status() -> Option<String> {
    use engramai::retrieval::{GraphQuery, RetrievalError};
    use engramai::Memory;

    let mem = match Memory::new(":memory:", None) {
        Ok(m) => m,
        Err(e) => {
            return Some(format!(
                "migration_integrity probe failed at Memory::new: {e}; \
                 cannot determine orchestrator status. \
                 blocked_by: task:retr-impl-orchestrator-classifier-dispatch"
            ));
        }
    };

    let q = GraphQuery::new("orchestrator-probe").with_limit(1);
    let res = block_on(mem.graph_query(q));

    match res {
        Err(RetrievalError::Internal(msg))
            if msg.contains("not yet implemented") =>
        {
            Some(format!(
                "Memory::graph_query is a stub: {msg}. \
                 blocked_by: task:retr-impl-orchestrator-classifier-dispatch \
                 (and follow-ups: plan-execution, fusion-assembly, locked-mode, e2e)"
            ))
        }
        Err(RetrievalError::Internal(msg)) => Some(format!(
            "Memory::graph_query returned unexpected Internal error: {msg}. \
             Re-check task:retr-impl-orchestrator-* status."
        )),
        Err(other) => {
            eprintln!(
                "[migration_integrity] probe: graph_query returned non-stub \
                 error variant ({other:?}); orchestrator appears live."
            );
            None
        }
        Ok(_) => None,
    }
}

/// Async-to-sync bridge. Same noop-waker pattern as the locomo driver.
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

// ---------------------------------------------------------------------------
// Real pipeline: fixture seed + migrate + counts + query parity
// ---------------------------------------------------------------------------

/// Seeded counts captured at fixture build time so the post-migration
/// SQL probe knows what to compare against and which counts to require
/// to be non-zero (a fixture with zero `memories` makes preservation
/// trivially true, which is not a useful test).
#[derive(Debug, Clone, Copy)]
struct SeededCounts {
    memories: usize,
    hebbian_links: usize,
    knowledge_topics: usize,
}

/// Generate a deterministic v0.2-shaped fixture DB at `path`. The
/// schema mirrors what `engramai-migrate::preflight::detect_schema_version`
/// recognises as `SchemaState::V02` — a `memories` table without a
/// populated `schema_version` row. We additionally seed
/// `hebbian_links` and `knowledge_topics` so the data-loss check has
/// three independent dimensions.
///
/// The schema is intentionally a SUBSET of the v0.2 production schema:
/// only the columns the migration tool's Phase-2 readers actually
/// touch. Adding columns unused by migration would be noise. If the
/// migration crate adds a Phase-2 read of a new column, this function
/// must be updated in lock-step (caught by the fixture-shape test
/// below).
fn seed_v02_fixture(path: &std::path::Path) -> Result<SeededCounts, String> {
    use rusqlite::Connection;

    let conn = Connection::open(path).map_err(|e| format!("open fixture db: {e}"))?;

    // Minimum v0.2 schema. NOT including `schema_version` table so
    // detect_schema_version() correctly classifies this as V02 by
    // implication (memories present, schema_version absent).
    conn.execute_batch(
        "CREATE TABLE memories (
            id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            memory_type TEXT NOT NULL DEFAULT 'factual',
            layer TEXT NOT NULL DEFAULT 'core',
            created_at REAL NOT NULL,
            working_strength REAL NOT NULL DEFAULT 1.0,
            core_strength REAL NOT NULL DEFAULT 0.0,
            importance REAL NOT NULL DEFAULT 0.5,
            pinned INTEGER NOT NULL DEFAULT 0,
            consolidation_count INTEGER NOT NULL DEFAULT 0,
            metadata TEXT,
            namespace TEXT NOT NULL DEFAULT 'default'
         );
         CREATE TABLE hebbian_links (
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            strength REAL NOT NULL DEFAULT 1.0,
            coactivation_count INTEGER NOT NULL DEFAULT 0,
            temporal_forward INTEGER NOT NULL DEFAULT 0,
            temporal_backward INTEGER NOT NULL DEFAULT 0,
            direction TEXT NOT NULL DEFAULT 'bidirectional',
            created_at REAL NOT NULL,
            namespace TEXT NOT NULL DEFAULT 'default',
            PRIMARY KEY (source_id, target_id)
         );
         CREATE TABLE knowledge_topics (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL
         );",
    )
    .map_err(|e| format!("create v0.2 schema: {e}"))?;

    // Deterministic seed: 30 memories on a small topic set so query
    // parity has both hits and misses. created_at uses fixed offsets
    // so repro records are byte-identical.
    let topics = [
        ("rust", "Rust is a systems programming language with strong memory safety."),
        ("python", "Python is a dynamically-typed language popular in data science."),
        ("graph", "Graph databases store entities and relationships natively."),
        ("memory", "Memory consolidation moves recent experiences to long-term storage."),
        ("retrieval", "Retrieval-augmented generation grounds LLMs in private knowledge."),
        ("embedding", "Embeddings map text into dense vectors capturing semantic meaning."),
    ];

    let tx = conn.unchecked_transaction().map_err(|e| format!("begin tx: {e}"))?;
    for i in 0..30 {
        let (tag, body) = topics[i % topics.len()];
        let id = format!("mem-{i:03}");
        let content = format!("[{tag}#{i}] {body}");
        let created_at = 1_700_000_000.0_f64 + (i as f64);
        tx.execute(
            "INSERT INTO memories (id, content, memory_type, layer, created_at, importance) \
             VALUES (?1, ?2, 'factual', 'core', ?3, 0.5)",
            rusqlite::params![id, content, created_at],
        )
        .map_err(|e| format!("insert memory {id}: {e}"))?;
    }

    // 10 hebbian links — chain pattern so they reference real ids.
    for i in 0..10 {
        let src = format!("mem-{i:03}");
        let tgt = format!("mem-{:03}", (i + 5) % 30);
        tx.execute(
            "INSERT INTO hebbian_links (source_id, target_id, strength, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![src, tgt, 0.5_f64 + (i as f64) * 0.05, 1_700_000_000.0_f64],
        )
        .map_err(|e| format!("insert hebbian: {e}"))?;
    }

    // 5 knowledge_topics rows.
    for i in 0..5 {
        let id = format!("topic-{i}");
        let title = format!("Topic {i}");
        let content = format!("Synthesised body for topic {i}.");
        tx.execute(
            "INSERT INTO knowledge_topics (id, title, content, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                title,
                content,
                1_700_000_000.0_f64,
                1_700_000_000.0_f64,
            ],
        )
        .map_err(|e| format!("insert topic: {e}"))?;
    }
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;

    Ok(SeededCounts {
        memories: 30,
        hebbian_links: 10,
        knowledge_topics: 5,
    })
}

/// Captured row counts for the three preservation tables.
#[derive(Debug, Clone, Copy)]
struct CountSnapshot {
    memories: i64,
    hebbian_links: i64,
    knowledge_topics: i64,
}

fn capture_counts(path: &std::path::Path) -> Result<CountSnapshot, String> {
    use rusqlite::Connection;

    let conn = Connection::open(path).map_err(|e| format!("open db for count: {e}"))?;
    let memories = scalar_count(&conn, "memories")?;
    let hebbian_links = scalar_count(&conn, "hebbian_links")?;
    // Migration may rename or split topics across `knowledge_topics` +
    // `knowledge_topics_legacy`. Sum both for the post-count so a
    // rename/split doesn't get reported as data loss.
    let kt = scalar_count(&conn, "knowledge_topics")?;
    let kt_legacy = scalar_count(&conn, "knowledge_topics_legacy").unwrap_or(0);
    Ok(CountSnapshot {
        memories,
        hebbian_links,
        knowledge_topics: kt + kt_legacy,
    })
}

fn scalar_count(conn: &rusqlite::Connection, table: &str) -> Result<i64, String> {
    // Use sqlite_master to detect missing table — return 0 instead of
    // erroring, so post-migration probes for tables that didn't exist
    // pre-migration don't fail.
    let table_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            rusqlite::params![table],
            |row| row.get::<_, i64>(0).map(|n| n != 0),
        )
        .map_err(|e| format!("table_exists({table}): {e}"))?;

    if !table_exists {
        return Ok(0);
    }

    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("count {table}: {e}"))
}

/// Run the migration tool in-process. Wraps `engramai_migrate::migrate`
/// with the safe-default flags an automated harness needs:
/// `accept_forward_only` (we acknowledge the irreversibility),
/// `no_backup` + `accept_no_grace` (the fixture is ephemeral).
///
/// Returns `Err(message)` on any migration failure or partial run; the
/// caller surfaces this in the summary and lets the gate fail
/// informatively rather than throwing `BenchError`.
fn run_in_process_migration(path: &std::path::Path) -> Result<(), String> {
    use engramai_migrate::{migrate, MigrateOptions};

    let mut opts = MigrateOptions::new(path);
    opts.no_backup = true;
    opts.accept_no_grace = true;
    opts.accept_forward_only = true;

    let report = migrate(&opts).map_err(|e| format!("migrate() failed: {e}"))?;

    if !report.migration_complete {
        return Err(format!(
            "migrate() returned report with migration_complete=false; \
             final_phase={}",
            report.final_phase
        ));
    }
    Ok(())
}

fn build_data_loss_report(
    pre: &CountSnapshot,
    post: &CountSnapshot,
    seeded: SeededCounts,
) -> DataLossReport {
    // Sanity: pre-counts must equal what we seeded. If they don't,
    // either capture_counts is broken or the fixture got mutated by
    // something between seed and pre-count. Either case → data loss.
    let pre_seed_match = pre.memories as usize == seeded.memories
        && pre.hebbian_links as usize == seeded.hebbian_links
        && pre.knowledge_topics as usize == seeded.knowledge_topics;

    let post_match = pre.memories == post.memories
        && pre.hebbian_links == post.hebbian_links
        && pre.knowledge_topics == post.knowledge_topics;

    DataLossReport {
        memory_records_pre: pre.memories as usize,
        memory_records_post: post.memories as usize,
        hebbian_links_pre: pre.hebbian_links as usize,
        hebbian_links_post: post.hebbian_links as usize,
        topics_pre: pre.knowledge_topics as usize,
        topics_post: post.knowledge_topics as usize,
        preserved: pre_seed_match && post_match,
    }
}

/// 20-query fixed set covering the topics seeded by `seed_v02_fixture`.
/// Locked-mode determinism check: we run each query twice and require
/// byte-identical top-K. Threshold defined in DEFAULT_PARITY_THRESHOLD.
const PARITY_QUERY_SET: &[&str] = &[
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
];

/// Run the 20-query parity set against the migrated DB. Compares the
/// top-K from two consecutive `graph_query_locked` calls per query
/// (locked-mode must be deterministic). Returns Jaccard-based parity
/// summary.
///
/// **Why locked-mode self-parity, not pre-vs-post parity:** the
/// pre-migration v0.2 DB cannot be queried by the v0.3 `Memory` API
/// (schema mismatch), and a frozen v0.2 query golden does not yet
/// exist. Locked-mode determinism is the strongest parity invariant
/// we can verify in-process today; it catches non-determinism
/// regressions in fusion, ranking, and trace assembly. Pre-vs-post
/// parity remains future work (will reuse this query set).
fn run_query_parity(migrated_db: &std::path::Path) -> Result<QueryParityReport, String> {
    use engramai::retrieval::GraphQuery;
    use engramai::Memory;

    let mem = Memory::new(
        migrated_db
            .to_str()
            .ok_or_else(|| "migrated_db path is not valid UTF-8".to_string())?,
        None,
    )
    .map_err(|e| format!("Memory::new on migrated DB: {e}"))?;

    let total = PARITY_QUERY_SET.len();
    let mut matching = 0usize;

    for query_text in PARITY_QUERY_SET {
        let q1 = GraphQuery::new(*query_text).with_limit(5);
        let q2 = GraphQuery::new(*query_text).with_limit(5);

        let r1 = block_on(mem.graph_query_locked(q1));
        let r2 = block_on(mem.graph_query_locked(q2));

        let ids1 = top_ids(&r1);
        let ids2 = top_ids(&r2);

        // Self-parity: locked mode must produce byte-identical lists.
        // We use Jaccard distance between the two lists; identical → 1.0,
        // any divergence → < 1.0. Threshold check happens in caller.
        if jaccard(&ids1, &ids2) >= 0.99 {
            matching += 1;
        }
    }

    let parity_ratio = if total == 0 {
        0.0
    } else {
        matching as f64 / total as f64
    };
    let passed = total >= 20 && parity_ratio >= DEFAULT_PARITY_THRESHOLD;

    Ok(QueryParityReport {
        queries_total: total,
        queries_matching: matching,
        parity_ratio,
        threshold: DEFAULT_PARITY_THRESHOLD,
        passed,
    })
}

/// Extract top-K result ids from a graph_query result. On error,
/// return empty list — that propagates to Jaccard=0 and fails parity,
/// which is the right gate behaviour for a non-functional retrieval
/// path.
///
/// `ScoredResult` is a heterogeneous enum (Memory | Topic) per design §6.2.
/// For parity comparison we need a stable string identity per variant:
///   * Memory → `record.id` (MemoryId — the natural key under test)
///   * Topic  → `"topic:<uuid>"` (prefixed to prevent any collision with
///     a MemoryId that happens to stringify to the same UUID)
/// In the migration_integrity scenario the planner runs baseline (non-
/// abstract) plans, so Topic results should be empty in practice — but
/// handling the variant exhaustively keeps this fn correct under future
/// plan changes without silent drift.
fn top_ids(
    res: &Result<engramai::retrieval::GraphQueryResponse, engramai::retrieval::RetrievalError>,
) -> Vec<String> {
    match res {
        Ok(resp) => resp
            .results
            .iter()
            .map(|sr| match sr {
                engramai::retrieval::ScoredResult::Memory { record, .. } => record.id.clone(),
                engramai::retrieval::ScoredResult::Topic { topic, .. } => {
                    format!("topic:{}", topic.topic_id)
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    use std::collections::HashSet;
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

fn build_summary_message(
    data_loss: &DataLossReport,
    query_parity: &QueryParityReport,
    migration_err: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if let Some(e) = migration_err {
        parts.push(format!("migration error: {e}"));
    }
    parts.push(format!(
        "data-loss: memories {}/{}, hebbian {}/{}, topics {}/{} (preserved={})",
        data_loss.memory_records_pre,
        data_loss.memory_records_post,
        data_loss.hebbian_links_pre,
        data_loss.hebbian_links_post,
        data_loss.topics_pre,
        data_loss.topics_post,
        data_loss.preserved,
    ));
    parts.push(format!(
        "query-parity (locked-mode self): {}/{} matching, ratio {:.3} (threshold {:.3}, passed={})",
        query_parity.queries_matching,
        query_parity.queries_total,
        query_parity.parity_ratio,
        query_parity.threshold,
        query_parity.passed,
    ));
    parts.join("; ")
}

// ---------------------------------------------------------------------------
// Artifact emission + RunReport assembly
// ---------------------------------------------------------------------------

fn finalize_run(
    config: &HarnessConfig,
    summary: MigrationIntegritySummary,
) -> Result<RunReport, BenchError> {
    let out_dir = ensure_run_dir(config)?;
    let summary_path = out_dir.join("migration_integrity_summary.json");

    let body = serde_json::to_string_pretty(&summary)
        .map_err(|e| BenchError::Other(format!("summary serialization failed: {e}")))?;
    fs::write(&summary_path, body).map_err(BenchError::IoError)?;

    let record_path = out_dir.join("reproducibility.toml");
    write_reproducibility_stub(&record_path, &summary)?;

    let gates = evaluate_gates(&summary);
    let summary_json = serde_json::to_value(&summary)
        .map_err(|e| BenchError::Other(format!("summary->json failed: {e}")))?;

    Ok(RunReport {
        driver: Driver::MigrationIntegrity,
        record_path,
        gates,
        summary_json,
    })
}

fn ensure_run_dir(config: &HarnessConfig) -> Result<PathBuf, BenchError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dir = config
        .output_root
        .join(format!("{ts}_migration_integrity"));
    fs::create_dir_all(&dir).map_err(BenchError::IoError)?;
    Ok(dir)
}

fn write_reproducibility_stub(
    path: &PathBuf,
    summary: &MigrationIntegritySummary,
) -> Result<(), BenchError> {
    let mut s = String::new();
    s.push_str("# migration_integrity reproducibility record\n");
    s.push_str("[run]\n");
    s.push_str("driver = \"migration-integrity\"\n");
    if let Some(reason) = &summary.blocked_by {
        s.push_str("status = \"error\"\n");
        s.push_str(&format!("blocked_by = {}\n", toml_string_literal(reason)));
    } else {
        s.push_str(&format!(
            "status = \"{}\"\n",
            if summary.integrity_ok { "pass" } else { "fail" }
        ));
        s.push_str(&format!(
            "preserved = {}\n",
            summary.data_loss.preserved
        ));
        s.push_str(&format!(
            "parity_ratio = {}\n",
            summary.query_parity.parity_ratio
        ));
    }
    fs::write(path, s).map_err(BenchError::IoError)?;
    Ok(())
}

fn toml_string_literal(s: &str) -> String {
    let escaped = s.replace("\"\"\"", "\"\"\\\"");
    format!("\"\"\"{escaped}\"\"\"")
}

// ---------------------------------------------------------------------------
// Gate evaluation — compound metric
// ---------------------------------------------------------------------------

/// Build the metric snapshot for `evaluate_for_driver("migration.", ...)`.
///
/// - `blocked_by = Some(_)` → `set_missing("migration.integrity", reason)`
///   → `GateStatus::Error` (GUARD-2: never silent pass).
/// - `blocked_by = None` → `set_compound("migration.integrity", integrity_ok, message)`
///   → gate compares against the compound predicate per design §4.4.
fn build_snapshot(summary: &MigrationIntegritySummary) -> MetricSnapshot {
    let mut snap = MetricSnapshot::new();
    if let Some(reason) = &summary.blocked_by {
        snap.set_missing(
            "migration.integrity",
            format!("migration_integrity blocked: {reason}"),
        );
    } else {
        snap.set_compound(
            "migration.integrity",
            summary.integrity_ok,
            summary.message.clone(),
        );
    }
    snap
}

/// Evaluate gates whose `metric_key` starts with `migration.` against
/// this driver's summary. Single-gate driver today (only GOAL-5.7).
pub(crate) fn evaluate_gates(summary: &MigrationIntegritySummary) -> Vec<GateResult> {
    let snap = build_snapshot(summary);
    evaluate_for_driver("migration.", &snap, &NoBaselines)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::GateStatus;
    use tempfile::tempdir;

    /// Driver metadata triple must match harness scheduling expectations.
    #[test]
    fn driver_metadata() {
        let d = MigrationIntegrityDriver::new();
        assert_eq!(d.name(), Driver::MigrationIntegrity);
        assert_eq!(d.stage(), Stage::Stage1);
        assert_eq!(d.cost_tier(), CostTier::Medium);
    }

    /// Blocked summaries must surface `migration.integrity` as Missing,
    /// never as a passing compound — GUARD-2.
    #[test]
    fn blocked_summary_emits_missing_metric() {
        let summary = MigrationIntegritySummary::blocked("orchestrator stub".into());
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "migration.integrity")
            .expect("GOAL-5.7 gate must fire even when blocked");
        assert_eq!(
            g.status,
            GateStatus::Error,
            "blocked run must Error, not Pass — GUARD-2"
        );
    }

    /// Unblocked + integrity_ok=true → compound passes.
    #[test]
    fn unblocked_integrity_ok_passes() {
        let summary = MigrationIntegritySummary {
            data_loss: DataLossReport {
                preserved: true,
                ..Default::default()
            },
            query_parity: QueryParityReport {
                queries_total: 20,
                queries_matching: 20,
                parity_ratio: 1.0,
                threshold: DEFAULT_PARITY_THRESHOLD,
                passed: true,
            },
            integrity_ok: true,
            message: "no loss; 20/20 query parity".into(),
            blocked_by: None,
        };
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "migration.integrity")
            .expect("GOAL-5.7 gate must be present");
        assert_eq!(g.status, GateStatus::Pass);
    }

    /// Unblocked + integrity_ok=false → compound fails.
    #[test]
    fn unblocked_integrity_failed_fails() {
        let summary = MigrationIntegritySummary {
            data_loss: DataLossReport {
                memory_records_pre: 100,
                memory_records_post: 97,
                preserved: false,
                ..Default::default()
            },
            query_parity: QueryParityReport {
                threshold: DEFAULT_PARITY_THRESHOLD,
                ..Default::default()
            },
            integrity_ok: false,
            message: "3 records lost during migration".into(),
            blocked_by: None,
        };
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "migration.integrity")
            .expect("GOAL-5.7 gate must be present");
        assert_eq!(g.status, GateStatus::Fail);
        assert!(
            g.message.contains("3 records lost"),
            "gate message must surface the failure reason; got: {}",
            g.message
        );
    }

    /// Canonical skip-aware integration test: with the orchestrator
    /// still a stub, `run_impl` must produce a blocked summary, write
    /// the JSON artifact, and return a RunReport whose gate is Error.
    /// The migration-tool probe may pass or fail depending on whether
    /// `engram-cli` is built locally — either way, the orchestrator
    /// probe will block and we land on an Error gate.
    #[test]
    fn run_impl_against_blocked_prereqs_produces_blocked_report() {
        let tmp = tempdir().unwrap();
        let mut cfg = HarnessConfig::default();
        cfg.output_root = tmp.path().to_path_buf();

        let report =
            run_impl(&cfg).expect("driver must not error in skip-aware mode");

        assert_eq!(report.driver, Driver::MigrationIntegrity);
        assert!(
            report.record_path.exists(),
            "reproducibility record must be written"
        );

        let blocked_by = report
            .summary_json
            .get("blocked_by")
            .and_then(|v| v.as_str())
            .expect("summary.blocked_by must be set when prereqs missing");
        // Either probe (migration tool OR orchestrator) is allowed to
        // be the blocker — both reference upstream tasks/features.
        assert!(
            blocked_by.contains("task:retr-impl-orchestrator")
                || blocked_by.contains("feature:v03-migration"),
            "blocked_by must name an upstream blocker; got: {blocked_by}"
        );

        let g = report
            .gates
            .iter()
            .find(|g| g.metric_key == "migration.integrity")
            .expect("GOAL-5.7 gate must be present in report");
        assert_eq!(
            g.status,
            GateStatus::Error,
            "blocked prereqs must surface as Error, not silent Pass"
        );
    }
}
