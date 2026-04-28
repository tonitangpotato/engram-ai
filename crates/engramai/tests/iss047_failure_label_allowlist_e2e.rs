//! ISS-047 regression: closed-set failure-label allowlist must accept every
//! label produced by the live resolution pipeline.
//!
//! Before the fix, every `record_failure(...)` / `failure_row(...)` call
//! site in `resolution/{pipeline,stage_persist,stage_edge_extract}.rs`
//! produced a `(stage, error_category)` pair that was **not** in the
//! audit allowlist (`audit::STAGE_*` / `CATEGORY_*`). The validator runs
//! inside `apply_graph_delta`'s persist transaction, so a single rejected
//! row aborted the whole transaction — including successfully extracted
//! entities and edges. End-to-end ingest = 100% data loss in the graph
//! layer for any input that triggered any stage failure.
//!
//! This test is the regression fence:
//!
//! 1. **`enumerate_closed_set_pairs`** — exhaustively records every
//!    `(stage, category)` pair the pipeline can emit. None of them should
//!    return `Err(Invariant)`.
//! 2. **`unresolved_subject_does_not_roll_back_pipeline`** — drives the
//!    exact failure mode that surfaced on LoCoMo conv-26: triple extractor
//!    emits a subject that EntityExtractor never saw → `unresolved_subject`
//!    failure on the `Resolve` stage. Asserts the failure row lands in
//!    `graph_extraction_failures` and the memory write itself was not
//!    rolled back.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use engramai::graph::audit::{
    CATEGORY_APPLY_GRAPH_DELTA_ERROR, CATEGORY_BUDGET_EXHAUSTED,
    CATEGORY_CANDIDATE_RETRIEVAL_ERROR, CATEGORY_CANONICAL_FETCH_ERROR, CATEGORY_DB_ERROR,
    CATEGORY_EXTRACTOR_ERROR, CATEGORY_FIND_EDGES_ERROR, CATEGORY_INTERNAL,
    CATEGORY_LLM_INVALID_OUTPUT, CATEGORY_LLM_TIMEOUT, CATEGORY_MISSING_CANONICAL,
    CATEGORY_QUEUE_FULL, CATEGORY_UNRESOLVED_DEFER, CATEGORY_UNRESOLVED_OBJECT,
    CATEGORY_UNRESOLVED_SUBJECT, ExtractionFailure, STAGE_DEDUP, STAGE_EDGE_EXTRACT,
    STAGE_ENTITY_EXTRACT, STAGE_INGEST, STAGE_KNOWLEDGE_COMPILE, STAGE_PERSIST,
    STAGE_RESOLVE,
};
use engramai::graph::store::GraphWrite;
use engramai::graph::SqliteGraphStore;
use engramai::memory::Memory;
use engramai::resolution::ResolutionConfig;
use engramai::store_api::StorageMeta;
use engramai::triple::{Predicate, Triple};
use engramai::triple_extractor::TripleExtractor;
use rusqlite::Connection;
use tempfile::tempdir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test 1 — closed-set enumeration
// ---------------------------------------------------------------------------

/// Every stage emitted by the pipeline must validate.
const PIPELINE_STAGES: &[&str] = &[
    STAGE_INGEST,
    STAGE_ENTITY_EXTRACT,
    STAGE_EDGE_EXTRACT,
    STAGE_RESOLVE,
    STAGE_PERSIST,
];

/// Every category the pipeline + knowledge_compile can emit. Constants are
/// imported so adding a new one here without adding it to `audit.rs` is a
/// compile error — the regression fence is at the type level.
const PIPELINE_CATEGORIES: &[&str] = &[
    // Coarse retry-aware classes (knowledge_compile + future ingest).
    CATEGORY_LLM_TIMEOUT,
    CATEGORY_LLM_INVALID_OUTPUT,
    CATEGORY_BUDGET_EXHAUSTED,
    CATEGORY_DB_ERROR,
    CATEGORY_INTERNAL,
    // Pipeline call-site labels (ISS-047).
    CATEGORY_EXTRACTOR_ERROR,
    CATEGORY_CANDIDATE_RETRIEVAL_ERROR,
    CATEGORY_CANONICAL_FETCH_ERROR,
    CATEGORY_UNRESOLVED_SUBJECT,
    CATEGORY_UNRESOLVED_OBJECT,
    CATEGORY_FIND_EDGES_ERROR,
    CATEGORY_APPLY_GRAPH_DELTA_ERROR,
    CATEGORY_MISSING_CANONICAL,
    CATEGORY_UNRESOLVED_DEFER,
    CATEGORY_QUEUE_FULL,
];

#[test]
fn enumerate_closed_set_pairs_all_validate() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("audit.db");
    // Fresh store also runs migrations / DDL so graph_extraction_failures
    // table exists.
    let mut conn = Connection::open(&db_path).expect("open conn");
    engramai::graph::init_graph_tables(&conn).expect("schema");
    let mut store = SqliteGraphStore::new(&mut conn);

    let mut accepted = 0usize;
    let mut rejected_pairs: Vec<(&str, &str)> = Vec::new();

    for stage in PIPELINE_STAGES {
        for category in PIPELINE_CATEGORIES {
            let f = ExtractionFailure {
                id: Uuid::new_v4(),
                episode_id: Uuid::new_v4(),
                stage: stage.to_string(),
                error_category: category.to_string(),
                error_detail: Some("regression fence".into()),
                occurred_at: 1.0,
                resolved_at: None,
            };
            match store.record_extraction_failure(&f) {
                Ok(()) => accepted += 1,
                Err(_) => rejected_pairs.push((*stage, *category)),
            }
        }
    }

    assert!(
        rejected_pairs.is_empty(),
        "ISS-047 regression: validator rejected {} (stage,category) pair(s) the \
         pipeline can produce: {:?}",
        rejected_pairs.len(),
        rejected_pairs
    );
    assert_eq!(
        accepted,
        PIPELINE_STAGES.len() * PIPELINE_CATEGORIES.len(),
        "every pair should validate"
    );
}

/// Also explicitly cover the dedup and knowledge_compile stages — they are
/// in the allowlist but not currently emitted by the resolution pipeline.
/// Belt-and-braces: if a future change folds dedup / KC failures into the
/// same path, the validator will still accept them.
#[test]
fn dedup_and_knowledge_compile_stages_validate() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("audit2.db");
    let mut conn = Connection::open(&db_path).expect("open conn");
    engramai::graph::init_graph_tables(&conn).expect("schema");
    let mut store = SqliteGraphStore::new(&mut conn);

    for stage in &[STAGE_DEDUP, STAGE_KNOWLEDGE_COMPILE] {
        let f = ExtractionFailure {
            id: Uuid::new_v4(),
            episode_id: Uuid::new_v4(),
            stage: stage.to_string(),
            error_category: CATEGORY_INTERNAL.to_string(),
            error_detail: Some("smoke".into()),
            occurred_at: 1.0,
            resolved_at: None,
        };
        store.record_extraction_failure(&f).unwrap_or_else(|e| {
            panic!("stage {stage} rejected: {e:?}")
        });
    }
}

// ---------------------------------------------------------------------------
// Test 2 — live pipeline drives an unresolved-subject failure end-to-end
// ---------------------------------------------------------------------------

/// `TripleExtractor` that returns one triple whose subject is a name the
/// `EntityExtractor` (pattern-based) never matches. Drives the exact LoCoMo
/// conv-26 failure mode: `Resolve` stage emits `unresolved_subject`,
/// `apply_graph_delta` is called with one `StageFailureRow`.
///
/// Pre-fix: validator rejected the row → entire transaction rolled back →
/// the memory itself wasn't lost (already committed pre-pipeline) but every
/// graph artifact was discarded.
///
/// Post-fix: row lands in `graph_extraction_failures`, transaction commits
/// cleanly even though no entity/edge was produced.
struct UnresolvableSubjectExtractor;

impl TripleExtractor for UnresolvableSubjectExtractor {
    fn extract_triples(
        &self,
        _content: &str,
    ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        // "Caroline Martinez" is a personal name; pattern-based
        // EntityExtractor in v0.2 does not match it, so the subject won't
        // be in `entity_drafts` at resolution time.
        Ok(vec![Triple::new(
            "Caroline Martinez".to_string(),
            Predicate::RelatedTo,
            "topic-x".to_string(),
            0.9,
        )])
    }
}

#[test]
fn unresolved_subject_does_not_roll_back_pipeline() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("unresolved.db");
    let db_path_str = db_path.to_str().expect("utf-8 db path");

    let triple_extractor: Arc<dyn TripleExtractor> =
        Arc::new(UnresolvableSubjectExtractor);

    let mut config = ResolutionConfig::default();
    config.worker_count = 1;
    config.queue_cap = 4;
    config.shutdown_drain = Duration::from_secs(2);
    config.worker_idle_poll = Duration::from_millis(10);

    let mut mem = Memory::new(db_path_str, None)
        .expect("memory boots")
        .with_pipeline_pool(&db_path, triple_extractor, config)
        .expect("pipeline pool wires up");

    // Bare conversational text — no patterns the v0.2 EntityExtractor
    // matches. EntityExtractor will produce zero entities, edge extractor
    // will produce one triple, resolve will fail with unresolved_subject.
    let meta = StorageMeta::default();
    mem.store_raw("Caroline Martinez said hello today.", meta)
        .expect("store_raw ok");

    // Wait for worker to drain the queue and run the pipeline.
    std::thread::sleep(Duration::from_millis(800));

    let _ = mem
        .shutdown_pipeline(Duration::from_secs(2))
        .expect("shutdown ok")
        .expect("snapshot");

    // Open the graph DB read-only and inspect what landed.
    let conn = Connection::open(&db_path).expect("reopen graph db");

    // (a) The failure row must be present and carry the exact label pair.
    let mut stmt = conn
        .prepare(
            "SELECT stage, error_category, error_detail FROM graph_extraction_failures",
        )
        .expect("prepare select");
    let rows: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
        })
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();

    let unresolved_count = rows
        .iter()
        .filter(|(s, c, _)| s == "resolve" && c == "unresolved_subject")
        .count();
    assert!(
        unresolved_count >= 1,
        "expected at least one (resolve, unresolved_subject) failure row, \
         got rows={rows:?}"
    );

    // (b) Pre-fix this would be 0 because validator rejected the failure
    // row and the whole transaction (including the pipeline_run audit row)
    // rolled back. Post-fix the run row commits cleanly.
    let pipeline_runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM graph_pipeline_runs", [], |r| r.get(0))
        .expect("count runs");
    assert!(
        pipeline_runs >= 1,
        "expected at least 1 pipeline_run row, got {pipeline_runs} — \
         transaction was rolled back (regression of ISS-047)"
    );

    // (c) Run status should be Succeeded — partial completion is success
    // per design §3 (GOAL-2.3). Failure ledger captures the missing edge.
    let succeeded_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_pipeline_runs WHERE status = 'succeeded'",
            [],
            |r| r.get(0),
        )
        .expect("count succeeded");
    assert!(
        succeeded_count >= 1,
        "expected the run to be marked 'succeeded' (partial-completion semantics)"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — extractor_error path lands cleanly
// ---------------------------------------------------------------------------

struct ErroringExtractor;

impl TripleExtractor for ErroringExtractor {
    fn extract_triples(
        &self,
        _content: &str,
    ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
        Err("simulated LLM 502".into())
    }
}

#[test]
fn extractor_error_lands_in_failure_ledger() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("extractor_err.db");
    let db_path_str = db_path.to_str().expect("utf-8 db path");

    let triple_extractor: Arc<dyn TripleExtractor> = Arc::new(ErroringExtractor);

    let mut config = ResolutionConfig::default();
    config.worker_count = 1;
    config.queue_cap = 4;
    config.shutdown_drain = Duration::from_secs(2);
    config.worker_idle_poll = Duration::from_millis(10);

    let mut mem = Memory::new(db_path_str, None)
        .expect("memory boots")
        .with_pipeline_pool(&db_path, triple_extractor, config)
        .expect("pipeline pool wires up");

    let meta = StorageMeta::default();
    mem.store_raw("Anything", meta).expect("store_raw ok");
    std::thread::sleep(Duration::from_millis(800));
    let _ = mem
        .shutdown_pipeline(Duration::from_secs(2))
        .expect("shutdown ok")
        .expect("snapshot");

    let conn = Connection::open(&db_path).expect("reopen graph db");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_extraction_failures \
             WHERE stage = 'edge_extract' AND error_category = 'extractor_error'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert!(
        count >= 1,
        "expected at least one (edge_extract, extractor_error) row, got {count}"
    );
}
